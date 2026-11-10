# Engine comparison through the app path

Measured November 9, 2026. MLX-Audio with MTP speculative decoding was **1.50–1.65x
faster than the bundled runtime on warm requests from 5 to 100 seconds of audio, with
identical word errors on every one of those clips**. It uses about 7 GB more memory.
The default engine is unchanged: MLX-Audio stays an opt-in for anyone who wants the
speed and has the memory.

Read first: [the inference pipeline](inference-pipeline.md) for why decode is the term
worth attacking, and [the MLX-Audio experiment](mlx-audio-experiment.md) for the earlier
HTTP-level measurements this one checks against.

## What was compared

One variable: `backend` in `config.toml`.

| | `nemo-speech` | `mlx-audio` |
| --- | --- | --- |
| engine | NeMo-Speech.cpp, Metal, `mtp_draft = 0` | MLX-Audio, MTP, 8 draft tokens |
| weights | IQ4_NL GGUF, revision `59c5191` | NVFP4 profile `main` |
| options | `inference_threads = 8` | `mtp_draft_tokens` default (8), 2 GB buffer cache, 25 GB wired |

Both were driven through the app's own path, not their HTTP contract directly:
`nemo transcribe` reads the isolated `config.toml`, the `Engine` interface in
`src/backend.rs` builds the launch command, the helper runs under the app's sandbox, and
the latency is the app's own `request_ms` event. The harness is
`scripts/benchmark-app.py`.

- **Corpus:** all twelve frozen fixtures, 1 to 300 seconds, seed 1729.
- **Rounds:** five, alternating which engine goes first.
- **Warm:** one process per engine per round serving a primer and a shuffled schedule:
  three requests per fixture up to 60 s (15 samples), one above (5 samples).
- **Process-cold:** a fresh `nemo` process per request on the 1, 5, 30 and 60 s clips
  (5 samples each). The total includes the model load.
- **Memory:** the engine's `phys_footprint`, sampled twice a second. Resident size is
  not used: it reports about 4 GB for an engine holding more than 20 GB of Metal memory.

## Warm requests

Median and min–max in milliseconds. Word errors are per response against the reference;
every response of an engine on a clip had the same transcript unless shown.

| clip | `nemo-speech` | `mlx-audio` | speedup | word errors (ref words) |
| --- | ---: | ---: | ---: | --- |
| 1 s | 163 (161–170) | 155 (151–160) | 1.05x | 0 / 0 (2) |
| 5 s | 310 (304–334) | 188 (183–192) | 1.65x | 0 / 0 (15) |
| 10 s | 465 (459–504) | 300 (292–316) | 1.55x | 0 / 0 (28) |
| 20 s | 761 (748–803) | 468 (458–488) | 1.63x | 0 / 0 (55) |
| 30 s | 1072 (1056–1140) | 670 (657–695) | 1.60x | 0 / 0 (87) |
| 40 s | 1377 (1352–1450) | 907 (872–944) | 1.52x | 0 / 0 (112) |
| 50 s | 1807 (1783–2031) | 1165 (1133–1390) | 1.55x | 0 / 0 (138) |
| 60 s | 1916 (1891–2003) | 1235 (1213–1298) | 1.55x | 2 / 2 (152) |
| 90 s | 2871 (2836–3016) | 1908 (1864–2038) | 1.50x | 5 / 5 (231) |
| 100 s | 3036 (2998–3139) | 1974 (1936–2019) | 1.54x | 0 / 0 (246) |
| 200 s | 7977 (7824–8227) | 6674 (6579–6922) | 1.20x | 619 / 454 (499) |
| 300 s | 2248 (2217–7061) | 6288 (6213–6470) | 0.36x | 713–734 / 3 (747) |

Three numbers here are not what they look like:

- **1.05x at 1 s** is the fixed per-request cost. A one-second clip generates almost no
  tokens, so there is nothing for speculation to speed up.
- **0.36x at 300 s is not a slowdown.** The bundled runtime answered in 2.2 s because it
  returned 40 words of a 747-word passage. MLX-Audio took 6.3 s and returned 748 words
  with three errors. This is the known 200/300 s failure recorded in
  `benchmarks/speech/nemo-check.json`.
- **200 s fails on both engines** (WER above 90% on each). Since two unrelated runtimes
  fail the same clip, the cause is more likely the model or the fixture than either
  runtime. Unresolved.

