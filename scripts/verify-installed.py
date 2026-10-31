#!/usr/bin/env python3
"""Exercise the installed Nemo daemon, then observe its full five-minute idle expiry.

Clipboard comparison is the default acceptance check. Use verify-clipboard.py for
a safe isolated paste=false run. A foreground editor is needed only for --paste-target.
Requires Python 3.11+. This command speaks a synthetic phrase through the speakers.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time
import tomllib

from nemo_status import control, lifecycle_status

PHRASE = "The quick brown fox jumps over the lazy dog. This is a local speech recognition test on my laptop."


class Inconclusive(RuntimeError):
    pass


class PasteTargetMismatch(Inconclusive):
    pass


def normalize(text):
    return re.findall(r"[a-z0-9]+", text.lower())


def processes():
    output = subprocess.check_output(["/bin/ps", "-A", "-o", "pid=,ppid=,comm="], text=True)
    return [(int(parts[0]), int(parts[1]), parts[2]) for line in output.splitlines()
            if len(parts := line.strip().split(None, 2)) == 3]


def observe_idle(args, report, save, status, runtime_pids):
    """Observe only: user dictation starts a new idle interval without interference."""
    report.pop("expected", None)
    report.pop("ui_document_check", None)
    report["paste_check_scope"] = "not exercised; read-only idle observation"
    report["method"] = "read-only installed-daemon idle observation; new dictations reset the interval"
    report["baseline_resets"] = []
    report["epochs"] = []
    log = args.data_dir / "daemon.log"
    initial_text = log.read_text()
    mtime = log.stat().st_mtime
    initial = status()
    initial_pids = runtime_pids()
    assert len(initial_pids) <= 1, initial_pids
    runtime_pid = initial_pids[0] if initial_pids else None
    now = time.monotonic()
    initial_age = max(0, time.time() - mtime)
    report["initial_status"] = initial
    report["initial_log_age_seconds"] = initial_age
    baseline = None
    events = []
    for line in initial_text.splitlines():
        try:
            event = json.loads(line)
            if isinstance(event, dict) and event.get("event") == "transcription":
                events.append(event)
        except ValueError:
            pass
    outcome_lines = [line for line in initial_text.splitlines() if not line.startswith("Readiness: ")]
    if (initial == "Ready" and runtime_pid and outcome_lines[-1:] in (["Ready"], ["Transcript delivered"])
            and events and events[-1].get("success") and initial_age < 295):
        baseline = now - initial_age
        report["baseline_resets"].append({"source": "initial Ready log mtime proxy",
            "monotonic": baseline, "initial_age_seconds": initial_age})
    report["timing_note"] = ("Initial Ready mtime is a proxy after worker completion and delivery; "
                             "fresh transcription baselines use monotonic event receipt. "
                             "299–305s acceptance accounts for this offset and one-second polling.")
    epoch_started = now
    report["epochs"].append({"runtime_pid": runtime_pid, "observed_monotonic": now})
    last_present = None
    announced = -1
    busy_states = {"Opening microphone", "Recording", "Transcribing", "Loading model", "Downloading model"}
    buffer = ""
    with log.open() as stream:
        stream.seek(0, os.SEEK_END)
        deadline = now + 900
        while time.monotonic() < deadline:
            current = status()
            fresh_transcription = False
            buffer += stream.read()
            lines = buffer.split("\n")
            buffer = lines.pop()
            for line in lines:
                if line in busy_states:
                    baseline = None
                try:
                    event = json.loads(line)
                except ValueError:
                    continue
                if not isinstance(event, dict) or event.get("event") not in {"model_load", "transcription"}:
                    continue
                observed = time.monotonic()
                report["events"].append({"observed_monotonic": observed, **event})
                if event["event"] == "transcription" and event.get("success"):
                    baseline = observed
                    fresh_transcription = True
                    announced = -1
                    report["baseline_resets"].append({"source": "fresh transcription telemetry",
                                                       "monotonic": baseline})
                    print("New transcription completed; restarting five-minute idle observation.", flush=True)
                else:
                    baseline = None
            if current in busy_states and not fresh_transcription:
                baseline = None
            pids = runtime_pids()
            assert len(pids) <= 1, pids
            current_pid = pids[0] if pids else None
            if current_pid != runtime_pid and current_pid is not None:
                if not fresh_transcription:
                    baseline = None
                runtime_pid = current_pid
                epoch_started = time.monotonic()
                last_present = None
                report["epochs"].append({"runtime_pid": runtime_pid,
                                          "observed_monotonic": epoch_started})
            elapsed = time.monotonic() - baseline if baseline is not None else None
            sample = {"idle_seconds": elapsed, "runtime_pid": current_pid, "status": current}
            report["samples"].append(sample)
            if current_pid:
                last_present = elapsed
            if elapsed is not None:
                minute = int(elapsed // 60)
                if minute != announced:
                    print(f"Idle {elapsed:.1f}s: model {'loaded' if current_pid else 'unloaded'}", flush=True)
                    announced = minute
                if current_pid is None:
                    assert 299 <= elapsed <= 305, f"Model unloaded outside expected window: {elapsed:.3f}s"
                    ui_deadline = time.monotonic() + 3
                    while current != "Ready · model unloaded" and time.monotonic() < ui_deadline:
                        time.sleep(.05)
                        current = status()
                    assert current == "Ready · model unloaded", current
                    report.update(full_five_minute_retention_verified=True,
                                  observed_unload_seconds=elapsed,
                                  last_observed_loaded_idle_seconds=last_present,
                                  observed_epoch_seconds=time.monotonic() - epoch_started,
                                  actual_observation_seconds=time.monotonic() - now)
                    save()
                    return
                assert elapsed <= 305, "Model did not unload within timeout plus polling tolerance"
            save()
            time.sleep(1)
    raise Inconclusive("No uninterrupted five-minute idle interval completed within 15 minutes")


def main():
    def interrupted(_signal, _frame):
        raise KeyboardInterrupt()

    signal.signal(signal.SIGTERM, interrupted)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, default=Path.home() / "Applications/Nemo.app")
    parser.add_argument("--data-dir", type=Path, default=Path.home() / "Library/Application Support/Nemo")
    parser.add_argument("--output", type=Path, required=True, help="new directory for incremental evidence")
    parser.add_argument("--expected", default=PHRASE, help="synthetic speech spoken during recording")
    parser.add_argument("--observe-idle", action="store_true",
                        help="read-only observation of the current model; new dictations reset the timer")
    parser.add_argument("--roundtrip-only", action="store_true",
                        help="finish after acoustic transcription/paste checks; do not wait for idle expiry")
    parser.add_argument("--warm", action="store_true",
                        help="require an already-ready model and verify reuse; requires --roundtrip-only")
    parser.add_argument("--paste-target", metavar="APP",
                        help="require this foreground app before start/stop and in the paste log; verify visible text separately")
    args = parser.parse_args()
    if args.warm and not args.roundtrip_only:
        parser.error("--warm requires --roundtrip-only")
    if args.observe_idle and args.roundtrip_only:
        parser.error("--observe-idle and --roundtrip-only are separate journeys")
    if args.observe_idle and args.paste_target is not None:
        parser.error("--paste-target applies to acoustic round trips, not --observe-idle")
    if args.paste_target is not None:
        args.paste_target = args.paste_target.strip()
        if not args.paste_target:
            parser.error("--paste-target must name an app")
    args.app = args.app.expanduser().resolve()
    args.data_dir = args.data_dir.expanduser().resolve()
    args.output = args.output.expanduser().resolve()
    if not normalize(args.expected):
        parser.error("expected text must contain words")
    args.output.mkdir(parents=True, exist_ok=False)
    binary = args.app / "Contents/MacOS/nemo"
    report = {"status": "running", "app": str(args.app), "expected": args.expected,
              "created_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
              "method": "installed daemon; real microphone and speaker playback; full configured idle expiry",
              "samples": [], "events": [],
              "ui_document_check": ("required separately by caller" if args.paste_target
                                    else "not requested; clipboard acceptance")}
    report["roundtrip_only"] = args.roundtrip_only
    report["warm_roundtrip"] = args.warm
    report["paste_check_scope"] = ("clipboard and requested target log; visible text requires separate UI observation"
                                   if args.paste_target else "clipboard only; no paste-target assertion")
    if args.paste_target:
        report["requested_paste_target"] = args.paste_target
        report["foreground_checks"] = []
    if args.roundtrip_only:
        report["method"] = "installed daemon; real microphone and speaker playback; acoustic round trip only"
    destination = args.output / "results.json"
    owns_recording = False
    owns_job = False
    speech = None
    original_clipboard = None
    daemon_pid = None
    log_stream = None
    log_buffer = ""
    receipt = None

    def save():
        temporary = destination.with_suffix(".tmp")
        temporary.write_text(json.dumps(report, indent=2) + "\n")
        temporary.replace(destination)

    def status():
        return lifecycle_status(lambda command: control(args.data_dir, command))

    def cli(command):
        result = subprocess.run([str(binary), command], capture_output=True, text=True, timeout=5,
                                env={**os.environ, "NEMO_DATA_DIR": str(args.data_dir)})
        if result.returncode:
            raise RuntimeError(result.stderr.strip() or f"{command} failed")
        return result.stdout.strip()

    def check_foreground(checkpoint):
        if not args.paste_target:
            return
        diagnostic = cli("doctor")
        match = re.search(r"^Foreground app: (.*)$", diagnostic, re.MULTILINE)
        foreground = match.group(1).strip() if match else None
        report["foreground_checks"].append({"checkpoint": checkpoint, "app": foreground,
                                            "observed_monotonic": time.monotonic()})
        save()
        if foreground != args.paste_target:
            raise PasteTargetMismatch(f"Expected {args.paste_target} foreground at {checkpoint}; observed {foreground!r}")

    def runtime_pids():
        rows = processes()
        if not any(pid == daemon_pid and command == str(binary) for pid, _, command in rows):
            raise Inconclusive("Installed daemon exited or was replaced during verification")
        return [pid for pid, parent, command in rows
                if parent == daemon_pid and Path(command).resolve() == runtime]

    def read_events():
        nonlocal log_buffer
        log_buffer += log_stream.read()
        lines = log_buffer.split("\n")
        log_buffer = lines.pop()
        fresh = []
        for line in lines:
            if line == "Shortcut toggled":
                raise Inconclusive("A user shortcut interrupted the controlled test")
            try:
                event = json.loads(line)
            except ValueError:
                if line.startswith("Pasting into "):
                    report["paste_destination_log"] = line.removeprefix("Pasting into ")
                continue
            if isinstance(event, dict) and event.get("event") in {
                "capture_start", "audio_finalized", "inference_started", "delivery"
            }:
                report.setdefault("pipeline_events", []).append(
                    {"observed_monotonic": time.monotonic(), **event})
            if isinstance(event, dict) and event.get("event") in {"model_load", "transcription"}:
                fresh.append(event)
                report["events"].append({"observed_monotonic": time.monotonic(), **event})
        return fresh

    def wait_for(expected, timeout):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            current = status()
            if current == expected:
                return
            if current not in {"Opening microphone", "Recording", "Transcribing", "Ready", "ready"}:
                raise RuntimeError(f"Unexpected daemon state: {current}")
            time.sleep(.05)
        raise RuntimeError(f"Timed out waiting for {expected}; last status: {current}")

    code = 0
    save()
    try:
        config = tomllib.loads((args.data_dir / "config.toml").read_text())
        report["configuration"] = {key: config.get(key) for key in
                                   ("idle_unload_seconds", "max_recording_seconds", "paste",
                                    "preload_model", "inference_threads", "encoder_batch_size", "model_warmup")}
        assert config["idle_unload_seconds"] == 300, "Installed configuration must use a 300-second timeout"
        assert config["max_recording_seconds"] >= 90, "Test needs at least a 90-second capture limit"
        runtime = Path(config["runtime"]).resolve()
        report["binary_sha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
        report["permissions"] = json.loads(control(args.data_dir, "permissions"))
        assert report["permissions"] == {"accessibility": True, "microphone": "authorized"}, \
            "Installed daemon must have Accessibility and Microphone permission"
        matches = [pid for pid, _, command in processes() if command == str(binary)]
        assert len(matches) == 1, f"Expected one installed daemon; found {matches}"
        daemon_pid = matches[0]
        report["daemon_pid"] = daemon_pid
        if args.observe_idle:
            observe_idle(args, report, save, status, runtime_pids)
            report["status"] = "passed"
            return 0
        if args.paste_target:
            assert config.get("paste") is True, "--paste-target requires the installed app's paste setting to be true"
        check_foreground("before_recording")
        initial_runtime = runtime_pids()
        if args.warm:
            assert len(initial_runtime) == 1, "Warm verification requires one already-ready runtime"
            runtime_pid = initial_runtime[0]
            report["runtime_pid"] = runtime_pid
            report["warm_runtime_ready_before_recording"] = True
        else:
            assert not initial_runtime, "Start with a cold model; do not interrupt or unload an active session"
            runtime_pid = None
        initial = status()
        allowed = {"Ready", "ready", "Canceled"}
        allowed.add("Model loaded" if args.warm else "Ready · model unloaded")
        assert initial in allowed, initial
        original_clipboard = subprocess.check_output(["/usr/bin/pbpaste"])
        log_stream = (args.data_dir / "daemon.log").open()
        log_stream.seek(0, os.SEEK_END)
        report["recording_command_monotonic"] = time.monotonic()
        cli("toggle")
        owns_recording = True
        owns_job = True
        wait_for("Recording", 10)
        report["recording_observed_monotonic"] = time.monotonic()
        save()
        print("Recording started with " + ("a ready model" if args.warm else "no model loaded") +
              "; speaking reference phrase.", flush=True)
        # Leave the start sound outside the spoken words.
        time.sleep(.6)
        speech = subprocess.Popen(["/usr/bin/say", "-v", "Samantha", args.expected],
                                  stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        deadline = time.monotonic() + 80
        load = None
        while time.monotonic() < deadline:
            if status() != "Recording":
                raise Inconclusive("Recording ended before the controlled stop")
            for event in read_events():
                if event["event"] == "transcription":
                    raise Inconclusive("Unexpected transcription before the controlled stop")
                if event["event"] == "model_load":
                    assert not args.warm, "Warm recording unexpectedly reloaded the model"
                    assert event["success"], "Model loading failed during recording"
                    load = event
            current_pids = runtime_pids()
            if args.warm:
                assert current_pids == [runtime_pid], "Warm recording lost or replaced its ready model"
            if current_pids:
                assert len(current_pids) == 1, current_pids
                if runtime_pid is None:
                    runtime_pid = current_pids[0]
                    report["runtime_pid"] = runtime_pid
                    report["load_started_while_recording"] = True
                    save()
                assert current_pids == [runtime_pid], "Model process changed during recording"
            if (args.warm or load) and speech.poll() is not None:
                assert speech.returncode == 0, "Synthetic speech failed"
                break
            time.sleep(.05)
        else:
            raise RuntimeError("Model preparation or reference speech exceeded 80 seconds")
        report["model_load"] = load
        if args.warm:
            report["warm_runtime_reused_during_recording"] = True
            report["model_load_events_during_recording"] = 0
        else:
            report["load_finished_while_recording"] = True
        check_foreground("before_stop")
        report["stop_command_monotonic"] = time.monotonic()
        save()
        cli("toggle")
        owns_recording = False
        deadline = time.monotonic() + 90
        transcription = None
        while time.monotonic() < deadline:
            for event in read_events():
                if event["event"] == "model_load":
                    raise RuntimeError("Transcription unexpectedly reloaded the prepared model")
                assert transcription is None, "Unexpected additional transcription"
                transcription = event
                receipt = time.monotonic()
            if transcription:
                break
            time.sleep(.02)
        assert transcription, "No transcription event observed"
        assert transcription["success"] and transcription["reused_model"], transcription
        assert transcription["model_load_ms"] == 0, "Prepared model incurred another load"
        assert runtime_pids() == [runtime_pid], "Transcription did not retain the prepared model"
        report["transcription"] = transcription
        report["transcription_observed_monotonic"] = receipt
        report["retention_timing_note"] = "Starts at telemetry observation; backend completion occurred just before it; polling tolerance is one second."
        wait_for("Ready", 10)
        owns_job = False
        read_events()
        actual = subprocess.check_output(["/usr/bin/pbpaste"]).decode("utf-8")
        report["clipboard_matches_reference"] = normalize(actual) == normalize(args.expected)
        assert report["clipboard_matches_reference"], "Captured transcript did not match reference speech"
        assert status() == "Ready", "Transcription/paste did not reach Ready"
        if args.paste_target:
            report["paste_target_log_matches"] = report.get("paste_destination_log") == args.paste_target
            save()
            if not report["paste_target_log_matches"]:
                raise PasteTargetMismatch(f"Paste log target differs from requested {args.paste_target}: "
                                          f"{report.get('paste_destination_log')!r}")
        save()
        if args.roundtrip_only:
            report["status"] = "passed"
            report["idle_expiry_checked"] = False
            print("Start → stop → transcribe completed; round-trip-only verification passed.", flush=True)
            return 0
        print("Start → stop → transcribe completed; observing five idle minutes.", flush=True)
        announced = -1
        while True:
            elapsed = time.monotonic() - receipt
            current = status()
            if current in {"Recording", "Opening microphone", "Transcribing", "Loading model", "Downloading model"}:
                raise Inconclusive("Another job started during the idle-retention observation")
            if read_events():
                raise Inconclusive("Another model job reset the idle timeout")
            current_pids = runtime_pids()
            assert not current_pids or current_pids == [runtime_pid], "A different model process appeared"
            report["samples"].append({"idle_seconds": elapsed, "runtime_present": bool(current_pids), "status": current})
            save()
            minute = int(elapsed // 60)
            if minute != announced:
                print(f"Idle {elapsed:.1f}s: model {'loaded' if current_pids else 'unloaded'}", flush=True)
                announced = minute
            if not current_pids:
                report["observed_unload_seconds"] = elapsed
                assert 299 <= elapsed <= 305, f"Model unloaded outside the 300-second window: {elapsed:.3f}s"
                wait_for("Ready · model unloaded", 3)
                report["full_five_minute_retention_verified"] = True
                break
            if elapsed > 305:
                raise RuntimeError("Model did not unload within the 300-second timeout plus polling tolerance")
            time.sleep(min(1, max(.05, 300 - elapsed)) if elapsed < 300 else .1)
        report["status"] = "passed"
    except Inconclusive as error:
        if not isinstance(error, PasteTargetMismatch):
            owns_recording = False
            owns_job = False
        report["status"] = "inconclusive"
        report["error"] = str(error)
        code = 2
    except KeyboardInterrupt:
        report["status"] = "interrupted"
        code = 130
    except Exception as error:
        report["status"] = "failed"
        report["error"] = f"{type(error).__name__}: {error}"
        code = 1
    finally:
        if speech is not None and speech.poll() is None:
            speech.terminate()
            try:
                speech.wait(timeout=5)
            except subprocess.TimeoutExpired:
                speech.kill()
                speech.wait()
        if owns_recording or owns_job:
            try:
                current = status()
                if log_stream is not None:
                    read_events()
                if current in {"Opening microphone", "Recording", "Transcribing"}:
                    cli("cancel")
            except Inconclusive as error:
                owns_recording = False
                owns_job = False
                report["cleanup_ownership_relinquished"] = str(error)
            except (OSError, RuntimeError):
                pass
        if original_clipboard is not None:
            current = subprocess.check_output(["/usr/bin/pbpaste"])
            if normalize(current.decode("utf-8", errors="replace")) == normalize(args.expected):
                subprocess.run(["/usr/bin/pbcopy"], input=original_clipboard, check=True)
                report["original_clipboard_restored"] = True
            else:
                report["original_clipboard_restored"] = False
        if log_stream is not None:
            log_stream.close()
        save()
    print(f"{report['status']}: {destination}", flush=True)
    return code


if __name__ == "__main__":
    sys.exit(main())
