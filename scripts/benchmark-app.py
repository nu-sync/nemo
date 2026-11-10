#!/usr/bin/env python3
"""Compare inference engines through the app's own path: config -> engine -> helper -> HTTP.

`benchmark-corpus.py` drives an engine's HTTP contract directly. This drives `nemo
transcribe`, so the engine is chosen by `config.toml`, launched by the app's `Backend`
under the app's sandbox, and timed by the app's own `transcription` event. Each engine
gets an isolated data directory whose only shared state is a link to the model store.

A visit is one engine in one round: process-cold requests (a fresh `nemo` per file, so
each pays a model load) and then one process serving a primer plus a shuffled warm
schedule. Engines alternate order across rounds. Nothing is installed or reconfigured.
"""

import argparse
import ctypes
import json
import os
from pathlib import Path
import platform
import random
import statistics
import subprocess
import sys
import threading
import time

from benchmark import check_daemon, command_output, gpu_info, stop
from speech_corpus import DEFAULT_CORPUS, load_corpus, local_file, prepare_audio, sha256, word_errors

ENGINES = ("nemo-speech", "mlx-audio")
RUSAGE_INFO_V4 = 4
MAX_REPEAT_SPREAD = 1.25  # slowest over fastest warm repeat of one engine on one clip
PHYS_FOOTPRINT, LIFETIME_MAX_PHYS_FOOTPRINT = 7, 28  # u64 slots after the 16-byte uuid


def footprint(pid):
    """(current, lifetime peak) phys_footprint in bytes: what Activity Monitor calls Memory.

    Resident size is not used: Metal allocations are not resident pages of the process,
    so RSS reports about 4 GB for an engine that holds more than 20 GB.
    """
    buffer = (ctypes.c_uint64 * 64)()
    libc = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
    if libc.proc_pid_rusage(pid, RUSAGE_INFO_V4, ctypes.byref(buffer)) != 0:
        return None
    return buffer[2 + PHYS_FOOTPRINT], buffer[2 + LIFETIME_MAX_PHYS_FOOTPRINT]


def engine_pid(parent):
    """The helper the app spawned. `sandbox-exec` and the MLX runner both exec, so the
    engine is the direct child."""
    children = (command_output("/usr/bin/pgrep", "-P", str(parent)) or "").split()
    return int(children[0]) if children else None


class MemoryWatch(threading.Thread):
    """Sample the engine's footprint twice a second; one syscall, no effect on timing."""

    def __init__(self, parent):
        super().__init__(daemon=True)
        self.parent, self.done = parent, threading.Event()
        self.first = self.last = self.peak = None

    def run(self):
        while not self.done.wait(.5):
            pid = engine_pid(self.parent)
            sample = pid and footprint(pid)
            if sample:
                self.first = self.first or sample[0]
                self.last, self.peak = sample[0], max(self.peak or 0, sample[1])

    def result(self):
        self.done.set()
        self.join()
        return {"engine_footprint_after_first_sample": self.first,
                "engine_footprint_last_sample": self.last, "engine_footprint_peak": self.peak}


def events(stderr):
    found = []
    for line in stderr.splitlines():
        if line.startswith("{"):
            try:
                found.append(json.loads(line))
            except ValueError:
                pass
    return found


def competing(args):
    """Whether a process the caller named as a GPU competitor is running right now."""
    return bool(args.competitor and command_output("/usr/bin/pgrep", "-f", args.competitor))


class EngineFailure(Exception):
    """`nemo transcribe` exited nonzero: the engine failed a load or a request."""


def undisturbed(args, report, run):
    """Run one process while the live daemon stays idle, repeating it otherwise.

    A dictation runs a second engine on the same GPU, and so does any other GPU job. The
    daemon and the competitor are never stopped: the measurement waits for them, and a
    process during which either was seen is discarded whole, not kept as a slow sample.
    A competitor that starts and finishes inside one process is not seen here; the
    repeat-spread check in `decide` is what catches that.
    """
    for _ in range(6):
        deadline = time.monotonic() + 600
        while True:
            try:
                before = check_daemon(args.daemon)
                if competing(args):
                    raise RuntimeError("busy: a competing GPU process is running")
                break
            except RuntimeError as busy:
                if "busy" not in str(busy) or time.monotonic() > deadline:
                    raise
                time.sleep(5)
        try:
            result = run()
        except EngineFailure as failure:
            # Kept as a finding against that engine, never dropped: the process is repeated
            # so the latency comparison stays complete, and the count blocks a candidate.
            report["failures"].append({**failure.args[0], "daemon_before": before,
                                       "competitor": competing(args)})
            print(f"engine failure recorded: {failure.args[0]['error'][-160:]}", flush=True)
            continue
        try:
            after = check_daemon(args.daemon)
        except RuntimeError as busy:
            after = str(busy)
        if competing(args):
            after = "a competing GPU process started"
        report["daemon"].append({"before": before, "after": after, "kept": before == after})
        if before == after:
            return result
        print(f"discarded one process: daemon went from {before!r} to {after!r}", flush=True)
    raise RuntimeError("six consecutive attempts were disturbed or failed")


