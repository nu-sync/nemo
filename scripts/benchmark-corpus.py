#!/usr/bin/env python3
"""Compare warm Nemo runtimes on frozen speech fixtures, with per-duration latency and WER."""

import argparse
import json
import os
from pathlib import Path
import platform
import random
import secrets
import signal
import socket
import statistics
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

from benchmark import candidate, check_daemon, command_output, gpu_info, identity, multipart, stop
from speech_corpus import (DEFAULT_CORPUS, distribution, load_corpus, local_file,
                           prepare_audio, sha256, word_errors)


def override(value):
    name, separator, path = value.partition("=")
    if not name or not separator or not path:
        raise argparse.ArgumentTypeError("use NAME=PATH")
    return name, Path(path).expanduser().resolve()


def summarize(records, configs, fixtures, max_wer, budget):
    result = {}
    for config in configs:
        per_fixture = {}
        for fixture in fixtures:
            samples = [row for row in records if row["candidate"] == config["name"]
                       and row["fixture"] == fixture["id"] and row["stage"] == "warm"]
            completed = [row for row in samples if "accuracy" in row]
            latency = distribution([row["request_ms"] for row in completed])
            errors = sum(row["accuracy"]["errors"] for row in completed)
            reference_words = sum(row["accuracy"]["reference_words"] for row in completed)
            per_fixture[fixture["id"]] = {
                "target_seconds": fixture["target_seconds"], "requests": len(samples),
                "request_errors": len(samples) - len(completed), "request_ms": latency,
                "corpus_wer": errors / reference_words if reference_words else None,
                "worst_wer": max((row["accuracy"]["wer"] for row in completed), default=None),
                "accuracy_failures": sum(row["accuracy"]["wer"] > max_wer for row in completed),
                "latency_budget_pass": latency["p95"] <= budget if latency and budget is not None else None,
            }
        result[config["name"]] = per_fixture
    return result


def compare(records, configs, fixtures):
    baseline = configs[0]["name"]
    result = {}
    for config in configs[1:]:
        per_fixture = {}
        for fixture in fixtures:
            paired = {}
            for row in records:
                if (row["stage"] != "warm" or row["fixture"] != fixture["id"]
                        or "accuracy" not in row or row["candidate"] not in (baseline, config["name"])):
                    continue
                paired.setdefault((row["round"], row["repeat"]), {})[row["candidate"]] = row["request_ms"]
            pairs = [pair for pair in paired.values() if len(pair) == 2]
            per_fixture[fixture["id"]] = {
                "baseline": baseline, "paired_samples": len(pairs),
                "median_paired_change_percent": statistics.median(
                    100 * (pair[config["name"]] / pair[baseline] - 1) for pair in pairs) if pairs else None,
                "note": "negative is faster; paired by round/fixture/repeat, not a significance test",
            }
        result[config["name"]] = per_fixture
    return result


def transcribe(opener, base, key, payload):
    body, content_type = payload
    request = urllib.request.Request(base + "/v1/audio/transcriptions", data=body,
                                     headers={"Authorization": "Bearer " + key, "Content-Type": content_type})
    with opener.open(request, timeout=300) as response:
        result = json.load(response)
    text = result["text"]
    if not isinstance(text, str):
        raise ValueError("runtime response text is not a string")
    return text.strip()


def runtime_command(config, port, key):
    command = [
        str(config["runtime"]), "serve", "--asr-model", str(config["model"]),
        "--host", "127.0.0.1", "--port", str(port), "--api-key", key,
        "--device", "metal", "--threads", "4", "--no-ui",
        "--asr.backend.threads", str(config["inference_threads"]),
        "--asr.salm.max_sequences", "1", "--asr.salm.n_ctx_per_sequence", "8192",
        "--asr.salm.max_context", "8192", "--asr.salm.n_batch", str(config["n_batch"]),
        "--asr.salm.encoder_max_batch", str(config["encoder_max_batch"]),
        "--asr.salm.max_new_tokens", "4096", "--asr.salm.mtp_draft", "0",
        "--write-timeout", "300", "--max-upload-mb", "64",
    ]
    if not config["startup_warmup"]:
        command.append("--no-warmup")
    return command


