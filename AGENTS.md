# Repository Guidelines

## Project Inspiration

Nemo is heavily inspired by [Handy](https://github.com/cjpais/Handy). Use [research-handy](.agents/skills/research-handy/SKILL.md) to find features in its docs and trace their implementation when building additional features.

## Project Structure & Module Organization

Nemo is a local dictation app for Apple Silicon Macs running macOS 26+. The Rust 2024 binary is `nemo` (`nemo-dictate` package).

- `src/`: CLI and menu event loop (`main.rs`), audio, inference backend, model management, configuration, settings, shortcuts, and paste integration.
- `tests/cli.rs`: CLI integration tests; unit tests live beside Rust implementations.
- `scripts/`: runtime builds, packaging, installation, and verification; Python tests live in `scripts/tests/`.
- `assets/sounds/`: bundled audio and licenses. `models/manifest.json` pins model downloads; `vendor/NeMo-Speech.cpp/` holds native runtime sources.
- `docs/`: development, installation, performance, and verification details.

## Build, Test, and Development Commands

Run commands from the repository root. Use Rust, Python 3.11+, Homebrew, and Xcode Command Line Tools.

- `cargo build --release --locked`: build `target/release/nemo`.
- `cargo run --locked -- run`: start the development daemon directly.
- `scripts/check.sh`: run formatting, Clippy with warnings denied, Rust tests, Python tests, and shell syntax checks. No model or microphone permissions required.
- `scripts/package.sh`: build `dist/Nemo.app`.
- `scripts/verify.sh`: run checks and real model transcription; requires downloaded weights and the native runtime.

See `docs/development.md` for setup and installation commands.

## Coding Style & Naming Conventions

Use `cargo fmt` and keep `cargo clippy --all-targets --locked -- -D warnings` clean. Follow existing four-space indentation, Rust `snake_case` functions/modules and `PascalCase` types, and Python standard-library conventions. Keep modules focused and changes small. The main event loop owns recording state and menu updates; keep microphone opening and inference work off the UI thread.

## Testing Guidelines

Use Rust `#[test]` and Python `unittest`; name tests after observable behavior (`fresh_configuration_works_outside_the_checkout`, `test_*`). Add regression coverage for changed behavior. No numeric coverage threshold is configured. Run `scripts/check.sh` before review. For dictation changes, follow `.agents/skills/verify-nemo/SKILL.md` to verify real recording, transcription, paste, and unloading.

## Commit & Pull Request Guidelines

Recent commits use `feat: ...` and `feat(install): ...`; prefer concise, imperative conventional subjects. PRs should explain the problem, resulting behavior, and exact validation commands/results. Link relevant issues and include screenshots for visible UI changes.

## Configuration & Agent Guidance

For isolated installation tests, set both `NEMO_DATA_DIR` and `NEMO_APP_PATH` to temporary absolute paths. Preserve pinned revisions and bundled licenses. Use `$wcygan-mode` for substantive engineering work when available; follow repository conventions and verify actual behavior.
