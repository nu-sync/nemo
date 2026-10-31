# Repeatable speech benchmarks

For the agent workflow, use the [benchmark-nemo skill](../.agents/skills/benchmark-nemo/SKILL.md)
for bounded experiments, paired accuracy/latency gates, evidence, and cleanup.
For what the request actually does and where its milliseconds go, see
[the inference pipeline](inference-pipeline.md).

Nemo's benchmark corpus covers exact file durations of **1, 5, 10, 20, 30, 40,
50, 60, 90, 100, 200, and 300 seconds**. Each clip has an original, distinct
passage, an English synthetic voice, a UTF-8 reference, and pinned audio/text
hashes. Nothing is recorded through a microphone or played through a speaker.

The goal is to improve transcription latency without accepting missing words as
a speedup. The product goal of **p95 stop-to-visible-text at or below one second**
is separate from the HTTP timings measured here.

## What is stored

```text
benchmarks/speech/
  sources.json       # Topics, voices, target lengths, reference paths
  text/*.txt         # Original synthesis input and scoring references
  audio/*.flac       # Frozen lossless audio, suitable for ordinary Git
  manifest.json      # Actual durations, PCM/file hashes, generation provenance
  reference-check.json  # Frozen independent Whisper check, raw text and WER
  nemo-check.json    # Frozen native-runtime smoke evidence, including failures
  README.md          # Corpus scope, attribution, and reference limitations
scripts/
  generate-speech-corpus.py       # Local Kokoro TTS; uv inline dependencies
  generate-speech-corpus.py.lock  # Locked generation environment
  verify-speech-references.py     # Optional independent, text-unprompted ASR check
  verify-speech-references.py.lock
  speech_corpus.py               # Integrity checks, decoding, WER
  benchmark-corpus.py            # Paired warm-runtime experiments
  latency-report.py              # Actual daemon telemetry, reported separately
```

Generated model weights, decoded WAV files, and benchmark results are **not**
source assets. TTS weights are cached in ignored `artifacts/tts/`; reports go in
ignored `artifacts/performance/`. The runner streams model hashes without loading
the model files into Python memory. It does not duplicate Nemo's weights.

## Validate or regenerate the corpus

Run all Python commands through **uv**. This Rust repository has no root Python
project: use `uv run --no-project` for standard-library scripts/tests, and
`uv run --locked` for the TTS/independent-ASR scripts with adjacent locks.

Normal benchmarking requires uv, Python 3.11+, macOS `afconvert`, an existing Nemo
runtime, and downloaded transcription weights. It does **not** need TTS packages,
network access (with Python already available), microphone permission, or
Accessibility permission.

Validate the frozen files, including lossless decoding and exact frame counts:

```sh
uv run --no-project scripts/speech_corpus.py
```

Only generation additionally needs `espeak-ng` (`brew install espeak-ng`). The Kokoro
ONNX model and voice archive are downloaded once from explicit release URLs and
verified against pinned SHA-256 digests. Python dependencies use the adjacent
script lock. The TTS inference session uses CPU, not Nemo's Metal backend.

```sh
uv run --locked scripts/generate-speech-corpus.py
```

An existing manifest is checked and reused, never silently replaced. To create a
new version, copy and edit `sources.json` and its text files in a separate source
directory, then use a new destination:

```sh
uv run --locked scripts/generate-speech-corpus.py \
  --source /absolute/path/to/new-sources \
  --output artifacts/performance/corpus-v2
uv run --no-project scripts/speech_corpus.py --corpus artifacts/performance/corpus-v2
```

The generator calibrates synthesis speed to fit the complete text within 97–100%
of the requested duration, then adds at most 3% silence at the end. It never cuts
speech, repeats paragraphs to fill time, or time-stretches a finished waveform.
It saves the chosen speed and all fitting attempts. Regeneration is **not promised
bit-for-bit deterministic**: TTS inference, dependency platforms, and resampling
can vary. Compare candidates using the checked-in audio, not freshly generated
clips. Keep the corpus and scoring version fixed throughout an experiment.

## Independently check speech against the reference text

