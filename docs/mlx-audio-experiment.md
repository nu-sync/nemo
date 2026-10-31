# MLX-Audio with MTP speculative decoding

On October 30, 2026, MLX-Audio running the NVFP4 checkpoint with MTP speculative
decoding **passed** a first measurement: **1.55–1.69x faster wall clock** than the
installed runtime on the frozen corpus, with unchanged word error rate.

Nothing was installed, merged, or changed for users by this experiment. The
installed app still runs the pinned NeMo-Speech.cpp Metal runtime with
`mtp_draft=0`.

Read first: [the inference pipeline](inference-pipeline.md), which establishes
that 81% of Nemo's latency slope is single-sequence decode. That is the term this
experiment attacks.

## The hypothesis

Nemo's runtime forces `mtp_draft = 0` on every non-CUDA backend
(`salm_engine.cpp`: `if (!fast) cfg.mtp_draft = 0;`), because NeMo's batched
serving engine rolls drafts back through replay rows the upstream llama.cpp path
does not have. So the app decodes one token per weight read, at 8.38 ms/token.

The model's MTP head already exists in the GGUF we own — block 52, 0.78 GB of the
19.68 GB file, reported by the loader as unused tensors. The hypothesis was that
speculative decoding, not a different framework, is what is missing: *produce
several tokens per weight read instead of one*.

MLX-Audio was chosen as the test vehicle because it is the only engine available
on Apple Silicon that implements MTP specification for this exact model.
`transcribe-cpp` cannot load `nemotron_h_moe` at all.

## Candidate and acceptance rule

Checkpoint `nvidia/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-NVFP4` — 21.6 GB over 24
files, 25.5 GB peak after loading. This checkpoint and not the community
early-access exports, because the EA exports omit both the MTP and CTC heads, and
not BF16 or FP8 because FP8 expands to BF16 on load and 8.0 GB per token is
hopeless against a 530 GB/s memory system.

Engine MLX-Audio at `7a0986280fe295f4cf40f64164bb23abed416a22`, mlx 0.32.4,
`mlx-metal` 0.32.4, CPython 3.12.12, uv 0.13.4.

The pre-registered gate was "effective ≤ 6.3 ms/token with WER no worse than the
baseline's zero". **That gate was mis-specified and is reported as missed.**
"Effective" includes encoding and prefill, while the 8.38 ms/token baseline it was
being compared against is decode-only, so the two sides measured different
denominators. On like-for-like decoding the result is 5.26 vs 8.38 ms/token
(1.59x); on wall clock it is 1.55x. The intent — at least 1.5x with no accuracy
regression — is met. The gate is recorded rather than quietly redefined.

## Method

- Frozen corpus fixtures at 5, 20 and 60 seconds, converted to mono 16 kHz PCM16
  WAV with `afconvert`-equivalent CPU conversion; source FLAC hashes, converted
  WAV hashes and reference hashes are recorded alongside the results.
- Greedy decoding, `temperature=0`, `max_tokens=2048`, default prompt, no word
  timestamps. Four configurations: speculation off, and 2, 4 and 8 draft tokens.
  **Eight is the engine's documented maximum** ("1–8 draft tokens per verification
  step"), so the top of the range was already reached.
- One warm-up then three measured repeats per configuration and clip; medians
  reported, warm-ups excluded from medians and scored all the same.
- A GPU lock was taken for the run and the installed daemon was confirmed idle
  (`Ready — … Transcript delivered`) before and after; no parallel builds, no
  competing inference.
- `effective_ms_per_token` is synchronised wall time over generation tokens and
  **includes encoding and prefill**. `decode_ms_per_token` instruments the token
  generator after the first emitted token. Neither is a profiler attribution.

## Results

Wall clock, median of three measured repeats. Baseline is the installed Metal
runtime on the same clips from the frozen corpus suite.

| clip | current runtime | MLX off | MLX 2 drafts | 4 drafts | **8 drafts** | speedup at 8 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 5 s | 310 ms | 263 ms | 219 ms | 196 ms | **183 ms** | **1.69x** |
| 20 s | 757 ms | 761 ms | 590 ms | 504 ms | **449 ms** | **1.69x** |
| 60 s | 1914 ms | 1967 ms | 1531 ms | 1337 ms | **1239 ms** | **1.55x** |

