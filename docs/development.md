# Development and advanced configuration

For installation and everyday dictation, start with the [README](../README.md). Run the source-build commands below from a repository checkout.

## Cargo bootstrap

The Cargo-installed `nemo` command prepares and opens a real `Nemo.app`. The app runs the dictation daemon; it starts a resumable download when the managed model is missing. Explicit commands such as `nemo status`, `nemo config`, and `nemo model verify` do not trigger installation. `nemo run` starts a daemon directly for development and isolated lifecycle tests.

First setup uses the pinned native build script and the existing Python standard-library packager and installer. It installs missing build dependencies with Homebrew; Python is used for setup, with no virtual environment or pip packages. The scripts, model manifest, and licenses are embedded in the Cargo binary, so the source checkout is not needed afterward. Native sources and build output are cached under `~/Library/Application Support/Nemo/runtime-builds/`.

A signed receipt records the original Cargo executable's SHA-256. Re-running the same command verifies and reuses the installed app. An update keeps the installer's busy-app refusal, signing-identity check, backup, and rollback behavior. `nemo install` prepares the app without launching it; bare `nemo` handles the launch and checks the responding daemon's executable identity.

| Environment variable | Purpose |
| --- | --- |
| `NEMO_DATA_DIR` | Absolute directory for configuration, models, logs, and build cache. |
| `NEMO_APP_PATH` | Absolute destination ending in `.app`; defaults to `~/Applications/Nemo.app`. |
| `NEMO_RUNTIME` | Override the inference executable when generating default configuration. |
| `NEMO_SIGN_IDENTITY` | Signing identity for packaging; defaults to ad hoc signing. |

For an isolated bootstrap test, set **both** `NEMO_DATA_DIR` and `NEMO_APP_PATH`. Changing only the data directory still uses the normal app destination. The launcher and installer forward data/runtime overrides when opening or restoring the app.

To build and verify a separate app without opening it or downloading model weights:

```sh
nemo_check=$(mktemp -d /tmp/nemo-bootstrap.XXXXXX)
cargo install --locked --path . --root "$nemo_check/cargo"
NEMO_DATA_DIR="$nemo_check/data" NEMO_APP_PATH="$nemo_check/Nemo.app" \
  "$nemo_check/cargo/bin/nemo" install
codesign --verify --deep --strict "$nemo_check/Nemo.app"
```

This can install missing build dependencies through Homebrew. Run the same isolated `install` command again to check reuse. `nemo install` does not open the app; bare `nemo` opens it and can start the model download. Keep both overrides when testing that path.

## Configure

Choose **Settings…** in the menu (or run `nemo settings`) to toggle **Remove trailing period**. Changes save immediately and apply to subsequent transcription output without restarting. The option is off by default and removes only the final `.`.

**Launch on startup** opens Nemo when you log in to your Mac. It uses macOS Login Items and reflects changes made in System Settings. If approval is required, enabling it opens Login Items. This setting is available when running Nemo.app.

For other preferences, use **Open configuration** in the menu, or:

```sh
./target/release/nemo config --hotkey RightOption
```

Quit and reopen Nemo to apply changes. Configuration lives in `~/Library/Application Support/Nemo/config.toml`. Available settings:

```toml
hotkey = "RightOption"
idle_unload_seconds = 60
max_recording_seconds = 120
paste = true
remove_trailing_period = false
sound_pack = "handy_marimba"
preload_model = false
model_warmup = false
inference_threads = 0
encoder_batch_size = 1
```

`model` and `runtime` are absolute paths; the generated configuration includes both. Model weights live in `~/Library/Application Support/Nemo/models/<pinned revision>/`, independently of this checkout and app upgrades. The installed app includes the inference executable and its libraries. Explicit custom paths remain supported.

For an older checkout with downloaded weights, migrate without downloading again:

```sh
./target/release/nemo model migrate --from "$PWD/models"
```

Migration verifies every SHA-256 first, publishes the managed files, and updates configuration while preserving preferences. It leaves compatibility symlinks so an already-running older app can still load its captured paths. Keep those symlinks until that app has restarted. On the same volume, migration uses hard links and needs no second copy of the weights.

Combinations such as `Control+Shift+Space` are also supported as the `hotkey` value.

For a latency-focused configuration on a Mac with ample unified memory, use `idle_unload_seconds = 300`. Model loading overlaps recording; `preload_model = false` and `model_warmup = false` avoid unnecessary startup work. The loaded model uses about 20 GB of resident memory. `inference_threads = 0` selects the runtime default (eight on this laptop); `encoder_batch_size` accepts 1–16. See [performance measurements and profiling](performance.md) before tuning these settings.

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

Requires Apple Silicon, macOS 26+, Rust, Homebrew, and Xcode Command Line Tools. The two model files total **21.35 GB**. The loaded model used about **20 GB of memory** in the M5 Max measurements; leave additional room for build outputs and other applications. This is currently macOS-only.

