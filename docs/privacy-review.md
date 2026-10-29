# Public-source privacy review — October 29, 2026

This review covers the five reachable commits ending at `2a3b1f7afbc8fe161ba0443d244ab86662ca0fc7` and the candidate source tree prepared for the Rust workflow and recording-indicator commit. It is an exposure review for source publication, not a comprehensive vulnerability audit or approval to distribute a macOS binary.

## Reviewed content

- All reachable branches and tags, commit messages, author metadata, and the current non-ignored source tree.
- Historical blob/path inventory, including removed files. The only historical binary assets are the two licensed Handy Marimba WAV cues. There are no user recordings, screenshots, model weights, app bundles, or signing files in that inventory.
- Source and historical URLs: public upstream repositories, documentation, model download hosts, and loopback test/runtime addresses. No private service endpoint or real home-directory path was found; `/Users/test` paths are synthetic tests.
- Documentation and fixture text: hardware measurements and synthetic or attributed reference phrases. Local clipboard contents, captured microphone audio, raw transcripts, UI observations, and detailed test artifacts remain outside version control.
- The maintainer's name and author email remain in Git metadata. The owner confirmed that this identity is already used publicly; history was preserved.

No credentials, signing keys, private service URLs, or sensitive user content requiring removal were found. Standard author attribution, upstream copyright notices, and documented hardware measurements were intentionally retained.

## Automated checks

Gitleaks **8.30.1**, using its default rules, reported no findings for all reachable history or a clean snapshot of the candidate source. Inline `gitleaks:allow` comments were ignored, and results were redacted. The official Apple Silicon archive was verified against the release's SHA-256:

```text
b40ab0ae55c505963e365f271a8d3846efbc170aa17f2607f13df610a9aeb6a5
```

The scanner was downloaded from the [official v8.30.1 release](https://github.com/gitleaks/gitleaks/releases/tag/v8.30.1) into ignored local artifacts. It is not a build or runtime dependency.

Re-run the review with an installed or separately verified scanner:

```sh
cargo xtask privacy-check --gitleaks /path/to/gitleaks
```

The command scans Git history and a snapshot containing only versioned/non-ignored candidate files. It rejects generated directories, credential/signing paths, unexpected audio/model files, symlinks, and files over 1 MiB for manual review. Redacted scanner output and scope summaries are retained under ignored `artifacts/privacy-review/`. `.gitignore` also excludes local credential and signing files.

A clean scan cannot prove the absence of every secret or privacy issue. Manual inspection of history, references, author metadata, media, and documentation remains part of the release checklist. Ignoring a path does not remove previously committed data.

## Source-release validation

The source archive was inspected for generated/private files and contained only the intended source, documentation, licenses, manifest, and the two licensed cue assets. Cargo's package file list was inspected separately; crates.io publication remains outside this source release.

The clean-source validation used archive SHA-256 `5cb1c65f58cf23d1535b260596525f334f51bb4ce05cde52d54a05a14d267743`, from candidate tree `1d74801816f4dc73658e3dcbf5733512fcdcd683`. It passed:

- `cargo xtask check`: formatting, workspace Clippy with warnings denied, and Rust tests.
- `cargo xtask release-check`: release documents and pinned model inputs.
- Cargo installation from the extracted source archive into a temporary prefix, using a fresh target directory and no `vendor/` or model weights in the source.
- The installed launcher's `nemo install`, run outside the extracted source directory with isolated `NEMO_DATA_DIR` and `NEMO_APP_PATH`. This fetched and built the pinned native source using the Rust bootstrap and produced an ad hoc signed app with a matching payload receipt.
- Strict/deep verification of that app's code signature and a second identical installation that reused it.
- No model download, app launch, microphone capture, clipboard operation, or speaker playback during this release validation.

The final documentation records these results; the tested Rust implementation did not change afterward. Redacted logs, source inventory, archive digest, and source-install results are retained locally under ignored `artifacts/privacy-review/`. The owner authorized publication on October 29, 2026. GitHub confirmed the repository is public and private vulnerability reporting is enabled. The pushed commit is reported separately so this document does not embed its own commit ID.

GitHub's private-vulnerability-reporting endpoint was unavailable during the private-source review. Immediately after publication, reporting was enabled and verified through the settings API. Vulnerabilities can now be submitted through the [private reporting form](https://github.com/nu-sync/nemo/security/advisories/new).

Downloadable-app distribution remains separate: no notarization, public binary upload, or new acoustic test was performed for this source review. See the [release checklist](releasing.md).
