# Cross-platform assessment

Researched October 28, 2026. Nemo currently supports Apple Silicon macOS. Windows and Linux are feasible porting targets, but neither is implemented or tested. No installation, restart, runtime upgrade, or platform refactor was performed for this assessment.

Handy source was inspected at commit [`7bfb2e2c934b798bcf6e3627653aed2439996f89`](https://github.com/cjpais/Handy/commit/7bfb2e2c934b798bcf6e3627653aed2439996f89), version 0.9.8. Native runtime findings use Nemo's existing pinned NVIDIA revision `6a62501107e1894723880f1094a286ea996a0ec7`.

## What Handy does

Handy supports Windows, Linux, and macOS, with a Rust/Tauri desktop backend. Its release matrix includes macOS Intel and ARM, Windows x64 and ARM64, and Linux x64 and ARM64. That demonstrates a maintained platform strategy; it does not prove our model works on each target. See Handy's [README](https://github.com/cjpais/Handy/blob/7bfb2e2c934b798bcf6e3627653aed2439996f89/README.md), [release matrix](https://github.com/cjpais/Handy/blob/7bfb2e2c934b798bcf6e3627653aed2439996f89/.github/workflows/release.yml), and [build guide](https://github.com/cjpais/Handy/blob/7bfb2e2c934b798bcf6e3627653aed2439996f89/BUILD.md).

Its Cargo configuration selects native dependencies and inference features per OS and architecture: Metal on Apple Silicon, CPU on Intel Macs and Windows ARM, and Vulkan with dynamic backends on Windows x64 and Linux. It uses `handy-recorder` for capture, `handy-keys` and a Tauri shortcut backend, clipboard integration, and Enigo for input injection. These are useful reference points without requiring Nemo to adopt Tauri. [Cargo configuration](https://github.com/cjpais/Handy/blob/7bfb2e2c934b798bcf6e3627653aed2439996f89/src-tauri/Cargo.toml).

Linux needs distinct desktop integration paths. Handy's paste code tries different native tools on Wayland and X11; GNOME and KDE Wayland do not take the same wtype route as other compositors. Its GNOME Wayland troubleshooting guide uses `ydotool` for typing and `handy_keys` for shortcuts. [Paste implementation](https://github.com/cjpais/Handy/blob/7bfb2e2c934b798bcf6e3627653aed2439996f89/src-tauri/src/clipboard.rs), [GNOME guide](https://github.com/cjpais/Handy/blob/7bfb2e2c934b798bcf6e3627653aed2439996f89/docs/troubleshooting/ubuntu-26-04-gnome-wayland/README.md).

The reusable [handy-keys library](https://github.com/handy-computer/handy-keys) supports modifier-only shortcuts. It uses low-level hooks on Windows and reads evdev devices on Linux; blocking additionally uses uinput. Linux device access therefore affects installation. Its current shipping guidance recommends active-seat udev `uaccess` rules rather than asking users to join the input group. This is a candidate to evaluate, not a dependency adopted by this task.

Handy's built-in catalog lists Nemotron 3.5 ASR Streaming **0.6B**, but does not list our **NVIDIA-Nemotron-3.5-Transcribe-30B-A4B** model. Those are different models. Catalog absence does not establish that every custom integration is impossible; adding our runtime would still need a separate compatibility experiment. [Pinned model catalog](https://github.com/cjpais/Handy/blob/7bfb2e2c934b798bcf6e3627653aed2439996f89/src-tauri/src/catalog/catalog.json).

## Inference portability

The pinned NVIDIA runtime documents speech-LLM/SALM operation on CPU, Vulkan, Metal, and CUDA. Its upstream llama.cpp path supports IQ4_NL, the decoder format we already use. Patched CUDA has a separate batched/speculative path with different GPU/toolkit requirements. Native Windows build documentation covers MSVC/Ninja with CUDA, Vulkan, and CPU, and the server presets include speech-LLM transcription. This makes retaining the model and runtime a reasonable first experiment; it is not a successful Windows/Linux run. [SALM platforms](https://github.com/NVIDIA/NeMo-Speech.cpp/blob/6a62501107e1894723880f1094a286ea996a0ec7/docs/salm/models.md), [server presets and build requirements](https://github.com/NVIDIA/NeMo-Speech.cpp/blob/6a62501107e1894723880f1094a286ea996a0ec7/docs/build.md), [Windows build guide](https://github.com/NVIDIA/NeMo-Speech.cpp/blob/6a62501107e1894723880f1094a286ea996a0ec7/docs/development/windows-build.md).

The model remains a roughly 21.35 GB download. The Mac's unified-memory measurements do not establish discrete-GPU VRAM requirements, offload behavior, or interactive latency elsewhere. Keep hardware qualification separate from OS support and measure it on the target machine.

## Nemo's current blockers

| Area | Current implementation | Porting work |
|---|---|---|
| Capture | CPAL and in-memory WAV | Retain; verify real devices and failure recovery per OS |
| Tray and event loop | Tao/tray-icon with unconditional macOS extensions | Retain shared shell; select native extensions with target configuration |
| Shortcut | AppKit Right Option monitoring | Preserve Mac behavior; add Windows/Linux backend and configurable combinations |
| Permissions | AVFoundation and Accessibility | Native permission/status adapter per OS; Linux desktop/device capability checks |
| Clipboard/paste | pbcopy and CoreGraphics Cmd+V | Native clipboard and input injection; distinguish Linux X11 from Wayland |
| Feedback | afplay and macOS system sounds | Bundle small sound assets and use portable playback |
| Storage/config | Application Support, Unix locks, POSIX publication | OS-specific data roots, portable locking, and tested atomic replacement |
| IPC/logging/randomness | Unix socket, flock, dup2, /dev/urandom | Windows-compatible IPC, process-owned logging, OS randomness |
| Inference launch | Hard-coded Metal device | Backend selection and target-specific runtime packaging |
| Download/install/checks | /usr/bin/curl; codesign/ditto; Darwin-only checks | Portable downloads, native installers, and local target check commands |

These are compile-time and runtime dependencies, not merely missing installers. Evidence is in `Cargo.toml`, `src/main.rs`, `src/shortcut.rs`, `src/permissions.rs`, `src/output.rs`, `src/control.rs`, `src/storage.rs`, `src/config.rs`, `src/model.rs`, and `src/backend.rs`.

## Recommended sequence

Keep Nemo's small Rust controller and native subprocess. Introduce a few OS modules and target-specific Cargo dependencies after the profiling work settles. CPAL, Tao, and tray-icon already supply shared building blocks; a webview/settings framework is not required to achieve portability. Preserve Right Option on macOS while making other platforms' shortcuts configurable and checking international keyboard layouts.

1. Prove the exact pinned weights can transcribe the same reference WAV through the native HTTP server on Windows x64 and Linux x64. Start with a suitably provisioned NVIDIA/CUDA machine; evaluate Vulkan/CPU separately. Record memory, transcript correctness, cancel/quit behavior, and latency. A build alone is insufficient.
2. Port the common CLI/model-store path and OS boundaries while preserving the Mac behavior. Keep the existing request protocol and state ownership. Evaluate Handy's shortcut library selectively, with real tap/chord tests, rather than replacing the application shell.
3. Complete Windows x64 recording, clipboard, paste, tray, permissions/device errors, and installation. Then qualify Linux X11, followed by explicit GNOME Wayland and KDE Wayland targets. Linux clipboard-only or externally configured `nemo toggle` can be a useful intermediate result, but should not be advertised as full Handy-equivalent support.
4. Verify installation and upgrade/rollback on each target, then extend the opt-in workflow matrix. Until Actions minutes are available, run checks on local machines or VMs. Actual microphone, physical shortcut, paste, and GPU tests require the target desktop/hardware; containers or cross-compilation alone do not establish them.

A Handy fork is an alternative if its broader UI and existing packaging become more valuable than Nemo's minimal footprint. It would still require proving support for the exact 30B model and its runtime. My recommendation is the smaller native Nemo port, borrowing targeted ideas or libraries from Handy.

## Checkpoint boundaries

Installation changes were committed locally as `e44460f` using a separate verified source snapshot. Profiling files and overlapping Rust changes were left uncommitted in the live checkout. No GitHub repository was created or pushed while publication remains on hold. The existing local Git repository already existed, with no commits before this checkpoint.

**Separate Before Serializing Shared State** shaped the isolated commit snapshot, protecting the profiling work. **Prove It Works** required checks against that exact snapshot: 20 unit tests, 5 CLI tests, and 11 installer tests, plus formatting and Clippy. Windows/Linux findings above are source-backed feasibility conclusions, not tested platform support.
