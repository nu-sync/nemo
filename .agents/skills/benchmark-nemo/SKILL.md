---
name: benchmark-nemo
description: Benchmark Nemo transcription and hill-climb latency with the frozen speech corpus, paired runtime/model experiments, WER regression checks, and preserved evidence. Use for Nemo performance baselines, p95 goals, runtime or quantization comparisons, benchmark regressions, and deciding whether an optimization is worth keeping.
---
# Benchmark and hill-climb Nemo

Improve latency without accepting lost words as a speedup. The product objective
is **p95 stop-to-visible-text ≤1 second for normal short-to-medium dictation**;
this harness measures **HTTP request latency**, not that end-to-end objective.
Do not add an estimated paste constant or pool the equally spaced fixtures into
a production p95.

Run commands from the repository root. Resolve links and bundled scripts relative
to this skill directory. Start with the [journey index](references/journeys.md),
then read [hill-climbing decisions](references/hill-climbing.md) before proposing
or selecting an optimization. Use [evidence and cleanup](references/evidence.md)
for every real runtime run. Existing entry points are documented in
[the benchmark guide](../../../docs/benchmarking.md).

## Scope and authority

- Use **uv for every Python invocation**: `uv run --no-project` for the
  standard-library harness/tests, and `uv run --locked` for generation and
  independent-ASR scripts with adjacent locks. The bundled audit helper has
  inline metadata with no third-party dependencies. Do not create a root Python
  project, install global packages, or bypass the locks. Record `uv --version`
  and the chosen Python interpreter in evidence.
- Preserve unrelated work. Read `git status --short` before edits. Keep candidate
  builds in separate worktrees/output directories; do not mix experiments into a
  dirty checkout or replace the installed runtime to compare candidates.
- Run **one GPU benchmark at a time**. Separate processes/ports/files isolate
  ownership, not GPU load. Avoid live dictation, TTS, builds, profiling, and other
  inference during timed comparisons. A busy-daemon refusal is a coverage gap,
  not a reason to stop the user's recording or bypass the guard silently.
- Benchmarking does not authorize installing, re-signing, merging, downloading a
  new transcription model, changing configuration/power/audio settings, or
  granting permissions. Candidate preparation follows the task's authorization.
  Explain any downloads needed for optional corpus generation/reference checks.
- These journeys require no speaker playback, microphone, clipboard, or paste.
  Installed acoustic/visible-insertion checks are separate, potentially audible
  work: use [verify-nemo](../verify-nemo/SKILL.md) within existing authorization.

## Execute a bounded loop

1. **Frame.** Write the workload, hypothesis, one changed variable, target fixtures,
   latency statistic, accuracy gate, minimum useful gain, and experiment/time
   budget in a new ignored `artifacts/performance/` run directory. If the task has
   no budget, start with one candidate and one paired screening experiment; do
   not launch an unbounded search. Keep the baseline binary/model available.
2. **Establish inputs.** Follow journey 1. Check corpus hashes and exact decoded
   frame counts; inspect reference-check provenance. Freeze audio, reference text,
   scoring code, harness, seed, power mode, and nonexperimental options throughout
   an A/B comparison. A changed harness/corpus requires remeasuring both sides.
3. **Verify the apparatus.** Follow journey 2 for a first use, harness change, or
   suspicious result. Run real A/A inference with two names for the same runtime,
   model, and options. This exercises health readiness, transcriptions, pairing,
   persistence, and process cleanup; it cannot establish an optimization.
4. **Fast screen first.** Start journey 3, stage A: baseline plus one candidate,
   **5/10/20/30 seconds, five alternating visits, one repeat**. This is roughly a
   one-minute paired experiment on the measured Mac. Reject clear regressions;
   advance only a promising signal. Five samples per fixture/candidate cannot
   certify a small gain or p95. Do not start every experiment with a full suite.
5. **Broaden only survivors.** Use stage B: **all normal 1–60-second fixtures,
   five visits, three repeats**. This is about 4½ minutes per pair, estimated from
   the measured single-configuration run. Check 1/40/50/60-second guard cases,
   per-response words/WER, and tails before spending a confirmation budget.
