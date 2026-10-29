# Nemo feature and journey index

Run commands from the project root. The default app and data paths belong to the
current macOS user, not a hard-coded account. `scripts/verify-installed.py --help`
lists target overrides. Run one acoustic test at a time.

| Journey | Surface and helper | Required proof |
| --- | --- | --- |
| Cold acoustic dictation | Installed app, real speakers/microphone, `verify-installed.py --roundtrip-only` | Model starts and becomes ready while Recording; stop produces expected words and visible paste using the prepared PID. |
| Warm follow-up dictation | Same installed daemon, `verify-installed.py --roundtrip-only --warm` | Another recording reuses the same runtime; no load event occurs during recording, and transcript telemetry reports zero load time and `reused_model=true`. This resets the idle window. |
| Full five-minute expiry | Live app with `verify-installed.py --observe-idle`, or the same installed executable in an isolated full-expiry check | Same runtime survives the idle interval, then exits near 300 seconds; daemon stays running and reports `Ready · model unloaded`. Label which instance was observed. |
| Preparation cancellation | Isolated supplemental `verify-performance.py --recording-only` | Cancel during load reaps it; cancel after readiness retains it; clipboard unchanged. This does not establish the Finder-launched app's permissions. |

## Installed-app readiness

Read configuration without changing it:

```sh
"$HOME/Applications/Nemo.app/Contents/MacOS/nemo" status
"$HOME/Applications/Nemo.app/Contents/MacOS/nemo" config
system_profiler SPAudioDataType
python3 --version
```

Query the daemon's permission snapshot through its existing control socket:

```sh
python3 - <<'PY'
from pathlib import Path
import socket
path = Path.home() / 'Library/Application Support/Nemo/run/control.sock'
with socket.socket(socket.AF_UNIX) as stream:
    stream.settimeout(3)
    stream.connect(str(path))
    stream.sendall(b'permissions\n')
    print(stream.makefile().readline().strip())
PY
```

The installed verifier requires `idle_unload_seconds=300`, at least a 90-second
recording limit, and daemon permissions `accessibility=true`,
`microphone="authorized"`. Preserve a different user preference and report that
this specific helper's prerequisites are missing instead of changing it silently.
A busy app is not available for a controlled round trip.
Nemo is a menu-bar app: a native UI tool can time out looking for a window even
when launch succeeded. Check the actual daemon status/socket before retrying a
launch; a missing standard window is not an instance-health failure.

For a cold start, wait for normal idle unloading or quit with `quit --if-idle` and
reopen the **same installed app** through the native app tool. Confirm no owned
runtime remains. Do not quit an active recording or bypass a refused idle quit.
Rebuilding/reinstalling can change the ad hoc identity and invalidate grants.

## Acoustic round trip and warm reuse

Create an empty scratch document through the native UI tool. Keep it focused while
the fixture is spoken and until paste completes. Coordinate this brief focus
requirement with the user; changing foreground apps changes Nemo's paste target.
Background AX clicks or window Raise actions did not reliably activate TextEdit
on this Mac. Confirm the installed app's `doctor` output says `Foreground app:
TextEdit`; if it does not, have the user click inside the scratch document. The
target guard below checks that diagnostic before recording and again before stop,
so a background editor cannot silently pass as the intended paste destination.

The helper's spoken reference is:

> The quick brown fox jumps over the lazy dog. This is a local speech recognition test on my laptop.

```sh
python3 scripts/verify-installed.py \
  --roundtrip-only --paste-target TextEdit \
  --output artifacts/performance/installed-cold-roundtrip-NEW

python3 scripts/verify-installed.py --roundtrip-only --warm --paste-target TextEdit \
  --output artifacts/performance/installed-warm-roundtrip-NEW
