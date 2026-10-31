---
name: verify-offline
description: Verify Nemo's local-only inference, OS network denial, constrained model downloader, and dependency/license gates on macOS. Use for privacy regressions and release audits; use verify-nemo for physical microphone and paste checks.
---
# Verify local-only Nemo

Work from the repository root on Apple Silicon macOS 26+. Read the
[journey index](references/journeys.md) and select the relevant checks.
Python 3.11+, a built Nemo binary, a local native runtime, and the two pinned model
files are required for real inference. Missing models are a coverage gap; never
replace the real recognizer with a stub to claim offline inference.

The helper uses isolated absolute `NEMO_DATA_DIR` and `NEMO_APP_PATH`, references
existing model files, and leaves the user's configuration and installed app alone.
It creates a new evidence directory, synthesizes a public test phrase to a WAV
without speaker playback, and runs the actual transcribe command under the same
network-deny profile embedded in Nemo. With `--daemon`, it also starts an isolated
menu daemon, checks its identity/readiness, warms the real model, quits, and checks
that its children disappear. A visible temporary menu item is expected.

```sh
cargo xtask package --output artifacts/offline-candidate/Nemo.app
python3 scripts/verify-offline.py \
  --binary artifacts/offline-candidate/Nemo.app/Contents/MacOS/nemo \
  --daemon --output artifacts/offline-NEW
```

Require `report.json` to have `passed: true`, external IPv4/IPv6 `EPERM`, successful
loopback, denied descendant curl, expected real transcript, successful model-load
telemetry, log mode 0600, and daemon cleanup when requested. The report hashes both
executables and fails if either changes during the run. Inspect the actual report;
directory existence and process exit alone are not proof.

Do not rebuild a candidate concurrently with verification. Retain failed reports.
The script terminates only its owned processes and removes its temporary data;
evidence and synthetic fixtures remain under the requested output directory.
If interrupted during daemon startup, inspect the retained startup log and verify
that no process from that candidate/isolated instance remains before rerunning.

Run `cargo xtask check` and the downloader/dependency journeys for a release audit.
For physical recording and visible paste, follow the existing
[verify-nemo skill](../verify-nemo/SKILL.md); file inference is not microphone proof.
Build/install tools and advisory-database refreshes deliberately access package
servers. They are distinct from installed dictation operation.
