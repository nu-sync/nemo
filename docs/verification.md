# Local verification — October 28, 2026

This records the initial bring-up. The user subsequently granted both permissions and confirmed that physical Right Option recording, transcription, sounds, and paste work correctly. See [installation reliability](installation-reliability.md) for the later model migration, staged upgrade, and local CI checkpoint.

Tested on the user's MacBook Pro, Apple M5 Max, 128 GB unified memory, macOS 26.5.2. All changes are local; no commit, push, or deployment was made.

## Model and runtime

- Downloaded both model files, 21,349,066,752 bytes in total, and verified their SHA-256 values against the pinned Hugging Face LFS metadata.
- Conversion: `AtomicChat/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-GGUF`, revision `59c519158371694388490c96741724a70c8c6276`; IQ4_NL decoder and F16 speech encoder. These are quantized weights of the requested model, rather than a smaller ASR model.
- Runtime: NVIDIA `NeMo-Speech.cpp`, revision `6a62501107e1894723880f1094a286ea996a0ec7`, built with Metal and speech-LLM support. One inference slot, 8,192 context tokens, no speculative decoding.
- Installed `~/Applications/Nemo.app`. Ad hoc signature verification passes. All 96 bundled Mach-O files resolve their non-system dependencies inside the bundle; dynamic-loader output confirms no Homebrew or source-build libraries are loaded. The app is about 27 MB, excluding weights.

## Observed behavior

| Check | Result |
|---|---|
| Formatting, Clippy with warnings denied, unit tests | Passed |
| Invalid WAV rejected before loading model | Passed |
| Actual synthetic speech transcription | Exact words and punctuation |
| Actual JFK sample transcription | Correct spoken words |
| Clipboard | Read back and matched actual model output |
| Automatic paste | Actual transcript appeared in a focused ChatGPT input; test draft cleared without sending |
| Duplicate daemon | Refused; original daemon stayed responsive |
| Cancel while model loads | Child reaped; clipboard unchanged |
| Inactivity unload | Model child stopped after 60.06 seconds |
| SIGTERM | Daemon exited successfully and reaped its model child |
| Microphone opened from permitted terminal, before the app prompt | Recording/cancel states reached |
| Standalone app microphone permission | Pending user click; automatic approval review blocks interacting with UserNotificationCenter |
| Full microphone → transcription → paste in installed app | Pending macOS permission and a live voice test |
| Physical global shortcut | Registered successfully; physical keypress remains to be checked |

The target-app test initially failed because the event-loop library activated the windowless daemon at startup, while the automation's app-scoped clicks did not activate TextEdit. Foreground-app diagnostics established the actual destination. Startup activation is now disabled; actual paste was then verified in the foreground input. This was a focus issue, not a transcript or clipboard failure.

## Timing and memory

Three successive transcriptions of the same 5.8635-second synthesized recording used one loaded model and produced the exact reference text every time: **0.399, 0.378, and 0.398 seconds**. Loading the model in that test took **2.910 seconds**, with weights already present in the filesystem cache. This measures short, clean, warm-model requests through the real Rust client and native server, not arbitrary speech or first-use latency. Separate process invocations varied substantially, including first-use/library/cache costs; do not interpret the warm numbers as an end-to-end latency guarantee.

During the daemon lifecycle check, the loaded native process had **19,353,136 KiB RSS**. After unloading, no model child existed and the controller had **46,944 KiB RSS**. These are process resident-set snapshots, not a measurement of all GPU allocations or total system memory. The model weights dominate active memory; unloading removes that process instead of merely clearing a logical flag.

Raw evidence is retained locally in ignored `artifacts/`: `model-verification.txt`, `verification-run.txt`, `lifecycle-verification.json`, transcript files, timing files, and bundled-library diagnostics. Rerun `scripts/verify.sh` and, with Nemo stopped, `python3 scripts/verify-lifecycle.py`.

## Default end-to-end check

Use real speaker playback and microphone capture, local inference, and clipboard
comparison. No text editor, foreground focus or visible paste is required:

```sh
uv run --no-project --python 3.13 python scripts/verify-clipboard.py \
  --output artifacts/performance/installed-clipboard-NEW
```

This runs the installed executable under external-network denial with an isolated
`paste=false` configuration, then restores the prior normal app session without
changing user preferences. Require `summary.json` to pass and
`roundtrip/results.json` to report `clipboard_matches_reference=true`.
See [verify-nemo](../.agents/skills/verify-nemo/SKILL.md) for prerequisites. Test
visible paste, physical shortcuts and full idle expiry separately when requested.

## Local-only audit

Use the [offline verification skill](../.agents/skills/verify-offline/SKILL.md) for
external-network denial, real local inference, constrained model acquisition, and
dependency/license checks. See [local-only operation and audit](local-only-audit.md)
for the enforced boundaries and audit limits.

The [offline lifecycle model](../verification/offline/README.md) checks finite
process-capability interleavings. All nine invariants pass, including the previously failing helper-ownership
and publication/cancellation retirement diagnostics.
Use [verify-offline-model](../.agents/skills/verify-offline-model/SKILL.md) when the
mapped lifecycle or sandbox source changes. This complements runtime verification.

## Decisions shaped by principles

- **Foundational Thinking / Model the Domain:** one UI owner and explicit idle, starting, recording, and working states; canceled results cannot paste and pending microphone requests cannot overlap.
- **Build the Lever / Prove It Works:** retained setup, hash verification, real-audio verification, and daemon lifecycle scripts. Successful compilation alone was insufficient.
- **Fix Root Causes:** instrumented the actual foreground app and disabled startup activation after observing failed paste delivery.
- **Explain the Number:** separated model load from warm requests, kept the repeated outputs and timings, and distinguished resident memory from total unified-memory use.

## Next manual check

Open Nemo, allow its Microphone prompt, and enable Nemo in Privacy & Security → Accessibility. Tap right Option in a scratch text field, speak, tap it again, and check both the field and clipboard. The application must receive its own permissions; terminal test permissions do not establish Finder-launched app permissions.

The shortcut was subsequently changed to right Option at the user's request. Native AppKit monitors distinguish the physical right key and toggle on release; typing an Option combination or clicking with Option cancels the tap. Pure event-sequence tests cover both Option keys, repeated flags, typing, mouse-click cancellation, other modifiers, and Caps Lock. Recording start, stop, and successful completion now have distinct Tink, Pop, and Glass sounds. The physical-key and sound check remains with the user after restart.
