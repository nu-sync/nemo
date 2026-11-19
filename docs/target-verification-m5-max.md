# Target verification on the M5 Max

On November 19, 2026, the shortlist decoder installed at the start of the experiment spent a median **280 ms**
verifying draft tokens in a **585 ms** warm helper request. The 30-second
project-update fixture produced 95 tokens. The verifier ran ten batches of
nine positions and one batch of seven. The reference transcript has 87 words.

The fastest confirmed scheduling change submitted the target backbone to Metal
in groups of twelve layers. Through the signed app's file-transcription path
it reduced median warm request time from **610 to 585 ms**. It won 11 of 12
pairs across six alternating visits. The median paired reduction was **4.1%**,
below the 5% retention threshold set before the run. Both paths returned
byte-identical text on all 12 frozen recordings and five previously prepared
30-second variants with changes to noise, volume, echo or padding. The candidate
calls `mx.async_eval(hidden_states)` after target layers 12, 24, 36 and 48
when the verification loop evaluates the backbone. The final token readback
still waits for the complete target result. The prefill and draft computations
use the existing backbone path.

## Where the time went

| Measurement on the 30-second file | Median | How it was timed |
| --- | ---: | --- |
| Full helper request | 585 ms | Five warm runs, 578–710 ms |
| Target verification within those requests | 280 ms | Existing token readback, 277–354 ms |
| Target backbone | 268 ms | Separate five runs with a synchronization after the backbone |
| Vocabulary projection and argmax | 16.5 ms | Time from that synchronization to the verified token IDs |
| Routed expert projections and activation | 145 ms | Isolated replay of the actual expert-layer inputs |
| Shared expert MLP | 24 ms | Same replay |
| Expert routing decisions | 17 ms | Same replay |

The backbone includes 23 mixture-of-experts layers. Saving their inputs for all
11 verification batches gave 253 layer calls to replay. Within the routed
expert work, the input projection took 67 ms and the output projection took
73 ms. Six measured replays alternated operation order after a warm-up. The
replays remove dependencies between layers, so their medians do not add up to
the time spent in the original request.

For a cross-check, synchronizing after every target layer put the experts first,
followed by the Mamba recurrent layers and attention. Those fences increased
verification time from 280 to 420 ms. The less intrusive measurement above is
the basis for the latency claim. Saving 5% of the full request would require
removing about 29 ms, or roughly one fifth of the routed expert replay time.

## Experiments

| Change | Original → candidate warm helper median on 30 s | Finding |
| --- | ---: | --- |
| Sort the 54 expert routes before projection | 592 → 618 ms | Sorting added work. The grouped Metal kernel requires at least four routes per available expert. This batch has 54 routes across 128 available experts. |
| Precompute the per-expert scale factor | 592 → 603 ms | Moving this calculation to model load gave no repeatable gain. |
| Fuse the cached recurrent scan | 592 → 576 ms | The 30-second transcript matched, but the long accuracy challenge changed output. |
| Submit completed target-layer groups asynchronously | 678 → 653 ms | Submitting after every four or twelve layers gave a similar 30-second median, below the 5% gate. The short-clip portion was interrupted by a separate app update. |
| Verify up to 12 or 16 draft proposals at once | 669 → 669 or 677 ms | Both produced the same 30-second text. Neither reduced median request time. |
| Submit the cache repair asynchronously | 590 → 584 ms | Combining it with twelve-layer submission reached 569 ms, a 3.5% reduction in this screen. |
| Specialize a Metal projection kernel to the two expert widths | 627 → 631 ms | The best tested output tile used eight rows. Tiles of 16 and 32 rows were slower. Disabling floating-point contraction and reassociation matched the sampled reference projection values. |

Each screen used six rounds with a control before and after the candidates.
The ordering of the candidates alternated. Each row uses its own contemporary
control because the laptop's clock and load varied between runs.

The most promising isolated change padded the input widths from 2688 and 1856
to multiples of 512. This selects MLX's faster packed NVFP4 matrix-vector
kernel. On ten real nine-position inputs from one expert layer, input
projection time fell from 7.13 to 4.97 ms and output projection time from
5.61 to 4.22 ms. The kernel sums values in a different order. On these inputs,
28 and 101 intermediate BF16 values changed, respectively. Applying the
layout to all target experts also used an extra 2.35 GB. It preserved the
30-second transcript but changed the raw text on the 200- and 300-second
fixtures. The 300-second difference disappeared under normalized word scoring.
Exact comparison caught it. [Pinned MLX kernel selection](https://github.com/ml-explore/mlx/blob/36969c094354086e15d2a327a9c50feb20b9cf11/mlx/backend/metal/quantized.cpp#L1391)

### A long-recording accuracy finding

The fused recurrent scan preserved 11 of the 12 baseline transcripts. On the
200-second library proposal, the original decoder started talking about a
different subject. It made 506 word errors on a 499-word reference. The fused
scan returned 499 words with three substitutions. A fresh run in the order
candidate, control, control, candidate reproduced the same result twice for
each. The control took 15.3 seconds per request and the candidate took 3.7
seconds. Those requests generated different text. The time difference is
therefore unsuitable as a decoder speed comparison. This case warrants a
separate accuracy investigation on independent long recordings.

### Metal command submission

Raising MLX's command-buffer memory threshold from the default to 256 MB
looked faster in a noisy process-level screen. A second comparison used the
same signed shortlist binary with isolated launcher settings. Six alternating
paired visits each warmed the 30-second shape before recording two requests.
Across those 12 requests per setting the default took **603 ms** (598–617 ms)
and the 256 MB setting took **616 ms** (608–623 ms). Both produced identical
text. The signed-app confirmation reversed the first screen's apparent gain.

A separate app update during the final screens installed a new controller with
the earlier MLX helper. All the results above refer to the pinned shortlist
baseline. The original signed shortlist package remains in the previous run
directory. This profiling run did not install either package.

## Reproducing the profile silently

The baseline was the installed MTP-8 helper with decoder module SHA-256
`b6aaf344dd48e1d58f444cc8f495c20b4827239207dcbf878647c5a9b6faba84`.
All requests used saved 48 kHz mono PCM16, the existing NVFP4 weights, the
25 GB wired limit and 2 GB cache on the 128 GB M5 Max. The harness waited
for AC power, idle Nemo and no Blender process, and rejected intervals when
the normal daemon log changed. The live app continued serving the user.
Only file requests were timed. Stop-to-dispatch and foreground paste require
different measurements.

From this checkout, the local run directory contains the instrumented scripts,
raw timings, and candidate transcripts. The following commands validate the
corpus, repeat the profiles and produce fresh JSON in that directory.

```sh
uv run --no-project scripts/speech_corpus.py
run=artifacts/performance/target-verify
sh "$run/run.sh" "$run/stages.py"
sh "$run/run.sh" "$run/expert.py"
```

The twelve-layer candidate is in `overlap-runtime/nemo_mtp.py` under the run
directory. Its six signed-app pairs are in `overlap-confirm.json` (key `0` is
the control). The 17 accuracy comparisons are in `overlap-accuracy.json`.
`recheck.json` contains the independent long-recording scan comparison.
All new runtime code from this experiment remains in the local run directory.
