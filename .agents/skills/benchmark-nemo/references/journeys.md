# Benchmark feature and journey index

Commands run from the repository root. No journey here opens a microphone, plays
speech, touches the clipboard, or pastes. Read the linked detail before execution.

| Journey | Prerequisites and real entry point | Observable success |
| --- | --- | --- |
| 1. Validate frozen speech | uv, Python 3.11+, macOS `afconvert`; `scripts/speech_corpus.py` | All twelve text/container/decoded-PCM hashes and exact frame counts pass; references and their independent-check provenance remain unchanged. |
| 2. Verify benchmark apparatus | Installed runtime/model, quiet GPU, idle daemon; real A/A run of `scripts/benchmark-corpus.py` | Loopback health readiness, real expected transcripts, alternating paired visits, persisted results/logs, and owned server PIDs absent after completion. No optimization claimed. |
| 3. Hill-climb one variable | Authorized candidate artifact or supported option, frozen baseline; same runner with two candidates | Complete paired experiment, per-fixture latency/accuracy review, bounded keep/reject decision, separate confirmation before promotion. |
| 4. Challenge long-form accuracy | Same runtime/model pair; `--suite stress` | Accurate outputs, or preserved and explicitly classified accuracy failures; independent ASR separates a bad-fixture hypothesis from native recognition failure. |
| 5. Check production relevance | Existing daemon telemetry; `scripts/latency-report.py` | Real audio-duration/request distributions and measured dispatch timings are reported separately, without guessed insertion latency or unkeyed event joins. |

## Shared preparation

```sh
git status --short
uv --version
uv run --no-project python --version
APP="$HOME/Applications/Nemo.app"
DAEMON="$APP/Contents/MacOS/nemo"
RUNTIME="$APP/Contents/Resources/runtime/nemo-speech"
MODEL="$("$DAEMON" model path)"
test -x "$DAEMON" && test -x "$RUNTIME" && test -f "$MODEL"
"$DAEMON" status
```

Check actual paths instead of assuming the installation exists. The installed
CLI's `model path` identifies the decoder; its matching encoder must be present.
If using another installation, assign `APP` explicitly and record why. Do not
launch the menu app just to obtain an idle guard. If no daemon is running, report
that gap; an explicitly documented standalone benchmark without `--daemon` is a
different environment, not permission to hide a busy live daemon.

For the documented live-daemon path, require idle status. The runner also checks
at visit/request boundaries, but that cannot detect dictation starting in the
middle of a request. Missing model/runtime, busy daemon, insufficient memory,
unsupported platform, or unavailable GPU isolation is blocked/inconclusive
coverage. Do not replace a real-model journey with the stub-server unit tests.

Create a unique parent directory for notes and process snapshots. The runner
itself requires a **new** child output directory:

```sh
RUN="artifacts/performance/skill-benchmark-$(date +%Y%m%d-%H%M%S)-$$"
mkdir -p "$RUN"
git rev-parse HEAD > "$RUN/head.txt"
git status --short > "$RUN/worktree.txt"
"$DAEMON" status > "$RUN/daemon-before.txt"
ps -A -o pid=,ppid=,comm= > "$RUN/processes-before.txt"
```

Record the hypothesis, artifact build revisions/flags, selected fixtures, gates,
and budget in `$RUN/decision.md` **before timing**. Run one experiment at a time.
Use the [evidence procedure](evidence.md) for post-run auditing and cleanup.

## 1. Validate and independently check references

```sh
uv run --no-project scripts/speech_corpus.py > "$RUN/corpus-check.txt" 2>&1
uv run --no-project python -m unittest discover -s scripts/tests -v > "$RUN/python-tests.txt" 2>&1
```

Check both exit codes; do not continue after a failed corpus check. The Python
suite includes a **simulated runtime boundary** (an owned stub HTTP server);
that is harness regression coverage, not native model inference evidence.

For a new or suspect corpus, read the generation/reference sections of
[the guide](../../../../docs/benchmarking.md), inspect the uv script locks and
cache/download prerequisites, then run the actual independent recognizer:

```sh
uv run --locked scripts/verify-speech-references.py \
  --output "$RUN/independent"
```

This can download a pinned Whisper model/Python dependencies into ignored
caches. It uses CPU and no reference prompt. Its output is independent ASR
agreement, not human-listened certification. Retain its failures and inspect
hypotheses; do not rewrite text to match them. Run it outside timing windows.

`uv run --locked scripts/generate-speech-corpus.py` reuses an existing verified
manifest rather than regenerating it. A fresh generation does download TTS
assets, synthesize audio, and write files; it is not a dry-run. Use a new corpus
version/output as described in the guide and rebaseline both A/B sides.

## 2. Real A/A apparatus check

After shared preparation and corpus validation, use two names for the **same**
artifacts and settings. These commands were exercised during skill creation:

