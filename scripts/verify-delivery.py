#!/usr/bin/env python3
"""Compare real native text insertion against the former pbcopy + 100 ms path.

Opens an owned scratch app; keep it foreground. Requires existing Accessibility
permission. Restores prior plain-text clipboard content if it still owns it.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import select
import shutil
import statistics
import subprocess
import tempfile
import time
import tomllib

ROOT = Path(__file__).resolve().parent.parent


def wait_for(check, description, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(0.01)
    raise RuntimeError("Timed out: " + description)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--rounds", type=int, default=10)
    args = parser.parse_args()
    if args.rounds < 5:
        parser.error("at least five paired rounds required")
    destination = args.output.resolve()
    destination.mkdir(parents=True, exist_ok=False)
    original = subprocess.check_output(["/usr/bin/pbpaste"])
    owned = None
    target = probe = None
    samples = []
    report = {"status": "running", "samples": samples,
              "output_sha256": hashlib.sha256((ROOT / "src/output.rs").read_bytes()).hexdigest(),
              "method": "alternating pairs; actual NSTextView insertion receipts; same paste keystroke code; no inference",
              "timing_note": "Insertion uses same-host wall clocks, not pixel rendering; dispatch uses Rust Instant.",
              "baseline": "recreated prior pbcopy + 100ms wait, using unchanged paste event dispatch"}
    try:
        with tempfile.TemporaryDirectory(prefix="nemo-delivery-") as temporary:
            work = Path(temporary)
            app = work / "Nemo Delivery Check.app"
            executable = app / "Contents/MacOS/target"
            executable.parent.mkdir(parents=True)
            (app / "Contents/Info.plist").write_bytes(plistlib.dumps({
                "CFBundleExecutable": "target", "CFBundleIdentifier": "computer.nemo.delivery-check",
                "CFBundleName": "Nemo Delivery Check", "CFBundlePackageType": "APPL"}))
            subprocess.run(["xcrun", "swiftc", str(ROOT / "scripts/verification/PasteTarget.swift"),
                            "-o", str(executable)], check=True)
            dependencies = tomllib.loads((ROOT / "Cargo.toml").read_text())["dependencies"]
            names = ["anyhow", "objc2", "objc2-app-kit", "objc2-foundation", "serde_json"]
            manifest = '[package]\nname="nemo-delivery-probe"\nversion="0.0.0"\nedition="2024"\n[dependencies]\n'
            for name in names:
                value = dependencies[name]
                if isinstance(value, str):
                    manifest += f'{name} = {json.dumps(value)}\n'
                else:
                    fields = [f'{key} = {json.dumps(val)}' for key, val in value.items()]
                    manifest += f'{name} = {{' + ', '.join(fields) + '}\n'
            manifest += '\n[[bin]]\nname="probe"\npath=' + json.dumps(str(ROOT / 'scripts/verification/delivery-probe.rs')) + '\n'
            (work / "Cargo.toml").write_text(manifest)
            env = {**os.environ, "NEMO_OUTPUT_SOURCE": str(ROOT / "src/output.rs")}
            subprocess.run(["cargo", "build", "--release", "--offline", "--manifest-path", str(work / "Cargo.toml")],
                           env=env, check=True)
            shutil.copyfile(work / "Cargo.lock", destination / "probe.Cargo.lock")
            receipts = destination / "paste-events.jsonl"
            target = subprocess.Popen([str(executable), str(receipts)])
            wait_for(receipts.exists, "scratch target startup")
            time.sleep(0.5)
            with (destination / "probe.log").open("w") as log:
                probe = subprocess.Popen([str(work / "target/release/probe")], stdin=subprocess.PIPE,
                                         stdout=subprocess.PIPE, stderr=log, text=True)

                def send(variant, text):
                    nonlocal owned
                    probe.stdin.write(variant + "\t" + text + "\n")
                    probe.stdin.flush()
                    if not select.select([probe.stdout], [], [], 10)[0]:
                        raise RuntimeError("Probe did not respond")
                    line = probe.stdout.readline()
                    if not line:
                        raise RuntimeError("Probe failed; see probe.log")
                    result = json.loads(line)
                    if variant != "canceled":
                        owned = text.encode()
                    return result

                def events():
                    return [json.loads(line) for line in receipts.read_text().splitlines()]

                sentinel = "Nemo clipboard check — café 🐟"
                send("copy", sentinel)
                assert subprocess.check_output(["/usr/bin/pbpaste"]) == sentinel.encode()
                send("canceled", "Must never be delivered")
                assert subprocess.check_output(["/usr/bin/pbpaste"]) == sentinel.encode()
                assert events() == [], "copy/cancellation unexpectedly pasted"
                report["copy_and_cancellation_passed"] = True
                for round_index in range(args.rounds):
                    order = ["baseline", "native"] if round_index % 2 == 0 else ["native", "baseline"]
                    for variant in order:
                        fixture = f"Nemo {variant} round {round_index}: café 🐟"
                        count = len(events())
                        result = send(variant, fixture)
                        received = wait_for(lambda: events() if len(events()) > count else None, "paste receipt")
                        assert len(received) == count + 1
                        event = received[-1]
                        assert event["text"] == fixture, event
                        result.update(variant=variant, round=round_index,
                                      insertion_ms=(event["unix_seconds"] - result["start_unix_seconds"]) * 1000)
                        assert result["insertion_ms"] >= 0
                        samples.append(result)
                assert len(events()) == 2 * args.rounds
                report["summary"] = {}
                for variant in ("baseline", "native"):
                    values = [item["insertion_ms"] for item in samples if item["variant"] == variant]
                    report["summary"][variant] = {"median_ms": statistics.median(values),
                                                  "min_ms": min(values), "max_ms": max(values), "runs": len(values)}
                report["native_paired_wins"] = sum(
                    next(item["insertion_ms"] for item in samples if item["round"] == index and item["variant"] == "native")
                    < next(item["insertion_ms"] for item in samples if item["round"] == index and item["variant"] == "baseline")
                    for index in range(args.rounds))
                report["status"] = "passed"
    except Exception as error:
        report.update(status="failed", error=str(error))
        raise
    finally:
        for process in (probe, target):
            if process is not None and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        if owned is not None and subprocess.check_output(["/usr/bin/pbpaste"]) == owned:
            subprocess.run(["/usr/bin/pbcopy"], input=original, check=True)
        (destination / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps({key: report[key] for key in ("status", "summary", "error") if key in report}))


if __name__ == "__main__":
    main()
