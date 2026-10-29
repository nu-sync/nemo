# Conservative VAD experiment

On October 28, 2026, a conservative edge-trimming VAD candidate was **rejected**. Production capture, inference, dependencies, configuration, and the installed app were not changed by this experiment.

## Candidate and acceptance rule

The experiment used Earshot 1.2.2, a speech-score threshold of 0.3, 500 ms of padding on each edge, and at least one second of total removal before trimming. It retained every internal pause. No detected speech, invalid input, or invalid scores kept the original buffer. This intentionally did not add an aggressive no-speech rejection rule.

Voice activity detection (VAD) estimates which audio frames contain speech. This candidate used those estimates only to select a contiguous region of the original recording; it did not alter the retained samples or join separate spoken segments. The padding was intended to protect quiet word beginnings and endings. Keeping the original buffer when detection found no speech protected against false negatives, but also meant this candidate could not reliably suppress background-only recordings.

Before inference, the retention gate required both primary cases (padded synthetic and padded natural speech) to improve median stop-to-insertion time by at least 5%, win all five matched pairs, and introduce no additional reference-word errors. Continuous, internally paused, quiet, noisy, and clipped speech could not regress more than 5%. Passing this screening experiment would still have required native microphone-rate integration and acoustic verification. The exact decisions and their order are retained in `artifacts/performance/vad-experiment/decisions.jsonl`.

The 5% threshold was an engineering choice made for this experiment, not a user-specified requirement or a measured threshold of human perception. It set a minimum benefit before adding a detector dependency and a new audio-processing step. Five paired wins checked consistency within this small run; they do not establish statistical significance or performance on other workloads.

## Method

- Frozen snapshot of the current working tree, based on commit `2a3b1f7afbc8fe161ba0443d244ab86662ca0fc7`, including concurrent indicator work. Snapshot hashes are retained in `baseline.json`. Production files were never edited for the candidate.
- M5 Max, macOS 26.5.2; existing installed Metal runtime and pinned Nemotron IQ4_NL decoder/F16 encoder. Eight inference CPU threads, encoder batch one, warmup disabled. Runtime hash: `a96b639aab5c7466762a5aab02ab6c7bbca52dab21de4e59f45d79e1aea53ac9`.
- Nine fixtures: seven speech cases, digital silence, and deterministic noise. Six speech cases use local Samantha synthesis; one is a licensed natural LibriSpeech excerpt. Exact text, transforms, source attribution, and hashes are in the fixture manifest. A second generation reproduced every fixture hash.
- One untimed warmup per variant/case, then five alternating baseline/candidate pairs: 18 warmups and 90 measured runs. The same persistent inference process handled both variants, with readiness checked outside each timed interval. Installed Nemo was checked idle before and after each run; other desktop applications were not isolated.
- PCM was decoded and preloaded before timing. The timer started before sending the run command; the harness validated the captured buffer, optionally ran VAD, then called the frozen production WAV encoder, backend, transcript formatter, and paste implementation. The target timestamp was emitted after `NSTextView.paste` inserted the text.
- All 96 dispatched pastes, including warmups, had exactly one matching target receipt. A separate native UI observation saw “Check the next step” in the target text view. These are **text-insertion receipts**, not pixel-render completion timestamps.
- Digital silence was rejected by the existing capture amplitude check and produced no paste in either mode. All errors and rejected setup attempts were retained.

This is a **16 kHz captured-buffer replay**, not a complete microphone round trip. It excludes device closure, the normal event-loop/worker queue, and conversion from the MacBook microphone's 48 kHz input to VAD's 16 kHz input. A production candidate would need to pay and validate that conversion. No microphone capture or acoustic validation was performed after the candidate failed the screening gate.

## Results

Milliseconds, median (minimum–maximum), five measured runs per cell. Positive improvement means VAD was faster.

| Fixture | Current behavior | Conservative VAD | Median improvement | VAD paired wins |
| --- | ---: | ---: | ---: | ---: |
| continuous | 385.9 (382.8–390.2) | 388.8 (379.2–392.0) | -0.76% | 1/5 |
| padded | 410.2 (404.8–421.3) | 396.8 (389.3–406.3) | +3.26% | 5/5 |
| internal_pause | 387.9 (384.9–395.8) | 389.7 (386.7–396.0) | -0.47% | 1/5 |
| quiet_padded | 408.1 (402.2–409.8) | 396.5 (392.8–399.9) | +2.83% | 5/5 |
| noisy_padded | 408.2 (402.5–409.6) | 398.7 (397.2–400.9) | +2.33% | 5/5 |
| clipped_edges | 297.9 (295.3–298.6) | 299.6 (295.4–301.9) | -0.57% | 1/5 |
| natural_padded | 498.9 (492.7–502.8) | 494.3 (493.0–499.4) | +0.93% | 4/5 |

The padded synthetic case improved by 13.4 ms (3.26%). The natural case improved by 4.6 ms (0.93%), had overlapping ranges, and lost one matched pair. Neither primary case met the pre-registered 5% gate. Continuous speech and clips with no eligible edge trim added a small amount of work.

All 70 measured speech transcriptions (35 per variant) matched their reference words after case/punctuation normalization. There were no additional word errors in any matched pair. This is limited English accuracy evidence, not proof of accuracy across speakers, languages, or real microphones.