def run_nemo(args, engine, files):
    """One `nemo transcribe` process. Returns its per-file rows and load record."""
    environment = {"HOME": os.environ["HOME"], "PATH": "/usr/bin:/bin",
                   "NEMO_DATA_DIR": str(args.output / "data" / engine)}
    started = time.perf_counter()
    process = subprocess.Popen([str(args.nemo), "transcribe", "--json", *map(str, files)],
                               stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, text=True, env=environment)
    watch = MemoryWatch(process.pid)
    watch.start()
    try:
        stdout, stderr = process.communicate(timeout=1800)
    finally:
        memory = watch.result()
        stop(process)
    wall_ms = 1000 * (time.perf_counter() - started)
    logged = events(stderr)
    if process.returncode:
        served = [event for event in logged if event.get("event") == "transcription"]
        raise EngineFailure({"engine": engine, "files": len(files), "served_before_failure": len(served) - 1,
                             "failed_audio_seconds": served[-1]["audio_seconds"] if served else None,
                             "error": stderr.strip().splitlines()[-1] if stderr.strip() else str(process.returncode)})
    demoted = [event for event in logged if event.get("event") == "backend_demoted"]
    if demoted:
        raise RuntimeError(f"{engine} was demoted, so another engine served: {demoted[0].get('reason')}")
    texts = [json.loads(line)["text"] for line in stdout.splitlines() if line.startswith("{")]
    timings = [event for event in logged if event.get("event") == "transcription"]
    loads = [event for event in logged if event.get("event") == "model_load"]
    if not (len(texts) == len(timings) == len(files)) or len(loads) != 1:
        raise RuntimeError(f"expected {len(files)} transcripts, got {len(texts)} texts, "
                           f"{len(timings)} timings, {len(loads)} loads")
    for timing in timings:
        if timing["engine"] != engine or not timing["success"]:
            raise RuntimeError(f"request was not served by {engine}: {timing}")
    return texts, timings, loads[0], memory, wall_ms


def visit(args, engine, round_number, cold, schedule, primer, wavs, references, report, save):
    def row(stage, fixture, repeat, text, timing):
        accuracy = word_errors(references[fixture], text)
        record = {"engine": engine, "round": round_number, "stage": stage, "fixture": fixture,
                  "repeat": repeat, "text": text, "accuracy": accuracy,
                  "request_ms": timing["request_ms"], "model_load_ms": timing["model_load_ms"],
                  "total_ms": timing["total_ms"], "reused_model": timing["reused_model"]}
        report["records"].append(record)
        print(f"round={round_number} {engine} {stage} {fixture} request={timing['request_ms']:.1f}ms "
              f"total={timing['total_ms']:.1f}ms WER={100 * accuracy['wer']:.2f}%", flush=True)
        return record

    for fixture in cold:
        texts, timings, load, memory, wall_ms = undisturbed(
            args, report, lambda: run_nemo(args, engine, [wavs[fixture]]))
        if timings[0]["reused_model"]:
            raise RuntimeError("a process-cold request reused a model")
        row("cold", fixture, 0, texts[0], timings[0]).update(
            load_ms=load["load_ms"], process_wall_ms=wall_ms, **memory)
        report["identities"][engine] = load["identity"]
        save()
    order = [(primer, 0)] + schedule
    texts, timings, load, memory, wall_ms = undisturbed(
        args, report, lambda: run_nemo(args, engine, [wavs[name] for name, _ in order]))
    for index, ((fixture, repeat), text, timing) in enumerate(zip(order, texts, timings)):
        if bool(index) != timing["reused_model"]:
            raise RuntimeError(f"request {index} reused_model={timing['reused_model']}")
        row("warm" if index else "primer", fixture, repeat, text, timing)
    report["warm_processes"].append({"engine": engine, "round": round_number, "requests": len(order),
                                     "load_ms": load["load_ms"], "process_wall_ms": wall_ms, **memory})
    save()


def spread(values):
    values = sorted(values)
    if not values:
        return None
    return {"n": len(values), "median": statistics.median(values), "min": values[0], "max": values[-1]}


