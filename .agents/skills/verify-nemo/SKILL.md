---
name: verify-nemo
description: Verify Nemo dictation on a MacBook using real speaker playback and microphone capture, installed-app transcription and clipboard delivery, optional paste checks, model preparation during recording, and idle unloading. Use for Nemo runtime regressions or post-install checks.
---
# Verify Nemo on a MacBook

Exercise the physical audio path: play known speech through the laptop speakers,
let the installed Nemo app record it through the laptop microphone, stop recording,
and compare the clipboard with the expected words. This is the default end-to-end
acceptance criterion; no editor or foreground UI check is required. A
file-transcription command does not exercise this physical path.

Work from the repository root. Choose the requested journeys from the
[feature and journey index](references/journeys.md); run only the needed checks.
The executable helpers live in the project's `scripts/` directory rather than
being copied into this skill.

## Prepare the actual app

- Identify the target: normally `~/Applications/Nemo.app`. Use the installed executable
  with its existing macOS grants. Preserve its installed binary, signature, and configuration during verification.
- Check the running daemon's `permissions` control command. Both Accessibility and
  Microphone must be authorized for the full dictation/paste journey. The CLI's
  `permissions` command describes the CLI process and can give a different answer.
- Inspect `system_profiler SPAudioDataType`: the chosen speaker output must actually
  reach the chosen microphone. Built-in MacBook speakers and microphone worked.
  Headphones or a virtual input are different boundaries; do not claim the same
  hardware coverage or silently change the user's audio routing/volume.
- Use clipboard completion by default. Do not open TextEdit, require foreground
  focus, or ask the user to click an editor for routine verification. The default
  helper uses an isolated configuration with `paste=false`, leaving the user's
  preferences unchanged and preventing insertion into unrelated applications.
- Test visible paste only when explicitly requested. For that separate journey,
  use a scratch document, verify foreground focus, and inspect actual insertion.
  A clipboard match is sufficient for the default journey, not proof of paste.
- Announce the audible fixture and microphone test. Honor existing authorization;
  avoid asking again for an already requested round trip. Permission changes and
  OS authentication remain separate actions under the active tool policy.

## Run and retain proof

Requires Python 3.11+, the installed model/runtime, macOS `say` with Samantha, and
an unused output directory. Read the relevant journey before running:

```sh
uv run --no-project --python 3.13 python scripts/verify-clipboard.py \
  --output artifacts/performance/installed-clipboard-NEW
```

This safely quits the existing app only if idle, launches the same installed
executable under external-network denial with an isolated `paste=false` copy of
its configuration, and drives real microphone capture through the existing
round-trip verifier. It checks model preparation while Recording, expected
clipboard words, and the prepared inference PID. It then stops its test daemon
and restores the prior normal app session. Original configuration is never edited.
It saves neither microphone audio nor the prior clipboard contents to evidence;
the round-trip verifier restores prior plain text if its fixture is still present.
If user activity takes over recording, cleanup is deferred rather than killing it.
Inspect both `summary.json` and `roundtrip/results.json` for a pass.

The default command checks the acoustic round trip only. Full idle expiry, warm
reuse, physical shortcut behavior, and visible paste are separate requested
journeys in the index; do not turn them into default prerequisites.

If the round trip already passed and further dictation reset the timer, observe
without playing audio or issuing a worker job:

```sh
uv run --no-project --python 3.13 python scripts/verify-installed.py --observe-idle \
  --output artifacts/performance/installed-idle-NEW
```

New dictation restarts the idle interval. `status` and `permissions` polling do not.
The full script treats outside activity as inconclusive; the observer waits for a
quiet interval, up to 15 minutes. If the user needs to keep dictating, use the
isolated full-expiry alternative in the journey index with the same installed
executable. Label that controlled timer result separately from the live app’s
permissions and acoustic-path proof. Report the measured unload interval and timing
basis from JSON, including its one-second polling tolerance. A 65-second check
or a shortened configured timeout does not establish full five-minute expiry.

## Judge the evidence

Require the actual installed executable identity, its daemon permissions, a new
owned runtime PID during recording, successful `model_load` before recording stops,
a successful transcription with `reused_model = true`, and
`clipboard_matches_reference=true`. Require visible insertion only for an explicit
paste check, and disappearance near the idle deadline only for an idle-expiry
check. The clipboard helper normally terminates its owned test model at teardown;
that cleanup is not idle-expiry evidence. Filter children by executable path and
parent PID: Nemo also launches sound helpers such as `afplay`.

Keep raw JSON and relevant telemetry in ignored `artifacts/`. Preserve failures and
inconclusive attempts. Do not store private clipboard text or user recordings.
The full helper restores prior plain-text clipboard content only if the clipboard
still contains its fixture; it does not preserve every rich pasteboard format.
Clean only the test's recording/process/document; leave user activity alone.

For a stale post-update grant, follow the targeted recovery in the journey index.
Do not rebuild, re-sign, reinstall, broadly reset privacy permissions, or edit the
TCC database merely to make a verification pass.
