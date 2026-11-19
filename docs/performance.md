# Performance on the M5 Max

For the versioned synthetic speech corpus and repeatable runtime/model comparison
harness, see [speech benchmarking](benchmarking.md). It reports per-duration HTTP
latency and word error rate separately from real installed-app delivery timings.

For how a dictation becomes text, which stage costs what, and how to find a
bottleneck before optimizing it, see [the inference pipeline](inference-pipeline.md).

The [delivery latency update](delivery-latency.md) describes the October 30
delivery changes and paste measurements. The engine comparison and 48 kHz
resampling measurements below are from November.

The 128 GB laptop now preloads MLX-Audio at launch and keeps it resident for the session. The model occupies about 27 GB. Recording can begin during the initial load. Between dictations the worker sleeps and the MLX HTTP helper blocks waiting for a request. During recording, the resident profile submits a small GPU operation every two seconds.

## Reducing warm latency below 585 ms on the 30-second project update

The final signed candidate took **552 ms** in the first six pairs and **558 ms**
in an independent six-pair repeat. The installed build took **684 ms** and
**693 ms** in those same series. All twelve measured warm requests on the
candidate finished below 585 ms and returned the control's 87-word transcript.

On November 19, 2026, the installed app had the full-vocabulary MLX draft decoder.
The earlier 585 ms measurement used an experimental decoder with a 16-token
shortlist and asynchronous submission every twelve target layers. For this
comparison we copied the installed, signed app and used its controller for
all candidates. We kept the shortlist and twelve-layer scheduling and made
two further changes in [the decoder](../scripts/nemo_mtp.py).

1. The draft decoder now keeps up to eight proposed token IDs on the GPU,
   reads them back together and truncates at the first proposed end token.
   This removes a CPU wait between successive proposals and preserves the
   target's verification batch shape.
2. While processing the prompt and updating the draft after verification, the
   draft attention layer still updates its cache for every input position.
   Only the last position supplies the next proposal. Running the draft expert
   layer on that one position skips work on the discarded outputs. The target
   model still verifies each batch and chooses the emitted tokens.

Each row uses six pairs of signed app visits on AC power on the 128 GB M5 Max
(macOS 26.5.2). The pair order alternates. Each visit loads the NVFP4 model and
transcribes the same saved 48 kHz mono WAV twice through `nemo transcribe --json`.
The table reports the second request on the warm helper, from request submission
through the returned text. Live Stop-to-text also includes capture finalization
and text delivery.

| Clip and trial | Installed median (range) | Final candidate median (range) | Median paired reduction |
| --- | ---: | ---: | ---: |
| 30 s project update, first series | 684 ms (680–809) | 552 ms (547–559) | 19.5% |
| 30 s project update, independent repeat | 693 ms (686–707) | 558 ms (556–566) | 19.4% |
| 5 s lunch message | 188 ms (188–195) | 161 ms (158–162) | 15.3% |

We also reran the decoder that originally produced 585 ms against the final
candidate using the same controller. It took **603 ms** (593–819) versus
**563 ms** (559–571). The final candidate won all six pairs with a median
paired reduction of **6.5%**. The historical 585 ms and the new 603 ms both
measure the earlier implementation under their respective run conditions.

Median sampled peak helper footprint on the 30-second file was 27.6 GB for
the installed build and 27.8 GB for the candidate in both series. Median
model load took 2.92 and 2.91 s for the control, and 3.01 and 2.92 s for the
candidate. The live app stayed available. When a real dictation overlapped a
comparison, we retained the incomplete run and started a new series when idle.

The first candidate only batched the draft reads on top of the earlier
prototype. Its 30-second medians were 567 and 612 ms against installed controls
of 676 and 708 ms. It won all twelve pairs but missed the absolute target in
the second series, so we looked for work we could remove. Against the
twelve-layer prototype itself, batching alone won four of six pairs, with a
3.2% median paired reduction (618 to 597 ms). Changing the shortlist selection
order saved only another 2.5 ms in the helper screen, so we kept the original
tie handling.

In a six-round helper screen, computing the draft expert output only at the
last position reduced the 30-second median from 625 to 585 ms, winning five of
six rounds against the adjacent control runs. The 5-second median went from
168 to 160 ms. Submitting the draft cache update asynchronously took 608 ms
alone and 611 ms combined with the last-position change. We kept the synchronous
cache update for the final signed comparison.

The two signed bundles returned byte-identical text on all twelve frozen
recordings (1–300 seconds) and five versions of the project update with changes
to volume, noise, echo or trailing silence. The 30-second original and all five
variants had zero word errors. Both builds made the same 506 errors against
499 reference words on the 200-second library proposal. The comparison checks
punctuation and capitalization as well as word errors.