def summarize(records, processes, fixtures, baseline, candidate):
    """Per-bucket medians with min-max, the paired speedup, and accuracy per engine."""
    def select(engine, fixture, stage, key):
        return [record[key] for record in records
                if (record["engine"], record["fixture"], record["stage"]) == (engine, fixture, stage)]

    buckets = []
    for fixture in fixtures:
        name = fixture["id"]
        bucket = {"fixture": name, "seconds": fixture["target_seconds"], "engines": {}}
        for engine in (baseline, candidate):
            answers = [record for record in records
                       if record["engine"] == engine and record["fixture"] == name]
            warm, cold = select(engine, name, "warm", "request_ms"), select(engine, name, "cold", "total_ms")
            errors = sorted({record["accuracy"]["errors"] for record in answers})
            bucket["engines"][engine] = {
                "warm_request_ms": spread(warm), "cold_total_ms": spread(cold),
                "cold_request_ms": spread(select(engine, name, "cold", "request_ms")),
                "cold_load_ms": spread(select(engine, name, "cold", "load_ms")),
                "word_errors": errors, "reference_words": answers[0]["accuracy"]["reference_words"],
                "distinct_transcripts": len({record["text"] for record in answers}),
                "responses": len(answers)}
        old, new = bucket["engines"][baseline], bucket["engines"][candidate]
        bucket["warm_speedup"] = old["warm_request_ms"]["median"] / new["warm_request_ms"]["median"]
        if old["cold_total_ms"]:
            bucket["cold_speedup"] = old["cold_total_ms"]["median"] / new["cold_total_ms"]["median"]
        bucket["accuracy"] = ("unchanged" if old["word_errors"] == new["word_errors"] else
                              "better" if max(new["word_errors"]) < min(old["word_errors"]) else
                              "worse" if min(new["word_errors"]) > max(old["word_errors"]) else "mixed")
        buckets.append(bucket)
    memory = {}
    for engine in (baseline, candidate):
        mine = [process for process in processes if process["engine"] == engine]
        memory[engine] = {key: spread([process[key] for process in mine if process[key]])
                          for key in ("engine_footprint_after_first_sample", "engine_footprint_last_sample",
                                      "engine_footprint_peak", "load_ms")}
    return {"baseline": baseline, "candidate": candidate, "buckets": buckets, "warm_process": memory}


def decide(summary, minimum_speedup):
    """The promotion gate, exactly as stated before the run: accuracy on every bucket,
    and the median over buckets of the warm speedup."""
    speedups = [bucket["warm_speedup"] for bucket in summary["buckets"]]
    changed = [bucket["fixture"] for bucket in summary["buckets"] if bucket["accuracy"] != "unchanged"]
    lost = [bucket["fixture"] for bucket in summary["buckets"] if bucket["accuracy"] in ("worse", "mixed")]
    median = statistics.median(speedups)
    # The same engine on the same clip should repeat within a few percent. A wide spread
    # means something else used the GPU, and no ratio from such a run is worth acting on.
    spreads = {f"{bucket['fixture']}/{engine}": numbers["warm_request_ms"]["max"] / numbers["warm_request_ms"]["min"]
               for bucket in summary["buckets"] for engine, numbers in bucket["engines"].items()
               if numbers["warm_request_ms"]["n"] >= 10}
    noisiest = max(spreads, key=spreads.get) if spreads else None
    quiet = not spreads or spreads[noisiest] <= MAX_REPEAT_SPREAD
    return {"minimum_speedup": minimum_speedup, "quiet_machine": quiet,
            "widest_repeat_spread": {"case": noisiest, "max_over_min": spreads.get(noisiest)}, "median_bucket_warm_speedup": median,
            "min_bucket_warm_speedup": min(speedups), "max_bucket_warm_speedup": max(speedups),
            "speed_gate": median >= minimum_speedup,
            "buckets_with_changed_accuracy": changed, "buckets_with_lost_accuracy": lost,
            "accuracy_unchanged_everywhere": not changed, "accuracy_not_worse_anywhere": not lost,
            "promote": quiet and median >= minimum_speedup and not changed}


