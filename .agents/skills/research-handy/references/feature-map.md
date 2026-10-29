# Handy feature lookup

Sources: [official docs](https://handy.computer/docs) and
[cjpais/Handy](https://github.com/cjpais/Handy).
Paths checked on 2026-10-28 against commit
`7bfb2e2c934b798bcf6e3627653aed2439996f89`; recheck them for newer revisions.
Paths below are relative to Handy, not Nemo. They are starting points, not complete
call graphs. Read only the row relevant to the question.

## Docs and code entry points

UI paths below are under `src/components/settings/`; Rust paths are under
`src-tauri/src/`.

| Feature | Official docs | UI entry points | Rust search starting points |
| --- | --- | --- | --- |
| Shortcuts and recording modes | [General](https://handy.computer/docs/general) | `ShortcutInput.tsx`, `ShortcutActivation.tsx` | `shortcut/`, `actions.rs`, `transcription_coordinator.rs` |
| Microphone, mute, feedback sounds | [General](https://handy.computer/docs/general) | `MicrophoneSelector.tsx`, `MuteWhileRecording.tsx`, `AudioFeedback.tsx`, `SoundPicker.tsx` | `commands/audio.rs`, `managers/audio.rs`, `audio_toolkit/`, `audio_feedback.rs` |
| Model selection and downloads | [Models](https://handy.computer/docs/models) | `models/`; also `src/stores/modelStore.ts` | `commands/models.rs`, `managers/model.rs`, `managers/transcription.rs` |
| Idle model unloading | [Advanced](https://handy.computer/docs/advanced) | `ModelUnloadTimeout.tsx` | `settings.rs`, `managers/transcription.rs`, `tray.rs` |
| Paste, clipboard, auto-submit | [Paste Methods](https://handy.computer/docs/paste-methods), [Advanced](https://handy.computer/docs/advanced) | `PasteMethod.tsx`, `TypingTool.tsx`, `ClipboardHandling.tsx`, `AutoSubmit.tsx` | `clipboard.rs`, `paste_tx/`, `input.rs`, `actions.rs` |
| Custom words and trailing space | [Advanced](https://handy.computer/docs/advanced) | `CustomWords.tsx`, `AppendTrailingSpace.tsx` | Search the setting keys in `settings.rs`, then their consumers |
| Transcript history and retention | [History](https://handy.computer/docs/history) | `history/`, `HistoryLimit.tsx`, `RecordingRetentionPeriod.tsx` | `commands/history.rs`, `managers/history.rs` |
| Post-processing and prompts | [Post-Processing](https://handy.computer/docs/post-processing) | `post-processing/`, `PostProcessingSettingsPrompts.tsx`, `PostProcessingSettingsApi/` | `actions.rs`, `llm_client.rs`, `settings.rs` |
| Startup, tray, overlay | [Advanced](https://handy.computer/docs/advanced) | `AutostartToggle.tsx`, `StartHidden.tsx`, `ShowTrayIcon.tsx`, `ShowOverlay.tsx` | `autostart.rs`, `tray.rs`, `overlay.rs`, `lib.rs` |
| Diagnostics and CLI | [Debug](https://handy.computer/docs/debug), [CLI](https://handy.computer/docs/cli) | `debug/` | `cli.rs`, `lib.rs` |

For an unfamiliar feature, browse the docs sidebar, then search the current source
tree. Settings commonly connect through `src/stores/settingsStore.ts` and
`src-tauri/src/settings.rs`; inspect the command mapping instead of guessing names.

## Example: clipboard preservation

Start at Advanced → Clipboard Handling. `ClipboardHandling.tsx` uses the
`clipboard_handling` key. `src/stores/settingsStore.ts` maps that key to
`commands.changeClipboardHandlingSetting`. Its Rust handler,
`change_clipboard_handling_setting` in `shortcut/mod.rs`, parses the option and
persists it with `settings::write_settings`. Follow `ClipboardHandling` in
`settings.rs`, then the consumers in `clipboard.rs` and
`paste_tx/`. Inspect the selected paste method and platform branches before
explaining restoration timing or error cleanup. Search nearby tests for `restore`
and `clipboard`; a UI default alone does not prove paste behavior.
