---
name: verify-nemo
description: Verify Nemo dictation on a MacBook using real speaker playback and microphone capture, installed-app transcription and paste, model preparation during recording, and idle unloading. Use for Nemo runtime regressions or post-install checks.
---
# Verify Nemo on a MacBook

Exercise the physical audio path: play known speech through the laptop speakers,
let the installed Nemo app record it through the laptop microphone, stop recording,
and inspect the resulting text. A file-transcription command does not exercise this path.

Work from the repository root. Choose the requested journeys from the
[feature and journey index](references/journeys.md); run only the needed checks.
The executable helpers live in the project's `scripts/` directory rather than
being copied into this skill.

## Prepare the actual app

- Identify the target: normally `~/Applications/Nemo.app`, launched through macOS.
  Preserve its installed binary, signature, and configuration during verification.
- Check the running daemon's `permissions` control command. Both Accessibility and
  Microphone must be authorized for the full dictation/paste journey. The CLI's
  `permissions` command describes the CLI process and can give a different answer.
- Inspect `system_profiler SPAudioDataType`: the chosen speaker output must actually
  reach the chosen microphone. Built-in MacBook speakers and microphone worked.
  Headphones or a virtual input are different boundaries; do not claim the same
  hardware coverage or silently change the user's audio routing/volume.
- Use a new scratch document and keep it foreground through the paste. Use the
  available native UI tool to read the actual inserted text. Do not submit a chat,
  email, or form as part of testing. A clipboard match alone does not prove paste.
- Announce the audible fixture and microphone test. Honor existing authorization;
  avoid asking again for an already requested round trip. Permission changes and
  OS authentication remain separate actions under the active tool policy.

## Run and retain proof

Requires Python 3.11+, the installed model/runtime, macOS `say` with Samantha, and
an unused output directory. Read the relevant journey before running:

```sh
python3 scripts/verify-installed.py \
  --paste-target TextEdit \
  --output artifacts/performance/installed-roundtrip-NEW
```

This drives the existing installed daemon. It requires a cold model, speaks a
known phrase, checks preparation while Recording, stops recording, validates the
transcript/clipboard, and observes the full 300-second idle window. Verify the
visible paste separately. `--paste-target` checks actual macOS foreground focus;
addressing a background window through automation does not establish that focus.
It does not save captured microphone audio.

If the round trip already passed and further dictation reset the timer, observe
without playing audio or issuing a worker job:

```sh
python3 scripts/verify-installed.py --observe-idle \
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
a successful transcription with `reused_model = true`, expected words, actual paste
where claimed, and that same model process disappearing near the idle deadline
while the daemon survives. Filter children by executable path and parent PID:
Nemo also launches sound helpers such as `afplay`.

Keep raw JSON and relevant telemetry in ignored `artifacts/`. Preserve failures and
inconclusive attempts. Do not store private clipboard text or user recordings.
The full helper restores prior plain-text clipboard content only if the clipboard
still contains its fixture; it does not preserve every rich pasteboard format.
Clean only the test's recording/process/document; leave user activity alone.

For a stale post-update grant, follow the targeted recovery in the journey index.
Do not rebuild, re-sign, reinstall, broadly reset privacy permissions, or edit the
TCC database merely to make a verification pass.