def write_config(args, engine):
    data = args.output / "data" / engine
    data.mkdir(parents=True)
    (data / "models").symlink_to(args.models, target_is_directory=True)
    runtime = args.nemo.parent.parent / "Resources/runtime/nemo-speech"
    lines = [f"model = {json.dumps(str(args.model))}", f"runtime = {json.dumps(str(runtime))}",
             f"backend = {json.dumps(engine)}", "idle_unload_seconds = 300",
             "max_recording_seconds = 120", "paste = false", 'hotkey = "RightOption"']
    if engine == "nemo-speech":
        lines.append(f"inference_threads = {args.inference_threads}")
    (data / "config.toml").write_text("\n".join(lines) + "\n")
    return data / "config.toml"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--nemo", type=Path, required=True, help="the app bundle's Contents/MacOS/nemo")
    parser.add_argument("--models", type=Path, required=True, help="the provisioned model store directory")
    parser.add_argument("--model", type=Path, required=True, help="decoder GGUF inside --models")
    parser.add_argument("--daemon", type=Path, help="reject a busy live daemon at process boundaries")
    parser.add_argument("--competitor", help="pgrep -f pattern of another GPU workload to wait out")
    parser.add_argument("--corpus", type=Path, default=DEFAULT_CORPUS)
    parser.add_argument("--durations", type=int, nargs="+", help="warm fixtures (default: all)")
    parser.add_argument("--cold-durations", type=int, nargs="+", default=[1, 5, 30, 60])
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--repeats", type=int, default=3, help="warm requests per fixture per visit")
    parser.add_argument("--long-repeats", type=int, default=1, help="warm requests for fixtures over 60 s")
    parser.add_argument("--inference-threads", type=int, default=8)
    parser.add_argument("--minimum-speedup", type=float, default=1.4)
    parser.add_argument("--seed", type=int, default=1729)
    parser.add_argument("--output", type=Path, required=True, help="evidence directory; data/ must not exist")
    args = parser.parse_args()
    args.nemo, args.models, args.model = (path.expanduser().resolve() for path in (args.nemo, args.models, args.model))
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    manifest = load_corpus(args.corpus)
    fixtures = sorted((fixture for fixture in manifest["fixtures"]
                       if not args.durations or fixture["target_seconds"] in args.durations),
                      key=lambda fixture: fixture["target_seconds"])
    by_seconds = {fixture["target_seconds"]: fixture["id"] for fixture in fixtures}
    cold = [by_seconds[seconds] for seconds in args.cold_durations if seconds in by_seconds]
    primer = min(fixtures, key=lambda fixture: abs(fixture["target_seconds"] - 5))["id"]
    references = {fixture["id"]: local_file(args.corpus, fixture["text"]).read_text() for fixture in fixtures}
    wav_directory = args.output / "wav"
    wav_directory.mkdir()
    wavs = prepare_audio(args.corpus, fixtures, wav_directory)
    runtime = args.nemo.parent.parent / "Resources/runtime"
    report = {
        "schema_version": 1, "state": "running", "started": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "arguments": {key: str(value) if isinstance(value, Path) else value for key, value in vars(args).items()},
        "machine": {"model": command_output("/usr/sbin/sysctl", "-n", "hw.model"),
                    "chip": command_output("/usr/sbin/sysctl", "-n", "machdep.cpu.brand_string"),
                    "memory_bytes": int(command_output("/usr/sbin/sysctl", "-n", "hw.memsize") or 0),
                    "macos": platform.mac_ver()[0], "gpu": gpu_info(),
                    "power": command_output("/usr/bin/pmset", "-g", "batt"),
                    "power_settings": command_output("/usr/bin/pmset", "-g")},
        "artifacts": {str(path.relative_to(args.nemo.parent.parent)): sha256(path) for path in
                      [args.nemo, runtime / "nemo-speech", runtime / "mlx-serve.py",
                       runtime / "mlx-serve.py.lock", runtime / "mlx-runner.sh"]},
        "corpus": {"fixtures": [fixture["id"] for fixture in fixtures], "manifest_sha256": sha256(args.corpus / "manifest.json")},
        "configs": {engine: write_config(args, engine).read_text() for engine in ENGINES},
        "identities": {}, "daemon": [], "failures": [], "records": [], "warm_processes": [],
    }

    def save():
        temporary = args.output / "results.json.tmp"
        temporary.write_text(json.dumps(report, indent=1))
        temporary.replace(args.output / "results.json")

    save()
    try:
        for round_number in range(1, args.rounds + 1):
            schedule = [(fixture["id"], repeat) for fixture in fixtures
                        for repeat in range(1, 1 + (args.repeats if fixture["target_seconds"] <= 60 else args.long_repeats))]
            random.Random(args.seed + round_number).shuffle(schedule)
            for engine in (ENGINES if round_number % 2 else ENGINES[::-1]):
                visit(args, engine, round_number, cold, schedule, primer, wavs, references, report, save)
        report["summary"] = summarize(report["records"], report["warm_processes"], fixtures, *ENGINES)
        report["decision"] = decide(report["summary"], args.minimum_speedup)
        candidate_failures = sum(failure["engine"] == ENGINES[1] for failure in report["failures"])
        report["decision"]["engine_failures"] = {engine: sum(failure["engine"] == engine for failure in report["failures"])
                                                 for engine in ENGINES}
        report["decision"]["promote"] = report["decision"]["promote"] and not candidate_failures
        report["state"] = "complete"
    except BaseException as error:
        report["state"] = "interrupted" if isinstance(error, KeyboardInterrupt) else "failed"
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        report["finished"] = time.strftime("%Y-%m-%dT%H:%M:%S%z")
        report["power_after"] = command_output("/usr/bin/pmset", "-g", "batt")
        save()
    print(json.dumps(report["decision"], indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