The checked-in `reference-check.json` contains actual independent Whisper base.en
responses for all twelve files: every clip passed a 5% WER gate, with 0–3.03% WER
per clip. This is an automated consistency check, not human certification that
every spoken word is correct. Spelling variants and recognizer errors count.
Neither Nemo nor Whisper receives the reference text as a prompt.

Rerun this check after generating a new corpus, before using it to judge Nemo:

```sh
uv run --locked scripts/verify-speech-references.py \
  --output artifacts/performance/corpus-reference-check
```

This optional script downloads a commit-pinned Whisper base.en model once into
ignored `artifacts/reference-asr/`. It uses CPU inference with locked Python
packages and no VAD or reference prompting. `av` is pinned to a tested compatible
version because newer PyAV releases removed an interface used by this
faster-whisper version. The report saves model-file hashes, options, every
hypothesis, and word edits. Use `--durations 200 300` to narrow an investigation.
A failed check requires review; do not automatically replace source references
with either recognizer's output.

The checked-in `nemo-check.json` also records the installed runtime's smoke test.
All clips through 100 seconds passed the same 5% WER gate. **The 200- and
300-second clips failed badly, repeatedly, even with a fresh server**, despite
independent Whisper WER of 0.20% and 0.40% on those files. The cause in the native
runtime/model is unresolved; the fixtures are retained as stress cases. Full or
stress-only benchmark runs therefore currently exit nonzero on that recorded
runtime/model. This is an accuracy finding, not an apparent speed improvement.

## Quick baseline and candidate comparison

Choose the installed runtime and model explicitly:

```sh
RUNTIME="$HOME/Applications/Nemo.app/Contents/Resources/runtime/nemo-speech"
MODEL="$("$HOME/Applications/Nemo.app/Contents/MacOS/nemo" model path)"
DAEMON="$HOME/Applications/Nemo.app/Contents/MacOS/nemo"

# One first request and one warm request for every duration; functional smoke test.
uv run --no-project scripts/benchmark-corpus.py \
  --runtime "$RUNTIME" --model "$MODEL" --daemon "$DAEMON" \
  --suite all --rounds 1 --repeats 1 \
  --output artifacts/performance/corpus-smoke

# Default typical suite: 1–60 seconds, 15 warm samples per fixture per candidate.
uv run --no-project scripts/benchmark-corpus.py \
  --runtime "$RUNTIME" --model "$MODEL" --daemon "$DAEMON" \
  --candidate baseline:8:512:1:0 --candidate experiment:8:512:1:0 \
  --candidate-runtime experiment=/absolute/path/to/experimental/nemo-speech \
  --output artifacts/performance/corpus-comparison
```

Candidate syntax is `NAME:THREADS:BATCH[:ENCODER_BATCH[:STARTUP_WARMUP_0_OR_1]]`.
Use `--candidate-model experiment=/absolute/path/to/model.gguf` for a model
comparison. Place the corresponding encoder beside that decoder as required by
the runtime. Model overrides and runtime overrides are recorded with hashes;
encoder files matching `*Encoder*.gguf` beside the model are fingerprinted too.
The first candidate is the comparison baseline. Native libraries accompanying a
runtime are not fully fingerprinted, so retain the candidate's build revision
and build flags with the experiment as well.

Use `--durations 5 20 30` to narrow an experiment, `--suite stress` for 90–300
seconds, or `--suite all` for every fixture. Stress cases measure the file-based
runtime, not the live app's current 120-second capture limit.

The runner uses a fresh server for each candidate visit, alternating forward and
reverse candidate order across rounds. After measuring load-to-health separately,
it sends one first request and records but excludes it from warm percentiles.
All selected fixtures then share a seeded, shuffled warm-request schedule,
identical for candidates within each paired round. The server stays loaded for
that schedule. First-request and warm-response text are both scored.

Avoid dictation during a benchmark. `--daemon` checks for busy states at visit and
request boundaries but cannot detect activity starting midway through a request.
It never stops or reconfigures the user's daemon. Other apps, an idle resident
model, thermal state, and GPU contention remain uncontrolled. Do not run TTS,
profilers, builds, or competing inference during comparisons. Keep AC power and
power mode constant; the runner records the observed settings without changing
them.

