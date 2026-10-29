# Contributing to Nemo

Nemo targets Apple Silicon Macs running macOS 26 or newer. Install Rust, Homebrew, and Xcode Command Line Tools before working on the app.

## Build and check

From the repository root:

```sh
cargo xtask check
cargo xtask build
cargo xtask package
```

`check` runs formatting, Clippy with warnings denied, and workspace tests without model weights, microphone access, or audio playback. Packaging needs the pinned native runtime; use `cargo xtask runtime` to build it.

Use `cargo xtask install` to update your local app. It preserves the signing-identity guard, refuses to interrupt active dictation, and retains the previous bundle. Set both `NEMO_DATA_DIR` and `NEMO_APP_PATH` to temporary absolute paths for isolated installation tests.

## Make a change

Keep modules focused, follow the existing Rust naming conventions, and cover changed behavior with regression tests. The main event loop owns recording state and native UI updates; microphone setup and inference belong off the UI thread. Preserve pinned model/runtime revisions and bundled license notices.

A pull request should explain the problem, resulting behavior, and exact verification commands. Include screenshots for visible UI changes. State which microphone, paste, signing, and installation paths were actually exercised; successful compilation does not prove those paths.

Use the [development guide](docs/development.md) for configuration and the [verify-nemo skill](.agents/skills/verify-nemo/SKILL.md) for authorized real-audio checks. Those checks can play speech and open the microphone. Respect requests to keep verification silent.

## Reports and private data

For ordinary bugs, include the Nemo version, macOS version, reproduction steps, and relevant sanitized logs. Do not attach recordings, clipboard contents, full configuration files, signing keys, or access tokens.

Do not disclose an exploitable vulnerability in a public issue. Use GitHub's **Report a vulnerability** action on the repository's Security page when it is available. If that private reporting channel is unavailable, request a private reporting route without posting exploit details or private data. The [release checklist](docs/releasing.md) requires enabling and verifying that channel immediately after changing repository visibility and before announcing the public source release.
