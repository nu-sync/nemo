# Development and advanced configuration

For installation and everyday dictation, start with the [README](../README.md). Run the source-build commands below from a repository checkout.

## Cargo bootstrap

The Cargo-installed `nemo` command prepares and opens a real `Nemo.app`. The app runs the dictation daemon; it starts a resumable download when the managed model is missing. Explicit commands such as `nemo status`, `nemo config`, and `nemo model verify` do not trigger installation. `nemo run` starts a daemon directly for development and isolated lifecycle tests.

First setup uses the shared Rust build, packaging, and installation code in `crates/nemo-build`. It installs missing native build dependencies with Homebrew. The model manifest and licenses are embedded in the Cargo binary, so the source checkout is not needed afterward. Python and Bash are not required for setup or local builds. Native sources and build output are cached under `~/Library/Application Support/Nemo/runtime-builds/`.

A signed receipt records the original Cargo executable's SHA-256. Re-running the same command verifies and reuses the installed app. An update keeps the installer's busy-app refusal, signing-identity check, backup, and rollback behavior. `nemo install` prepares the app without launching it; bare `nemo` handles the launch and checks the responding daemon's executable identity.

| Environment variable | Purpose |
| --- | --- |
| `NEMO_DATA_DIR` | Absolute directory for configuration, models, logs, and build cache. |
| `NEMO_APP_PATH` | Absolute destination ending in `.app`; defaults to `~/Applications/Nemo.app`. |
| `NEMO_RUNTIME` | Override the inference executable when generating default configuration. |
| `NEMO_SIGN_IDENTITY` | Explicit signing identity for packaging. Reuse the installed signing identity for local updates; a first unsigned build uses ad hoc signing. |

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
./target/release/nemo doctor --color always
./target/release/nemo assist
./target/release/nemo assist --dry-run
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
cargo xtask check          # formatting, Clippy, workspace tests; no microphone or model
cargo xtask build          # build the release nemo executable
cargo xtask runtime        # build the pinned native Metal runtime
cargo xtask package        # create dist/Nemo.app
cargo xtask install        # build, package, safely install, and confirm the running build
cargo xtask setup          # prepare native dependencies, model, and app
cargo xtask download-model # resume model downloads and verify hashes
cargo xtask release-check  # inspect release inputs without publishing
cargo xtask privacy-check --gitleaks /path/to/gitleaks # optional source/history review
cargo xtask verify         # explicit model test using speech generated to a file
```

`xtask` is a workspace member. The `.cargo/config.toml` alias runs it through Cargo; no separately installed task runner is needed. `crates/nemo-build` owns the native builder, packager, and installer shared by `xtask` and the Cargo bootstrap. The root package remains `nemo-dictate`, and Cargo shares the workspace lockfile and build directory.

The runtime revision is pinned in `crates/nemo-build/src/native.rs`; model revisions and hashes are in `models/manifest.json`. `cargo xtask check` needs no model, native runtime, microphone, or Accessibility permission. `NEMO_DATA_DIR` selects a separate absolute data directory for tests or another installation; `NEMO_RUNTIME` overrides the default runtime for fresh configurations.

Python 3.11+ is only needed for the optional diagnostics under `scripts/`, such as `verify-package.py`, `verify-lifecycle.py`, benchmarks, and the installed-audio verification helper. These helpers do not participate in the Rust build or installation path. `cargo xtask verify` needs the native runtime and model; it generates speech to a file and transcribes it without speaker playback or microphone capture. Live dictation checks require explicit authorization when speaker playback or microphone use would be disruptive.

See [release preparation](releasing.md) for source publication and signed app distribution. The dated verification documents record earlier implementations and test runs; their old commands are historical evidence, not current setup instructions.

### Silent indicator preview

```sh
cargo run --locked -- preview-indicator --output artifacts/indicator-preview
```

This previews Recording → Transcribing → hidden using the same native panel as dictation, saves PNGs of both visible states, and checks panel visibility, labels, click-through behavior, and foreground-app preservation. It exits automatically without starting the daemon, opening the microphone, playing sounds, loading a model, or changing configuration. Omit `--output` to preview without saving images. Use this check when an audible dictation test would be disruptive; it does not verify the microphone, shortcut, transcription, or paste path.

### Safe upgrades

The installer copies and verifies the candidate before stopping an idle app, rejects upgrades during recording/transcription/downloads, retains the previous bundle in `~/Applications/.Nemo.backups/`, and restores it if replacement, signature verification, or launch fails. The daemon checks the safe-quit request on its event loop so a newly started recording cannot race the installer. An idle running app is reopened after a successful upgrade. Quit older versions that lack safe-quit support manually before installing. The configuration and model directory are retained.

Use the same signing identity for successive local builds. An ad hoc rebuild can change its macOS permission identity, so the installer refuses that upgrade by default. After reviewing the candidate, explicitly allow a permission reset if needed:

```sh
cargo xtask install --allow-permission-reset
```

Then re-enable Nemo in Accessibility if macOS requires it. Retain the backup path printed by the installer for recovery; an ad hoc rollback can also require permission approval. For stable signed releases, set `NEMO_SIGN_IDENTITY` to a Developer ID Application identity when packaging. Developer ID signing, notarization, and permission continuity across signed releases still require release validation before public distribution.

### CI without hosted minutes

`.github/workflows/check.yml` contains the same local check command. It has **no push or pull-request trigger**. Manual dispatch is also gated on the repository variable `NEMO_ENABLE_HOSTED_CI=true`; leave it unset while Actions minutes are unavailable. Run `cargo xtask check` locally before review. Hosted CI has not been run.

Logs: `~/Library/Application Support/Nemo/daemon.log` and `inference.log`. Transcription timing events in `daemon.log` report model loading, request latency, audio duration, model reuse, and success as JSON, without audio or transcript content. CLI timing events go to stderr. Startup failures from Finder appear in `daemon.log`; `nemo doctor` checks paths and the input device, and distinguishes CLI permissions from those reported by the running daemon. `nemo permissions` inspects its own process without opening a microphone stream. An app launched from Finder can have different permissions than a command run from a permitted terminal.

### Readiness and recovery

The first tray row shows readiness independently of recording activity or the last result. When it says **Right Option blocked: enable Accessibility**, click that row to open the Accessibility settings pane and enable Nemo. Permission changes are observed every two seconds. Failed shortcut registration is retried on that interval; a granted permission alone does not count as a registered shortcut.

`nemo doctor` reads the same structured readiness snapshot from the running app. It lists all known blockers and their remedies, including commands that open the appropriate macOS settings pane. Older apps without the readiness protocol are reported as unknown; a stopped app is never reported as ready. Colors are automatic in terminals, disabled for redirected output or `NO_COLOR`, and overridable with `--color always` or `--color never`.

`nemo assist` prints the next blocker and its remedy, speaks the explanation with macOS `say`, then opens the relevant System Settings pane or Nemo Settings when available. Enable the permission yourself, then rerun the command to check the next blocker. The command does not change permissions, start recording, install software, or download models. Use `--silent` to skip speech, or `--dry-run` to print guidance without speech or opening settings. Speech failures still allow the settings action to run and are reported as errors. Assistance reads the running app's readiness without loading local configuration.

`src/readiness.rs` derives readiness from observed permissions, shortcut registration, microphone discovery, model assets, and runtime availability. The event loop owns these observations and projects its existing lifecycle into an `Activity` enum. Readiness does not require a loaded model: the inference worker can load it during recording.

| Observation | Readiness transition |
| --- | --- |
| Accessibility absent for Right Option | Accessibility blocker; settings action available |
| Accessibility granted, registration unsuccessful | Shortcut unavailable; retry every two seconds |
| Shortcut registered and all dependencies available | Ready when idle |
| Permission revoked or dependency lost | Corresponding blocker returns |
| Recording/transcription begins | Activity changes; existing blockers remain visible |

## Verify the MacBook audio path

Use the project skill [$verify-nemo](../.agents/skills/verify-nemo/SKILL.md) to test the installed app with real speaker playback and microphone capture. It checks loading during recording, start → stop → transcribe, actual text delivery, and the full five-minute idle timeout. The skill includes permission checks, evidence capture, and cleanup. It requires an audible speaker output, authorized microphone access, and a controlled paste destination.

## Design

The app and `xtask` share the Rust native builder, packaging, and installer library; the developer command does not wrap Python or Bash scripts. The main event loop owns the recording state, indicator, and all menu updates. Microphone opening and model work happen off the UI thread. Starting, recording, and working are explicit states; busy toggles cannot create overlapping inference jobs. Working records the job kind so only transcription shows the processing indicator. Cancellation marks a job before stopping its owned subprocess, so a late result cannot paste text. One authenticated loopback server serves the daemon's requests; the control socket is in a private directory. No window takes focus during dictation.

The indicator is a borderless, nonactivating AppKit panel with click-through behavior, positioned above the Dock on the pointer's display. Recording appears only after microphone startup succeeds; stopping switches to Transcribing, and completion, cancellation, errors, and shutdown hide it. It joins Spaces and full-screen apps. This adapts [Handy's documented overlay](https://handy.computer/docs/advanced#overlay-position) and its macOS [panel](https://github.com/cjpais/Handy/blob/7bfb2e2c934b798bcf6e3627653aed2439996f89/src-tauri/src/overlay.rs#L450-L485), [placement](https://github.com/cjpais/Handy/blob/7bfb2e2c934b798bcf6e3627653aed2439996f89/src-tauri/src/overlay.rs#L180-L277), and [stop transition](https://github.com/cjpais/Handy/blob/7bfb2e2c934b798bcf6e3627653aed2439996f89/src-tauri/src/actions.rs#L640-L652) at commit `7bfb2e2c934b798bcf6e3627653aed2439996f89`. Handy uses a Tauri webview; Nemo draws its own native pill without reusing Handy code or assets. Handy's live docs describe position and visibility together, while that source revision separates overlay style and position.

The controller is MIT licensed. The model uses OpenMDW-1.1; the native runtime uses Apache-2.0 and includes its third-party notices. Bundled runtime notices are under `Nemo.app/Contents/Resources/licenses`.

## Model and runtime

Uses [NVIDIA Nemotron 3.5 Transcribe 30B-A4B](https://huggingface.co/nvidia/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B), the [AtomicChat IQ4_NL conversion](https://huggingface.co/AtomicChat/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-GGUF), and NVIDIA's [NeMo-Speech.cpp](https://github.com/NVIDIA/NeMo-Speech.cpp) Metal backend. The controller, microphone capture, shortcut, and paste integration are Rust. Inference runs in a managed native C++ subprocess. No Python environment, cloud service, or API key is required at runtime.

## Microphone selection

Choose **Settings → Microphone** to use the system default or a specific input.
Nemo saves the device ID and name in `config.toml`; an omitted `microphone` table
uses the system default. The menu and Settings show the current input. Available
inputs refresh every two seconds without opening a recording stream.

If a saved device is unavailable, Nemo asks before using the current system default
for one recording. Cancel leaves the microphone closed. The saved device remains
selected and is used again when it reconnects. Disconnecting an active input stops
and discards the recording; Nemo does not switch inputs mid-recording. With no
usable default, recording reports an error.

To verify on macOS, select each available input and record, reopen Nemo to check
persistence, then disconnect and reconnect the selected input. Check both Cancel
and Use system default in the fallback prompt, confirm reconnection restores the
saved input, and verify an active disconnect discards the recording. Check the
empty-device case on a host with no inputs. Follow the `verify-nemo` skill for the
acoustic transcription and paste journey; unit tests simulate device inventories.

For a brief hardware stream check that discards captured samples without saving
or transcribing them:

```sh
cargo test --locked audio::tests::real_microphone_opens_by_id_and_missing_selection_requires_confirmation -- --ignored --exact
```

This opens the current system-default input both automatically and by its saved
ID, checks that samples arrive, and verifies a missing device requires fallback
confirmation before opening a stream. It does not replace the UI, physical
unplug/reconnect, or installed-app transcription/paste checks.