```sh
uv run --no-project scripts/benchmark-corpus.py \
  --runtime "$RUNTIME" --model "$MODEL" --daemon "$DAEMON" \
  --candidate baseline:8:512:1:0 --candidate control:8:512:1:0 \
  --candidate-runtime "control=$RUNTIME" \
  --durations 5 20 --rounds 5 --repeats 1 \
  --output "$RUN/aa" > "$RUN/aa-console.txt" 2>&1
BENCH_EXIT=$?
printf '%s\n' "$BENCH_EXIT" > "$RUN/aa-exit.txt"
uv run .agents/skills/benchmark-nemo/scripts/audit-run.py \
  "$RUN/aa/results.json" --check-cleanup --output "$RUN/aa-audit.json"
```

Require benchmark exit 0 and a successful audit. There are ten visits, ten first
requests, and twenty warm requests: five warm samples per fixture per candidate.
Require equal runtime/model/encoder hashes and inference options for the A/A
names. Inspect actual response text, health readiness times, and paired metrics.
A small A/A gap is noise evidence, not a candidate gain; a large persistent gap
calls for apparatus/load investigation. This run does not establish a p95 SLO.

## 3. Paired screening and confirmation

Read [hill-climbing decisions](hill-climbing.md) first. Use an authorized candidate
path; the placeholder below is not a command to download or build a new runtime:

```sh
CANDIDATE_RUNTIME="/absolute/path/to/authorized/candidate/nemo-speech"
test -x "$CANDIDATE_RUNTIME"
uv run --no-project scripts/benchmark-corpus.py \
  --runtime "$RUNTIME" --model "$MODEL" --daemon "$DAEMON" \
  --candidate baseline:8:512:1:0 --candidate candidate:8:512:1:0 \
  --candidate-runtime "candidate=$CANDIDATE_RUNTIME" \
  --suite typical --rounds 5 --repeats 3 \
  --output "$RUN/screen" > "$RUN/screen-console.txt" 2>&1
```

Capture the exit code as in journey 2, audit the report, and review each paired
transcript. To compare inference threads instead, omit the runtime override and
change only the second candidate's thread count. To compare models, use
`--candidate-model "candidate=/absolute/path/to/decoder.gguf"`, with its matching
encoder colocated. Do not change threads/model/runtime simultaneously.

For a promising candidate, repeat with the **same two artifacts**, harness,
corpus, seed, settings, and predeclared durations, using a new output directory
and `--rounds 10 --repeats 10`. That yields 100 warm samples per selected fixture
per candidate. Repeat the confirmation separately to challenge drift; 100
replays of one passage do not provide diverse accuracy coverage or a confidence
interval. Save both wins and losses. Cleanup and final installed verification
remain required before an authorized promotion.

## 4. Stress and failure diagnosis

Use the same candidate pair and overrides as journey 3, replacing its suite with
`--suite stress --rounds 1 --repeats 1` and output with `$RUN/stress`. This is an
accuracy/functional smoke, not a performance comparison. For the recorded old
runtime, a complete run is expected to fail the accuracy gate at 200/300 seconds.
Inspect the current response text and identities rather than assuming that
historical outcome still applies.

For a reproduced known accuracy failure, audit explicitly:

```sh
uv run .agents/skills/benchmark-nemo/scripts/audit-run.py \
  "$RUN/stress/results.json" --expect-accuracy-failures --check-cleanup \
  --output "$RUN/stress-audit.json"
```

The audit's successful exit then means **a complete accuracy failure was
reproduced**, not that the model passed. Transport errors, incomplete visits,
missing samples, and absent proof of cleanup do not become expected successes.
If the current model passes all cases, use the normal audit and report that
improvement rather than demanding a failure.

If references are suspect, use journey 1's independent recognizer outside the
benchmark window, optionally with `--durations 200 300`. Preserve both reports.
No stage here authorizes changing the live app's recording cap or decoder flags.

## 5. Production telemetry and installed-path handoff

```sh
uv run --no-project scripts/latency-report.py --output "$RUN/live-latency.json" \
  > "$RUN/live-latency-console.txt"
```

Inspect request timing by real audio-duration bucket, sample counts, model reuse,
and failures. `stop_to_dispatch_ms` measures dispatch, not visible insertion.
The delivery and transcription populations may differ; they lack correlation
IDs and are deliberately not joined. Missing events are missing coverage, not
zero latency. The log may span different app versions; it lacks per-event binary
identities, so do not use an unsegmented historical report as an A/B experiment.

File-based HTTP results alone cannot establish the one-second product p95. Read
[verify-nemo](../../verify-nemo/SKILL.md) for separately authorized installed
recording/transcription/paste checks. Its acoustic checks may play audible speech
and need permissions; do not invoke them as an unnoticed benchmark substep.
