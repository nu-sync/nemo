# Combined installation verification

Settings and runtime changes were frozen before the final build. The combined app is installed at `~/Applications/Nemo.app`. Its core `verify-nemo` journeys passed: real speaker/microphone capture, loading during recording, warm reuse, visible paste into an unsent ChatGPT draft, and full idle expiry in an isolated instance of the same installed executable. TextEdit paste is not claimed.

## Installed artifact

- Executable SHA-256: `2782c3bb97c1ca0787038de1e7f62110efa9a284854d14e3f0838b2693ecaa86`.
- Installed bytes match `artifacts/combined-install/Nemo.app`; strict/deep signature verification passed, and the signed native runtime executes.
- The 21-file frozen source snapshot still matches the checked release build.
- Checks passed: formatting, Clippy, 28 Rust unit tests, 5 CLI tests, 13 installer tests, 2 verifier ownership tests, and shell syntax checks. One model-dependent unit test was ignored by the ordinary check suite.
- The installer retained the previous app at `~/Applications/.Nemo.backups/a2c45fba2dde4bf9a58dc320c9a779d8.app`.

This build uses the existing local Apple Development identity. A probe with two different signed payloads verified that the same certificate and bundle identifier preserve the signing requirement. Packaging still defaults to ad hoc signing: future local packages must use the same `NEMO_SIGN_IDENTITY` to retain that identity. This does not establish Developer ID distribution or notarization.

## Verified behavior

The actual installed daemon reports Accessibility enabled and Microphone authorized. It was reopened through macOS after the targeted permission refresh, without rebuilding or re-signing the app.

Cold recording started with no model child. PID 87488 began loading and became ready while the app still reported Recording; loading took 2,910.267 ms. Stop → transcribe reused that PID with zero additional loading and completed in 406.763 ms for 7.317 seconds of captured audio. A warm follow-up kept the same daemon and runtime, incurred no load event, and completed transcription in 384.369 ms for 7.232 seconds of audio. These are functional-check timings, not a repeated latency benchmark.

The expected words matched the actual clipboard. Focus stayed outside the intended TextEdit document, so initial paste logs alone were not accepted as visible delivery evidence. A later native UI observation read the exact fixture, including its final period, in an unsent ChatGPT draft. That field was then cleared and observed empty; nothing was submitted. The empty test-owned TextEdit document received a close action. Earlier System Settings paste logs establish no visible insertion there. Raw acoustic results are `final-cold-roundtrip`, `final-warm-roundtrip`, and `final-warm-visible` under `artifacts/combined-install`; `visible-paste-observation.json` records the actual UI scope.

The same final installed executable passed a full 300-second expiry check with an isolated data directory, startup preload, no microphone capture, and no clipboard operations. Its model PID was present at 299.047 seconds and first absent at 300.049 seconds, using one-second process snapshots. The daemon survived expiry, all owned processes were reaped, and the installed hash matched afterward. This proves the full timer after preload in that controlled instance; it does not establish idle behavior after dictation in the normal desktop session.

The final installed native Settings window was exercised directly:

- Remove trailing period toggled off → on → off, with live config persistence.
- Sound pack changed Handy Marimba → macOS Recording → Handy Marimba; both start/stop preview buttons were exercised for both packs.
- Launch on startup toggled off → on → off. The checkbox reads `SMAppService` status after registration changes. An additional `sfltool dumpbtm` query stalled and its owned read-only process was stopped; no logout/login was performed.

The original preference values were restored. Saving omits default-valued config fields; their effective values remain unchanged. The settings chat's isolated persistence, rollback, and startup evidence remains under `artifacts/settings-check`, `artifacts/sound-settings-check`, and `artifacts/startup-check`.

## Coverage limits and verifier guard

The identity migration initially reset Accessibility and Microphone grants. The user approved macOS Touch ID, the exact installed app was added to Accessibility, and the actual daemon subsequently reported both permissions authorized. The final app remains running and reports `Ready · model unloaded`. Original effective configuration values are preserved. No microphone recording was saved.

The native UI tool addressed the background TextEdit document without activating it as macOS's foreground app. Repeated attempts were preserved, and a four-minute foreground wait started no recording because TextEdit never became active. The actual visible ChatGPT draft insertion establishes paste in that app; it does not establish TextEdit paste. The full timer result remains an isolated preload check rather than continuous observation of the normal session after dictation. No logout/login startup test was performed. The trailing-period setting was toggled and persisted live; default-off output retained its final period, while enabled-on acoustic output was not separately exercised on this desktop instance.

The reusable helper now accepts `--paste-target TextEdit`, checks the installed app's foreground diagnostic before capture and before stop, and requires a matching paste destination log. A wrong-target check with ChatGPT active returned inconclusive before toggling: daemon status and empty runtime-child list stayed unchanged, with no recording or model job. This guard does not replace reading actual inserted text through the native UI tool. No further user action is needed for the core verification reported here.

Cleanup drains pending shortcut telemetry before canceling an owned capture. The model-free tests in `scripts/tests/test_verify_installed.py` exercise the actual helper entry point: an uninterrupted owned capture is canceled on target mismatch; a shortcut arriving during the foreground check relinquishes ownership and leaves the newer user recording untouched. Both cases passed with all app/audio/process calls mocked. Run them from a clean checkout with `python3 -m unittest discover -s scripts/tests -p test_verify_installed.py -v`; the full `scripts/check.sh` suite includes them.

Raw local evidence is under the ignored `artifacts/combined-install/` directory, including package/readback hashes, checks, source snapshot, settings observations, and `isolated-full-expiry/results.json`. These machine-specific outputs are not included in a source checkout; the published verification scripts and skill describe how to reproduce the journeys.

## Principles applied

- **Separate Before Serializing Shared State:** the settings owner froze its source and one owner built, installed, and tested the final app.
- **Prove It Works:** verification targets the installed executable, native UI, daemon permission snapshot, real acoustic transcription and visible delivery, and a full-duration timer; each result retains its actual scope.
- **Fix Root Causes:** corrected `codesign` requirement syntax in the installer and explicit file-lock release when a child inherits the locked file description. Regression checks reproduced both failures before the fixes.
- **Attack the Premise:** a rerunnable foreground census rejected the assumption that background TextEdit interaction activates it. Foreground checks now precede recording, and observed delivery is attributed to its actual app.
