# Installation reliability checkpoint — October 28, 2026

The installation candidate is ready for review. The installed app was not replaced or restarted. Nothing was pushed, published, or dispatched to GitHub Actions.

## Local Git checkpoint

At the user's request, the installation work is being saved as the repository's initial local commit. The overlapping profiling edits remain in the working tree, uncommitted. A separate source snapshot excludes those edits and passed 20 unit tests, 5 CLI tests, and 11 installer tests, plus formatting and Clippy. The earlier candidate and measurements below included the parallel performance configuration; it remains staged separately and has not been reinstalled. The local checkpoint uses only source, licenses, model metadata, and verification scripts; weights, recordings, app bundles, and build outputs are excluded.

## Changes

- Managed, revision-specific model storage under `~/Library/Application Support/Nemo/models/`. Defaults no longer embed the checkout path. Downloads resume into partial files, validate pinned sizes and SHA-256 values, then publish by rename. A shared lock prevents overlapping downloads and migrations.
- Verified migration of both existing weights, totaling 21,349,066,752 bytes, to revision `59c519158371694388490c96741724a70c8c6276`. Compatibility symlinks keep the old running app's captured paths usable. The configuration's model path changed; Right Option and other existing preferences were retained.
- Permission preflight uses explicit microphone states. Missing Accessibility no longer prevents the menu bar app from starting. The daemon observes permission changes every two seconds, registers or removes Right Option accordingly, blocks denied microphone capture, and provides links to both settings pages. Diagnostics distinguish the command process from the daemon.
- Packaging stages a separately verified, versioned bundle. Installation stages before stopping, requires explicit review when the code identity changes, asks the daemon to quit only if idle, retains a previous bundle, and rolls back on replacement, verification, or launch-command failure. Older daemons without safe-quit support must be quit manually before upgrading.
- Local checks run without model weights or the native runtime. The future Actions workflow has only manual dispatch and requires `NEMO_ENABLE_HOSTED_CI=true`. Leaving that variable unset prevents hosted execution even if someone dispatches it manually.

## Verification

Tested locally on the Apple M5 Max / macOS 26.5.2 with Rust 1.95.0.

| Check | Evidence |
|---|---|
| Formatting and Clippy with warnings denied | Passed |
| Rust unit tests | 23 passed; one hardware/model test intentionally ignored |
| CLI integration tests | 5 passed, including safe-quit acknowledgements and preference preservation |
| Older-app configuration compatibility | Installed 0.1.0 CLI successfully read a fresh configuration saved by candidate 0.2.0 in an isolated directory |
| Installer tests | 11 passed, including staging/commit/backup/launch failures, rollback, signing-identity refusal, and a recording beginning after preflight |
| Interrupted HTTP download | Local HTTP fixture interrupted a real curl transfer; retry sent a Range request and published only after full hash verification |
| Clean checkout | Same local suite passed from a temporary checkout containing no vendor runtime, weights, user configuration, or artifacts |
| Relocated candidate | Signature verified; all 97 Mach-O dependency lists resolved within the bundle or macOS |
| Real inference from relocated bundle | Exact reference transcript using the managed model, bundled runtime, and isolated configuration/logs; clipboard untouched |
| Working installation | Same PID 81834; executable SHA-256 unchanged; daemon responds to status; old model aliases resolve |
| Actions workflow | YAML syntax and opt-in/manual-only triggers checked locally; no hosted run |

The relocation transcript was: “The quick brown fox jumps over the lazy dog. This is a local speech recognition test on my laptop.” This is functional verification, not a latency benchmark.

Raw evidence is in ignored `artifacts/installation-reliability/`: `check.txt`, `installer-tests.txt`, `clean-checkout.txt`, `config-compatibility.txt`, `migration.txt`, `relocation.txt`, `package.txt`, and `handoff.json`. The latter records the installed and candidate executable hashes.

## Review candidate

Candidate: `artifacts/installation-reliability/Nemo.app`, version 0.2.0. Its ad hoc identity differs from the installed app, and the installer correctly requires explicit permission-reset review. No Developer ID Application identity was available for release validation. The candidate contains the current checkout, including the parallel performance changes; this checkpoint does not establish their performance claims.

After reviewing the candidate, quit the older installed Nemo, then install with:

```sh
python3 scripts/install_app.py \
  --source artifacts/installation-reliability/Nemo.app \
  --allow-permission-reset
open ~/Applications/Nemo.app
```

The installer retains the old app in `~/Applications/.Nemo.backups/`. The managed model and preferences survive installation. Re-enable Nemo in Accessibility if macOS invalidates its prior grant. These commands have **not** been run against the working installation.

Before public release, validate permission grants/revocations and recording/paste through the candidate's actual Finder-launched identity, then validate Developer ID signing, notarization, and permission continuity across two signed builds. Permission policy is covered by tests; live OS permission transitions were deliberately not exercised against the user's working app. Successful `open` means LaunchServices accepted the command, not that a newly launched process passed a health check. The installer restores failures it observes; interruption between directory renames can require manually restoring the retained backup. Automatic updates and public distribution remain out of scope.

## Principles that changed the design

- **Make Operations Idempotent:** migration can be retried, downloads resume, and verified files are published only after validation.
- **Separate Before Serializing Shared State:** candidate checks use separate data roots; the working app is not a test fixture. One model-store lock coordinates operations that must share a destination.
- **Build the Lever / Prove It Works:** reusable local checks, injected installer failures, a real interrupted HTTP transfer, and relocated-bundle inference supply rerunnable evidence.
- **Boundary Discipline:** permission states, configuration writes, model publication, and bundle identity checks are handled at their external boundaries.
