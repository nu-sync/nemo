# Evidence, readiness, side effects, and cleanup

## What counts as a real run

The benchmark owns one native `nemo-speech serve` child at a time. It chooses a
loopback port and random API key, polls `GET /health` until HTTP 200, then sends
actual WAV multipart requests to `/v1/audio/transcriptions`. Startup has a
120-second readiness deadline; requests have a 300-second timeout. A healthy
server alone is insufficient: require actual fixture words and full recorded
first/warm requests. Warm excludes the first request, not arbitrary slow samples.

The runner's `results.json` contains:

- `state`, corpus manifest identity/selection, runtime/model/encoder identities,
  candidate options, script/scorer identities, environment, rounds/repeats/seed.
- `visits`: candidate, round, native PID, redacted command, log path,
  load-to-health milliseconds, and final status.
- `records`: fixture, candidate, round/repeat, first/warm stage, request time,
  actual text, word edits/WER, and status. `accuracy_failed` is not success.
- `summary`: per-candidate/per-fixture timing percentiles and error counts.
- `comparisons`: warm pairs and median paired percentage change per fixture.

The key is redacted in saved commands. Keep report/logs in ignored `artifacts/`;
records contain synthetic words, while generic logs and build notes can expose
local paths. Do not copy private daemon-log content into tracked skill references.
A fresh clone may not have ignored historical evidence; rerun, do not manufacture
missing files or substitute stored hypotheses for new inference.

## Audit every new runtime report

```sh
uv run .agents/skills/benchmark-nemo/scripts/audit-run.py \
  "$RUN/aa/results.json" --check-cleanup --output "$RUN/aa-audit.json"
```

Use the actual child output (`aa`, `screen`, `confirm`, or `stress`). The helper
requires complete alternating visits, successful recorded health/readiness,
response text and consistent WER status, all warm sample keys, identical paired
schedules, and no failed declared HTTP budget. Its output is bound to the report
SHA-256. It does not judge speed gains, independently listen/transcribe audio,
verify native-library build provenance, or establish an end-to-end SLO.

For a known quality failure, `--expect-accuracy-failures` requires at least one
actual failed WER check and labels the audit `complete_accuracy_failure_reproduced`.
Its exit 0 is reproduction success, not model acceptance. Never use this switch
to promote inaccurate output or mask a transport/readiness failure.

`--check-cleanup` probes only recorded positive PIDs with signal zero: it never
terminates a process. Run immediately after the benchmark. A PID that exists or
cannot be inspected makes cleanup inconclusive; it may have been reused. Omit
this flag for offline historical audits, and label their missing live-cleanup
coverage. A structural audit without the flag is not cleanup proof.

## Check side effects after success or failure

```sh
"$DAEMON" status > "$RUN/daemon-after.txt"
ps -A -o pid=,ppid=,comm= > "$RUN/processes-after.txt"
```

Compare the owned visit PIDs to those snapshots, and verify that the original
installed daemon remains available. The live app may naturally unload its own
idle model or receive outside activity; do not require its entire process list
to stay identical or stop it to manufacture that assertion.

The benchmark should create only its new report/log directory and temporary WAV
files, read existing corpus/runtime/models/configuration, use GPU/memory, and
launch/reap its own servers. Decoded WAVs live in a temporary directory that is
removed on normal completion, handled errors, or Ctrl-C. It makes no microphone,
speaker, clipboard, permission, installed-bundle, configuration, or power-setting
changes. Optional TTS/reference checks have separate documented cache downloads
and writes; they are not dry-runs and stay outside timing windows.

If installed/configuration immutability is a criterion, capture SHA-256 of the
actual installed executable and existing config **before and after**, retaining
only hashes/existence in the run evidence, not private config contents. Declare
outside user changes separately. Do not infer immutability from HTTP success.

## Interrupt and recover safely

Prefer foreground runs and Ctrl-C. For a background run, retain the runner's own
`$!` PID and send TERM only to that process. The runner handles Ctrl-C/TERM,
preserves partial results, and terminates/reaps its owned server. Preserve the
nonzero exit and partial report; do not resume by appending to that directory.
The helper rejects incomplete reports as intended.

After an unhandled crash or kill -9, inspect `results.json`, its visit PID,
redacted command/runtime path, start time, and OS process/parent identity before
any cleanup. Confirm it is still the test-owned process, then terminate only
that process if authorized. A stale PID can identify another application.
Never `pkill nemo-speech`, stop the installed daemon, or delete all temporary
files to clean a benchmark. If ownership cannot be established, report cleanup
inconclusive rather than guessing.

Retain `results.json`, console output/exit code, native logs, corpus check,
process snapshots, audit JSON, build identity notes, and the decision record.
Verify those files survive cleanup. End with the installed path unchanged unless
an explicitly authorized later step intentionally promoted the candidate.
