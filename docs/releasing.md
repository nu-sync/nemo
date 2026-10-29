# Preparing a public release

Making the source repository public and distributing a downloadable macOS app are separate release decisions. This document records the gates for each. The repository tooling does not change repository visibility, publish packages, create tags, or upload release assets.

## Publish the source repository

Before changing visibility:

- Run `cargo xtask check`, `cargo xtask release-check`, and `cargo xtask privacy-check --gitleaks /path/to/gitleaks` against the intended commit. Gitleaks is an optional release-review tool; it is not needed to build or install Nemo.
- Review the full Git history as well as the current tree for credentials, private URLs, signing material, recordings, transcripts, and personal data. Rotate any exposed credential before publication; deleting the current file does not remove it from history.
- Keep `target/`, `dist/`, `vendor/`, `artifacts/`, model weights, and local configuration out of the source release. Inspect the actual source archive and Cargo package file lists.
- Confirm that the README's public repository URL, supported macOS version, Apple Silicon restriction, storage requirement, installation commands, and license links match the release.
- Retain the controller's MIT license, Handy sound attribution, the model license and pinned manifest, and native runtime notices.
- Plan the visibility change and private-reporting activation together. GitHub supports [private vulnerability reporting](https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/configure-vulnerability-reporting/configure-for-a-repository) for public repositories. The endpoint is unavailable while this repository is private. Immediately after making it public, enable the setting and confirm the private reporting form before announcing the source release.
- Decide whether to enable hosted CI. The current workflow is manual and requires `NEMO_ENABLE_HOSTED_CI=true`; making the repository public does not change that setting.

The dated performance and verification documents describe specific local runs. Their measurements are not general hardware requirements or guarantees. Their older commands are historical; use the current development guide for setup.

## Validate installation from source

Use a fresh checkout of the intended commit and an isolated app/data directory. Verify that the Cargo-installed launcher prepares an app without relying on an existing checkout, Python, Bash, or previously downloaded model weights. Verify a second identical installation reuses the app and that a changed build retains the signing-identity and busy-app guards.

The source installation path is:

```sh
cargo install --locked --git https://github.com/nu-sync/nemo.git nemo-dictate
nemo
```

Record the commit used for the release. Pin a tag or revision when validating a candidate so later branch changes cannot change the tested inputs. A source install still needs Rust, Homebrew, Xcode Command Line Tools, native compilation, and the model download. It is not equivalent to a prebuilt app download.

## Prepare a downloadable app

Use `cargo xtask package` to build a candidate. Keep the candidate separate from the working installed app until it passes review.

| Component | License and release input |
| --- | --- |
| Nemo controller and Rust tooling | MIT; retain `LICENSE` |
| Nemotron model | OpenMDW-1.1; retain `models/LICENSE` and the pinned download manifest |
| NeMo-Speech.cpp runtime | Apache-2.0 plus its bundled third-party notices; retain the upstream notices in the app |
| Handy Marimba sounds | MIT; retain `assets/sounds/Handy-MIT.txt` |
| Rust and native dependencies | Review dependency licenses and include all notices required for the shipped artifact |

A local Apple Development identity is useful for maintaining a developer's local signing identity. It does not establish public app distribution readiness. Before distributing a downloadable app, use an appropriate Developer ID Application identity, validate the entitlements and hardened runtime, and complete Apple's [notarization workflow](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution). Staple and validate the notarization ticket where applicable. Keep signing keys and notarization credentials outside the repository and release archives.

Verify the actual downloadable archive:

- Its SHA-256, release version, source commit, runtime revision, and model revision are recorded with the release.
- Strict/deep code-signature verification succeeds after extracting it in a new location.
- Native libraries resolve inside the app or to macOS system libraries; no developer-specific Homebrew paths remain.
- The bundle contains its licenses, notices, model manifest, and installation receipt, but no model weights, local configuration, recordings, transcripts, or credentials.
- A clean supported Mac can open the downloaded artifact through the normal macOS launch path. Gatekeeper, notarization, permissions, and first-run downloads are checked using that artifact.
- Two successive signed builds preserve expected permission behavior. Recording, transcription, cancellation, paste, shutdown, and model unloading are verified when real-audio testing is authorized.
- Busy upgrade refusal, startup confirmation, rollback, and recovery from the retained backup are exercised.

A successful `open` command only means macOS accepted a launch request. Confirm that the responding daemon reports the expected startup build identity. The silent indicator preview validates the panel; it does not replace the microphone, shortcut, transcription, or paste checks.

## Source-release checkpoint — October 29, 2026

See the [privacy review](privacy-review.md) for the examined history, current source, scanner provenance, and limits.

- [x] Review reachable Git history, commit messages, current source paths, URLs, and binary assets.
- [x] Run Gitleaks against all reachable history and a clean candidate-source snapshot; retain redacted reports outside version control.
- [x] Preserve the maintainer's public author identity, as confirmed by the owner; no history rewrite is needed.
- [x] Retain the MIT license, model license and manifest, Handy sound attribution, and pinned native-runtime notices.
- [x] Add contribution guidance, changelog, a pinned Rust toolchain, and Rust-only build/install commands.
- [x] Keep hosted CI manual and opt-in; pushing this change will not automatically consume hosted minutes.
- [x] Inspect the clean source archive and Cargo file list; pass workspace checks and isolated clean-source installation, as recorded in the privacy review.
- Commit and push the reviewed tree, then compare the local commit with `origin/main` before changing visibility.
- [x] Owner authorized the visibility change; `nu-sync/nemo` is public as of October 29, 2026.
- [x] Enable private vulnerability reporting immediately after the visibility change; GitHub's settings API confirmed `enabled: true` on October 29, 2026. Report privately through the [security reporting form](https://github.com/nu-sync/nemo/security/advisories/new).

The source release does not include a prebuilt app or model weights. Developer ID signing, notarization, Rust/native dependency notice completion for a shipped binary, Gatekeeper validation, and final real-audio checks remain gates for a future downloadable-app release. Publishing to crates.io is also a separate decision and requires validating its packaged path dependencies and source contents.
