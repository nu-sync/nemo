# Recording cues

Handy Marimba start and stop are unmodified files from
[cjpais/Handy](https://github.com/cjpais/Handy/tree/7bfb2e2c934b798bcf6e3627653aed2439996f89/src-tauri/resources),
commit `7bfb2e2c934b798bcf6e3627653aed2439996f89`:

- `handy-marimba-start.wav`: `marimba_start.wav`
- `handy-marimba-stop.wav`: `marimba_stop.wav`

The upstream MIT license is retained in `Handy-MIT.txt` and copied into the app's
licenses directory during packaging. The audio is embedded in the executable so
it works outside this checkout and without the Downloads audition folder.

macOS Recording loads `begin_record.caf` and `end_record.caf` from the local
CoreAudio component's `Contents/SharedSupport/SystemSounds/system` directory.
Apple's audio files are not copied into this repository or redistributed in the
bundle. If macOS no longer supplies a decodable pair at that location, playback
falls back to Handy Marimba and logs the condition.
