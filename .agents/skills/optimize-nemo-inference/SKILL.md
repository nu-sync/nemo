---
name: optimize-nemo-inference
description: Profile and speed up Nemo dictation on Apple Silicon MacBooks. Use for Stop-to-text latency, MLX/Metal encoder or decoder tuning, model residency, and silent accuracy-preserving inference experiments. The reference laptop is the 128 GB M5 Max.
---

# Optimize Nemo inference on a MacBook

Optimize the time a person waits after pressing Stop. Nemo serves one dictation
at a time. On the maintainer's MacBook, use saved speech files exclusively for
experiments. Leave the speakers, microphone, clipboard, foreground app and
dictation shortcut alone. Run commands from the repository root.

## Establish the baseline

The reference MacBook Pro is a Mac17,6 with an M5 Max, 18 CPU cores, 40 GPU
cores and 128 GB of unified memory. Record the host actually tested. On a
smaller MacBook, establish a fresh baseline and check memory pressure before
trying the reference laptop's residency settings.

```sh
sysctl -n hw.model machdep.cpu.brand_string hw.physicalcpu hw.memsize
system_profiler SPDisplaysDataType
sw_vers -productVersion
pmset -g batt
pmset -g custom
memory_pressure -Q
uv run --no-project scripts/speech_corpus.py
```

Use AC power for comparable timings. For a battery-specific question, measure
a separate battery series. Keep the user's power mode intact. Run one benchmark
at a time and yield to dictation or competing GPU work. Check memory pressure
and swap before and after memory-intensive trials. Use the helper's physical
footprint to account for Metal allocations in unified memory.

Identify the exact executable, runtime resources, active engine, model profile
and options. The installed app, a signed candidate and the checkout can all
differ. The install receipt and code signature identify a bundle. Record the
model revision or verified manifest, helper lock, draft depth, wired/cache
limits, preload and idle expiry. The M5 Max reference experiments used MLX-Audio
with NVFP4 weights, eight-token greedy speculation, a 25 GB wired limit and a
2 GB MLX cache. Resident inference used about 27 GB. Check each build before
reusing those settings. [Performance history](../../../docs/performance.md)

## Pick the measurement that answers the question

| Question | Silent measurement |
| --- | --- |
| Does a decoder change help normal dictation? | Replay the same 30-second project update at 48 kHz mono PCM16 through each signed app. Include a 5-second guard for short utterances. |
| Is loading or keeping the model warm worthwhile? | Separate load time, first request after load, warm reuse, and first request after a real idle interval. Record helper identity, footprint and idle jobs. Test progressively less work and longer intervals against a zero-work control. Include recording and cancellation boundaries when interpreting passive telemetry. |
| Did a change reduce actual Stop latency? | Read existing delivery events with `uv run --no-project scripts/latency-report.py`. Report `stop_to_dispatch_ms` separately from file `request_ms`. Visible text needs its own observation. |
| Which subsystem limits the request? | Time resampling, encoder and prompt, prefill, draft proposals, target verification and cache repair. Start at existing MLX synchronization points; extra fences can change GPU scheduling. Confirm with uninstrumented replay. [Pipeline](../../../docs/inference-pipeline.md) · [Decoder profile](../../../docs/target-verification-m5-max.md) |

Before changing code, calculate how much of the measured stage must disappear
to matter for the whole request. For example, saving 5% of a 585 ms request
requires about 29 ms. A 16 ms vocabulary projection cannot deliver that alone.
For a bandwidth hypothesis, count the weights actually read across verification
passes and compare against measured bandwidth on this machine. For speculation,
record proposed and accepted tokens, verification passes and emitted length.
Trace the actual engine launch and request timing in [the Rust backend](../../../src/backend.rs).
For MLX, follow the [bundled helper](../../../scripts/mlx-serve.py) into the
decoder shipped with that bundle. A helper-only change can alter latency while
the Rust controller payload stays identical.

## Compare one change

Write the hypothesis, target duration, transcript contract, minimum worthwhile
gain and trial budget in a new ignored directory under `artifacts/performance/`.
Read the last decision on that lever before repeating it.

For two builds of the **same MLX engine**, use the skill's bundle comparison.
Supply existing signed bundles and the provisioned model store. Each visit
writes its own config, shares the model files, runs `nemo transcribe --json`
and measures a second request on the same warm helper. The output directory
must be new. The script uses `NEMO_DATA_DIR` or Nemo's normal Application
Support directory to locate the source model store and observe daemon activity.

```sh
skill=.agents/skills/optimize-nemo-inference
baseline="$HOME/Applications/Nemo.app"
candidate="$PWD/dist/Nemo.app"
mkdir -p "$PWD/artifacts/performance"
run=$(mktemp -d "$PWD/artifacts/performance/nemo-inference-XXXXXX")
uv run --no-project "$skill/scripts/compare-bundles.py" \
  --baseline "$baseline" --candidate "$candidate" \
  --seconds 30 --rounds 6 --output "$run/30s"
```

Check bundle identities before interpreting an apparent win. Passing the same
bundle on both sides exercises the apparatus. Use `--seconds 5` and a new
output directory for the short guard. Alternate which build goes first, inspect
paired reductions and their spread, and repeat a promising result independently.
An incomplete run retains its evidence. Retry disturbed timings in a fresh
directory when the laptop is idle.

For MLX versus the native engine, use `scripts/benchmark-app.py`. For native
runtime options, broader fixture sweeps and corpus auditing, follow the
[benchmark-nemo journeys](../benchmark-nemo/references/journeys.md). The corpus
harness decodes 16 kHz audio. Recheck capture-path changes at 48 kHz.

## Decide and document

- For a speed-only change, compare raw transcripts across all twelve frozen fixtures, including 200 and 300 seconds. Check reference word errors per fixture too. Equal WER can hide punctuation changes, and early termination or changed output length can look like faster decoding. For resampling or precision changes, also compare identical inputs with noise, echo, padding or lower volume.
- Use the predeclared gate. Nemo's default is a repeatable 5% improvement on the primary warm request, exact transcripts, and no material regression on short requests. Record smaller repeatable gains as prototypes. Treat an accuracy improvement that changes text as a separate result.
- Keep raw fixture responses, per-request timings, bundle and model identities, peak footprint and rejected trials in the run directory. Record the decision and reproduction command in [the performance notes](../../../docs/performance.md). Report the timing boundary, sample count and paired change. Installation is a separate delivery action.

The experimental method is adapted from Sybil Solutions' MIT-licensed
[inference-optimisation skill at fda968e](https://github.com/sybil-solutions/inference-optimisation-skill/tree/fda968e7fc1996aa23c67139c755fe541c9d4be0/skills/inference-optimisation).
[Upstream license](LICENSE.upstream)