Against the earlier HTTP-level confirmation (5/10/20/30 s: 1.72/1.62/1.69/1.65x at p50),
the app path measured 1.65/1.55/1.63/1.60x, within 5% on every clip, even with the new
2 GB cache bound.

## Process-cold requests

Median and min–max in milliseconds, five fresh processes each.

| clip | `nemo-speech` total | `mlx-audio` total | load: `nemo-speech` / `mlx-audio` | first request: `nemo-speech` / `mlx-audio` |
| --- | ---: | ---: | --- | --- |
| 1 s | 2694 (2584–3645) | 3330 (2379–4131) | 2510 / 3143 | 186 / 190 |
| 5 s | 2730 (2722–2835) | 2421 (2306–2540) | 2399 / 2196 | 328 / 225 |
| 30 s | 3488 (3485–3503) | 2900 (2891–3329) | 2405 / 2197 | 1093 / 703 |
| 60 s | 4352 (4308–4416) | 3467 (3452–3559) | 2404 / 2190 | 1944 / 1275 |

These are process-cold with the operating system's file cache warm, because each engine
had just been loaded minutes earlier. Under that condition the two engines load in about
the same time (2.2–2.4 s). The 1 s row runs first in each visit, straight after the
other engine's 20+ GB was in use, and is the only row where MLX-Audio loads slower.

The earlier record's "5.5 s against 2.6 s" load was not reproduced here. True disk-cold
loads were not measured as a series: the only two seen in this session were the first
loads of the day, 16.0 s for the bundled runtime and 3.8 s for MLX-Audio. Treat those as
anecdotes.

## Memory

| | after load | after a warm pass | peak |
| --- | ---: | ---: | ---: |
| `nemo-speech` | 20.0 GB | 20.2 GB (20.19–20.22) | 20.2 GB |
| `mlx-audio`, as shipped now (2 GB cache) | 26.8 GB | 27.0 GB (26.96–27.03) | 28.5 GB (28.33–28.54) |
| `mlx-audio`, before this change (unbounded cache) | 30.4 GB | 37.7–39.7 GB | same |

The documented "25.5 GB" for MLX-Audio was MLX's own peak *active* memory. The process
held far more, because MLX keeps freed buffers in a cache that had no limit: 5.6 GB left
over from loading, growing with every new audio length to 12.9 GB after 100 s clips and
14.8 GB after a 300 s clip. The live daemon was observed at 35.6 GB.

`scripts/mlx-runner.sh` now sets a 2 GB cache limit. In a paired screen on AC power
(three visits per setting, 5/20/60/100 s clips) the limit cost about 2–5% of warm
latency against an unbounded cache, with visit-to-visit noise of about 3%; the gated
table above already includes that cost. A zero-size cache cost 9–11% and is not used.

Active memory is 24.4 GB for this checkpoint, so MLX-Audio cannot go below the bundled
runtime's 20 GB without different weights.

## Reliability

The bundled runtime failed seven warm processes during the gated run, and one in each
of two earlier attempts: an HTTP 500 on a 90 s or 300 s clip, with this in its log:

```text
ggml_metal_synchronize: error: command buffer 0 failed with status 5
error: Invalid Resource (00000009:kIOGPUCommandBufferCallbackErrorInvalidResource)
ggml_metal_graph_compute: backend is in error state from a previous command buffer failure
```

Every failure happened while another GPU client was active: a separate project's video
renders in the first attempt, and the live Nemo daemon taking dictations on its own
MLX engine for the rest. That is a correlation from nine failures, not an established
cause. MLX-Audio had no failures. Each failed process was recorded and repeated, so the
latency table is complete.

Because the runtime stays broken after such a failure, `Backend` now retires an engine
that answers a 5xx, so the next dictation pays one load instead of failing again.

## Decision

The task's rule was: make MLX-Audio with MTP the default only if word error rate is
unchanged on every duration bucket and the median warm latency is at least 1.4x faster.

- **Speed: met.** The median over the twelve buckets is 1.54x. From 5 to 100 s every
  bucket is between 1.50x and 1.65x.
- **Accuracy: met where the two engines can be compared, and different where they
  cannot.** Word errors are identical on all ten clips up to 100 s. On 200 s and 300 s
  they differ because the bundled runtime fails those clips; MLX-Audio is better on
  both. Read literally, "unchanged on every bucket" is false. No bucket lost accuracy,
  and both differing clips are longer than the 120 s recording limit.

