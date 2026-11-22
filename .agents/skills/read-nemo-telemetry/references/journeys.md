# Telemetry journey index

Run from the repository root. Every journey except the last only reads the log.

| Journey | Entry point | Success looks like |
| --- | --- | --- |
| Health summary | `telemetry-report.py` | A group with `dictations` above zero, `outcomes` that sum to `dictations - unfinished`, and a saved report |
| Trace one dictation | `telemetry-report.py --dictation last` | Every line shares one `dictation`; the last is a `dictation` event unless one is in progress |
| Compare builds | `telemetry-report.py --by build` | One group per build; the running build's key matches `nemo identity` |
| Find when it started | `telemetry-report.py --by day`, then `--since`/`--until` | The window narrows to the first day or minute the outcome or timing changed |
| Isolated CLI events | Dev binary, temporary `NEMO_DATA_DIR` | Stamped `model_load` and `transcription` events from a build that is not installed |

## Health summary

```sh
"$HOME/Applications/Nemo.app/Contents/MacOS/nemo" identity
uv run --no-project scripts/telemetry-report.py --by build \
  --output artifacts/telemetry/health-NEW.json
```

Replace `NEW` with an unused name. Nothing to clean up; the report stays in
`artifacts/telemetry/`.

## Trace one dictation

```sh
uv run --no-project scripts/telemetry-report.py --dictation slowest
```

Subtract `ts` values to see gaps the timing fields do not cover. For the plain
status lines around it, search `daemon.log` for lines near that time; they have
no timestamp of their own, so use the neighboring events.

## Compare builds

```sh
uv run --no-project scripts/telemetry-report.py --by build
```

Compare groups only when each has enough dictations for the number in question.
A build that never ran as the daemon shows as `no build`.

## Isolated CLI events

This owns everything it starts: a temporary data directory and a short-lived
engine process that exits with the command. It does not touch the installed
app, the microphone, the speakers or the clipboard. It loads the model on the
GPU, so do not run it during a benchmark.

```sh
cargo build --locked
work="$(mktemp -d)"
cp "$HOME/Library/Application Support/Nemo/config.toml" "$work/"
ln -s "$HOME/Library/Application Support/Nemo/models" "$work/models"
say -v Samantha -o "$work/phrase.wav" --data-format=LEI16@16000 "The quick brown fox."
NEMO_DATA_DIR="$work" ./target/debug/nemo transcribe --json "$work/phrase.wav" 2> "$work/events.log"
uv run --no-project scripts/telemetry-report.py --log "$work/events.log"
rm -rf "$work"
```

Success: the transcript prints, and the report shows one successful
`model_loads` entry. CLI events have no `dictation` or `build`, so `dictations`
is zero; outcome events need the installed app. Copy `events.log` into
`artifacts/telemetry/` before removing the directory if the proof must outlive
the run. The model directory is a symlink, so removing `$work` removes only the
link.

## Not covered

- Producing `canceled` or `failed` outcomes on purpose. They need a real
  recording in the installed app; read them when they occur in ordinary use.
- Visible insertion in the target app. Telemetry stops at paste dispatch; use
  [verify-nemo](../../verify-nemo/SKILL.md) for that.
