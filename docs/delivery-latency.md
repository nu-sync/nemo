# Dictation delivery latency

The October 30 source update reduces paste overhead and moves audio finalization
and delivery off the menu event loop. It starts model preparation when microphone
opening is requested, overlapping both operations. The existing five-minute
retention and optional startup preload settings are unchanged. Streaming audio
recognition is outside this change.

## Runtime ownership

The event loop owns Starting, Recording, and Working. Microphone attempts and stopped dictations have
IDs that completion events match against the active state. The cancellation token
follows the session through opening, preparation, finalization, inference, and delivery. The existing model worker still owns the inference process.

On Stop, the UI enters Transcribing immediately and transfers the recording to a
background finalizer. The stop cue plays only after that finalizer returns and successful audio is
queued for inference, so it cannot enter the recording or delay inference dispatch. That thread closes the microphone and encodes WAV without
waiting behind model loading. Its result returns to the event loop, which queues
inference only if the request is still active. A successful transcript starts a
background delivery task; the UI remains Transcribing until delivery completes.
Canceled or stale results cannot schedule another stage. Delivery checks cancellation
before touching the clipboard and again before posting paste events. Cancellation
after clipboard writing has begun cannot restore the previous clipboard, and posted
paste events cannot be recalled.

Clipboard writes use NSPasteboard instead of spawning pbcopy. Successful synchronous
clipboard writes are followed by the existing Command-V event pair without the
former fixed 100 ms wait. Accessibility checks and copy-only behavior remain.
Microphone setup failure or declined device fallback cancels speculative preparation.

## Timing events

The existing backend events still measure inference and loading. Additional local,
content-free JSON events cover these boundaries:

| Event | Field | Boundary |
| --- | --- | --- |
| `capture_start` | `microphone_start_ms` | Start request to microphone setup result, including thread scheduling |
| `audio_finalized` | `finalization_ms` | Stop handling to stream closure and WAV completion |
| `inference_started` | `stop_to_inference_ms` | Stop handling to model worker beginning the transcription job |
| `delivery` | `delivery_ms` | Background delivery call, including clipboard writing and paste dispatch |
| `delivery` | `stop_to_dispatch_ms` | Stop handling to delivery call completion |

Dispatch completion does not establish text insertion or pixel rendering in another
app. `stop_to_inference_ms` includes finalization and queueing; backend readiness can
still wait for an unfinished preparation job. These events contain no audio or text.

## Verification and measured scope

On the M5 Max, ten alternating baseline/native pairs exercised actual NSTextView
paste insertion with distinct Unicode fixtures. Every one of the twenty pastes
produced exactly one receipt containing the expected text. Copy-only and pre-canceled
delivery checks also passed. The baseline recreates the previous pbcopy plus 100 ms
wait and shares the same paste-event implementation with the current code.

| Path | Median insertion time | Minimum–maximum | Runs |
| --- | ---: | ---: | ---: |
| Previous delivery behavior | 119.6 ms | 113.3–181.3 ms | 10 |
| Native clipboard, no fixed wait | 44.8 ms | 36.3–51.0 ms | 10 |

The median difference was 74.8 ms in this scratch app. Dispatch itself took a median
115.4 ms for the baseline and 0.26 ms for native delivery. The removed fixed wait
explains the dispatch improvement; recipient event processing still separates
dispatch from insertion, so a 100 ms removed wait does not promise a 100 ms visible
gain. Insertion receipts use same-host wall clocks and are not rendered-frame
measurements. Other apps, system contention, and real dictations may differ.
No inference speedup is claimed.

A final rerun of the same ten-pair method also passed: baseline median 125.7 ms
(120.1–188.6 ms), native median 47.4 ms (35.3–50.6 ms), a 78.2 ms median difference.
Native delivery won all ten pairs in that rerun. Across both runs, all forty paste
receipts contained exactly the expected text. Both copy/cancellation checks passed.

Evidence is retained locally in `artifacts/performance/latency-update/delivery-01/`
and `artifacts/performance/latency-update/delivery-final/`.
The reusable check builds an isolated probe against the actual `src/output.rs`,
opens its own scratch app, alternates both paths, and restores prior plain-text
clipboard content if the clipboard still contains its fixture:

```sh
python3 scripts/verify-delivery.py \
  --output artifacts/performance/delivery-NEW
```

The new release build also passed isolated preparation checks with the real model:
cancellation during loading reaped the runtime, cancellation after readiness retained
it, and neither changed the clipboard. Cold and warm acoustic dictation through the
MacBook speakers and microphone produced the expected reference text in the native
scratch window, checked through the native UI as well as insertion receipts. These
are functional checks, not a paired end-to-end performance comparison. The development
instance used isolated data/app paths and the installed model/runtime. The initial development checks did not replace the installed app. A separate
TextEdit attempt was inconclusive because the foreground guard refused to begin;
it provides no TextEdit compatibility proof. The installed verification below uses
the native scratch target instead.

Run standard verification and the supplemental preparation checks:

```sh
cargo xtask check
cargo build --release --locked
python3 scripts/verify-performance.py --recording-only \
  --binary target/release/nemo \
  --runtime "$HOME/Applications/Nemo.app/Contents/Resources/runtime/nemo-speech" \
  --model "$(target/release/nemo model path)" \
  --output artifacts/performance/preparation-NEW
```