| clip | decode ms/token (off → 8) | accept rate at 8 | tokens per verification step | WER, both sides |
| --- | --- | ---: | ---: | ---: |
| 5 s | 9.35 → 5.54 | 93.8% | 8.5 | 0 → 0 |
| 20 s | 9.65 → 5.04 | 100% | 9.0 | 0 → 0 |
| 60 s | 9.45 → 5.26 | 98.7% | 8.6 | 1.32% → 1.32% |

Transcripts were identical across configurations, including the 60 s clip's two
edits against its reference — the same two the current runtime makes. Accuracy is
unchanged on these clips, not merely within the 5% gate.

## The control that explains it

Non-speculative MLX is **not** faster than Nemo's current runtime:

| clip | current runtime | MLX, speculation off |
| --- | ---: | ---: |
| 20 s | 757 ms | 761 ms |
| 60 s | 1914 ms | 1967 ms |

Both engines land on the same bandwidth wall. MLX's MoE decode is no better than
ggml-metal's at one token per pass. **The entire gain comes from producing ~8.6
tokens per weight read instead of one.**

This matters beyond this experiment: it means the 1.55x is a property of the
model's MTP head, not of MLX. Any engine that implements draft-and-verify for this
model should reach the same acceptance, including the runtime we already ship.

## Projected product effect

Fitting the medians gives `request_ms ≈ 77 + 19.30 × audio_seconds`, against
`135 + 29.70 ×` for the current runtime — better on both the fixed and the
per-second term. Adding the measured 21 ms finalization and ~45 ms insertion:

| | today | MLX + MTP-8 |
| --- | ---: | ---: |
| 1 s stop-to-visible ceiling | 26.9 s of dictation | **44.5 s** |
| p50 (6.5 s dictation) | 328 ms | 202 ms |
| p95 (25.8 s) | 901 ms | **575 ms** |
| p99 (38.7 s) | 1284 ms | 824 ms |
| 60 s | 1917 ms | 1235 ms |

These are projections from a fitted model, **not** installed measurements.

## Costs

| | current runtime | MLX-Audio NVFP4 |
| --- | --- | --- |
| weights on disk | 21.35 GB (already owned) | +21.6 GB (new download) |
| peak resident | ~20 GB | 25.5 GB |
| load to ready | ~2.6 s | 5.5 s |
| runtime | C++ helper process | Python + MLX helper process |
| weight license | current mirror | OpenMDW-1.1 |

The load time matters because it sits inside the recording window; loading is
already overlapped with capture, so a 2.9 s increase reduces the margin for a very
short dictation.

## Cold load is a real cost the warm measurement hides

Everything above is warm. The 183/449/1239 ms figures require a retained process,
and MLX loads in **5.5 s** against the current runtime's **2.6 s**. That difference
lands entirely on dictations that arrive after an unload.

From the live daemon log (591 dictations, 74 cold loads — 13% pay a load at all):

| cold load | dictations that wait for it | median excess wait |
| --- | ---: | ---: |
| 2.6 s (today, measured p50 2508 ms) | 14.6% | 0.79 s |
| 5.5 s (MLX) | **42.0%** | **2.16 s** |

So adopting MLX without changing retention would make roughly two in five
dictations wait about two seconds. The warm-corpus numbers say nothing about this,
and no amount of decode speed fixes it.

It is solvable, but not free: `preload_model = true` moves the load to app launch
and removes most of the exposure, at the cost of holding 25.5 GB whenever Nemo is
running and a longer launch. That trade-off should be decided explicitly rather
than discovered, and it is the strongest argument for putting the port into the
runtime we already have rather than shipping a faster-but-slower-to-start engine.

## What this experiment did not do

- **No end-to-end measurement.** This drove MLX-Audio in-process in Python. It was
  not behind Nemo's `/health` + `/v1/audio/transcriptions` contract, and it did not
  pass through audio finalization, clipboard write or paste.
- **Three of twelve fixtures**, one passage per duration. Not the gated full-corpus
  A/B, and not broad accuracy coverage.