```sh
scripts/setup.sh          # install build dependencies, build, download, verify hashes, install
open "$HOME/Applications/Nemo.app"
scripts/download-model.sh # resume model downloads and verify SHA-256
scripts/build-runtime.sh # rebuild the pinned Metal runtime
scripts/package.sh       # build dist/Nemo.app without changing the installed app
scripts/install.sh       # stage, verify, and install; retain the previous app
scripts/check.sh         # local CI: formatting, Clippy, Rust and installer tests
scripts/verify.sh        # static checks, tests, actual synthetic-speech transcription
python3 scripts/verify-package.py --app dist/Nemo.app # relocated bundle and library checks
python3 scripts/verify-lifecycle.py # cancellation, single instance, idle unload, SIGTERM (quit Nemo first)
```

The runtime and model revisions are pinned in `scripts/build-runtime.sh` and `models/manifest.json`. Use Python 3.11+ for setup and verification scripts. `scripts/check.sh` needs no model, native runtime, microphone, or Accessibility permission. `scripts/verify.sh` creates sample speech with macOS `say`, uses an isolated data directory, and compares the actual model transcript against the spoken words. `NEMO_DATA_DIR` selects a separate absolute data directory for tests or another installation; `NEMO_RUNTIME` overrides the default runtime for fresh configurations.

### Safe upgrades

The installer copies and verifies the candidate before stopping an idle app, rejects upgrades during recording/transcription/downloads, retains the previous bundle in `~/Applications/.Nemo.backups/`, and restores it if replacement, signature verification, or launch fails. The daemon checks the safe-quit request on its event loop so a newly started recording cannot race the installer. An idle running app is reopened after a successful upgrade. Quit older versions that lack safe-quit support manually before installing. The configuration and model directory are retained.

Local builds use ad hoc signatures. A rebuilt app can change its macOS permission identity, so the installer refuses that upgrade by default. After reviewing the candidate, explicitly allow a permission reset if needed:

```sh
python3 scripts/install_app.py --source dist/Nemo.app --allow-permission-reset
```

Then re-enable Nemo in Accessibility if macOS requires it. Use the retained backup as `--source` to roll back; an ad hoc rollback can also require permission approval. For stable signed releases, set `NEMO_SIGN_IDENTITY` to a Developer ID Application identity when packaging. Developer ID signing, notarization, and permission continuity across signed releases still require release validation before public distribution.

### CI without hosted minutes

`.github/workflows/check.yml` contains the same local check command. It has **no push or pull-request trigger**. Manual dispatch is also gated on the repository variable `NEMO_ENABLE_HOSTED_CI=true`; leave it unset while Actions minutes are unavailable. Run `scripts/check.sh` locally before review. Hosted CI has not been run.

Logs: `~/Library/Application Support/Nemo/daemon.log` and `inference.log`. Transcription timing events in `daemon.log` report model loading, request latency, audio duration, model reuse, and success as JSON, without audio or transcript content. CLI timing events go to stderr. Startup failures from Finder appear in `daemon.log`; `nemo doctor` checks paths and the input device, and distinguishes CLI permissions from those reported by the running daemon. `nemo permissions` inspects its own process without opening a microphone stream. An app launched from Finder can have different permissions than a command run from a permitted terminal.

## Verify the MacBook audio path

Use the project skill [$verify-nemo](../.agents/skills/verify-nemo/SKILL.md) to test the installed app with real speaker playback and microphone capture. It checks loading during recording, start → stop → transcribe, actual text delivery, and the full five-minute idle timeout. The skill includes permission checks, evidence capture, and cleanup. It requires an audible speaker output, authorized microphone access, and a controlled paste destination.

## Design

The main event loop owns the recording state and all menu updates. Microphone opening and model work happen off the UI thread. Starting, recording, and working are explicit states; busy toggles cannot create overlapping inference jobs. Cancellation marks a job before stopping its owned subprocess, so a late result cannot paste text. One authenticated loopback server serves the daemon's requests; the control socket is in a private directory. No window takes focus during dictation.

The controller is MIT licensed. The model uses OpenMDW-1.1; the native runtime uses Apache-2.0 and includes its third-party notices. Bundled runtime notices are under `Nemo.app/Contents/Resources/licenses`.

## Model and runtime

Uses [NVIDIA Nemotron 3.5 Transcribe 30B-A4B](https://huggingface.co/nvidia/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B), the [AtomicChat IQ4_NL conversion](https://huggingface.co/AtomicChat/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-GGUF), and NVIDIA's [NeMo-Speech.cpp](https://github.com/NVIDIA/NeMo-Speech.cpp) Metal backend. The controller, microphone capture, shortcut, and paste integration are Rust. Inference runs in a managed native C++ subprocess. No Python environment, cloud service, or API key is required at runtime.
