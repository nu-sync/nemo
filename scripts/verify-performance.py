#!/usr/bin/env python3
"""Verify preload, five-minute retention, cancellation, and idle unload in isolated daemons."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time

from nemo_status import control, lifecycle_status

ROOT = Path(__file__).resolve().parent.parent


def children(pid):
    result = subprocess.run(["/usr/bin/pgrep", "-P", str(pid)], capture_output=True, text=True)
    return [int(value) for value in result.stdout.split()]


def runtime_children(pid, runtime):
    snapshot = subprocess.run(["/bin/ps", "-A", "-o", "pid=,ppid=,comm="],
                              capture_output=True, text=True, check=True)
    matches = []
    for line in snapshot.stdout.splitlines():
        fields = line.strip().split(None, 2)
        if len(fields) == 3 and int(fields[1]) == pid and Path(fields[2]).resolve() == runtime:
            matches.append(int(fields[0]))
    return matches


def alive(pid):
    return subprocess.run(["/bin/ps", "-p", str(pid), "-o", "pid="], capture_output=True).returncode == 0


def wait_until(check, timeout, description):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError("Timed out: " + description)


def terminate(process):
    interrupted = False
    if process.poll() is None:
        process.terminate()
    deadline = time.monotonic() + 10
    while process.poll() is None:
        try:
            process.wait(timeout=max(0.1, deadline - time.monotonic()))
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
        except KeyboardInterrupt:
            interrupted = True
    return interrupted


def phase(args, name, idle_seconds, operation, preload=True):
    destination = args.output / name
    destination.mkdir()
    result = {"status": "error", "idle_unload_seconds": idle_seconds,
              "model_warmup": args.model_warmup, "preload_model": preload}
    process = None
    observed = set()
    with tempfile.TemporaryDirectory(prefix="nemo-perf-", dir="/tmp") as temporary:
        data = Path(temporary).resolve()
        config = (
            'hotkey = "Control+Shift+F18"\n'
            f'model = {json.dumps(str(args.model))}\n'
            f'runtime = {json.dumps(str(args.runtime))}\n'
            f'idle_unload_seconds = {idle_seconds}\n'
            f'preload_model = {str(preload).lower()}\n'
            f'model_warmup = {str(args.model_warmup).lower()}\n'
            'inference_threads = 8\nencoder_batch_size = 1\n'
            'max_recording_seconds = 120\npaste = false\n'
        )
        (data / "config.toml").write_text(config)
        env = {**os.environ, "NEMO_DATA_DIR": str(data)}
        env.pop("NEMO_SPEECH_TIMING", None)

        def cli(command, required=True):
            if command == "status":
                try:
                    return lifecycle_status(lambda action: control(data, action))
                except OSError:
                    if required:
                        raise
                    return None
            response = subprocess.run([str(args.binary), command], env=env,
                                      capture_output=True, text=True, timeout=5)
            if required and response.returncode:
                raise RuntimeError(response.stderr.strip() or f"{command} failed")
            return response.stdout.strip() if response.returncode == 0 else None

        def status(expected, timeout=120):
            started = time.monotonic()
            last = None

            def check():
                nonlocal last
                observed.update(children(process.pid))
                if process.poll() is not None:
                    raise RuntimeError(f"daemon exited with status {process.returncode}")
                last = cli("status", required=False)
                return last == expected

            try:
                wait_until(check, timeout, f"status {expected!r}")
            except AssertionError as error:
                raise AssertionError(f"{error}; last status {last!r}") from error
            return time.monotonic() - started

        def model_pid():
            current = children(process.pid)
            observed.update(current)
            runtimes = runtime_children(process.pid, args.runtime)
            observed.update(runtimes)
            assert len(runtimes) == 1, f"Expected one runtime child; got {runtimes}"
            return runtimes[0]

        def no_children():
            return not children(process.pid) and not any(alive(pid) for pid in observed)

        try:
            process = subprocess.Popen([str(args.binary), "run"], env=env,
                                       stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                       stderr=subprocess.DEVNULL, start_new_session=True)
            result["daemon_pid"] = process.pid
            operation(cli, status, model_pid, no_children, process, result, data)
            if result["status"] == "error":
                result["status"] = "passed"
        except KeyboardInterrupt:
            result["status"] = "interrupted"
            raise
        except Exception as error:
            result["error"] = f"{type(error).__name__}: {error}"
            raise
        finally:
            if process is not None:
                observed.update(children(process.pid))
                if terminate(process):
                    result["status"] = "interrupted"
                survivors = [pid for pid in observed if alive(pid)]
                if survivors:
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    wait_until(lambda: not any(alive(pid) for pid in survivors), 5,
                               "owned runtime processes stop")
                result["owned_children_reaped"] = not any(alive(pid) for pid in observed)
            for filename in ("config.toml", "daemon.log", "inference.log"):
                if (data / filename).exists():
                    shutil.copy2(data / filename, destination / filename)
            (destination / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    if result["status"] == "interrupted":
        raise KeyboardInterrupt()
    return result


def main():
    def interrupted(_signal, _frame):
        raise KeyboardInterrupt()

    signal.signal(signal.SIGTERM, interrupted)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/nemo")
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="new directory for evidence")
    parser.add_argument("--resident-seconds", type=float, default=65,
                        help="idle observation duration below 300 seconds; use >60 to cover the old timeout")
    parser.add_argument("--model-warmup", action="store_true",
                        help="enable runtime warmup (default: disabled, matching the selected profile)")
    parser.add_argument("--recording", action="store_true",
                        help="also verify model preparation during microphone capture; requires existing permission")
    parser.add_argument("--recording-only", action="store_true",
                        help="run only recording preparation checks; implies --recording")
    parser.add_argument("--full-idle-expiry-only", action="store_true",
                        help="observe the full 300-second idle expiry in one isolated daemon; no microphone or clipboard use")
    args = parser.parse_args()
    if args.full_idle_expiry_only and (args.recording or args.recording_only):
        parser.error("full-idle-expiry-only cannot be combined with recording checks")
    if args.full_idle_expiry_only:
        args.model_warmup = False
    args.recording = args.recording or args.recording_only
    if not 0 < args.resident_seconds < 300:
        parser.error("resident-seconds must be greater than zero and below the 300-second retention limit")
    for field in ("binary", "runtime", "model"):
        path = getattr(args, field).expanduser().resolve()
        if not path.is_file():
            parser.error(f"{field} missing: {path}")
        setattr(args, field, path)
    args.output = args.output.expanduser().resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    binary_sha256 = hashlib.sha256(args.binary.read_bytes()).hexdigest()
    report = {"binary": str(args.binary), "binary_sha256": binary_sha256,
              "runtime": str(args.runtime),
              "model": str(args.model),
              "resident_seconds": None if args.recording_only or args.full_idle_expiry_only else args.resident_seconds,
              "idle_unload_seconds": 300, "model_warmup": args.model_warmup,
              "recording_requested": args.recording,
              "recording_only": args.recording_only,
              "full_idle_expiry_only": args.full_idle_expiry_only,
              "covers_old_60_second_timeout": not (args.recording_only or args.full_idle_expiry_only) and args.resident_seconds > 60,
              "full_300_second_expiry_observed": False,
              "expiry_check": "Full 300-second observation requested; see idle_expiry phase evidence." if args.full_idle_expiry_only else
                              "Not run in recording-only mode." if args.recording_only else
                              "A separate one-second timeout exercises the positive idle-unload path.",
              "phases": {}}

    def cancel(cli, status, model_pid, no_children, process, result, data):
        clipboard = subprocess.check_output(["/usr/bin/pbpaste"])
        status("Loading model", 10)
        cli("cancel")
        status("Canceled", 10)
        wait_until(no_children, 5, "canceled startup releases its runtime")
        assert subprocess.check_output(["/usr/bin/pbpaste"]) == clipboard
        result["clipboard_unchanged"] = True

    def resident(cli, status, model_pid, no_children, process, result, data):
        result["preload_seconds"] = status("Model loaded")
        pid = model_pid()
        result["runtime_pid"] = pid
        result["rss_kib"] = int(subprocess.check_output(["/bin/ps", "-p", str(pid), "-o", "rss="]))
        print(f"Model loaded with five-minute retention; observing {args.resident_seconds:g} idle seconds", flush=True)
        deadline = time.monotonic() + args.resident_seconds
        while time.monotonic() < deadline:
            assert process.poll() is None and alive(pid), "resident runtime exited"
            time.sleep(min(0.2, max(0, deadline - time.monotonic())))
        assert children(process.pid) == [pid], "resident runtime changed"
        assert cli("status") == "Model loaded"
        cli("warm")
        time.sleep(0.1)
        status("Model loaded", 10)
        assert model_pid() == pid, "warm reloaded an already resident model"
        result["warm_reused_runtime"] = True
        process.terminate()
        assert process.wait(timeout=10) == 0
        wait_until(no_children, 5, "SIGTERM releases the resident runtime")
        result["sigterm_reaped_runtime"] = True

    def unload(cli, status, model_pid, no_children, process, result, data):
        result["preload_seconds"] = status("Model loaded")
        model_pid()
        result["unload_seconds"] = status("Ready · model unloaded", 10)
        wait_until(no_children, 5, "positive idle timeout releases runtime")

    def full_idle_expiry(cli, status, model_pid, no_children, process, result, data):
        result["preload_seconds"] = status("Model loaded")
        ready_observed = time.monotonic()
        result["ready_observed_monotonic"] = ready_observed
        pid = model_pid()
        result["runtime_pid"] = pid
        result["timing_basis"] = "Monotonic time since the first Model loaded status observation; the worker idle timer starts shortly before that observation."
        result["poll_interval_seconds"] = 1
        result["last_present_seconds"] = 0.0
        result["samples"] = []
        for second in range(1, 306):
            time.sleep(max(0, ready_observed + second - time.monotonic()))
            assert process.poll() is None, "daemon exited during idle expiry observation"
            current = runtime_children(process.pid, args.runtime)
            elapsed = time.monotonic() - ready_observed
            result["samples"].append({"elapsed_seconds": elapsed, "runtime_pids": current})
            if not current:
                result["first_absent_seconds"] = elapsed
                break
            assert current == [pid], f"runtime changed during idle observation: {current}"
            result["last_present_seconds"] = elapsed
            if second % 60 == 0:
                print(f"Idle expiry observation: {second}/300 seconds; same model PID is alive", flush=True)
        assert "first_absent_seconds" in result, "runtime remained loaded beyond 305 seconds"
        assert result["last_present_seconds"] >= 299, "runtime did not remain loaded through the 299-second observation"
        assert 299 <= result["first_absent_seconds"] <= 301.5, "runtime expiry was outside the 300-second polling tolerance"
        result["expiry_interval_seconds"] = [result["last_present_seconds"], result["first_absent_seconds"]]
        status("Ready · model unloaded", 5)
        wait_until(no_children, 5, "expired runtime is reaped")
        assert process.poll() is None, "daemon exited instead of remaining ready"
        result["daemon_survived_expiry"] = True
        result["full_300_second_expiry_observed"] = True
        result["microphone_used"] = False
        result["clipboard_used"] = False
        result["binary_sha256_after"] = hashlib.sha256(args.binary.read_bytes()).hexdigest()
        assert result["binary_sha256_after"] == binary_sha256, "executable changed during verification"

    def recording(cli, status, model_pid, no_children, process, result, data):
        permissions = json.loads(cli("permissions"))
        result["microphone_permission"] = permissions.get("microphone")
        if permissions.get("microphone") != "authorized":
            result["status"] = "skipped"
            result["reason"] = "Microphone permission is not authorized; no permission request or recording attempted."
            return
        status("Ready", 10)
        assert not children(process.pid), "model loaded before recording"
        clipboard = subprocess.check_output(["/usr/bin/pbpaste"])
        cli("toggle")
        status("Recording", 10)
        def runtime_started():
            try:
                model_pid()
                return True
            except AssertionError:
                return False

        wait_until(runtime_started, 10, "preparation starts during recording")
        early_pid = model_pid()
        result["early_cancel_runtime_pid"] = early_pid
        canceled_at = time.monotonic()
        cli("cancel")
        status("Canceled", 10)
        wait_until(no_children, 5, "cancel during preparation releases its runtime")
        result["early_cancel_cleanup_seconds"] = time.monotonic() - canceled_at
        result["cleanup_includes_sound_helpers"] = True
        assert subprocess.check_output(["/usr/bin/pbpaste"]) == clipboard
        result["early_cancel_reaped_runtime"] = True
        time.sleep(0.25)
        log = data / "daemon.log"
        log_offset = log.stat().st_size if log.exists() else 0
        cli("toggle")
        status("Recording", 10)
        result["recording_started_without_loaded_model"] = True

        def prepared_while_recording():
            assert cli("status") == "Recording", "model preparation interrupted recording"
            if runtime_started():
                result["load_started_while_recording"] = True
            events = []
            for line in log.read_bytes()[log_offset:].decode().splitlines() if log.exists() else []:
                try:
                    events.append(json.loads(line))
                except ValueError:
                    pass
            assert not any(event.get("event") == "transcription" for event in events), "unexpected transcription"
            loads = [event for event in events if event.get("event") == "model_load"]
            if not loads or loads[-1].get("success") is not True:
                return False
            result["load_finished_while_recording"] = True
            result["model_load_ms"] = loads[-1]["load_ms"]
            return True

        wait_until(prepared_while_recording, 45, "model preparation completes while recording")
        pid = model_pid()
        assert result.get("load_started_while_recording") is True
        cli("cancel")
        status("Canceled", 10)
        assert model_pid() == pid and alive(pid), "cancel discarded the prepared model"
        assert subprocess.check_output(["/usr/bin/pbpaste"]) == clipboard
        result["runtime_pid"] = pid
        result["prepared_model_retained_after_cancel"] = True
        result["clipboard_unchanged"] = True
        result["transcription_requested"] = False

    code = 0
    try:
        phases = [] if args.recording_only or args.full_idle_expiry_only else [("cancel_startup", 300, cancel),
                                                ("resident", 300, resident), ("idle_unload", 1, unload)]
        for name, idle, operation in phases:
            print("Verifying " + name, flush=True)
            report["phases"][name] = phase(args, name, idle, operation)
        if args.full_idle_expiry_only:
            print("Verifying full five-minute idle expiry in an isolated daemon", flush=True)
            expiry = phase(args, "idle_expiry", 300, full_idle_expiry)
            report["phases"]["idle_expiry"] = expiry
            report["full_300_second_expiry_observed"] = True
            report["covers_old_60_second_timeout"] = True
            report["expiry_check"] = "Observed the full configured 300-second interval with one-second polling."
        if args.recording:
            print("Verifying recording preparation", flush=True)
            overlap = phase(args, "recording_preparation", 300, recording, preload=False)
            report["phases"]["recording_preparation"] = overlap
            if overlap["status"] == "skipped":
                report["inconclusive"] = True
                code = 2
    except KeyboardInterrupt:
        report["interrupted"] = True
        code = 130
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
        code = 1
    finally:
        (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    displayed = {**report, "phases": {
        name: {key: value for key, value in result.items() if key != "samples"}
        for name, result in report["phases"].items()
    }}
    print(json.dumps(displayed, indent=2))
    print(f"Evidence: {args.output / 'results.json'}")
    return code


if __name__ == "__main__":
    sys.exit(main())