The local evidence directory is `artifacts/performance/sub585-pmwykO`. It
contains signed app copies, the input hashes, exact responses and individual
timings. `trial1` and `trial2` record the first candidate. `refined-1`,
`refined-2`, `refined-short-retry3` and `prototype-vs-final` record the
final decoder. `accuracy` and `refined-accuracy` hold the per-file transcripts
and word-error counts. The control and measured build have code-signing hashes
`d2b83d5c7463c8a2763cd93c9e24f286411fa47a` and
`b28113184ded5be644f039ba1b6325553f4d8a83`. The signed handoff bundle
(`56a39e031c5324b0b11b3e2756bc82381fcdd7cb`) adds a source attribution
comment. Its compiled Python code is byte-identical to the measured build.
Two more pairs with that bundle took 561 and 571 ms and returned exactly the
same text. The installed controls took 696 and 703 ms. These visits are in
`handoff`. To repeat the silent comparison from the repository root, use a
fresh output directory.

```sh
run=artifacts/performance/sub585-pmwykO
out=$(mktemp -d "$run/replay-XXXXXX")
uv run --no-project .agents/skills/optimize-nemo-inference/scripts/compare-bundles.py \
  --baseline "$run/Control.app" --candidate "$run/Nemo.app" \
  --seconds 30 --rounds 6 --output "$out/30s"
```

The signed handoff bundle was installed at `~/Applications/Nemo.app` on
November 19. The menu bar daemon reported the installed executable's SHA-256
and ran the bundled MLX helper. A silent replay through the installed binary
returned the same 87-word transcript on both requests. The warm request took
546 ms. The terminal launcher recognized the signed receipt and connected to
the running app. The installer log, backup path and replay evidence are in
`artifacts/performance/sub585-pmwykO/installation-2FGtxr`.

## Faster speculative decoding on 30-second dictation

On November 19, 2026, the signed Nemo app on the 128 GB M5 Max transcribed the
same 87-word project update in **601 ms**, down from **688 ms** on the previous
build. These are median warm file-request times. The helper proposes up to eight
tokens at a time. Each proposal previously read the original 131,072-row BF16
vocabulary projection, about 705 MB. The new path first scores the vocabulary
using a groupwise 4-bit copy, then scores the best 16 token IDs again using the
original BF16 rows. The full target model still chooses the first token and
verifies every subsequent batch. The additional resident weights occupy 189 MiB.
They are prepared once during model load and require no work between requests.

Six alternating pairs of signed app runs used `nemo transcribe --json`, an
isolated config per build, and the same 48 kHz PCM16 files. Each visit loaded
the model, then transcribed 5, 30, 5 and 30 seconds. The first request primed
the model. All 12 warm 30-second requests on the new build beat their paired
controls, with a 12.7% median paired reduction. Warm 5-second requests fell
from 187 to 170 ms (six pairs). Median model load was 2.19 s before and 2.20 s
after. File replay exercised Nemo's normal inference request and returned text
to stdout without operating the speakers, microphone or clipboard.

The separate in-process experiment measured the helper itself.

| Warm measurement | Original | Shortlist | Median reduction within pairs |
| --- | ---: | ---: | ---: |
| 30 s speech, helper | 736.2 ms | 667.2 ms | 12.2% |
| 5 s speech, helper | 206.1 ms | 192.7 ms | 6.8% |

Eight pairs per duration alternated which decoder ran first. Each candidate
beat its paired control on both durations. The two timing columns are medians
of the individual builds. The percentage column is calculated within each pair.
Both paths ran in the same process with the same resident NVFP4 model, eight
draft tokens, 25 GB wired limit and 2 GB cache. Input was mono 48 kHz PCM16 on
AC power. Helper timing includes resampling and detokenization.

### Where the decoder spent its time

Seven warm profiles of the original 30-second path had a median of 698 ms.
The input produced 439 prompt positions and 95 output tokens. Across 11 target
verification passes, the target accepted 85 of 86 proposed tokens.

| Stage | Median |
| --- | ---: |
| Resample microphone-format audio | 12 ms |
| Encode audio and prepare prompt | 29 ms |
| Prefill decoder and initialize draft state | 136 ms |
| Draft tokens | 187 ms |
| Verify proposals with the target | 291 ms |
| Repair caches and prepare next draft | 38 ms |
| Finish transcript | <1 ms |

