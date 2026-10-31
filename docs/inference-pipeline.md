# How a dictation becomes text

This describes the inference path end to end: what Nemo hands the runtime, what
the model does with it, where the milliseconds go, and how to measure any of it
again. It is written for anyone who wants to change the latency without guessing.

Numbers here are measured on the M5 Max described in
[performance on the M5 Max](performance.md), warm, with no competing inference.
They are published with the commands that reproduce them; a different machine,
runtime revision, or model will produce different values.

For the reproducible corpus and the paired A/B harness, see
[speech benchmarking](benchmarking.md). For replacing the engine behind these
numbers with a measurable alternative, see
[pluggable inference backends](pluggable-inference-backend.md). For the agent
workflow around it, see the
[benchmark-nemo skill](../.agents/skills/benchmark-nemo/SKILL.md).

## 1. System shape

Two processes, one model.

```text
Nemo.app/Contents/MacOS/nemo           menu app; owns hotkey, UI, microphone, clipboard
  └─ model worker thread               owns the runtime process handle
       └─ nemo-speech serve            HTTP server on 127.0.0.1:<ephemeral>, --device metal
            ├─ PEE encoder  (F16)      80 ms frames, 1.67 GB
            └─ Nemotron-H MoE (IQ4_NL) 53 blocks, 128 experts, 19.68 GB
```

- Nemo never links the inference code. It spawns `nemo-speech serve` and talks
  HTTP to loopback with a per-process random bearer key.
- The runtime is loaded on demand and kept for `idle_unload_seconds` (300 s here)
  after the last job. Loading takes ~2.6 s and is overlapped with recording, so
  it usually costs nothing user-visible.
- Exactly one transcription is in flight at a time. The runtime is configured
  with `max_sequences=1`; the app has a single worker, so the model's own
  batching and continuous-serving machinery is idle by design. Anything tuned
  for concurrent throughput does not apply to Nemo's workload.
- The main event loop never blocks on the model. Microphone open, audio
  finalization, inference, and clipboard delivery are each off the UI thread.

## 2. The request contract

On Stop, Nemo finalizes the capture and POSTs it:

```http
POST /v1/audio/transcriptions
Authorization: Bearer <32 hex chars>
Content-Type: multipart/form-data; boundary=...

file=<WAV>              (see below)
response_format=json
```

**The audio.** Mono, 16-bit signed PCM, little-endian WAV container, written by
`hound`. Two things about it matter:

- **The sample rate is the microphone device rate, not 16 kHz.** Nemo does no
  resampling (`src/audio.rs` uses `supported.sample_rate()`). A 48 kHz capture is
  uploaded at 48 kHz and resampled inside the runtime.
- Capture is rejected below 250 ms, and an all-silent capture is rejected with
  "No audio detected" before any request is made.

**The response.** `{"text": "..."}` only. Nemo reads the string, trims it,
rejects an empty result, optionally strips a trailing period, and hands it to
clipboard/paste delivery.

The runtime offers much more than Nemo uses, and the extra fields are the easiest
way to inspect behaviour without touching the app:

| Endpoint | Gives you |
| --- | --- |
| `POST /v1/audio/transcriptions` | text; `verbose_json` adds words, timings, speakers |
| `POST /v1/chat/completions` | same model as a chat turn, plus **streaming** and a `usage` block with `audio_tokens`, `prompt_tokens`, `completion_tokens` |
| `GET /health` | readiness; Nemo polls it at 100 ms after spawn |
| `GET /v1/models` | the model id the server reports |

Streaming `chat/completions` is the single most useful diagnostic: time to first
token separates encoder+prefill from decode, and `usage` tells you the exact
token counts for that clip.

## 3. Inside the runtime

```text
uploaded WAV
  │  1  decode container, downmix, resample to 16 kHz
  │  2  log-mel: 25 ms window, 10 ms hop, n_fft 512  -> 100 frames/s
  │  3  PEE encoder (+ speaker branch, + CTC timestamp head) -> 12.5 frames/s
  │  4  fusion.proj -> 2688-dim LLM embeddings
  │  5  splice into the text prompt at the <|audio|> locator
  │  6  LLM prefill  (one llama_decode per n_batch=512 chunk)
  │  7  greedy decode until <|im_end|>
  ▼
"text"
```