6. **Confirm only finalists.** Use stage C: the same candidate pair on declared
   important durations, initially **5/10/20/30 seconds, ten visits, ten repeats**
   (about ten minutes per pair, estimated). Repeat confirmation separately;
   budget for that additional run. Expand to all normal durations when the claim
   or observed tradeoff requires it (about 27–28 minutes per pair, estimated).
   Apply the decision gates in the hill-climbing reference: initially ≥5%
   repeatable improvement, no accuracy regression, and no material tail slowdown.
   The runner's 5% WER ceiling alone is not a no-regression test. Remeasure the
   baseline alongside the candidate at every stage; never compare unpaired days.
7. **Challenge.** Follow journey 4 on the same candidates. Known 200/300-second
   failures are real accuracy failures even when the run completes. Preserve
   them, compare actual outputs, and report whether they improved or worsened;
   do not relax WER, regenerate speech, or drop failed rows to obtain a win.
8. **Connect to the product.** Follow journey 5 to inspect real workload timing.
   Dispatch is not visible insertion. Before claiming the product SLO or shipping
   a winner, obtain authorized installed-path evidence for the exact artifact.
9. **Stop and hand off.** Finish with keep/reject/no measurable difference/
   inconclusive, fixture-level results, gaps, artifact paths, and next experiment.
   Stop at the budget or when repeated results remain within noise. Promotion or
   installation is a separate authorized action, not part of this skill.

## Iteration cost and setup reuse

These budgets exclude candidate builds, downloads, optional reference checks,
initial A/A validation, and separately repeated confirmation. They describe the
current baseline on the M5 Max, not a deadline for other models/hardware. The
normal suite was measured at about 2 min 20 sec per configuration; the earlier
5/20-second A/A pair took about 47 seconds. Broader paired/confirmation budgets
are estimates, not newly measured full runs. Record actual elapsed time and
request counts for every experiment; correctness failures never count as wins.

Generate/independently check speech only for a new or suspect corpus. Reuse the
frozen clips and validated apparatus across a batch; do not rerun TTS, independent
ASR, or A/A before every unchanged candidate. Still run both baseline and candidate
in each stage, audit each result/cleanup, and serialize GPU work. The CLI defaults
remain the broader five-visit/three-repeat normal suite, so pass stage A's focused
flags explicitly. See journey 3 for the exact commands and escalation rules.

## Harness defaults and current limits

- Corpus: `benchmarks/speech/`, twelve different passages at 1, 5, 10, 20, 30, 40,
  50, 60, 90, 100, 200, and 300 seconds. Normal suite is 1–60 seconds; stress is
  90–300 seconds. One passage per duration is not broad accuracy coverage.
- Baseline profile: `baseline:8:512:1:0` (inference threads, decoder batch,
  encoder batch, startup warmup). These are explicit experiment settings, not a
  claim that every installed app currently uses them.
- The pinned corpus's independent Whisper check is 0–3.03% WER per clip. It is
  automated agreement with synthesis input, not human transcript certification.
- The recorded native runtime/model passes through 100 seconds but fails the
  200/300-second clips, reproduced with a fresh server. Check the recorded hashes
  in `benchmarks/speech/nemo-check.json`; do not generalize to every later build.
- File-based 200/300-second inference does not prove live capture at those lengths;
  the documented live recording cap is currently 120 seconds. Inspect current
  configuration/code before making a live-path claim.

## Required handoff

Report the actual runtime/model/encoder and corpus identities; changed variable;
selected durations; visit/sample counts; per-duration latency and WER; errors and
accuracy regressions; cold/first/warm distinctions; matched-load limitations;
cleanup observations; and whether any installed/end-to-end journey ran. Link raw
results and the experiment decision. Do not turn HTTP p95 into visible-insertion
p95 or call a completed-but-inaccurate run a success.

See [skill verification coverage](references/verification.md) for the journeys
actually exercised while creating this skill and the remaining gaps.