- **No streaming.** This model refuses streamed audio on every engine, so
  `stop → visible` still cannot overlap decode with recording.
- **No profiling.** No claim is made about which Metal kernel limits either engine.
- **Not reproduced.** One measurement run, no repeat session.

## Conditions for revisiting or adopting

Adoption requires all of: the full normal-duration corpus A/B through the
harness with the existing WER gates; an installed recording and visible-insertion
check on the exact artifact; a decision on shipping a Python runtime; and the
21.6 GB download plus license accepted into the tree. The separate option of
implementing draft-and-verify in the runtime we already ship is analysed in
[pluggable inference backends](pluggable-inference-backend.md).

## Evidence and reproduction

Measurement harness (repo convention: PEP 723 inline metadata, adjacent lock):

```sh
uv lock --script scripts/measure-mlx-audio.py          # already committed beside the script
uv run --locked scripts/measure-mlx-audio.py --prepare # CPU-only preflight
uv run --locked scripts/measure-mlx-audio.py --run --nemo /path/to/nemo --repeats 3
```

The `--run` path takes `artifacts/coordination/gpu.lock`, requires the installed
daemon to report idle, and refuses to start otherwise. It makes 48 generations and
writes `results.json` and `report.md` under `artifacts/mlx-audio/`.

Raw evidence in the worktree `nemo-inference-backends` on branch
`feat/inference-backends`: `artifacts/mlx-audio/results.json` (every repeat,
transcript, timing, accept count, checkpoint file hash and package version),
`artifacts/mlx-audio/report.md`, and `artifacts/mlx-audio/verdict.md`.

## Re-measured behind Nemo's own contract

The numbers above are in-process. They were re-run through the boundary the app
actually uses: a helper speaking `GET /health` and `POST /v1/audio/transcriptions`,
driven by `scripts/benchmark-corpus.py` with the shipped runtime as control. Paired,
alternating, fresh server per visit, `state=complete`, WER 0.00% on every record.

| clip | control (nemo-speech) | MLX helper | speedup | paired change |
| --- | ---: | ---: | ---: | ---: |
| 5 s | 307.5 ms | 180.4 ms | **1.70x** | -41.3% |
| 10 s | 463.7 ms | 294.4 ms | **1.58x** | -37.0% |
| 20 s | 752.9 ms | 464.1 ms | **1.62x** | -38.8% |
| 30 s | 1062.9 ms | 659.5 ms | **1.61x** | -38.2% |

The HTTP hop, multipart upload, JSON response and a Python process cost nothing
measurable. This is still not the product objective: finalization, clipboard write
and visible insertion are outside it, and it is 4 of 12 fixtures at five visits and
one repeat, so it is screening rather than confirmation.

## The same gain in the runtime we already ship: where it stops

The 1.55x is decode-side, not MLX-side: non-speculative MLX runs at the same speed as
`nemo-speech` (761 vs 757 ms at 20 s), so the gain is producing ~8.6 tokens per weight
read instead of one. Our own runtime already contains the whole MTP decode loop,
un-gated from CUDA: `salm_engine.cpp` selects `step_mtp()` whenever a drafter exists
and it compiles for Metal; rollback is supported for `nemotron_h_moe`; and the MTP
head is already the 0.78 GB block 52 of the GGUF we download. No new weights needed.

Four blockers were cleared locally and built, then it stopped:

| Blocker | State |
| --- | --- |
| `if (!fast) cfg.mtp_draft = 0` | removed; `mtp_draft=8` honoured |
| target `positions remain consecutive` abort | fixed by per-ubatch prefill ingestion |
| `at most 9 outputs per sequence` abort | fixed by not requesting logits per prompt row |
| drafts rejected, `llama_decode(ctx_dft)` fails | **open** |

The open blocker is precise: the drafter needs the target's per-row NextN hidden
states, the target emits them only for rows that request output, and
`n_outputs_max_per_seq` is `mtp_draft + 1 = 9`. Prompt rows therefore have to be
replayed to the target in groups of at most nine — what the CUDA-only
`cp.serving.mtp_replay` does and the non-batched path never had.