## Decisions shaped by principles

- **Experience First:** remove a fixed wait on every paste and keep feedback responsive
  while finalization and delivery run.
- **Foundational Thinking:** keep inference ownership in the existing worker and pass
  IDs/cancellation tokens through completion events, retaining Working until delivery.
- **Build the Lever / Prove It Works:** retain a repeatable insertion checker using
  production delivery code and validate real acoustic dictation and visible text.
- **Explain the Number:** report actual insertion separately from event dispatch,
  preserve all paired timings, and limit the gain to the measured target.
- **Test Behavior, Not Implementation:** assert exact inserted Unicode text, exactly
  one receipt per paste, copy-only output, and unchanged clipboard on pre-cancellation.

## Installed verification

The final October 30 installed build includes the overlay Stop/Cancel controls
from PR #5 and this latency update. The installer reused the existing signing
identity and preserved configuration. Deep/strict signature verification passed;
the installed executable matched the signed package, and its receipt matched the
release source executable. Both Microphone and Accessibility permissions remained
authorized. Active dictations caused safe installation refusals; the final prepared
package was installed through the same `nemo_build::install` API during an idle window.

The final cue-ordering build passed one cold and one warm acoustic dictation using
the actual installed daemon, MacBook speakers/microphone, and native scratch target.
Both insertions matched the reference exactly with the configured trailing-period
removal, and native UI inspection confirmed the text. Both runs reused the same
model PID after its initial preparation. A read-only process sampler observed that
runtime before the successful `capture_start` event, confirming preparation began
before microphone setup finished. Cold loading completed during Recording in
2.624 seconds.

| Final installed span | Cold run | Warm run |
| --- | ---: | ---: |
| Microphone setup | 107.3 ms | 125.3 ms |
| Audio finalization, off the event loop | 15.79 ms | 18.76 ms |
| Stop to inference-worker dispatch | 15.93 ms | 19.08 ms |
| Clipboard/paste-event dispatch call | 8.526 ms | 0.606 ms |
| Stop to delivery-call completion | 421.85 ms | 410.98 ms |

Queueing inference before playing the stop cue leaves about 0.14–0.31 ms between
finalization completion and inference dispatch in these two functional runs. The
cue starts after microphone closure, so it cannot enter the recording. These are
not a paired before/after benchmark or pixel-render measurements. Source ownership
and the matching installed binary establish the background finalization/delivery
paths; no frame-rate improvement is claimed.

An earlier installed build of the same delivery implementation also passed one
cold and five warm acoustic runs, all with exact insertion. Those five warm delivery
calls took a median 0.674 ms (0.643–0.774 ms). This confirms the fixed paste wait is
absent across repeated installed requests. The separate 75–78 ms insertion gain
belongs to the paired delivery probe, not an installed before/after comparison.

The final installed executable passed isolated preparation cancellation: early
cancellation reaped the loading runtime, cancellation after readiness retained it,
and the clipboard remained unchanged. Its test processes were reaped. The verifier
retains the new pipeline events without treating them as backend jobs; its regression
test exercises that distinction.

The live five-minute observer checked the first combined installed build before
the cue-ordering correction. It saw the same model PID present at 299.675 seconds
and absent at 300.740 seconds, measured from a Ready-log mtime proxy with roughly
one-second process polling; the daemon remained ready. Cue ordering does not change
the worker's idle timeout. Evidence is retained in `installed-latency/idle/`; its old
note about a fixed 100 ms delivery offset is superseded by the corrected verifier.

Local evidence is under `artifacts/performance/installed-latency/`:
`release-summary.json`, `release-cold/`, `release-warm/`,
`release-paste-events.jsonl`, `release-startup-overlap.json`, and
`release-cancellation/`. The initial six-run measurements are in `summary.json`,
`cold/`, and `warm-1/` through `warm-5/`. Foreground guards refused intermediate
attempts when ChatGPT had focus; those attempts produced no test paste and remain
recorded separately.

Final release source SHA-256:
`47ad01f8629af69f20e2d26921ea10f463ab5b84c438cc82e79a8ba16a201eee`.
Final installed signed executable SHA-256:
`ad379905e389b7cf4366226ae27d668d788fb927dcfac855d443afd1cd51a3b9`.

```sh
cargo xtask install
codesign --verify --deep --strict "$HOME/Applications/Nemo.app"
# Keep a scratch target foreground; this plays the known reference aloud.
python3 scripts/verify-installed.py --roundtrip-only \
  --paste-target "Nemo Delivery Check" \
  --output artifacts/performance/installed-cold-NEW
python3 scripts/verify-installed.py --roundtrip-only --warm \
  --paste-target "Nemo Delivery Check" \
  --output artifacts/performance/installed-warm-NEW
python3 -m unittest discover -s scripts/tests -v
```

The paste probe creates its scratch target temporarily. To use that target for an
installed acoustic check, build `scripts/verification/PasteTarget.swift` inside a
scratch app bundle named `Nemo Delivery Check` and pass it an insertion-event file,
or use an existing scratch editor and its actual foreground application name.
Visible insertion must still be checked separately.
