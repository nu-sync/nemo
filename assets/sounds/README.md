# Dictation cues

Handy Marimba start and stop are unmodified files from
[cjpais/Handy](https://github.com/cjpais/Handy/tree/7bfb2e2c934b798bcf6e3627653aed2439996f89/src-tauri/resources),
commit `7bfb2e2c934b798bcf6e3627653aed2439996f89`:

- `handy-marimba-start.wav`: `marimba_start.wav`
- `handy-marimba-stop.wav`: `marimba_stop.wav`

The upstream MIT license is retained in `Handy-MIT.txt` and copied into the app's
licenses directory during packaging. The audio is embedded in the executable so
it works outside this checkout and without the Downloads audition folder.

`handy-marimba-complete.wav` is derived from the licensed start sound: two
resampled strikes at F#5 and B5, 110 ms apart, with a 45 ms fade and a peak
amplitude of 0.60. It retains the same MIT attribution.

`minimal-{start,stop,complete}.wav` and `chime-{start,stop,complete}.wav` are
original synthesized Nemo sounds. Minimal uses single decaying sine tones with
a quiet second harmonic; Chime uses rising/falling pairs and a three-note
completion. These files and the Marimba completion are the approved Downloads
auditions, copied without modification and embedded in the executable.

macOS Recording loads `begin_record.caf` and `end_record.caf` from the local
CoreAudio component's `Contents/SharedSupport/SystemSounds/system` directory.
Completion uses `/System/Library/Sounds/Glass.aiff`.
Apple's audio files are not copied into this repository or redistributed in the
bundle. If macOS no longer supplies a decodable set, playback
falls back to Handy Marimba and logs the condition.