Following the [inference optimization skill](https://github.com/sybil-solutions/inference-optimisation-skill),
we timed the decoder at its existing CPU/GPU synchronization boundaries. Target
verification was the largest stage. Drafting offered a smaller, independently
testable change because it read the full vocabulary matrix 86 times for this
clip. A cached recurrent scan improved the exploratory median by 4.2%, below
the 5% retention threshold. Deferring draft-token reads improved it by 0.6%.

### Accuracy and reproduction

Both builds returned byte-identical text on every frozen fixture from 1 to
300 seconds, including punctuation and capitalization. The 30-second clip had
zero word errors. Both builds made 506 word errors against 499 reference words
on the 200-second stress case. Directly using a 4-bit or 8-bit head for draft
selection changed the transcript on that case, so neither passed. Re-scoring
the shortlist restored the baseline transcript. Different draft proposals can
change target batch shapes and low-precision rounding near ties, so future
changes still need the full corpus comparison.

Five additional silent replays varied volume, background noise, room echo,
padding and combined quiet speech with noise. Both decoders returned identical
text with zero word errors on each identical input.

Earlier live checks delivered the project update to the clipboard in 759 ms on
the control and 747 and 707 ms on the candidate. Playback and microphone
capture produced a different waveform on every take. The control matched the
reference. Both candidate takes differed, and the second added three words.
A same-waveform comparison of those microphone takes was unavailable. These
are individual Stop-to-clipboard observations, not an estimate of typical
delivery latency or proof of equivalence on those captures.

The installed bundle uses the previous controller payload and the new runtime
receipt `d66f6be4bc8548501b12eb2c998377d270715c7eb8f379cbc331498b01917b57`.
Its signature verified after a silent transcription through the installed
binary. That final 30-second request took 640 ms and matched all 87 words.

Validate the frozen corpus with `uv run --no-project scripts/speech_corpus.py`.
For an A/B, start the pinned helper with the old and new runtime on the same
48 kHz copies of the fixtures, warm both, alternate first position and compare
raw transcripts before calculating latency. The local run in
`artifacts/performance/decoder-ZqoljL/` includes the pinned baseline helper,
signed-package pairs (`app.json`), helper samples (`production.json`),
stage samples (`stages.json`), perturbed inputs (`variants.json`) and the
final installed verification (`installed-silent.json`). Target verification
remains the largest stage in the baseline profile.

For a quiet spot check, pass a decoded fixture directly to the signed app:

```sh
wav=$(mktemp -d)/project-update.wav
afconvert -f WAVE -d LEI16@48000 -c 1 benchmarks/speech/audio/030s-project-update.flac "$wav"
"$HOME/Applications/Nemo.app/Contents/MacOS/nemo" transcribe --json "$wav" "$wav"
```

The second result uses the resident model in the same CLI process. For actual
Stop-to-dispatch timings, `uv run --no-project scripts/latency-report.py`
reads the daemon's existing timing events from ordinary dictation. Both checks
can run without playback or test microphone capture.

The follow-up [target verification profile](target-verification-m5-max.md)
measures the installed shortlist decoder and breaks down its expert layers.

## Engines

Measured November 9, 2026 through the app's own request path, five alternating rounds,
medians with min–max. Full tables, cold loads, failures and limits are in the
[engine comparison](engine-comparison-2026-11-09.md).

| clip | `nemo-speech` warm | `mlx-audio` warm (MTP, 8 drafts) | speedup | word errors |
| --- | ---: | ---: | ---: | --- |
| 5 s | 310 ms (304–334) | 188 ms (183–192) | 1.65x | 0 / 0 |
| 20 s | 761 ms (748–803) | 468 ms (458–488) | 1.63x | 0 / 0 |
| 30 s | 1072 ms (1056–1140) | 670 ms (657–695) | 1.60x | 0 / 0 |
| 60 s | 1916 ms (1891–2003) | 1235 ms (1213–1298) | 1.55x | 2 / 2 |
| 100 s | 3036 ms (2998–3139) | 1974 ms (1936–2019) | 1.54x | 0 / 0 |

- **What shipped:** both engines sit behind one interface and are selected by `backend`
  in `config.toml`. The default is unchanged: the bundled `nemo-speech` runtime (still
  `mtp_draft = 0`). `mlx-audio` is opt-in with `nemo backend setup mlx-audio` and
  `nemo backend use mlx-audio`.
- **Memory:** `nemo-speech` 20.2 GB; `mlx-audio` 27.0 GB after a warm pass and 28.5 GB
  at peak, with its buffer cache now bounded at 2 GB. Unbounded it reached 38–40 GB.
- **Process-cold, file cache warm:** both engines load in 2.2–2.4 s; a cold 30 s request
  totals 3.5 s on `nemo-speech` and 2.9 s on `mlx-audio`.
- **One second of audio is not faster** (1.05x): that request is fixed cost.
- **Not measured on this build:** capture, clipboard, visible insertion, and p95.

Setting it up by hand is [documented here](mlx-audio-setup.md). The earlier HTTP-level
[MLX-Audio experiment](mlx-audio-experiment.md) and the
[interface proposal](pluggable-inference-backend.md) are kept as history.

A separate [conservative VAD experiment](vad-experiment.md) tested trimming leading and trailing silence before inference. It preserved reference words in the tested speech cases, but the delivery-time gains missed the experiment's retention gate, so VAD was not added to the app. The report records the measurements, limits, and conditions for revisiting that decision.

## MLX-Audio resampling for 48 kHz microphones

On November 19, 2026, faster resampling reduced warm MLX-Audio request latency by
9–14% for 5–30 second recordings on the 128 GB M5 Max. All 48 warm transcript
pairs matched exactly. Each duration had 12 warm requests per build across six
alternating rounds through `nemo transcribe` in the signed test package.

| Audio | Before | After | Median paired reduction |
| --- | ---: | ---: | ---: |
| 5 s | 210.5 ms | 191.6 ms | 8.6% |
| 10 s | 348.8 ms | 308.6 ms | 10.7% |
| 20 s | 566.4 ms | 481.7 ms | 14.2% |
| 30 s | 825.4 ms | 707.0 ms | 14.2% |

The frozen corpus decodes at 16 kHz, which skips the resampling used by the
laptop's 48 kHz microphone inputs. Timing the audio reader separately on 48 kHz
copies of the same speech exposed about 130 ms of CPU work for the 30-second
clip. Using overlap-add FFT convolution with the same anti-alias filter, edge
padding, and decimation phase cut the audio reader and resampling stage to
about 9 ms. The helper uses this path for 48 kHz mono PCM16 going into the 16 kHz
model. Other formats use MLX-Audio's existing reader.

The experiment held the NVFP4 weights, eight-token speculative decoding, and
2 GB buffer cache fixed. Early exploratory timings were set aside after Blender
overlapped the cache experiment and a live dictation interrupted the first
resampling screen. The first screen also shared a control path with the working
tree and picked up an edit. The final comparison used separate helper paths and
retried interrupted pairs. The two stress clips produced identical text between
builds, including the existing 200-second recognition failure. Peak helper memory
through 100 seconds rose from 28.50 GB to 28.85 GB. These file replays measured
request latency and did not measure microphone-to-paste latency.

To reproduce the workload, convert the frozen FLAC fixtures with
`afconvert input.flac output.wav -f WAVE -d LEI16@48000 -c 1`. Give both builds
the same WAVs and use the same installed weights with isolated configurations.
Use one fresh CLI process per visit, record its first request separately, and
compare the subsequent `request_ms` events and transcripts for matched clips.
Alternate build order and repeat a pair if another GPU workload or live
dictation overlaps it. The local run retains its launch scripts, fixture hashes,
raw transcripts, and decision log in
`artifacts/performance/m5-max-fHQUHk/`. See the
[inference optimization guide](https://github.com/sybil-solutions/inference-optimisation-skill)
for the one-variable and alternating-control method.

After installation, the running app reported MLX-Audio and a 48 kHz mono input.
The installed bundle passed signature verification and its CLI transcribed the
5-second 48 kHz fixture with the expected words.

## First request after model loading

A profile of 48 kHz capture pointed to SciPy's first import on the resampling path.
We tested two changes against the installed overlap-add build on November 19.
Importing SciPy concurrently with model loading shortened the first request, but
increased median load plus first request from 2.76 to 2.84 seconds. We dropped it.

The installed version computes the same 385-tap Kaiser filter with NumPy and
uses 8,192-frame overlap-save FFT blocks. NumPy already loads with the model.
Across all 12 frozen 48 kHz fixtures (1–300 seconds), the largest absolute
sample difference from the pinned MLX-Audio reader was below 1.17 × 10⁻¹⁰ at full scale.

| Median across six fresh processes per build | SciPy overlap-add | NumPy overlap-save |
| --- | ---: | ---: |
| First 5 s request after model load | 625 ms | 229 ms |
| Model load plus first 5 s request | 3,034 ms | 2,824 ms |
| Repeat 5 s request | 192 ms | 192 ms |
| Warm 30 s request | 723 ms | 737 ms |

Each pair alternated build order on AC power and used the same weights, draft
length, cache settings and three recordings (5 s, 5 s, 30 s). All 18 transcript
pairs matched exactly. The installed CLI also matched the previous build on
60-, 200- and 300-second recordings and on 16 kHz mono, 44.1 kHz mono and
48 kHz stereo inputs. The 200-second stress case still had 101.4% word error
rate. The 30-second median rose 2%. These are process-cold
comparisons with normal macOS file caching. Model load and request medians
come from different observations and need not sum to the median total.
The local comparisons, fixture checks and scripts are in
`artifacts/performance/first-request-REO5q7/`.

To replay the three requests on the installed build from the repository root:

```sh
capture_dir=$(mktemp -d)
for clip in 005s-lunch 030s-project-update; do
  afconvert "benchmarks/speech/audio/$clip.flac" "$capture_dir/$clip.wav" -f WAVE -d LEI16@48000 -c 1
done
"$HOME/Applications/Nemo.app/Contents/MacOS/nemo" transcribe --json \
  "$capture_dir/005s-lunch.wav" "$capture_dir/005s-lunch.wav" "$capture_dir/030s-project-update.wav"
```

The CLI starts its own model process. Read its `model_load` and `transcription`
events on stderr to distinguish loading, first-use work, and reuse. The desktop
app has a separate resident engine.

## Keeping the model ready between dictations

The installed M5 Max configuration uses these preferences in
`~/Library/Application Support/Nemo/config.toml`. Restart Nemo after editing them.

```toml
idle_unload_seconds = 0
preload_model = true
```

Zero disables idle expiry. The model loads in the background as Nemo opens. A
hotkey press can start recording immediately, and transcription reuses that
worker. Stopping a very short recording during the initial load can still wait
for readiness. Quitting Nemo releases the model. To restore the previous
five-minute profile, use `idle_unload_seconds = 300` and `preload_model = false`.

On the resident build before the recording-only GPU change, one observation spanning 310.16 idle seconds kept
the same engine PID. Its footprint was 26.67 GB at both endpoints. It accrued
3.7 ms of process CPU time and logged zero reloads or transcriptions. This
measures the engine process only. The next live dictation emitted a successful
transcription with `reused_model = true`, zero model-load time and successful
delivery.

A five-second fixture took 188–191 ms in two immediate repeats, then 505 ms
after 310 seconds without a request to the same isolated model. This single
idle trial demonstrates reuse and shows how much HTTP response time varied within one
resident process. The measurement includes both host and device work.

To test periodic warming, we used 100 ms of 48 kHz silence, capped generation
at one token, and let the engine sit for 15 seconds before each comparison.
Four alternating pairs produced these medians:

| Work issued after the idle interval | Probe occupancy | Following 5 s request |
| --- | ---: | ---: |
| Real dictation only | 0 ms | 386 ms |
| Silent probe immediately followed by real dictation | 325 ms | 190 ms |

The helper processes one request at a time. A dictation arriving during a probe
could wait for its remaining runtime. The installed profile sends zero synthetic
transcriptions.

A second experiment replaced decoding with a 32-element MLX GPU reduction.
After 15 seconds, the median control request took 378 ms. Issuing the small
operation immediately before transcription occupied the helper for 183 ms,
then the request took 206 ms. With five seconds between the operation and
transcription, the request took 411 ms. Each condition had four observations,
in alternating order. The measured cost includes the local HTTP round trip.
We then varied the gap between two synchronized GPU operations. Across three
observations per gap, the second operation took a median 2.3 ms at one second,
7.2 ms at two seconds and 225 ms at five seconds, including local HTTP. These
measurements bracket the useful interval on this laptop. They do not establish
a hardware timeout across workloads or power modes.

The resident MLX profile submits the 32-element operation every two seconds **only while the microphone is recording**. The call submits GPU work
asynchronously. The worker can accept a real job between pulses, and a failed
wake request disables pulses for that recording. The app caps the optional
HTTP wait at 50 ms. Leaving the recording state on Stop, Cancel or input loss
releases the recording's token. A queued pulse checks that token before it runs.
Neither audio nor a generated token is involved.

To screen the recording-only approach, each visit had six seconds idle and a simulated
five-second recording, followed by the same real five-second speech fixture.
Four alternating pairs compared capture with and without one asynchronous
submission per second. Median request latency immediately after the simulated
Stop fell from **395 to 223 ms (43%)**. The 20 submission acknowledgments took
**0.7–1.5 ms** each, with a 0.95 ms median. That measures CPU-side request
occupancy, not GPU execution time or battery energy. Transcripts matched in
every pair. This comparison used the production MLX endpoint. Physical capture
and clipboard correctness were checked separately.

To select the longest useful interval, each trial waited six seconds after the
previous request, sent two asynchronous GPU submissions at the tested spacing,
then waited one full interval before sending real speech. This measures the case
where the user stops just before another submission would be due. Four rounds
rotated the interval order and retained identical transcripts.

| Interval during recording | Median 5 s speech request |
| --- | ---: |
| 1 s | 222 ms |
| **2 s** | **227 ms** |
| 3 s | 406 ms |
| 4 s | 412 ms |

The two-second interval halves the number of submissions and stays within the
preselected 3% median latency tolerance of one second. It was the longest
tested interval that qualified. Intervals between two and three seconds were
not measured. A four-second trial reached 635 ms. Desktop activity and clock
changes remain sources of variation.

The signed app also captured a spoken fixture through the Samson Q9U and
delivered the expected words to the clipboard in the isolated acoustic check.
The final installed build transcribed 7.34 seconds of captured audio in 269 ms,
with zero additional loading time. Stop to clipboard dispatch took 289 ms. A separate startup check held model
preparation open, issued the recording command, and observed microphone
capture while the loader was still running. In a lifecycle check with the
installed executable and an instrumented helper, the helper saw zero wake
requests before recording, three during capture at 2.006-second intervals,
and zero in the 2.3 seconds after cancellation. The final bundle passed code
signature verification and matched the responding installed executable.

The decoder experiment below uses the November 19 resident build as its
baseline. Its installed executable had SHA-256
`1f699d7e88e3b333b54ffe9001567681adaa10ba642099955e85874f4dd364ce`.
The acoustic acceptance record is `artifacts/performance/first-request-REO5q7/a3/roundtrip/results.json`;
the wake interval and cancellation check is `two-seconds.log` in the same run.

### Ranked latency opportunities on this Mac

| Priority | User outcome and baseline | Target and experiment |
| --- | --- | --- |
| 1. Avoid reloads after a pause | Five-minute expiry previously discarded a roughly 2.3 s load | Keep the same engine past 300 s with zero inference jobs. Observed on the installed app. |
| 2. Reduce the first real request | The 5 s recording previously took 625 ms after loading | Save at least 150 ms, with 350 ms as the stretch target. Six alternating pairs showed a 396 ms difference between medians. |
| 3. Reduce steady decoding latency | The new build measured 737 ms for a warm 30 s recording | Proposed goal of 5%, stretch 15%. Profile token generation and replay identical audio before changing the decoder. Token-level time has yet to be measured in this experiment. |

Metal already offloads the decoder to the GPU. The pinned llama.cpp includes M5 tensor support. `serve --threads 4` controls HTTP workers; actual inference CPU threads are `--asr.backend.threads` (zero selects up to eight). The bundled nemo-speech engine cannot use MTP or DFlash on Metal. The MLX-Audio results above use MTP. The high-concurrency 128 GB server preset targets throughput, whereas Nemo submits one dictation at a time. See the pinned [platform support](https://github.com/NVIDIA/NeMo-Speech.cpp/blob/6a62501107e1894723880f1094a286ea996a0ec7/docs/salm/models.md), [configuration](https://github.com/NVIDIA/NeMo-Speech.cpp/blob/6a62501107e1894723880f1094a286ea996a0ec7/docs/salm/configuration.md), and [Metal device implementation](https://github.com/ggml-org/llama.cpp/blob/bd4f514db14d87fded667787a7a963bfbaa98e89/ggml/src/ggml-metal/ggml-metal-device.m).

The laptop was on AC power with `powermode = 2` for both AC and battery. High Power Mode was already enabled. Apple describes it as extra cooling capacity for sustained intensive workloads; these tests do not establish a benefit over Automatic Mode. See [Apple's power mode guidance](https://support.apple.com/en-us/101613).

## October installation history

The combined performance/settings update was installed at `~/Applications/Nemo.app` with five-minute retention, parallel loading during recording, local JSON telemetry, trailing-period settings, launch-on-startup controls, and recording sound packs. Its executable matches the signed package, and code-signature verification passes. Its settings were `idle_unload_seconds = 300`, `preload_model = false`, `model_warmup = false`, `inference_threads = 8`, and `encoder_batch_size = 1`. The final app passed cold/warm acoustic transcription and visible paste into an unsent ChatGPT draft, which was cleared without submission. Both daemon permissions are authorized. The final executable's full isolated idle-expiry check also passed. See [combined installation verification](combined-install-verification.md) for exact evidence and coverage limits, including the unverified TextEdit target and isolated timer scope.

For the earlier performance-only installation, the original ad hoc code identity differed, so the installer initially refused replacement. After explicit user approval for the possible permission reset, installation succeeded and retained the previous app at `~/Applications/.Nemo.backups/d10f8081b5e945acb3ef1293eb875817.app`. The initial daemon reported Accessibility disabled and Microphone not determined. That earlier verification refreshed a stale Accessibility grant and confirmed both permissions, then exercised the speaker-to-microphone path described below. Those grants and acoustic results do not establish the final combined build's permissions or paste behavior.

## Measurements

Measured October 28, 2026 on macOS 26.5.2, Apple M5 Max, 18 CPU cores, 40 GPU cores, 128 GB unified memory. Native runtime revision `6a62501107e1894723880f1094a286ea996a0ec7`; llama.cpp `bd4f514db14d87fded667787a7a963bfbaa98e89`. Release build with native CPU tuning, Accelerate, and Metal enabled; pinned IQ4_NL decoder and F16 encoder from `models/manifest.json`.

Each candidate used five fresh server processes, alternating candidate order between rounds. Each process received the same recording twice: its first request, then a warm request. The timer includes HTTP upload, inference, response reading, and JSON decoding. Loading is measured separately from process creation to the successful health check, at 100 ms polling resolution. No profiling was enabled in comparison runs. Results below are medians with min–max in parentheses.

| Inference CPU threads | Warm request, 5.8635 s recording | First request |
| --- | --- | --- |
| 8 (original default) | 348 ms (343–352) | 367 ms (366–396) |
| 4 | 370 ms (345–438) | 368 ms (364–385) |
| 6 | 344 ms (344–372) | 366 ms (365–369) |
| 18 | 344 ms (344–347) | 367 ms (365–374) |

The small 6/18-thread differences overlap normal variation. Keep eight threads; this experiment does not establish a meaningful CPU-thread improvement. Typical process loading added 2.6–2.9 seconds; the first baseline launch took 17.4 seconds. OS file caches were not flushed, so this is process-cold latency with uncontrolled disk caching, rather than a cold-disk benchmark.

| Encoder batch size | Warm request, 63.90125 s recording | First request |
| --- | --- | --- |
| 1 | 2.141 s (2.132–2.270) | 2.167 s (2.153–2.169) |
| 4 | 2.289 s (2.146–2.601) | 2.193 s (2.157–2.314) |
| 8 | 2.151 s (2.142–2.419) | 2.165 s (2.162–2.170) |

| Startup warmup | Load to readiness | First request | Warm request |
| --- | --- | --- | --- |
| Off | 2.716 s (2.623–2.928) | 367 ms (366–371) | 346 ms (345–349) |
| On | 2.915 s (2.818–2.923) | 362 ms (347–374) | 363 ms (345–379) |

These are another five alternating rounds per candidate on the short fixture, with 20 correct reference transcriptions. Warmup added about 0.2 seconds before readiness and did not establish a useful steady-state gain. Overlapping loading with recording and five-minute retention address the substantial cost.

Larger encoder batches did not improve this multi-window fixture. Keep one. Increasing decoder `n_batch` would not raise its physical Metal microbatch above the pinned runtime's `n_ubatch` cap of 512.

All 70 transcriptions in the CPU/encoder comparisons matched their synthetic reference words, ignoring case and punctuation. This verifies that the timed requests performed the intended work. It is not a broad accuracy evaluation. Other desktop applications remained open; the benchmark can reject a busy Nemo daemon at request boundaries, but does not isolate system-wide contention or detect a dictation beginning midway through a request. The numbers exclude microphone startup, recording, WAV finalization, and clipboard/paste delivery (which includes an intentional 100 ms delay).

Raw results, fixture text/audio, logs, and runtime binary identities are in ignored `artifacts/performance/`. The retained script recreates the method; model weights are never duplicated.

## Lifecycle and recording overlap

The real daemon retained the same model PID beyond the old 60-second timeout with `idle_unload_seconds = 300`; a warm request reused it, and SIGTERM reaped it. A separate one-second timeout unloaded correctly. This verifies the timeout path and retention beyond 60 seconds; it does not claim an observed full 300-second expiry. Cancellation during startup preserved the clipboard and reaped the child.

A separate real microphone check started with no model loaded and startup preloading disabled. Capture remained in Recording while the model started and finished loading in **2.600 seconds**. Canceling after readiness kept the prepared model available; early cancellation produced a failed model-load event after **104 ms** and reaped the runtime. No transcription or paste was requested, no audio file was saved, and the clipboard was unchanged. The verifier's overall early-cancel cleanup duration also includes sound helpers and should not be interpreted as model cancellation latency. Both checks used isolated data/config directories and left the installed daemon's microphone session alone.

```sh
python3 scripts/verify-performance.py \
  --runtime "$HOME/Applications/Nemo.app/Contents/Resources/runtime/nemo-speech" \
  --model "$(target/release/nemo model path)" \
  --output artifacts/performance/new-lifecycle

# Requires already-authorized microphone access; capture is canceled without transcription.
python3 scripts/verify-performance.py --recording-only \
  --runtime "$HOME/Applications/Nemo.app/Contents/Resources/runtime/nemo-speech" \
  --model "$(target/release/nemo model path)" \
  --output artifacts/performance/new-recording-check
```

The recording check reports inconclusive without requesting permission when microphone access is missing. Static checks and all 28 non-model tests passed; the real-model reuse test also passed with content-free JSON timings. One initial parallel test-suite run hit an existing lock-release test flake; it passed alone and in subsequent complete runs. An initial recording-verifier attempt counted the sound helper as another model process; the verifier now identifies the runtime by executable path and preserves all owned children for cleanup.

## Installed-app acoustic verification

The installed app captured the known reference phrase through the MacBook Pro microphone while `say` played it through the built-in speakers. Output volume was already 100 and input volume 59; neither routing nor volume was changed. The app created runtime PID 66818 under daemon PID 64630 while Recording and completed model loading in 2.807 seconds before capture stopped. Its 7.168-second recording transcribed successfully in 0.415 seconds, with `reused_model = true` and zero additional loading time. These are one functional run's timings, not a new performance comparison.

The clipboard words matched the known reference. Foreground focus changed from the scratch TextEdit document to ChatGPT, and Nemo logged its actual paste destination as ChatGPT; the user returned the exact reference sentence in the chat. This verifies transcription and delivery there, but does not establish controlled TextEdit paste. The empty test document was closed; no captured audio file was saved. Later dictations continued to reuse the same model PID.

The full helper's original idle observation was marked inconclusive when further dictation began. A separate read-only observer is used to measure an uninterrupted five-minute interval without blocking user activity. New dictation resets that interval. `status` and `permissions` queries do not enqueue work or reset the timeout. Raw evidence is in `artifacts/performance/installed-full-verification/` and `installed-idle-verification/`.

A full-duration controlled check then used the **same installed executable** with a private data directory, a different hotkey, and no microphone or clipboard operations. The model remained present through **299.063 seconds** and was first absent at **300.049 seconds**, using one-second process snapshots. Its daemon stayed ready after expiry, and both owned test processes were reaped afterward. The installed executable SHA-256 matched before and after. This measures expiry after a preload job in the isolated instance; it does not claim that the user's continuously used desktop session sat idle for five minutes. Raw evidence is `artifacts/performance/installed-isolated-full-expiry-retry/results.json`, including 300 samples and the loaded/absent interval.

The first controlled expiry attempt exposed a verifier race: the runtime exited between separate liveness and child-identity checks at the deadline. Its last loaded sample was 299.032 seconds and the daemon reported normal unloading. The verifier now uses one process snapshot per tick; the complete five-minute rerun passed. This changed the checker, without changing or reinstalling Nemo.

Coordination with **Add trailing period setting** confirmed that its settings and startup checks use isolated bundles and data directories. Its source changes were preserved; no installed-app replacement occurred during this verification. The live installed daemon's Accessibility and Microphone grants were confirmed again after the controlled test.

The reusable project skill is [verify-nemo](../.agents/skills/verify-nemo/SKILL.md). It routes to `scripts/verify-installed.py` for the installed app and the existing isolated lifecycle helper for cancellation. Skill validation uses `uv run --no-project --with pyyaml "${CODEX_HOME:-$HOME/.codex}/skills/.system/skill-creator/scripts/quick_validate.py" .agents/skills/verify-nemo` when the selected Python does not already have PyYAML. This only supplies the validator's temporary dependency; the acoustic verifier itself uses the Python 3.11+ standard library.

## Local telemetry

Each transcription emits one JSON event to stderr. Finder-launched Nemo redirects stderr to `~/Library/Application Support/Nemo/daemon.log`; CLI stdout remains only the transcript. Fields include `audio_seconds`, `model_load_ms`, `request_ms`, `total_ms`, `reused_model`, `real_time_factor`, and `success`. A separate `model_load` event measures startup, including configured runtime warmup. Failure events retain available measurements and omit unknown spans with JSON null. Timing events contain no audio, transcript, authorization key, or error text. Nothing is sent to a telemetry service.

`real_time_factor` is request seconds divided by recording seconds: smaller is faster. `total_ms` begins when the backend receives the WAV, so it excludes capture/finalization and paste. Request time includes transfer, response reading, parsing, and any runtime queue/setup cost. Model loading includes process startup and readiness polling; a reused live model records zero model-loading time.

## Repeat the benchmark

Generate the existing verification fixture with `scripts/verify.sh`, or supply a mono WAV and known transcript. Use the exact runtime/model paths printed by `target/release/nemo config`:

```sh
python3 scripts/benchmark.py artifacts/sample.wav \
  --expected 'The quick brown fox jumps over the lazy dog. This is a local speech recognition test on my laptop.' \
  --runtime "$HOME/Applications/Nemo.app/Contents/Resources/runtime/nemo-speech" \
  --model "$(target/release/nemo model path)" \
  --candidate baseline:8:512:1:0 \
  --candidate prepared:8:512:1:1 \
  --daemon target/release/nemo \
  --output artifacts/performance/new-comparison
```

Candidate fields are `name:inference_threads:decoder_batch[:encoder_batch[:startup_warmup]]`; warmup is 0 or 1. Default candidates compare inference CPU threads 8, 4, and 6. Five alternating rounds are the default. Output directories must be new. Failed/mismatching/aborted requests are retained in JSON and cause a nonzero exit; SIGINT and SIGTERM clean up the owned server. Omit `--daemon` when Nemo is stopped. Use a long fixture when changing encoder batching; short recordings do not exercise several 30-second encoder windows.

## Profile separately

Run the same command with one candidate, `--profile`, and a new output directory. This enables `NEMO_SPEECH_TIMING=1` and takes a five-second CPU sample of its owned runtime. Profiling results are deliberately excluded from comparison summaries. CPU samples may include idle time when the two requests complete before sampling ends.

The runtime graph log reports input, allocation, submission/compute, output, and total milliseconds. Graph submission is asynchronous: synchronization can appear in output copying, and some device-only graphs return without synchronizing. Those columns cannot be interpreted as isolated GPU execution time or a complete decoder/encoder split. The HTTP API does not expose SALM prefill/decode stage timers. `verbose_json` requests alignment work and does not add performance measurements.

The separate CPU profile found the decoder worker waiting in `llama_context::synchronize → ggml_metal_synchronize → MTLCommandBuffer waitUntilCompleted` for 2,633 of its 3,891 sampled stack observations. This identifies GPU completion on the decoder’s critical path and explains why more host threads had little effect; it does not distinguish GPU bandwidth from compute limits. The sample also includes idle time and profiler overhead. One earlier profile was rejected because the user began recording after readiness; it performed no inference and remains recorded as a failed visit.

For a deeper GPU investigation, use a separate Instruments capture with the owned runtime PID:

```sh
xcrun xctrace record --template 'Metal System Trace' \
  --attach PID --time-limit 15s \
  --output artifacts/performance/metal.trace
```

In Instruments, add Performance Limiters counters to investigate memory bandwidth, execution occupancy, and host submission gaps. Full Xcode and Metal System Trace are available on this laptop. See [Apple's Metal profiling guide](https://developer.apple.com/documentation/xcode/analyzing-the-performance-of-your-metal-app). CPU samples alone cannot establish which GPU resource limits throughput.

## Decisions shaped by principles

- **Fix Root Causes:** measured loading separately before changing inference settings; traced the misleading HTTP thread flag to its parser.
- **Explain the Number:** retained five alternating trials, ranges, reference checks, and profile caveats; rejected thread/batch claims inside the observed variability.
- **Build the Lever / Prove It Works:** retained the benchmark and local lifecycle verifier rather than relying on one-off timings or compilation.
- **Experience First:** retained the model for the user’s five-minute reuse window to remove repeated startup waits, then released memory.
- **Laziness Protocol:** used existing JSON/stderr and flat configuration, without another telemetry service or dependency.
- **Foundational Thinking / Model the Domain:** one model worker owns preparation and transcription; a shared cancellation token follows the recording into preparation and transcription, preventing a late-spawned loader from delaying cancellation.
- **Boundary Discipline:** kept tuning bounds in configuration validation and preserved existing configuration defaults.