With drafts rejected the build returns the **correct transcript** but is 3.5-8.5%
*slower* on every duration, because it pays for a drafter it cannot use. No speedup
is claimed. The local changes are not in git: see `artifacts/mtp-metal/` in the
`nemo-inference-backends` worktree and `~/Library/Caches/nemo-mtp/attempt-1-full.diff`.
Any future attempt should adopt the `positions remain consecutive` failure as its
acceptance test.

## Confirmation at 100 warm samples per fixture

Two separate stage-C runs measured the shipped `nemo-speech` runtime against the
bundled MLX helper through the same HTTP contract. Each run used 5/10/20/30-second
fixtures, ten alternating visits and ten repeats per fixture, giving 100 warm
samples per fixture per engine. Both runs finished `state=complete`, passed the
structure/accuracy audit, reproduced the same transcripts at 0.00% WER, and left
zero MLX engines behind. The first ran with the installed daemon idle; the second
ran with Nemo closed because live dictation had twice made the daemon guard refuse
a measurement. Every round had MLX faster on every fixture.

| clip | run | nemo-speech p50 / p95 | mlx-audio p50 / p95 | median paired change |
| --- | --- | ---: | ---: | ---: |
| 5 s | 1 | 307.0 / 326.3 ms | 178.9 / 183.2 ms | -41.95% |
| 5 s | 2 | 306.8 / 316.4 ms | 179.8 / 185.6 ms | -41.52% |
| 10 s | 1 | 462.7 / 490.4 ms | 285.7 / 294.8 ms | -38.35% |
| 10 s | 2 | 462.3 / 486.5 ms | 287.3 / 297.5 ms | -38.18% |
| 20 s | 1 | 752.0 / 789.7 ms | 445.8 / 459.7 ms | -40.93% |
| 20 s | 2 | 751.9 / 784.5 ms | 445.8 / 462.0 ms | -40.88% |
| 30 s | 1 | 1062.9 / 1121.8 ms | 642.6 / 667.5 ms | -39.93% |
| 30 s | 2 | 1061.4 / 1101.7 ms | 641.5 / 660.4 ms | -39.88% |

That is **1.61–1.72x** on p50 and the same direction on p95, consistent with the
earlier screening numbers. The 40/50/60-second guard screen (five visits, three
repeats) also stayed in the same range: 1.56x, 1.60x and 1.58x on p50. The 60-second
MLX p95 was a single 1431 ms sample against a 1199 ms p50, so the tail at the
longest guard duration is noisier than the confirmation fixtures. Both engines made
the same two substitutions on that 60-second fixture; the accuracy audit reported
no regression. This is a screening sample count for the guards, not a p95 claim.

**Verdict: keep.** The 1.6x HTTP latency gain is confirmed on the important
durations with 100 warm samples per fixture and no WER regression. It is still not
the product objective: these numbers exclude capture, finalization, clipboard write
and visible insertion, and four fixtures at each of four durations are not broad
accuracy coverage.

## Installed path, including visible insertion

Two installed acoustic checks passed on the bundled helper with
`"engine":"mlx-audio"` and `"reused_model":true`. The clipboard-only run used a
7.23 s capture: 920 ms request and 942 ms stop-to-dispatch. Two separate runs used an
empty scratch TextEdit document, verified TextEdit foreground before start and
stop, and confirmed the inserted text afterwards. The first measured a 7.73 s
capture, 940 ms request and **970 ms stop-to-dispatch**; the final installed
build measured 7.73 s, 915 ms and **950 ms**. A third run polled the document every
50 ms while the same installed MLX path ran, so it could timestamp the visible
text rather than only observe it afterwards: a 7.67 s capture, 832 ms request,
860 ms stop-to-dispatch and **986 ms stop-to-visible**. The poll interval and the
AppleScript read cost make that an upper bound, not millisecond accuracy, and it is
one run at one duration rather than a p95. Dispatch is not insertion; here insertion
was separately confirmed to have happened. The user's configuration was preserved;
each scratch document was closed without saving.

Evidence: `artifacts/performance/mlx-ship-20261031-124041/` (`confirm1`,
`confirm2c`, `guards`, their audit JSON, console output and exit codes). The
rejected guarded attempts during live dictation are retained there as
`confirm2/` and `confirm2b-console.txt`.