**The default was left alone.** `nemo-speech` remains the engine Nemo runs unless
`config.toml` names another. Three things weigh against promoting MLX-Audio, beyond the
literal accuracy rule:

- It cannot run after a standard install. It needs `uv`, Python 3.12+ and a second
  21.6 GB download that only `nemo backend setup mlx-audio` provides.
- It holds about 7 GB more memory (27.0 GB against 20.2 GB), and no setting brings it
  under the bundled runtime's footprint.
- Its measured advantage is speed alone. At one second of audio there is none.

MLX-Audio is therefore an option to opt into for faster dictation:
`nemo backend setup mlx-audio`, then `nemo backend use mlx-audio`. An automatic default
(MLX-Audio wherever it was already provisioned) was built and tried during this work
and removed before review.

## What changed in the code

- `src/backend.rs`: an `Engine` trait with `NemoSpeech` and `MlxAudio` implementations
  (assets, launch command, identity). Process ownership, health, cancellation and the
  request stay shared. Each engine now receives only the options it implements; the MLX
  helper used to be sent every `--asr.salm.*` flag, including `mtp_draft 0`.
- `mtp_draft_tokens` (0–8, default 8) in `config.toml`. Setting it for `nemo-speech` is
  an error, not a silent no-op.
- `scripts/mlx-serve.py`: `--mtp-draft`, a bearer-key check, memory counters on
  `/health`. `scripts/mlx-runner.sh`: the 2 GB cache limit.
- `nemo transcribe` accepts several files in one process and `--json`.
- `nemo doctor` reports whether speculative decoding is active.
- A demotion keeps the failed engine's output as `inference-failed.log`.

## Limits of this measurement

- One machine, one session, one passage per duration, synthetic speech.
- 15 warm samples per clip up to 60 s and 5 above: medians, not p95.
- The live daemon was in use during the run. Nine processes that overlapped a dictation
  were discarded and repeated; a dictation wholly inside one process would not have been
  seen, though no warm repeat spread exceeded 1.23x.
- This is the app's request path. Microphone capture, finalization, clipboard and visible
  insertion were not exercised, and no installed-app acoustic check was run on this
  build.
- The gated run used the packaged build before the demotion-log change was added. That
  does not touch the measured request path. An intermediate build was checked
  separately for transcripts and sockets; the final tree differs from it only in
  engine selection, which is covered by unit tests.
- The 5xx retirement has a unit test against a stub server and the demotion log has
  none; neither was triggered against a real engine.

## Machine and revisions

- MacBook Pro `Mac17,6`, Apple M5 Max, 128 GB, macOS 26.5.2 (25F84), AC power,
  `powermode 2`.
- Nemo: `733c87c` plus the working-tree changes described above, release build, rustc
  1.95.0. Measured binary SHA-256 `99a9484a…527e436a`.
- NeMo-Speech.cpp `6a62501107e1894723880f1094a286ea996a0ec7`; packaged `nemo-speech`
  SHA-256 `1efc24a4…d0b06467`.
- MLX-Audio `7a0986280fe295f4cf40f64164bb23abed416a22`, mlx 0.32.4, CPython 3.12.12,
  uv 0.13.5. `mlx-serve.py` SHA-256 `541ff089…0a191428`.

## Evidence and reproduction

Everything is under ignored `artifacts/performance/engine-ab-20261109/`: `frame.md`
(written before the gated run), `gated/results.json` (every response, timing, word
error, identity, discard and failure), the three set-aside attempts, the cache-limit
screens, and `final-check.log`.

```sh
cargo xtask package --output "$PWD/artifacts/performance/run/Nemo.app"
uv run --no-project scripts/benchmark-app.py \
  --nemo artifacts/performance/run/Nemo.app/Contents/MacOS/nemo \
  --models "$HOME/Library/Application Support/Nemo/models" \
  --model "$(nemo model path)" \
  --daemon "$HOME/Applications/Nemo.app/Contents/MacOS/nemo" \
  --rounds 5 --repeats 3 --long-repeats 1 \
  --output artifacts/performance/run/gated
```

Three earlier attempts were set aside and are kept: two were contaminated by another
project's GPU renders (timings 30–60% slow, then up to 3x apart between rounds), and one
stopped on the runtime failure above before the harness learned to record and continue.
`--competitor PATTERN` makes the harness wait out a named GPU workload.