def visit(args, config, round_number, fixtures, payloads, references, schedule, report, save):
    check_daemon(args.daemon)
    key = secrets.token_hex(24)
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    base = f"http://127.0.0.1:{port}"
    command = runtime_command(config, port, key)
    log_path = args.output / f"{round_number:02d}-{config['name']}.log"
    visit_record = {"candidate": config["name"], "round": round_number, "log": str(log_path),
                    "command": ["[redacted]" if arg == key else arg for arg in command], "status": "starting"}
    report["visits"].append(visit_record)
    save()
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    environment = os.environ.copy()
    environment.pop("NEMO_SPEECH_TIMING", None)
    process = None
    try:
        with log_path.open("wb") as log:
            started = time.perf_counter()
            process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=log, stderr=log, env=environment)
            visit_record["pid"] = process.pid
            save()
            while True:
                if process.poll() is not None:
                    raise RuntimeError(f"runtime exited with {process.returncode}; see {log_path}")
                if time.perf_counter() - started > 120:
                    raise TimeoutError("model readiness exceeded 120 seconds")
                try:
                    with opener.open(base + "/health", timeout=.5) as response:
                        if response.status == 200:
                            break
                except (urllib.error.URLError, TimeoutError):
                    pass
                time.sleep(.1)
            visit_record["load_to_health_ms"] = 1000 * (time.perf_counter() - started)
            # Keep the first request as evidence, but exclude it from warm percentiles.
            warmup = min(fixtures, key=lambda fixture: abs(fixture["target_seconds"] - 5))["id"]
            for stage, fixture_id, repeat in [("first", warmup, 0)] + [("warm", name, repeat) for name, repeat in schedule]:
                check_daemon(args.daemon)
                row = {"candidate": config["name"], "round": round_number, "fixture": fixture_id,
                       "repeat": repeat, "stage": stage, "status": "error"}
                report["records"].append(row)
                started = time.perf_counter()
                try:
                    text = transcribe(opener, base, key, payloads[fixture_id])
                    row["request_ms"] = 1000 * (time.perf_counter() - started)
                    row["text"] = text
                    row["accuracy"] = word_errors(references[fixture_id], text)
                    row["status"] = "ok" if row["accuracy"]["wer"] <= args.max_wer else "accuracy_failed"
                except Exception as error:
                    row["request_ms"] = 1000 * (time.perf_counter() - started)
                    row["error"] = f"{type(error).__name__}: {error}".replace(key, "[redacted]")
                save()
                accuracy = f" WER={100 * row['accuracy']['wer']:.2f}%" if "accuracy" in row else ""
                print(f"round={round_number} {config['name']} {stage} {fixture_id} "
                      f"{row['request_ms']:.1f}ms {row['status']}{accuracy}", flush=True)
                if row["status"] == "error":
                    raise RuntimeError(row["error"])
            visit_record["status"] = "ok"
    except BaseException as error:
        visit_record["status"] = "interrupted" if isinstance(error, KeyboardInterrupt) else "error"
        visit_record["error"] = f"{type(error).__name__}: {error}".replace(key, "[redacted]")
        raise
    finally:
        cleanup_interrupted = process is not None and stop(process)
        if cleanup_interrupted:
            visit_record["status"] = "interrupted"
        save()
        if cleanup_interrupted:
            raise KeyboardInterrupt()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, default=DEFAULT_CORPUS)
    parser.add_argument("--suite", choices=("typical", "stress", "all"), default="typical")
    parser.add_argument("--durations", type=int, nargs="+", help="explicit fixture durations; overrides --suite")
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--daemon", type=Path, help="reject a busy live daemon; cannot detect mid-request activity")
    parser.add_argument("--candidate", type=candidate, action="append", help="NAME:THREADS:BATCH[:ENCODER_BATCH[:WARMUP_0_OR_1]]")
    parser.add_argument("--candidate-runtime", type=override, action="append", default=[], metavar="NAME=PATH")
    parser.add_argument("--candidate-model", type=override, action="append", default=[], metavar="NAME=PATH")
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--repeats", type=int, default=3, help="requests per fixture per visit (default: 3)")
    parser.add_argument("--seed", type=int, default=1729)
    parser.add_argument("--max-wer", type=float, default=.05, help="maximum WER on every response, including first (default: 0.05)")
    parser.add_argument("--request-budget-ms", type=float, help="optional warm p95 HTTP budget per selected fixture; NOT end-to-end")
    parser.add_argument("--output", type=Path, required=True, help="new evidence directory")
    args = parser.parse_args()
    if args.rounds < 1 or args.repeats < 1 or not 0 <= args.max_wer <= 1:
        parser.error("rounds/repeats must be positive and max-wer between zero and one")
    if args.request_budget_ms is not None and not 0 < args.request_budget_ms < float("inf"):
        parser.error("request budget must be finite and positive")
    configs = args.candidate or [candidate("baseline:8:512:1:0")]
    names = {config["name"] for config in configs}
    if len(names) != len(configs):
        parser.error("candidate names must be unique")
    for config in configs:
        config.setdefault("encoder_max_batch", 1)
        config.setdefault("startup_warmup", False)
        config.update(runtime=args.runtime.expanduser().resolve(), model=args.model.expanduser().resolve())
    for kind in ("runtime", "model"):
        seen = set()
        for name, path in getattr(args, "candidate_" + kind):
            if name not in names or name in seen:
                parser.error(f"unknown or duplicate {kind} override: {name}")
            seen.add(name)
            next(config for config in configs if config["name"] == name)[kind] = path
        for config in configs:
            if not config[kind].is_file():
                parser.error(f"{kind} file missing: {config[kind]}")
    if args.daemon:
        args.daemon = args.daemon.expanduser().resolve()
        if not args.daemon.is_file():
            parser.error(f"daemon CLI missing: {args.daemon}")
    args.corpus = args.corpus.expanduser().resolve()
    manifest = load_corpus(args.corpus)
    fixtures = [fixture for fixture in manifest["fixtures"]
                if (fixture["target_seconds"] in args.durations if args.durations else
                    args.suite == "all" or (fixture["target_seconds"] <= 60 if args.suite == "typical" else fixture["target_seconds"] > 60))]
    if not fixtures or (args.durations and set(args.durations) != {f["target_seconds"] for f in fixtures}):
        parser.error("no fixtures selected, or requested duration missing from corpus")
    check_daemon(args.daemon)
    args.output = args.output.expanduser().resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    fingerprint_cache = {}
    def fingerprint(path):
        if path not in fingerprint_cache:
            print(f"Hashing {path.name}", flush=True)
            fingerprint_cache[path] = {**identity(path), "sha256": sha256(path)}
        return fingerprint_cache[path]
    report = {
        "schema_version": 1, "state": "running",
        "created_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "method": "alternating candidate visits; fresh server per visit; first request excluded; identical seeded shuffled warm schedule per paired round",
        "scope": "HTTP upload + inference + response/JSON; excludes capture, finalization, paste, model load; no production-weighted or end-to-end p95",
        "percentile_method": "nearest rank; per fixture; fewer than 100 warm samples flagged as screening only",
        "corpus": {"path": str(args.corpus), "manifest_sha256": sha256(args.corpus / "manifest.json"),
                   "manifest": manifest, "selected": [fixture["id"] for fixture in fixtures]},
        "environment": {"os": platform.platform(), "python": platform.python_version(),
                        "chip": command_output("sysctl", "-n", "machdep.cpu.brand_string"),
                        "memory_bytes": command_output("sysctl", "-n", "hw.memsize"),
                        "gpu": gpu_info(), "power": command_output("pmset", "-g", "batt"),
                        "power_modes": command_output("pmset", "-g", "custom")},
        "script_sha256": sha256(Path(__file__)), "scoring_sha256": sha256(Path(__file__).with_name("speech_corpus.py")),
        "rounds": args.rounds, "repeats": args.repeats, "seed": args.seed,
        "daemon_guard": str(args.daemon) if args.daemon else None,
        "accuracy_metric": "word Levenshtein; NFKC/lowercase; punctuation ignored, internal apostrophes retained; no spelling or number aliases",
        "max_wer": args.max_wer, "request_budget_ms": args.request_budget_ms,
        "candidates": [{**{key: value for key, value in config.items() if key not in ("runtime", "model")},
                        "runtime": fingerprint(config["runtime"]), "model": fingerprint(config["model"]),
                        "encoders": [fingerprint(path) for path in sorted(config["model"].parent.glob("*Encoder*.gguf"))]}
                       for config in configs], "visits": [], "records": [],
    }
    destination = args.output / "results.json"
    def save():
        report["summary"] = summarize(report["records"], configs, fixtures, args.max_wer, args.request_budget_ms)
        report["comparisons"] = compare(report["records"], configs, fixtures)
        temporary = destination.with_suffix(".tmp")
        temporary.write_text(json.dumps(report, indent=2) + "\n")
        temporary.replace(destination)
    save()
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt()
    signal.signal(signal.SIGTERM, interrupted)
    try:
        with tempfile.TemporaryDirectory(prefix="nemo-corpus-run-") as directory:
            audio = prepare_audio(args.corpus, fixtures, Path(directory))
            payloads = {name: multipart(path.read_bytes()) for name, path in audio.items()}
            references = {fixture["id"]: local_file(args.corpus, fixture["text"]).read_text() for fixture in fixtures}
            for round_number in range(1, args.rounds + 1):
                schedule = [(fixture["id"], repeat) for repeat in range(1, args.repeats + 1) for fixture in fixtures]
                random.Random(args.seed + round_number).shuffle(schedule)
                order = configs if round_number % 2 else list(reversed(configs))
                for config in order:
                    visit(args, config, round_number, fixtures, payloads, references, schedule, report, save)
        report["state"] = "complete"
    except KeyboardInterrupt:
        report["state"] = "interrupted"
    except Exception as error:
        report["state"] = "error"
        report["error"] = f"{type(error).__name__}: {error}"
    finally:
        save()
    print("\ncandidate | fixture | n | HTTP p50 / p95 ms | worst WER")
    for name, per_fixture in report["summary"].items():
        for fixture_id, stats in per_fixture.items():
            latency = stats["request_ms"]
            if latency:
                print(f"{name} | {fixture_id} | {latency['n']} | {latency['p50']:.1f} / {latency['p95']:.1f} | {100 * stats['worst_wer']:.2f}%")
    print(f"Evidence: {destination}")
    if args.rounds * args.repeats < 100:
        print("Screening only: fewer than 100 warm samples per fixture; do not treat p95 as a stable tail estimate.")
    if report["state"] == "interrupted":
        return 130
    failed = report["state"] != "complete" or any(row["status"] != "ok" for row in report["records"])
    failed |= any(stats["latency_budget_pass"] is False for group in report["summary"].values() for stats in group.values())
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