Noise-only input hallucinated in both variants: the baseline produced 16 inserted words and VAD produced one. Its large apparent latency reduction is excluded from the speech-performance decision because the output was incorrect. The separate safe-no-speech criterion was therefore not met, although VAD introduced no additional reference-word errors. Digital silence behaved identically in both modes. The verdict script explicitly enforces the no-speech criterion.

VAD scoring plus WAV preparation took roughly 0.3–3.7 ms across measured non-silent cases. Trimming several seconds of synthetic edge silence reduced inference time only modestly; the measured request and fixed paste path still dominate elapsed time. This experiment does not identify a new GPU/CPU hardware limiter or establish an encoder-versus-decoder split, and no claim of a general inference speedup is made.

## Why trimming audio did not produce a larger benefit

The padded synthetic buffer shrank from 8.41 seconds to 4.54 seconds, while median inference-request time fell from 289.9 ms to 277.0 ms. VAD plus WAV preparation rose from 0.46 ms to 2.71 ms. The natural buffer shrank from 10.86 seconds to 9.34 seconds, while request time fell from 381.5 ms to 375.7 ms. Shorter input therefore did not translate into a proportional reduction in request time for these cases.

Paste dispatch took about 112–113 ms in both variants, including Nemo's intentional 100 ms delay. VAD did not address that part of the delivery path. These are medians of separate spans; their differences should not be added together to reconstruct the median end-to-end result.

The decision is specific to this detector, trimming policy, corpus, model, and machine. Some padded cases did improve consistently. The evidence was insufficient to justify adoption under the chosen gate, rather than evidence that every VAD approach would be ineffective.

## Conditions for revisiting VAD

A further experiment should start from a concrete workload where edge silence contributes materially to delivery time, such as longer real dictations with substantial pauses before or after speech. Reuse the current behavior as the baseline and register the new acceptance rule before measuring.

Any candidate that passes replay screening still needs the full native-rate recording path: microphone capture, conversion for detection, stop handling, inference, and insertion into a foreground scratch document. Include quiet speech, clipped boundaries, more natural speakers, and the languages being targeted. Check punctuation separately if changing audio boundaries changes it; the current word-error comparison ignores punctuation and case.

Treat suppression of background-only recordings as a separate correctness goal. A detector that returns the original audio when uncertain protects speech but can still allow hallucinations. A faster incorrect transcript is a failed result, even when it contains fewer invented words than the baseline.

## Evidence and reproduction

Validation passed: candidate formatting, Clippy, release build and nine unit tests; harness release build and Clippy; repository `scripts/check.sh` (44 unit tests, eight CLI tests, 21 Python tests, formatting, Clippy and shell syntax; the existing real-model unit test remained ignored). Frozen snapshot hashes were unchanged, and all experiment processes were gone after cleanup. No installation was performed.

Local, intentionally ignored artifacts are under `artifacts/performance/vad-experiment/`:

- `candidate/`: pinned standalone Rust candidate, CLI, nine behavioral tests, and verification output.
- `snapshot/` and `harness/`: frozen production modules and isolated comparison harness. No application VAD dependency was added.
- `make-fixtures.py` and `fixtures/manifest.json`: reproducible corpus with natural-speech license and provenance.
- `run-comparison.py`, `PasteTarget.swift`, `comparison-03/results.json`, and `comparison-03/paste-events.jsonl`: paired run and insertion evidence.
- `judge.py` and `judgment.json`: full sample-count, error, receipt/text/timestamp, accuracy, and latency checks; final verdict `reject`.

`comparison-01` is excluded because foreground guards refused paste. `comparison-02` is excluded because the initial scratch app lacked a standard Edit/Paste menu. `comparison-03` reran both variants after fixing the target. A subsequent audit added strict receipt assertions to the runner and validated all existing successful samples with `judge.py`; the measured code and successful samples did not change.

To reproduce locally, rebuild the candidate/harness and Swift target from those retained sources, then use a fresh output directory:

```sh
cargo test --locked --manifest-path artifacts/performance/vad-experiment/candidate/Cargo.toml
cargo build --release --locked --manifest-path artifacts/performance/vad-experiment/harness/Cargo.toml
xcrun swiftc artifacts/performance/vad-experiment/PasteTarget.swift -o 'artifacts/performance/vad-experiment/Nemo VAD Benchmark.app/Contents/MacOS/paste-target'
python3 artifacts/performance/vad-experiment/run-comparison.py --paste --output artifacts/performance/vad-experiment/comparison-NEW
python3 artifacts/performance/vad-experiment/judge.py
```

The final command audits the fixed retained `comparison-03`; change its input explicitly to audit a new run. The app bundle and frozen source snapshot are local experiment artifacts and are not supplied by a fresh clone.

## Decisions shaped by principles

- **Explain the Number:** registered a gate before measuring, alternated five pairs, retained ranges/failures, and rejected the candidate rather than treating a small noisy natural-speech difference as sufficient.
- **Experience First:** required unchanged reference words and protected quiet/clipped speech with generous margins and fail-open behavior.
- **Separate Before Serializing Shared State:** isolated the experiment and model process from concurrent indicator edits and the installed app.
- **Build the Lever / Prove It Works:** retained rerunnable fixtures, a harness using actual production functions, target insertion receipts, and a machine-checked verdict.
- **Test Behavior, Not Implementation:** tested exact retained ranges, preservation of internal pauses, partial tails, and fail-open outcomes.
