# Hill-climbing decisions

## Declare the experiment before running

Create `$RUN/decision.md` with:

```text
Goal: improve normal short/medium dictation latency; do not degrade words.
Hypothesis and limiting stage:
One variable changed:
Baseline runtime/build flags/model/encoder:
Candidate runtime/build flags/model/encoder:
Frozen corpus + scoring/harness identity:
Important durations and statistic:
Other durations and allowable latency regression:
Accuracy gate: per-response WER <= 5%, plus no fixture-level regression vs baseline.
Minimum useful improvement: initially 5%, unless the task specifies otherwise.
Budget: candidate count, visits/requests, wall-clock/build/download limits.
Verdict and evidence: fill after the experiment; preserve rejected runs too.
```

Record source revisions and build flags separately from the binary hashes. The
runner hashes decoder/runtime and colocated `*Encoder*.gguf` files, but does not
fingerprint every native library or Metal resource in a runtime bundle. Compare
complete candidate build directories rather than transplanting a binary with
incompatible supporting resources. Do not mutate a candidate while it is running.

The known profile is Metal, one sequence, contexts 8192, decoder batch 512,
encoder batch 1, MTP draft 0, eight inference threads, startup warmup off. Inspect
current `scripts/benchmark-corpus.py` and `src/backend.rs` before claiming these
still match Nemo. HTTP `--threads 4` is not the inference thread count.

## Choose a plausible lever

Start from evidence, not a knob sweep:

1. Inspect `scripts/latency-report.py` output and current per-fixture baselines.
   Your normal workload is short/medium dictation, typically under 60 seconds;
   do not optimize a five-minute clip and label that a normal-usage p95 win.
2. The previous measurements in `docs/performance.md` found no reliable benefit
   from thread/encoder-batch/startup-warmup tuning. Treat those as historical
   evidence for that hardware/build, not an eternal ban on rechecking.
3. A runtime/Metal kernel change or another quantization/model is a candidate
   only after checking exact upstream revisions, platform applicability, build
   compatibility, and task authorization. An upstream PR title is not a speedup.
   Changing the encoder and decoder together is a model comparison, not an
   isolated kernel result; label it accurately.
4. Use separate profiling to identify the limiting stage when explaining a gain
   or selecting a consequential rewrite. Profiling changes timing, so keep it
   out of comparison runs. `scripts/benchmark.py --profile` is a separate one-WAV
   diagnostic; inspect its `--help` and retain its logs. GPU wait stacks alone do
   not prove memory-bandwidth saturation or identify a particular slow kernel.
5. VAD, streaming/chunking, capture overlap, and paste changes need their own
   behavioral/installed verification. Whole-WAV HTTP replay does not implement
   or prove streaming latency. Read existing VAD/delivery reports first.

Do not change system power policy or unload the user's model merely to improve
one side. Record the observed load/power environment; reject confounded comparisons
or repeat both sides after it stabilizes.

## Apply gates in order

1. **Validity:** `state=complete`; full visits and paired schedules; real runtime
   identities; healthy servers; all expected requests present; no transport
   failures. An interrupted or busy run is incomplete, not a slow candidate or
   a reason to discard only its worst rows.
2. **Accuracy:** inspect every first/warm transcript and its edits. The runner's
   5% ceiling alone permits a candidate to get worse while still passing. Compare
   errors for the same `(round, fixture, repeat, stage)` and require no increase
   for a strict no-regression decision. Review changed words even when WER ties;
   equal scores can hide different meaning-changing technical mistakes.
   If results vary, increase matched visits and review the distribution before
   claiming a regression or equivalence. Do not add spelling/number aliases to
   rescue only the candidate; a scoring change requires both sides remeasured.
3. **Speed:** read `comparisons[candidate][fixture]`. Negative median paired
   change is faster. Judge each declared important duration, per-round direction,
   p50/range, and tails; do not average all durations or cherry-pick a lucky clip.
   Initially require a ≥5% paired improvement with a consistent direction across
   visits and no material slowdown on other declared durations. Set any allowed
   regression/tradeoff before running and surface it to the user.
4. **Confirmation:** screen with at least five alternating visits. For a promising
   candidate, use ten visits with ten repeats (100 warm samples per fixture), then
   repeat separately. Fewer than 100 warm samples are flagged as screening by the
   harness; 100 is a practical floor, not a formal p95 confidence guarantee. If
   repeat runs overlap noise or disagree, report no measurable difference or
   inconclusive rather than a precise percentage win.
5. **Stress:** compare 90/100/200/300-second correctness. The stored runtime's
   200/300-second failures have independent Whisper agreement and remain defects,
   not permission to call those outputs accurate. A short-clip candidate may be
   promising while long-form remains blocked, but that is not an all-suite pass.
   Do not promote a new long-form accuracy regression behind an existing failure.
6. **Product:** require separate authorized recording/transcription/visible-paste
   checks for the exact candidate before claiming the product objective. HTTP
   upload/inference/JSON excludes finalization/paste. Dispatch telemetry is not
   visible insertion. Cold load and first request are separate from warm latency.

Keep a candidate only after the applicable gates and explicit tradeoff decisions.
A meaningful but unconfirmed improvement is **promising**, not retained. Leave the
installed app untouched until installation is explicitly part of the task.

## Stop rules and handoff

Stop at the experiment budget, an established winner, a safety/prerequisite gap,
or repeated results within noise. When two attempts based on the same presumed
bottleneck fail, revisit the premise before trying another nearby parameter.
Preserve baseline and rejected artifacts so another reader can reproduce the
choice. Do not turn this skill into an unattended install/restart loop.

Report: hypothesis; actual single variable; artifact and corpus identities;
matched environment and limitations; selected durations; visits and first/warm
counts; latency distribution and paired changes; transcript/WER changes;
stress defects; cleanup; installed-path coverage; verdict; evidence paths; and
the next bounded experiment if still useful.