```

It starts through the installed CLI's `toggle`, which drives the actual daemon's
normal recording state machine. It uses `say -v Samantha`, waits for speech and
model preparation, then toggles stop. It verifies normalized words, not a broad
accuracy score. The installed app's real paste preference is used.
`--roundtrip-only` finishes after those acoustic/transcription checks without an
idle wait. `--warm` requires it and one already-ready runtime; the same PID must
survive recording and transcription without another model-load event. The default
command without either option still requires a cold model and includes the full
idle observation.
`--paste-target` requires the existing `paste=true` preference, verifies actual
foreground app checkpoints, and checks the final `Pasting into ...` log. A
different target is inconclusive, even if recognition and clipboard checks pass.
Without this option, the helper's default result remains an acoustic/transcription
and clipboard result without a paste-target assertion.

Read the scratch document with the native UI tool after the helper reaches
transcription completion. Match its words against the reference and retain this
observation alongside `results.json`. Check `paste_destination_log`; a successful
clipboard assertion or a `Pasting into ...` log does not replace a visible text
check. If focus changed, report the actual destination and the coverage gap; never
claim TextEdit paste when the text went elsewhere. The helper does not press Send.
Even `paste_target_log_matches=true` does not establish visible insertion: Nemo
logs the destination about 100 ms before emitting the paste keys. Keep TextEdit
foreground through completion and retain the actual native UI text observation.

For a warm follow-up, run the second command in the existing session before its
timeout. Do not run a file-transcription command: that starts a
separate model process. Use installed-daemon telemetry and its exact child PID to
verify reuse. Do warm follow-ups before the final idle observation, since each job
restarts the clock. Physical shortcut behavior requires a separate shortcut-driven
run when specifically requested; CLI toggle is not proof of the physical key.

## Full idle expiry

After the cold and warm round trips, observe expiry independently with the command
below. This avoids making successful round-trip checks inconclusive merely because
a follow-up recording resets the timer. The default full round-trip helper still
observes the idle window automatically; if user activity interrupts it, retain that
inconclusive report and use the same read-only observer:

```sh
python3 scripts/verify-installed.py --observe-idle \
  --output artifacts/performance/installed-idle-NEW
```

This observer issues no Warm, Prepare, Toggle, recording, speech, or clipboard
operations. It polls the daemon, its telemetry log, and the actual model PID.
New dictation invalidates the old interval and starts a new interval after the
fresh transcription. A genuine quiet interval is necessary; status polling is safe.

The worker's timeout starts at backend job completion, slightly before UI Ready
and paste finish. Fresh telemetry observations use a monotonic clock. When joining
an existing session, the helper can use the last Ready log modification time as a
proxy, about 100 ms after completion. JSON labels this basis, resets, PID epochs,
last-loaded/first-absent observations, and actual observation duration. Report the
299–305-second tolerance and any initial unobserved span rather than claiming
millisecond accuracy or uninterrupted sampling you did not perform.

### While the user keeps dictating

A continuously used live app cannot complete an idle window. Coordinate other
active chats before changing shared sources or the installed app. If they use
isolated test bundles, their tests can continue; live dictation still resets the
normal app’s timer. Avoid repeatedly blocking the user for a quiet interval.

Observe the full timeout in an isolated data directory using the **same installed
executable**, without capturing microphone audio, playing speech, or touching the
clipboard:

```sh
python3 scripts/verify-performance.py --full-idle-expiry-only \
  --binary "$HOME/Applications/Nemo.app/Contents/MacOS/nemo" \
  --runtime "$HOME/Applications/Nemo.app/Contents/Resources/runtime/nemo-speech" \
  --model "$("$HOME/Applications/Nemo.app/Contents/MacOS/nemo" model path)" \
  --output artifacts/performance/isolated-full-expiry-NEW
```

The helper preloads a model in its owned daemon with a real 300-second timeout,
records the same PID through 299 seconds, observes its exit near 300 seconds,
checks that its daemon survives, and cleans up only that isolated daemon. It
records the installed executable hash before and after. Configuration and model
paths are private to the test; the production app remains usable. This costs an
additional loaded model’s memory during the observation; ensure the Mac has room.

This is full-duration timer evidence after a preload job. It is not an observation
of the normal Finder-launched session sitting idle after dictation. Keep the real
installed-app acoustic/transcription proof and the controlled timer proof distinct.
Do not run the single-instance installed round-trip helper concurrently with this
second instance of the same executable; its process-identity precondition will
find both instances. Status queries to the normal app remain safe.

## Supplemental cancellation

```sh
python3 scripts/verify-performance.py --recording-only \
  --binary "$HOME/Applications/Nemo.app/Contents/MacOS/nemo" \
  --runtime "$HOME/Applications/Nemo.app/Contents/Resources/runtime/nemo-speech" \
  --model "$("$HOME/Applications/Nemo.app/Contents/MacOS/nemo" model path)" \
  --output artifacts/performance/cancellation-NEW