## Reading and gating results

Each new output directory contains raw response text, per-request timings and
word errors, per-duration p50/p95/p99, paired latency changes, runtime logs, corpus
identity, configuration, and hardware/power metadata. Results are saved after
every request, including failed ones. Owned servers are terminated on completion,
failure, or interruption; system-wide processes are never killed.

- **Request timing:** HTTP upload, inference, response reading, and JSON decoding
  on the same host. WAV decoding and multipart construction occur before timing.
  Model load, recording finalization, paste, and visible insertion are excluded.
- **Percentiles:** nearest rank. The default 15 warm samples per fixture are for
  screening, not a reliable p95 estimate. Even 100 repeated requests assess timing
  variability on one passage, not accuracy diversity or a statistical guarantee.
- **Accuracy:** word-level Levenshtein distance, divided by reference word count.
  Case and punctuation are ignored; apostrophes are preserved within words.
  Spelling variants, contractions, and written versus spoken numbers are not
  automatically treated as equivalent. Insertions can make WER exceed 100%.
- **No selection bias:** an inaccurate response retains its latency in the
  summary and fails the accuracy gate. A transport failure is counted separately,
  marks the run failed, and is never treated as a successful fast response.
- **No synthetic usage p95:** the equal-duration corpus is not the user's workload
  distribution. There is deliberately no pooled percentile labelled production
  or end-to-end latency, and no guessed constant added to HTTP percentiles.

The default accuracy gate is WER at most 5% on **each** first or warm response.
For short fixtures, a single wrong word will usually fail. `--max-wer 0` requires
exact normalized words. Nonzero exit means a failed accuracy/latency gate,
transport failure, or incomplete run; `results.json` distinguishes these cases.
A passed threshold is not proof of no accuracy regression: inspect baseline and
candidate response differences even when both pass.

For confirmation rather than screening, increase independent visits and sample
count; for example, 10 visits with 10 requests per fixture yields 100 warm
samples per duration. Test the stress suite separately instead of diluting the
short-clip latency metric with a five-minute recording.

```sh
uv run --no-project scripts/benchmark-corpus.py \
  --runtime "$RUNTIME" --model "$MODEL" --daemon "$DAEMON" \
  --rounds 10 --repeats 10 --durations 5 10 20 30 \
  --request-budget-ms 1000 \
  --output artifacts/performance/corpus-confirmation
```

Here `1000` is an **HTTP p95 budget for each selected duration**, not proof of a
one-second stop-to-visible-text SLO. A sensible hill-climbing gate is a repeatable
improvement of at least 5% on the important durations, no accuracy regression,
no serious stress-case regression, and a final installed-path check. Do not call
a candidate faster merely because a single warm request beat the baseline.

## Observe real usage separately

```sh
uv run --no-project scripts/latency-report.py \
  --output artifacts/performance/live-latency.json
```

This reads only JSON timing events from the live daemon log. It reports audio
length and request-time distributions, model reuse groups, request timings by
audio-duration bucket, failure counts, and any measured `stop_to_dispatch_ms`.
Dispatch means the insertion action was dispatched, **not that the target
application visibly inserted the text**. The older events lack request IDs, so
the report does not join transcription and delivery records or pretend their
populations are identical. It does not add a fixed delivery estimate.

Use [installed verification](verification.md) and the delivery scratch-target
checks in [delivery latency](delivery-latency.md) for final application-path
validation. Those checks have separate permission and possible acoustic-playback
requirements; they are not invoked by this harness.

## Checks without transcription weights

```sh
uv run --no-project python -m unittest discover -s scripts/tests -v
uv run --no-project scripts/speech_corpus.py
cargo xtask check
```

Tests exercise real local HTTP against an owned stub server, candidate pairing,
accuracy failure with retained latency, transport failure, checksum drift,
scoring, duration fitting, and telemetry scope. The opt-in hosted check workflow
also runs the Python tests and corpus integrity check, without TTS or ASR weights.
Full latency runs remain explicit local experiments, not hosted CI jobs.
