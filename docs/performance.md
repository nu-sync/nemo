# Performance on the M5 Max

Nemo starts loading the model in the background when microphone capture begins, so recording and loading overlap. It then keeps the model for five idle minutes on this laptop. This uses about 20 GB of resident memory; the model and transcription quality stay unchanged.

## Configuration

For the 128 GB M5 Max, use these settings in `~/Library/Application Support/Nemo/config.toml`, then restart Nemo:

```toml
idle_unload_seconds = 300
preload_model = false
model_warmup = false
inference_threads = 8
encoder_batch_size = 1
```

The five-minute idle timeout starts again after each completed worker job, including transcription. After five idle minutes the model process exits and releases its memory; cancellation, inference errors, and quitting can release it sooner. On-demand preparation uses the existing model worker while the UI remains in Recording. Transcription queues behind preparation and reuses the same process. Very short recordings can still wait for the remaining loading time. Optional startup preloading runs a cancelable model-loading job when the app opens. Optional warmup prepares the runtime before reporting the model loaded. It is disabled in this profile because the measured startup cost exceeded its small, noisy first-request benefit. Inference CPU threads and encoder batches are exposed for future measurements; increasing them did not produce a useful gain in this experiment. Existing configurations retain their previous behavior until edited.

Metal already offloads the decoder to the GPU. The pinned llama.cpp includes M5 tensor support. `serve --threads 4` controls HTTP workers; actual inference CPU threads are `--asr.backend.threads` (zero selects up to eight). MTP and DFlash speculative decoding are unavailable on Metal. The high-concurrency 128 GB server preset targets throughput, whereas Nemo submits one dictation at a time. See the pinned [platform support](https://github.com/NVIDIA/NeMo-Speech.cpp/blob/6a62501107e1894723880f1094a286ea996a0ec7/docs/salm/models.md), [configuration](https://github.com/NVIDIA/NeMo-Speech.cpp/blob/6a62501107e1894723880f1094a286ea996a0ec7/docs/salm/configuration.md), and [Metal device implementation](https://github.com/ggml-org/llama.cpp/blob/bd4f514db14d87fded667787a7a963bfbaa98e89/ggml/src/ggml-metal/ggml-metal-device.m).

The laptop was on AC power with `powermode = 2` for both AC and battery. High Power Mode was already enabled. Apple describes it as extra cooling capacity for sustained intensive workloads; these tests do not establish a benefit over Automatic Mode. See [Apple's power mode guidance](https://support.apple.com/en-us/101613).

## Delivery state

The combined performance/settings update is installed at `~/Applications/Nemo.app` with five-minute retention, parallel loading during recording, local JSON telemetry, trailing-period settings, launch-on-startup controls, and recording sound packs. Its executable matches the signed package, and code-signature verification passes. Active settings are `idle_unload_seconds = 300`, `preload_model = false`, `model_warmup = false`, `inference_threads = 8`, and `encoder_batch_size = 1`. The final app passed cold/warm acoustic transcription and visible paste into an unsent ChatGPT draft, which was cleared without submission. Both daemon permissions are authorized. The final executable's full isolated idle-expiry check also passed. See [combined installation verification](combined-install-verification.md) for exact evidence and coverage limits, including the unverified TextEdit target and isolated timer scope.

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