```

This helper uses isolated data/config directories. It checks early cancellation,
then preparation while recording and cancellation after readiness. Microphone
capture is discarded without requesting transcription or paste. Missing permission
is an explicit skipped/inconclusive result. Filter model children by runtime path;
overall cleanup can also include the Tink/Pop sound helpers and is not pure model
cancellation latency.

## Recover a stale permission grant only when needed

A verified failure was Settings showing Nemo enabled while the actual installed
daemon reported Accessibility false. Unified TCC logs showed the old ad hoc code
requirement no longer matching the updated binary. An ordinary toggle and app
restart did not repair that grant.

With authorization to refresh **Nemo's existing access**, and after any required
user Touch ID approval, the observed recovery was: quit Nemo only if idle; reset
only `Accessibility` for bundle ID `computer.nemo.dictate`; re-add the exact current
installed app in System Settings; reopen it; query the daemon again. Do not reset
other services or apps. Do not treat this reference as permission to change grants.

```sh
python3 - <<'PYRESET'
from pathlib import Path
import subprocess, time
binary = Path.home() / 'Applications/Nemo.app/Contents/MacOS/nemo'
rows = subprocess.check_output(['/bin/ps', '-A', '-o', 'pid=,comm='], text=True)
pids = [int(parts[0]) for line in rows.splitlines()
        if len(parts := line.strip().split(None, 1)) == 2 and parts[1] == str(binary)]
if len(pids) > 1:
    raise SystemExit('Multiple installed daemons; permission reset was not attempted')
if pids:
    quit_result = subprocess.run([str(binary), 'quit', '--if-idle'],
                                 capture_output=True, text=True, check=True)
    if quit_result.stdout.strip() != 'accepted':
        raise SystemExit('Idle quit refused; permission reset was not attempted')
for _ in range(100):
    alive = any(subprocess.run(['/bin/ps', '-p', str(pid), '-o', 'pid='],
                               capture_output=True).returncode == 0 for pid in pids)
    if not alive:
        subprocess.run(['/usr/bin/tccutil', 'reset', 'Accessibility',
                        'computer.nemo.dictate'], check=True)
        break
    time.sleep(0.1)
else:
    raise SystemExit('Nemo did not exit; permission reset was not attempted')
PYRESET
```

In the Add dialog, selecting a row via accessibility automation left Open disabled.
Go to Folder (`Command+Shift+G`), resolve the exact installed `Nemo.app` path, press
Return, then click the now-enabled Open button. Use fresh UI observations; never
reuse element indices or bypass a disabled control. The actual daemon finally
reported both permissions authorized. Keep authentication with the user.

## Settings coverage boundaries

The acoustic helper compares normalized words, so a pass does not test the
**Remove trailing period** preference. When verifying that setting, compare the
exact delivered string with the preference on and off, including its final
punctuation. Launch-on-startup registration requires its own macOS login-item
check; recording and an idle-expiry test do not establish it.

## Evidence and cleanup

Use new output directories. Captured microphone audio stays in Nemo's memory and
is not saved by these helpers. Keep fixture words, binary identity, timing events,
process observations, and verdicts; avoid persisting private clipboard contents.
The installed helper leaves the app running after unloading and restores the prior
plain-text clipboard only if it still contains the test fixture. It leaves detected
user recording/jobs alone. Close only your own scratch document after retaining
proof; preserve existing documents and the installed configuration.

The original installed acoustic run is retained locally under
`artifacts/performance/installed-full-verification/results.json`: its acoustic and
transcription criteria passed, but later user dictation made its idle verdict
inconclusive. The read-only live observer was stopped to preserve that activity.

The complete isolated expiry journey passed on the same installed executable;
`artifacts/performance/installed-isolated-full-expiry-retry/results.json` records
300 process samples, last loaded at 299.063 seconds and first absent at 300.049
seconds. Its daemon survived expiry and all owned test processes were reaped.
This was exercised on the Apple M5 Max MacBook Pro with 128 GB RAM. Keep this
controlled timer proof separate from the live acoustic proof and do not claim a
quiet desktop session or a TextEdit paste that was not observed.