Stages 1-5 are the "encoder+prefill" half; stage 7 is the decode half. Stage 6
is cheap relative to 7 because prefill reuses weights across the whole batch
while each decode step re-reads them for a single token.

**The model.** Nemotron-3.5-Transcribe-30B-A4B, `nemotron_h_moe`:

| | |
| --- | --- |
| blocks / embedding | 53 / 2688 |
| experts | 128, top-6 per token, plus 1 shared expert of 3712 |
| vocabulary | 131,072 |
| trained context | 1,048,576 tokens |
| MTP head | present (`nextn_predict_layers = 1`) but **unused** — see §7 |

**The prompt.** One fixed user turn with exactly one `<|audio|>` locator, wrapped
by the chat template, ending in the assistant turn. The audio locator is
replaced by the encoder's projected frames, so audio tokens and text tokens
share one sequence. Greedy sampling, no temperature, no prompt caching between
dictations.

**Encoder geometry** (read from the encoder GGUF's metadata):
`sample_rate 16000`, `window 25 ms`, `stride 10 ms`, `n_fft 512`,
`fusion.proj_dim 2688`. The 10 ms mel hop and the 80 ms encoder stride are what
produce the 12.5 audio tokens per second of audio that everything below is
priced in.

## 4. The token economy

Two rates set the whole cost curve, and both are properties of speech, not of the
machine:

| Quantity | Rate | For 60 s |
| --- | --- | --- |
| audio tokens (prompt) | 12.5 / s of audio | 751 |
| transcript tokens (generated) | 2.87 / s of audio | 172 |

Transcript tokens track speaking rate (~2.8 words/s, ~1.13 tokens/word in this
corpus), so a dense speaker costs more than a sparse one over the same duration.
Word count does not, however, explain the one fixture that breaks the cost model;
see the open questions (§9).

Measured widths for the same clips, from the streaming diagnostic:

| audio | audio tokens | transcript tokens | total |
| --- | --- | --- | --- |
| 5 s | 63 | 18 | 285 ms |
| 20 s | 251 | 64 | 735 ms |
| 60 s | 751 | 172 | 1891 ms |

## 5. The cost model

Measured decomposition (three repeats each, one warm runtime, medians):

| audio | total | encoder + prefill | decode | decode share |
| --- | --- | --- | --- | --- |
| 5 s | 285 ms | 138 ms | 146 ms | 51% |
| 20 s | 735 ms | 195 ms | 540 ms | 74% |
| 60 s | 1891 ms | 422 ms | 1469 ms | 78% |

Which resolves into three per-request constants:

| Term | Value | Per audio second |
| --- | --- | --- |
| fixed per-request cost | ~80 ms (streaming path), ~135 ms (`/v1/audio/transcriptions`) | — |
| encoder + prefill | 0.455 ms per audio token (2 198 tok/s) | 5.7 ms |
| **decode** | **8.38 ms per transcript token (119 tok/s)** | **24.0 ms** |

```text
request_ms ≈ 135 + 29.7 × audio_seconds
```

That reproduces the frozen corpus at every normal length — 60 s predicts 1917 ms
against 1914 ms measured — and matches the live workload's p99.

**Read the third row again: 81% of the latency slope is single-sequence
autoregressive decode.** Every audio second buys 12.5 prompt tokens that prefill
in ~5.7 ms, but also ~2.9 transcript tokens that cost ~24 ms to emit one at a
time. Inter-token interval is flat at 8.54-8.61 ms across a whole 60 s transcript,
so this is a steady-state property of the decode loop, not a startup effect, a
scheduler effect, or a symptom of long input.

Consequences worth internalizing before choosing an optimization:

- Tuning anything that does not change the decode loop cannot move the slope.
  This is why inference threads, `encoder_max_batch`, and startup warmup all
  measured as zero-effect on this build.
- The fixed term dominates short dictation — 135 ms is 39% of a 5 s request but
  7% of a 60 s one.
- **The 1-second budget runs out at ~27 s of audio** ((935 − 135) / 29.7),
  which independently predicts the observed live numbers: p95 audio is 25.8 s,
  and 2.9% of real dictations exceed one second stop-to-visible.

## 6. End to end, beyond inference

HTTP latency is not what the user experiences. Nemo emits a content-free JSON
event at each boundary on `daemon.log`, which is what makes the rest measurable:

| Event | Field | Measured (n ≈ 239) | Meaning |
| --- | --- | --- | --- |
| `capture_start` | `microphone_start_ms` | — | microphone opened |
| `audio_finalized` | `finalization_ms` | p50 20.9, p95 28.0 | Stop to closed WAV |
| `inference_started` | `stop_to_inference_ms` | p50 21.8, p95 34.2 | Stop to worker took the job |
| `transcription` | `request_ms` | see §5 | the HTTP call |
| `delivery` | `delivery_ms` | p50 0.7, p95 4.5 | clipboard write + paste dispatch |
| `delivery` | `stop_to_dispatch_ms` | p50 411, p95 824 | Stop to dispatch |

```text
stop -> visible  ≈  finalization (21)  +  request_ms  +  insertion (~45)
```

The ~45 ms insertion figure is a separate scratch-app measurement of an actual
NSTextView paste, not a constant invented here. `delivery_ms` is *dispatch*;
dispatch is not insertion, and the two must not be conflated — recipient app
event processing sits between them.

Two cautions about this table. The transcription and delivery populations do not
carry correlation IDs, so they cannot be joined per dictation; they are reported
as separate distributions. And the log accumulates across app builds without
per-event binary identity, so an unsegmented report is not an A/B experiment.

## 7. The knobs, and what each can actually touch

Everything Nemo passes to the runtime is in `src/backend.rs::load`. The current
profile: `max_sequences=1`, `n_ctx_per_sequence=8192`, `max_context=8192`,
`n_batch=512`, `encoder_max_batch=1`, `max_new_tokens=4096`, `mtp_draft=0`,
`--threads 4` (HTTP workers), `--asr.backend.threads <inference_threads>`,
`--no-warmup` unless `model_warmup` is on.

| Setting | File | Reaches | Measured effect |
| --- | --- | --- | --- |
| `inference_threads` | config.toml | host-side decoder threads | 4/6/8/18 all within noise |
| `encoder_batch_size` | config.toml | encoder windows per graph launch | 1/4/8 within noise |
| `model_warmup` | config.toml | enables runtime warmup | +0.2 s to readiness, no steady-state gain |
| `preload_model` | config.toml | load at app start | trades startup for first-request |
| `idle_unload_seconds` | config.toml | retention after last job | affects reuse, not per-request speed |
| `n_batch` | `backend.rs` | prompt tokens per prefill pass | capped by `n_ubatch=512`; 751 audio tokens take two passes |
| `mtp_draft` | `backend.rs` | speculative decoding | **forced to 0 on Metal** |
| `max_recording_seconds` | config.toml | capture cap | currently 120 s |

Two structural facts that close doors, both verifiable in the vendored runtime
source under `vendor/NeMo-Speech.cpp/src/speechlm/`:

- **Speculative decoding is not merely disabled, it is unavailable.**
  `salm_engine.cpp` sets `cfg.mtp_draft = 0` unless the CUDA `SALM_FAST_PATH`
  engages, with the comment that MTP rolls drafts back through the batched
  path's replay rows, which upstream llama.cpp's slot-compacting path cannot
  carry. The GGUF *does* carry the NextN head; the loader reports those tensors
  as unused. Enabling it on Metal is a port, not a flag.
- **Streaming recognition is unsupported by this model.** The CLI `--stream`,
  the HTTP realtime socket, and gRPC `StreamingRecognize` all refuse for SALM:
  the interface takes complete audio. So "start decoding while the user is still
  talking" is not a tuning change in Nemo; it would require a different
  recognition architecture.

## 8. How to find the bottleneck

The general method, in the order that costs least:

1. **Get the shape of the curve, not a single number.** Sweep audio lengths and
   fit. A slope plus an intercept tells you whether you have a per-request
   problem (intercept) or a per-second problem (slope) before you profile
   anything. The corpus exists for this and is frozen.
2. **Split the request.** Time to first token from streaming
   `chat/completions` divides encoder+prefill from decode; `usage` confirms the
   token counts rather than assuming them. If decode is most of the total, no
   upstream tuning can save you.
3. **Check whether the cost is steady-state or a step.** A flat inter-token
   interval means the decode loop is the bottleneck. A step at a boundary means
   admission, windowing, context limits, or preemption.
4. **Only then profile inside the runtime.** `NEMO_SPEECH_TIMING=1` makes the
   runtime print stage timings on several code paths, and
   `scripts/benchmark.py --profile` sets it and wraps one visit in `sample(1)`.
   Profiling changes timing, so keep it out of any comparison run and treat its
   output as explanatory, not as a score. CPU samples do not see GPU stalls;
   a Metal System Trace is what separates encoder kernels from decode kernels.
5. **Confirm on the real path.** HTTP latency excludes finalization and paste;
   only an installed recording-and-insertion check speaks to the product goal.

Two failure modes this ordering prevents: running a knob sweep with no
hypothesis about which term it should move (how the exhausted settings in §7 got
their `null` result), and explaining a gain after the fact with a profiler that
was not measuring the workload you gated on.

Applied here, the method produced one number worth acting on: **decode is 81% of
the slope at 8.38 ms/token.** Anything else is a rounding error by comparison.

## 9. Known limits and open questions

- **The 1 s budget ends at ~27 s of audio.** 60 s dictation cannot reach one
  second by engine speed alone, even at half the decode cost.
- **The 50 s corpus fixture is 242 ms (15%) slower than the cost model
  predicts**, reproducibly across 15 samples, and not explained by length or word
  count. Unresolved.
- **Encoder and LLM prefill are measured together**, not separately. This build
  has no diarization (`/v1/audio/diarizations` returns 501, `bench diarize` is
  not compiled in), which would have isolated the encoder via its speaker
  branch. Splitting them needs a Metal trace.
- **The ~135 ms fixed cost has never been broken down.** It is 39% of a 5 s
  dictation.
- **The 200 s and 300 s corpus clips fail accuracy badly** on the recorded
  runtime/model while independent Whisper agrees with the references at
  0.2-0.4% WER. A real defect in the model/runtime pair, not a fixture problem.
  The live capture cap is 120 s, so it does not currently reach users.
- **Only one passage per duration** is not broad accuracy coverage, and the
  independent reference check is automated agreement with the synthesis input,
  not human certification.

## 10. Reproducing the numbers

Corpus timing (paired, alternating, fresh runtime per visit):

```sh
RUNTIME="$HOME/Applications/Nemo.app/Contents/Resources/runtime/nemo-speech"
DAEMON="$HOME/Applications/Nemo.app/Contents/MacOS/nemo"
MODEL="$("$DAEMON" model path)"

uv run --no-project scripts/benchmark-corpus.py \
  --runtime "$RUNTIME" --model "$MODEL" --daemon "$DAEMON" \
  --candidate baseline:8:512:1:0 --candidate experiment:8:512:1:0 \
  --candidate-runtime "experiment=/abs/path/to/candidate/nemo-speech" \
  --suite typical --rounds 5 --repeats 3 \
  --output artifacts/performance/corpus-screen
```

The stage split (§5) is a separate streaming diagnostic rather than part of the
harness; it needs a warm runtime, three repeats per length, and streamed
`/v1/chat/completions` requests recording time-to-first-token and `usage`.
Whatever produces it should record the runtime and model hashes alongside, and
should never be confused with the paired gated measurement.

Live telemetry, which never touches the model:

```sh
uv run --no-project scripts/latency-report.py \
  --output artifacts/performance/live-latency.json
```

Evidence for the figures in this document is in
`artifacts/performance/framing-20261030-210101-91766/` (stage split, corpus
baseline, workload distributions) and `artifacts/performance/corpus-typical-baseline/`
(frozen-corpus per-duration results).
