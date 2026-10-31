# Skill verification coverage

## Exercised environment

Observed 2026-10-31 UTC (2026-10-30 local) on an Apple M5 Max MacBook,
macOS 26.5.2, 128 GiB memory, AC power, existing power mode 2. Settings were
observed, not changed. Commands used uv 0.13.4 and its existing managed Python
3.12.12. No root Python project, global package installation, or new model
artifact was needed for these journeys.

This is a local execution check of the skill instructions, not an independent
agent-trigger evaluation or evidence that an optimization was found.

## End-to-end apparatus journey

Executed the shared preparation, journey 1 checks, and journey 2 A/A commands
from `journeys.md`, followed by the live cleanup audit and side-effect checks.
The installed `nemo-speech` runtime and decoder/encoder were the real artifacts;
no HTTP stub substituted for native inference.

- Selected actual 5-second lunch and 20-second weekend FLAC fixtures.
- Same runtime/model/encoder hashes and inference options under `baseline` and
  `control`, with the explicit candidate-runtime override pointing to the same
  native executable.
- Five alternating rounds: ten health-ready fresh-server visits, ten first
  requests, twenty warm requests, zero WER on every response.
- Five warm samples per fixture per candidate: screening only, not p95 attainment.
- The immediate audit returned `complete_accurate_run`, with cleanup checked.
  All ten owned runtime PIDs were absent; native logs and results remained.
- The installed daemon stayed available. Its executable and existing config
  hashes matched before and after; no install, microphone, playback, clipboard,
  paste, permission, audio-route, or power-setting change was performed.
- A/A median paired differences were about -0.20% (5 seconds) and -0.13%
  (20 seconds), not a useful improvement and not an optimization claim.

Evidence is retained locally in ignored:

```text
artifacts/performance/skill-benchmark-20261030-194502-44070/
  decision.md
  interpreter.txt
  corpus-check.txt
  python-tests.txt
  python-tests-final.txt
  rust-check.txt
  aa/results.json
  aa/*.log
  aa-console.txt
  aa-exit.txt
  aa-audit.json
  processes-before.txt
  processes-after.txt
  daemon-before.txt
  daemon-after.txt
  side-effects-before.json
  side-effects-after.json
  live-latency.json
  historical-stress-audit.json
```

Those ignored files are not guaranteed to exist after a fresh clone. Follow the
journey to create new evidence; never treat this coverage note as a fresh pass.

## Coverage and gaps

| Journey/check | Evidence during skill creation | Limit |
| --- | --- | --- |
| 1. Corpus integrity/harness tests | All twelve decoded fixture checks and 25 Python tests passed using uv. | Stub-server/audit-unit tests are simulated boundaries, not native quality evidence. TTS and independent Whisper were not rerun here; their frozen existing corpus reports were inspected. |
| 2. A/A apparatus | Full native lifecycle above, real health/transcription, immediate PID cleanup, unchanged app/config hashes. | Only 5/20-second fixtures and identical candidates. No optimization or production SLO established. |
| 3. Candidate hill-climbing | Instructions and supported CLI options checked against current harness. | No distinct runtime/model candidate, 100-sample confirmation, build, or promotion executed in this task. |
| 4. Stress | Audit helper accepted the existing full-suite report only with explicit expected accuracy failures; two historical failures retained. | Offline historical audit, no fresh stress inference or current PID-cleanup claim. |
| 5. Live telemetry | Current log report produced through uv and retained. | Historical unkeyed event populations, no visible insertion observation or version-segmented A/B result. |

The audit helper's five added unit tests cover complete reports, missing/duplicate
samples, explicit quality-failure reproduction, readiness/transport/budget
failures, and read-only cleanup probes. The existing harness tests also cover
real owned stub-server HTTP and interruption cleanup. No independent agent
with/without-skill comparison or trigger optimization was run.

The skill frontmatter and local Markdown links were validated after authoring.
`cargo xtask check` also passed, with its log retained alongside the Python checks.
Hosted CI remains opt-in; its uv setup/check changes were inspected but no hosted
workflow was run as part of this task.
