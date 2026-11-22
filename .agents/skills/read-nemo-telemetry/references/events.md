# Nemo event reference

One JSON object per line. Source: `src/telemetry.rs` and its call sites.

## On every event

| Field | Meaning |
| --- | --- |
| `event` | The event name |
| `ts` | When it was written, UTC, RFC 3339 with milliseconds |
| `version` | Cargo version of the build |
| `build` | First 12 characters of the daemon executable's SHA-256. Absent from CLI runs |
| `dictation` | The dictation's start time in Unix milliseconds. Present while a dictation is under way; shared by all its events |
| `success`, `error` | On steps that can fail. `error` is the text the menu shows |

## Events

| Event | Written when | Fields |
| --- | --- | --- |
| `daemon_started` | The app starts | `backend`, `idle_unload_seconds`, `preload_model`, `paste`, `max_recording_seconds` |
| `model_load` | An engine load finishes or fails | `engine`, `identity`, `load_ms` |
| `capture_start` | The microphone opened or failed to | `microphone_start_ms`, `recording_started` |
| `audio_finalized` | Stop closed the stream and built the WAV | `finalization_ms` |
| `inference_started` | The worker took the transcription job | `stop_to_inference_ms` |
| `transcription` | The engine answered or failed | `engine`, `audio_seconds`, `request_ms`, `model_load_ms`, `total_ms`, `reused_model`, `real_time_factor` |
| `delivery` | The clipboard write and paste dispatch finished | `delivery_ms`, `stop_to_dispatch_ms` |
| `dictation` | A dictation ended, once per dictation | `outcome`, `phase`, `duration_ms`, `error` |
| `backend_demoted` | The selected engine failed and the included one took over | `engine`, `fallback`, `reason`, `log` |

`dictation.outcome` is `delivered`, `canceled` or `failed`. `dictation.phase` is how
far it got: `microphone`, `recording`, `audio`, `transcription` or `delivery`.

A `model_load` has a `dictation` only when it finished while one was under way.
The launch preload has none.

## A delivered dictation, in order

```text
capture_start -> audio_finalized -> inference_started -> transcription -> delivery -> dictation
```

`stop_to_inference_ms`, `stop_to_dispatch_ms` and `finalization_ms` all start at
Stop. `microphone_start_ms` starts at the record request. `duration_ms` runs from
the record request to the end, so it includes the time spent speaking.

## What is not recorded

Audio levels, transcript length, whether a recording preview was used, model
unloads and engine memory have no event yet.
