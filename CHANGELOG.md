# Changelog

## Unreleased

- Show a native recording and transcription indicator above the Dock.
- Build, package, install, and check Nemo through a Rust Cargo workspace and `cargo xtask`.
- Share packaging and guarded installation between local development and Cargo bootstrap.
- Report the running daemon's executable fingerprint for post-install confirmation.
- Keep the model loaded, and the selected engine selected, after a dictation with no speech.
- Keep the model loaded through a dictation longer than the idle timeout, and when canceling while audio is finishing or text is being pasted.
- Canceling during the launch preload no longer makes Nemo fall back to the included engine.

## 0.2.0

- Local Nemotron dictation on Apple Silicon Macs running macOS 26 or newer.
- Global shortcut, native settings, model management, and automatic paste.
