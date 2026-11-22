---
name: read-nemo-telemetry
description: Read Nemo's local dictation telemetry in daemon.log to find failures, slow steps and regressions, compare builds, and decide what to improve. Use for "what does the log say", dictation success rates, failure reasons, tracing one dictation, checking whether an installed change helped, or adding a new event or field.
---

# Read Nemo telemetry

Nemo records what each dictation did in a local log. This skill reads that log,
turns it into evidence, and uses the evidence to choose and check improvements.
Run commands from the repository root.

## Where the data comes from

1. Code calls `telemetry::emit`, `emit_result`, or the `*_dictation` functions in
   [`src/telemetry.rs`](../../../src/telemetry.rs). That module is the only writer.
2. Each call prints one JSON object on stderr.
3. The daemon redirects stderr to `~/Library/Application Support/Nemo/daemon.log`
   (mode 0600, append-only; `NEMO_DATA_DIR` moves it). Plain status lines share the
   file. `nemo transcribe` prints the same events to the terminal instead.

The engine's own output goes to `inference.log` beside it. That file is truncated
on every model load; `inference-failed.log` keeps the copy from an engine that
was replaced by the included one.

[Event reference](references/events.md) lists every event and field. The data
is local and never leaves the Mac. It has no audio and no transcript text, but
`error` can name a microphone or a path, so do not paste raw lines into a pull
request, issue, or any outside service.

## Whose instance this is

The installed app is the user's and is usually in use. Reading the log changes
nothing and needs no permission. Do not quit, restart, or reconfigure the app,
and do not record, play audio, or paste to produce events. To produce events
without the user's app, use the isolated CLI journey in the
[journey index](references/journeys.md).

## Check the data is usable

```sh
"$HOME/Applications/Nemo.app/Contents/MacOS/nemo" identity
uv run --no-project scripts/telemetry-report.py --by build
```

The first 12 characters of `identity`'s `sha256` are the `build` on every event
that app writes. Usable means the report has a group for that build with
`dictations` above zero. `unstamped_events_skipped` counts lines written before
events had timestamps; `scripts/latency-report.py` still reads those.

## Read

| Question | Command |
| --- | --- |
| How are dictations going overall? | `uv run --no-project scripts/telemetry-report.py` |
| Did a new build change anything? | `... --by build` |
| When did something start? | `... --by day`, then narrow with `--since 2026-11-22T01:24 --until ...` (UTC) |
| What happened in one dictation? | `... --dictation last`, `--dictation slowest`, or `--dictation <id>` |
| I want to analyze it myself | `... --rows` prints one JSON row per dictation |

Keep proof with `--output artifacts/telemetry/NAME.json`. `artifacts/` is ignored
by git, and the script refuses to overwrite a file.

Read a summary in this order:

1. `outcomes` and `delivered_rate`: how many dictations reached the user.
2. `failures` and `failed_steps`: the reasons, most common first.
3. `delivered_timings`: where a delivered dictation spent its time.
4. `cold_dictations` and `model_loads`: how often someone waited for a load.

## Turn a reading into an improvement

1. Save a baseline report for the running build before changing anything.
2. Name one finding and the number that should move, such as "`failed at
   microphone` falls" or "`stop_to_dispatch_ms` p50 falls".
3. Choose the route:
   - A failure reason: trace one case with `--dictation <id>`, then reproduce
     and fix it as a bug.
   - Inference time (`request_ms`, `load_ms`): use
     [optimize-nemo-inference](../optimize-nemo-inference/SKILL.md), which
     measures on frozen audio. Field data only confirms the result afterward.
   - A step the log cannot explain: add the missing event or field first.
4. Make the change and run `cargo xtask check`.
5. Installing replaces the app the user is dictating with, so get a go-ahead,
   then run `cargo xtask install`. It refuses while a dictation is in progress;
   retry once the app is idle.
6. Let ordinary dictation accumulate, then compare `--by build`. Report the
   sample size with the result.

## Limits to state with any result

- `stop_to_dispatch_ms` ends when the paste keystroke is sent, not when text
  appears in the other app.
- Timings cover delivered dictations only.
- A distribution with `p95_sample_warning: true` has fewer than 100 samples; its
  p95 and p99 are close to the maximum.
- Field dictations differ in length and content between builds. A difference in
  `request_ms` across builds is a lead, not a measurement; confirm it on frozen
  audio.
- `unfinished` dictations have no closing event: one is in progress, or the
  daemon stopped mid-dictation.
- `version` is the Cargo version and is the same for every development build.
  Compare by `build`.

## Add an event or a field

1. Emit through `src/telemetry.rs`. Use `emit_result` for a step that can fail so
   it gets `success` and `error`.
2. Never include audio, transcript text, clipboard contents or the target app.
   Counts, durations, sizes and enumerated labels are fine.
3. Keep existing field names: `scripts/verify-installed.py`,
   `scripts/benchmark-app.py` and `scripts/latency-report.py` read them.
4. If a dictation can end in a new place, call `finish_dictation` there, and
   `advance_dictation` when it enters a new phase.
5. Add the field to [events.md](references/events.md) and
   `docs/delivery-latency.md`, read it in `scripts/telemetry-report.py`, and cover
   both with a test.
6. Check a real line: run the isolated CLI journey, or install and read
   `--dictation last`.
