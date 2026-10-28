# Nemo

Rust dictation daemon for Apple Silicon Macs. Tap **right Option** to record; tap it again to transcribe, copy, and paste into the focused app.

Uses [NVIDIA Nemotron 3.5 Transcribe 30B-A4B](https://huggingface.co/nvidia/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B), the [AtomicChat IQ4_NL conversion](https://huggingface.co/AtomicChat/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-GGUF), and NVIDIA's [NeMo-Speech.cpp](https://github.com/NVIDIA/NeMo-Speech.cpp) Metal backend. The controller, microphone capture, shortcut, and paste integration are Rust. Inference runs in a managed native C++ subprocess. No Python environment, cloud service, or API key is required at runtime.

## Run

```sh
open ~/Applications/Nemo.app
```

Allow **Microphone** when prompted. Enable **Nemo** under **System Settings → Privacy & Security → Accessibility** for Right Option and automatic paste. The menu bar includes links to both settings pages. Nemo stays open when permission is missing and detects grants or revocations within two seconds. A keyboard combination can still record and copy without Accessibility; automatic paste requires it.

The menu bar dot is gray when idle, red when recording, and amber during microphone setup or transcription. **Cancel** discards the current operation. **Quit** stops recording and releases the model. A second daemon refuses to start.

Right Option toggles on release. Left Option and Option used with another key or a mouse click keep their normal behavior. This shortcut uses the same Accessibility permission required for automatic paste.

Sounds: **Tink** when recording starts, **Pop** when recording stops, and **Glass** after transcription is pasted. **Basso** signals an error.

The model unloads after 60 seconds of inactivity. **Load model** prepares it ahead of the next recording. Recording stops automatically after 120 seconds; change the limit in configuration, up to 300 seconds.

## Configure

Use **Open configuration** in the menu, or:

```sh
./target/release/nemo config --hotkey RightOption
```

Quit and reopen Nemo to apply changes. Configuration lives in `~/Library/Application Support/Nemo/config.toml`. Available settings:

```toml
hotkey = "RightOption"
idle_unload_seconds = 60
max_recording_seconds = 120
paste = true
```

`model` and `runtime` are absolute paths; the generated configuration includes both. Model weights live in `~/Library/Application Support/Nemo/models/<pinned revision>/`, independently of this checkout and app upgrades. The installed app includes the inference executable and its libraries. Explicit custom paths remain supported.

For an older checkout with downloaded weights, migrate without downloading again:

```sh
./target/release/nemo model migrate --from "$PWD/models"
```

Migration verifies every SHA-256 first, publishes the managed files, and updates configuration while preserving preferences. It leaves compatibility symlinks so an already-running older app can still load its captured paths. Keep those symlinks until that app has restarted. On the same volume, migration uses hard links and needs no second copy of the weights.

Combinations such as `Control+Shift+Space` are also supported as the `hotkey` value.

## CLI

```sh
./target/release/nemo status
./target/release/nemo toggle
./target/release/nemo cancel
./target/release/nemo warm
./target/release/nemo quit
./target/release/nemo doctor
./target/release/nemo permissions
./target/release/nemo model path
./target/release/nemo model download
./target/release/nemo model verify
./target/release/nemo transcribe recording.wav
./target/release/nemo transcribe recording.wav --copy
./target/release/nemo transcribe recording.wav --paste
```

`transcribe` starts its own temporary model process and shuts it down on completion. Use the daemon for repeated dictation so recordings can reuse a loaded model. A normal exit, Control-C, or SIGTERM releases the child process. Audio captured by the daemon stays in memory and is discarded after use; no transcript history is saved. The clipboard retains the latest transcript.

**Download model** in the menu or `model download` resumes interrupted downloads into `.part` files. Only files with the pinned size and SHA-256 become usable weights. Downloads and migrations share an exclusive lock; canceling retains resumable data. `model verify` checks the managed model, regardless of custom configuration paths.

## Build and verify

Requires Apple Silicon, macOS 26+, Rust, Homebrew, and Xcode Command Line Tools. The two model files total **21.35 GB**; quantization reduces the language decoder's size, but a 34B-parameter model still needs tens of gigabytes while loaded. Leave room for the model, build outputs, and inference. This is currently macOS-only.

```sh
scripts/setup.sh          # install build dependencies, build, download, verify hashes, install
scripts/download-model.sh # resume model downloads and verify SHA-256
scripts/build-runtime.sh # rebuild the pinned Metal runtime
scripts/package.sh       # build dist/Nemo.app without changing the installed app
scripts/install.sh       # stage, verify, and install; retain the previous app
scripts/check.sh         # local CI: formatting, Clippy, Rust and installer tests
scripts/verify.sh        # static checks, tests, actual synthetic-speech transcription
python3 scripts/verify-package.py --app dist/Nemo.app # relocated bundle and library checks
python3 scripts/verify-lifecycle.py # cancellation, single instance, idle unload, SIGTERM (quit Nemo first)
```

The runtime and model revisions are pinned in `scripts/build-runtime.sh` and `models/manifest.json`. Python 3.9+ is used only by setup/verification scripts. `scripts/check.sh` needs no model, native runtime, microphone, or Accessibility permission. `scripts/verify.sh` creates sample speech with macOS `say`, uses an isolated data directory, and compares the actual model transcript against the spoken words. `NEMO_DATA_DIR` selects a separate absolute data directory for tests or another installation; `NEMO_RUNTIME` overrides the default runtime for fresh configurations.

### Safe upgrades

The installer copies and verifies the candidate before stopping an idle app, rejects upgrades during recording/transcription/downloads, retains the previous bundle in `~/Applications/.Nemo.backups/`, and restores it if replacement, signature verification, or launch fails. The daemon checks the safe-quit request on its event loop so a newly started recording cannot race the installer. An idle running app is reopened after a successful upgrade. Quit older versions that lack safe-quit support manually before installing. The configuration and model directory are retained.

Local builds use ad hoc signatures. A rebuilt app can change its macOS permission identity, so the installer refuses that upgrade by default. After reviewing the candidate, explicitly allow a permission reset if needed:

```sh
python3 scripts/install_app.py --source dist/Nemo.app --allow-permission-reset
```

Then re-enable Nemo in Accessibility if macOS requires it. Use the retained backup as `--source` to roll back; an ad hoc rollback can also require permission approval. For stable signed releases, set `NEMO_SIGN_IDENTITY` to a Developer ID Application identity when packaging. Developer ID signing, notarization, and permission continuity across signed releases still require release validation before public distribution.

### CI without hosted minutes

`.github/workflows/check.yml` contains the same local check command. It has **no push or pull-request trigger**. Manual dispatch is also gated on the repository variable `NEMO_ENABLE_HOSTED_CI=true`; leave it unset while Actions minutes are unavailable. Run `scripts/check.sh` locally before review. Hosted CI has not been run.

Logs: `~/Library/Application Support/Nemo/daemon.log` and `inference.log`. Startup failures from Finder appear in `daemon.log`; `nemo doctor` checks paths and the input device, and distinguishes CLI permissions from those reported by the running daemon. `nemo permissions` inspects its own process without opening a microphone stream. An app launched from Finder can have different permissions than a command run from a permitted terminal.

## Design

The main event loop owns the recording state and all menu updates. Microphone opening and model work happen off the UI thread. Starting, recording, and working are explicit states; busy toggles cannot create overlapping inference jobs. Cancellation marks a job before stopping its owned subprocess, so a late result cannot paste text. One authenticated loopback server serves the daemon's requests; the control socket is in a private directory. No window takes focus during dictation.

The controller is MIT licensed. The model uses OpenMDW-1.1; the native runtime uses Apache-2.0 and includes its third-party notices. Bundled runtime notices are under `Nemo.app/Contents/Resources/licenses`.
