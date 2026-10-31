# Synthetic English dictation corpus, version 1

Twelve **different original passages**, generated directly to lossless files with
Kokoro v1.0. No personal recordings, speaker loopback, or microphone capture.
Every FLAC decodes to mono 16 kHz PCM16 with an exact target frame count.

| Seconds | Topic | Kokoro voice | English variety |
| ---: | --- | --- | --- |
| 1 | Brief confirmation | `af_heart` | US |
| 5 | Lunch arrangements | `am_michael` | US |
| 10 | Garden instructions | `bf_emma` | UK |
| 20 | Weekend travel | `bm_george` | UK |
| 30 | Search project update | `af_sarah` | US |
| 40 | Application feedback | `am_adam` | US |
| 50 | Dinner recipe | `bf_isabella` | UK |
| 60 | Design review | `bm_daniel` | UK |
| 90 | Coastal field report | `af_nicole` | US |
| 100 | Incident handoff | `am_fenrir` | US |
| 200 | Library orientation proposal | `bf_alice` | UK |
| 300 | River restoration workshop | `bm_lewis` | UK |

## Reference and verification contract

`text/*.txt` is the actual source passed to TTS, not a transcript produced by the
model under test. Sources spell out numbers and generally avoid ambiguous
abbreviations to reduce normalization disputes. The texts are authored for this
repository, not copied from a book or repeated to fill a longer recording.

`manifest.json` binds each reference, FLAC container, and decoded waveform with
SHA-256 hashes. It records the TTS model/voice hashes, package versions, fitting
attempts, speaking speed, tail padding, and exact frame count. Run:

```sh
uv run --no-project scripts/speech_corpus.py
```

This **verifies integrity and duration**, not perfect spoken-word fidelity. TTS
can mispronounce or omit words; a known synthesis input alone does not prove
exact agreement with the waveform. Benchmark reports retain every ASR response
and word-level error counts for review against the source. Agreement with Nemo
is an ASR consistency check, not independent ground-truth certification.

The frozen [independent check](reference-check.json) uses pinned Whisper base.en,
with no reference prompt: all twelve clips pass a 5% per-clip WER gate, with
0–3.03% WER and 0.74% word-weighted WER overall (17 edits in 2,312 reference words).
This is strong automated agreement, not proof of perfect spoken-word fidelity or
human ground-truth certification. These fixtures have not been human-listened.
Do not rewrite references just to make a recognizer's mistakes pass.

There is one passage per duration and twelve voices, not a representative accent
or speaker evaluation. Clean synthetic speech omits natural hesitations, room
noise, microphone processing, and speaker variation. This is a frozen performance
and gross-accuracy regression suite. Real acoustic and installed-path tests
remain necessary before shipping a dictation change.

## Known runtime results

The frozen [Nemo smoke check](nemo-check.json) records a real installed-runtime
run and a fresh-server stress confirmation. It is **not a latency baseline**.
The IQ4_NL decoder and runtime hashes are included; reference texts were not
changed to match predictions.

| Fixture group | Nemo WER | Result at the 5% gate |
| --- | ---: | --- |
| 1–60 seconds | 0–1.32% | Pass |
| 90–100 seconds | 0–2.16% | Pass |
| 200 seconds | 124.05% | Fail: largely unrelated output |
| 300 seconds | 98.26% | Fail: largely unrelated output |

The long-case failures repeat with a fresh server, including the 200-second clip
as the first request. Independent Whisper WER on those same files is 0.20% and
0.40%, respectively. These are retained as meaningful stress failures, not
removed or counted as fast successes. Their runtime/model root cause has not
been established. `--suite all` and `--suite stress` therefore currently exit
nonzero on this recorded runtime/model; the default typical suite is separate.

## Provenance and licenses

- Original text and the corpus materials authored here use the repository's
  [MIT license](../../LICENSE), to the extent rights apply.
- TTS weights: [Kokoro-82M](https://huggingface.co/hexgrad/Kokoro-82M), v1.0,
  published under Apache-2.0. The
  [ONNX release](https://github.com/thewh1teagle/kokoro-onnx/releases/tag/model-files-v1.1)
  supplies the pinned model and voice archive; URLs and full digests are in the
  generator and manifest. Model weights are **not distributed in this repo**.
- Generation engine: [kokoro-onnx](https://github.com/thewh1teagle/kokoro-onnx),
  MIT. Its dependencies, including eSpeak NG used for phonemization, are installed
  only in the isolated generation environment. They are not bundled into Nemo.
- Independent recognizer: OpenAI Whisper base.en via
  [faster-whisper](https://github.com/SYSTRAN/faster-whisper), MIT code and model
  provenance. The converted model is pinned to a repository commit in the
  verifier; weights stay in ignored `artifacts/reference-asr/`.
- No Apple system-voice recordings or third-party literary passages are included.

Use ordinary Git for these modest FLAC assets; no LFS service or remote fixture
download is required to benchmark after cloning. Do not regenerate existing
files between A/B runs. To change content, create a new corpus version and
rebaseline. See [benchmarking instructions](../../docs/benchmarking.md) for
regeneration, candidate runtime/model comparisons, telemetry, and acceptance gates.
