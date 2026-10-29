#!/usr/bin/env python3
"""Compare Nemo's cold and warm HTTP transcription latency on one WAV fixture."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import secrets
import signal
import socket
import statistics
import subprocess
import sys
import time
import urllib.error
import urllib.request
import wave


def candidate(value):
    try:
        parts = value.split(":")
        if not 3 <= len(parts) <= 5:
            raise ValueError()
        name, threads, batch = parts[:3]
        threads, batch = int(threads), int(batch)
        if not re.fullmatch(r"[A-Za-z0-9_-]+", name) or threads < 0 or batch < 1:
            raise ValueError()
        result = {"name": name, "inference_threads": threads, "n_batch": batch}
        if len(parts) >= 4:
            encoder_batch = int(parts[3])
            if not 1 <= encoder_batch <= 16:
                raise ValueError()
            result["encoder_max_batch"] = encoder_batch
        if len(parts) == 5:
            if parts[4] not in ("0", "1"):
                raise ValueError()
            result["startup_warmup"] = parts[4] == "1"
        return result
    except ValueError:
        raise argparse.ArgumentTypeError("use NAME:INFERENCE_THREADS:N_BATCH[:ENCODER_BATCH[:WARMUP_0_OR_1]]; encoder batch must be 1–16")


def normalize(text):
    return re.findall(r"[a-z0-9]+", text.lower())


def command_output(*command):
    try:
        return subprocess.check_output(command, text=True, stderr=subprocess.DEVNULL, timeout=10).strip()
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired):
        return None


def gpu_info():
    raw = command_output("/usr/sbin/system_profiler", "SPDisplaysDataType", "-json")
    try:
        displays = json.loads(raw or "{}").get("SPDisplaysDataType", [])
        keys = ("sppci_model", "spdisplays_vendor", "spdisplays_cores", "spdisplays_metal")
        return [{key: display[key] for key in keys if key in display} for display in displays]
    except (ValueError, TypeError):
        return None


def check_daemon(path):
    if path is None:
        return None
    result = subprocess.run([str(path), "status"], capture_output=True, text=True, timeout=5)
    if result.returncode:
        raise RuntimeError("cannot check daemon status: " + (result.stderr.strip() or str(result.returncode)))
    status = result.stdout.strip()
    if not status:
        raise RuntimeError("daemon returned an empty status")
    if status.startswith(("Recording", "Opening microphone", "Transcribing", "Loading model",
                          "Downloading model", "Canceling")):
        raise RuntimeError("daemon is busy: " + status)
    return status


def identity(path):
    stat = path.stat()
    return {"path": str(path), "bytes": stat.st_size, "mtime_ns": stat.st_mtime_ns}


def multipart(audio):
    boundary = "nemo-benchmark-" + secrets.token_hex(12)
    body = (
        f"--{boundary}\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\njson\r\n"
        f"--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"fixture.wav\"\r\n"
        "Content-Type: audio/wav\r\n\r\n"
    ).encode() + audio + f"\r\n--{boundary}--\r\n".encode()
    return body, "multipart/form-data; boundary=" + boundary


def stop(process):
    interrupted = False
    deadline = time.monotonic() + 10
    while process.poll() is None:
        try:
            process.terminate()
            try:
                process.wait(timeout=max(0.1, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        except KeyboardInterrupt:
            interrupted = True
    return interrupted


def visit(args, config, round_number, body, content_type, opener):
    record = {"round": round_number, "candidate": config["name"], "status": "error"}
    log_path = args.output / f"{round_number:02d}-{config['name']}.log"
    record["log"] = str(log_path)
    key = secrets.token_hex(24)
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    base = f"http://127.0.0.1:{port}"
    command = [
        str(args.runtime), "serve", "--asr-model", str(args.model),
        "--host", "127.0.0.1", "--port", str(port), "--api-key", key,
        "--device", "metal", "--threads", "4", "--no-ui",
        "--asr.backend.threads", str(config["inference_threads"]),
        "--asr.salm.max_sequences", "1", "--asr.salm.n_ctx_per_sequence", "8192",
        "--asr.salm.max_context", "8192", "--asr.salm.n_batch", str(config["n_batch"]),
        "--asr.salm.encoder_max_batch", str(config["encoder_max_batch"]), "--asr.salm.max_new_tokens", "4096",
        "--asr.salm.mtp_draft", "0", "--write-timeout", "300", "--max-upload-mb", "64",
    ]
    if not config["startup_warmup"]:
        command.append("--no-warmup")
    env = os.environ.copy()
    env.pop("NEMO_SPEECH_TIMING", None)
    if args.profile:
        env["NEMO_SPEECH_TIMING"] = "1"
    started = time.perf_counter()
    process = None
    sampler = None

    def interrupted():
        record["status"] = "interrupted"
        record["error"] = "Interrupted by signal"

    try:
        record["daemon_before_visit"] = check_daemon(args.daemon)
        with log_path.open("wb") as log:
            started = time.perf_counter()
            process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=log, stderr=log, env=env)
            while True:
                if process.poll() is not None:
                    raise RuntimeError(f"runtime exited with status {process.returncode}; see {log_path}")
                if time.perf_counter() - started > 120:
                    raise TimeoutError("model readiness exceeded 120 seconds")
                try:
                    with opener.open(base + "/health", timeout=0.5) as response:
                        if response.status == 200:
                            break
                except (urllib.error.URLError, TimeoutError):
                    pass
                time.sleep(0.1)
            record["load_ms"] = 1000 * (time.perf_counter() - started)
            if args.profile:
                sample_path = args.output / "cpu-sample.txt"
                record["cpu_sample"] = {
                    "path": str(sample_path), "duration_seconds": 5, "interval_ms": 1,
                    "window": "starts after health; covers first/warm requests and may include idle time",
                }
                try:
                    sampler = subprocess.Popen(
                        ["/usr/bin/sample", str(process.pid), "5", "1", "-file", str(sample_path)],
                        stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                    )
                except OSError as error:
                    record["cpu_sample"]["error"] = str(error)
            for stage in ("first", "warm"):
                record["daemon_before_" + stage] = check_daemon(args.daemon)
                request = urllib.request.Request(
                    base + "/v1/audio/transcriptions", data=body,
                    headers={"Authorization": "Bearer " + key, "Content-Type": content_type},
                )
                request_started = time.perf_counter()
                try:
                    with opener.open(request, timeout=300) as response:
                        result = json.load(response)
                finally:
                    record[stage + "_ms"] = 1000 * (time.perf_counter() - request_started)
                text = result["text"].strip()
                record[stage + "_text"] = text
                record[stage + "_correct"] = normalize(text) == normalize(args.expected)
                if not record[stage + "_correct"]:
                    raise RuntimeError(f"{stage} transcript differs from expected text")
            record["cold_total_ms"] = record["load_ms"] + record["first_ms"]
            record["status"] = "ok"
    except KeyboardInterrupt:
        interrupted()
    except Exception as error:
        record["error"] = f"{type(error).__name__}: {error}".replace(key, "[redacted]")
    finally:
        if sampler is not None:
            try:
                sampler.wait(timeout=15)
            except subprocess.TimeoutExpired:
                record["cpu_sample"]["error"] = "CPU sampler exceeded its 15-second cleanup deadline"
            except KeyboardInterrupt:
                interrupted()
            if stop(sampler):
                interrupted()
            record["cpu_sample"]["returncode"] = sampler.returncode
        if process is not None:
            if stop(process):
                interrupted()
        record["visit_ms"] = 1000 * (time.perf_counter() - started)
    return record


def summary(records):
    result = {}
    for name in dict.fromkeys(record["candidate"] for record in records):
        samples = [record for record in records if record["candidate"] == name]
        successful = [record for record in samples if record["status"] == "ok"]
        checks = [record[stage + "_correct"] for record in samples
                  for stage in ("first", "warm") if stage + "_correct" in record]
        stats = {"errors": len(samples) - len(successful), "transcript_checks": len(checks),
                 "correct_transcripts": sum(checks)}
        for stage in ("load_ms", "first_ms", "warm_ms", "cold_total_ms"):
            values = [record[stage] for record in successful]
            if values:
                stats[stage] = {"n": len(values), "median": statistics.median(values),
                                "min": min(values), "max": max(values)}
        result[name] = stats
    return result


def main():
    def handle_termination(_signal, _frame):
        raise KeyboardInterrupt()

    signal.signal(signal.SIGTERM, handle_termination)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("audio", type=Path)
    parser.add_argument("--expected", required=True, help="known transcript; punctuation/case ignored")
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--daemon", type=Path, help="Nemo CLI path; reject visits when its daemon is busy")
    parser.add_argument("--output", type=Path, required=True, help="new directory for JSON and runtime logs")
    parser.add_argument("--candidate", type=candidate, action="append", help="NAME:INFERENCE_THREADS:N_BATCH[:ENCODER_BATCH[:WARMUP_0_OR_1]]; omitted options use global flags")
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--warmup", action="store_true", help="enable runtime startup warmup (default: disabled, matching Nemo)")
    parser.add_argument("--encoder-batch", type=int, default=1)
    parser.add_argument("--profile", action="store_true", help="separate one-visit timing profile; excluded from comparisons")
    args = parser.parse_args()
    configs = args.candidate or [candidate("baseline:0:512"), candidate("cpu4:4:512"), candidate("cpu6:6:512")]
    if args.rounds < 1 or len({config["name"] for config in configs}) != len(configs):
        parser.error("rounds must be positive and candidate names unique")
    if args.profile and len(configs) != 1:
        parser.error("--profile requires exactly one --candidate")
    if not 1 <= args.encoder_batch <= 16 or not normalize(args.expected):
        parser.error("encoder-batch must be 1–16 and expected text must contain words")
    for config in configs:
        config.setdefault("encoder_max_batch", args.encoder_batch)
        config.setdefault("startup_warmup", args.warmup)
    for field in ("audio", "runtime", "model") + (("daemon",) if args.daemon else ()):
        path = getattr(args, field).expanduser().resolve()
        if not path.is_file():
            parser.error(f"{field} file missing: {path}")
        setattr(args, field, path)
    args.output = args.output.expanduser().resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    audio = args.audio.read_bytes()
    if len(audio) > 64 * 1024 * 1024:
        parser.error("WAV exceeds 64 MiB")
    with wave.open(str(args.audio), "rb") as wav:
        seconds = wav.getnframes() / wav.getframerate()
        if not 0 < seconds <= 300:
            parser.error("WAV duration must be 0–300 seconds")
    report = {
        "profile": args.profile,
        "method": "alternate candidate order; fresh process per visit; load-to-health, first request, second (warm) request; OS file cache uncontrolled",
        "created_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "environment": {"os": platform.platform(), "machine": platform.machine(),
                        "chip": command_output("sysctl", "-n", "machdep.cpu.brand_string"),
                        "memory_bytes": command_output("sysctl", "-n", "hw.memsize"),
                        "physical_cpu_cores": command_output("sysctl", "-n", "hw.physicalcpu"),
                        "logical_cpu_cores": command_output("sysctl", "-n", "hw.logicalcpu"),
                        "gpu": gpu_info(), "power": command_output("pmset", "-g", "batt"),
                        "power_modes": command_output("pmset", "-g", "custom")},
        "runtime": identity(args.runtime), "model": identity(args.model),
        "audio": {**identity(args.audio), "sha256": hashlib.sha256(audio).hexdigest(), "seconds": seconds},
        "expected": args.expected, "candidates": configs,
        "runtime_options": {"startup_warmup": args.warmup, "encoder_max_batch": args.encoder_batch,
                            "http_threads": 4, "max_sequences": 1, "context": 8192, "mtp_draft": 0},
        "records": [],
    }
    report["runtime"]["sha256"] = hashlib.sha256(args.runtime.read_bytes()).hexdigest()
    body, content_type = multipart(audio)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    destination = args.output / "results.json"

    def save():
        if not args.profile:
            report["summary"] = summary(report["records"])
        temporary = destination.with_suffix(".tmp")
        temporary.write_text(json.dumps(report, indent=2) + "\n")
        temporary.replace(destination)

    save()
    try:
        for round_number in range(1, (1 if args.profile else args.rounds) + 1):
            order = configs if round_number % 2 else list(reversed(configs))
            for config in order:
                record = visit(args, config, round_number, body, content_type, opener)
                report["records"].append(record)
                save()
                if record["status"] == "interrupted":
                    report["interrupted"] = True
                    save()
                    return 130
                print(f"round {round_number} {config['name']}: {record['status']} "
                      f"load={record.get('load_ms', 0):.1f}ms first={record.get('first_ms', 0):.1f}ms "
                      f"warm={record.get('warm_ms', 0):.1f}ms", flush=True)
    except KeyboardInterrupt:
        report["interrupted"] = True
        save()
        return 130
    print(json.dumps(report.get("summary", {"profile": "see runtime log"}), indent=2))
    print(f"Evidence: {destination}")
    return int(any(record["status"] != "ok" for record in report["records"]))


if __name__ == "__main__":
    sys.exit(main())
