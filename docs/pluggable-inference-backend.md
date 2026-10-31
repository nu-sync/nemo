# Pluggable inference backends

How to make the inference engine a replaceable component, so a different runtime,
engine, or model can be measured against the same end-to-end suite instead of
argued about from documentation.

Status: proposal. No code written. The measurements it rests on are in
`artifacts/performance/framing-20261030-210101-91766/` and are cited inline.

Read first: [the inference pipeline](inference-pipeline.md) (where the
milliseconds are) and [speech benchmarking](benchmarking.md) (how they are
measured).

## 1. The question we cannot answer today

Inference touched 81% of the latency slope (24.0 of 29.7 ms per audio second) is
single-sequence decode at 8.38 ms per transcript token. Everything downstream of
that number — which engine to use, whether to drop HTTP, whether speculative
decoding pays — is currently decided by reading upstream documentation, because
swapping the engine means editing `src/backend.rs` and rebuilding the app.

That is the whole argument for this document: **the engine choice is a
consequential decision being made without a measurement harness.**

Three concrete questions, all unanswered:

1. Does MTP speculative decoding actually beat 8.38 ms/token on this laptop?
2. Does removing the HTTP hop (the in-process C ABI proven by the native spike on
   `codex/explicit-lifecycles`) matter? *Answered, and no: the request floor is
   0.53 ms at 5 s and 4.31 ms at 60 s of audio, so HTTP is 0.19–0.22% of a
   request.* Recorded here because it closes the question.
3. If we change model or quantization, what happens to both latency and WER —
   measured on the same corpus, in the same session, with the same gates?

## 2. What the evidence changes about the plan

The MTP path is much cheaper than the earlier write-up assumed. Three findings,
each reproduced locally.

**The MTP head is already inside the 19.68 GB GGUF we own.** The file carries a
16-tensor block 52 (`blk.52.nextn.*` plus a full MoE layer) totalling **0.78 GB**,
and `nemotron_h_moe.nextn_predict_layers = 1`. The runtime loads it and says so:

```text
model has unused tensor blk.52.nextn.eh_proj.weight (size = 15353856 bytes) -- ignoring
model has unused tensor blk.52.ffn_up_exps.weight (size = 359202816 bytes) -- ignoring
```

**So a new 22 GB download is not required for the GGUF path.** The 21.6 GB NVFP4
checkpoint is needed only for MLX-Audio, and only because MLX cannot read GGUF at
all. Anything that keeps the GGUF — including our own runtime — needs zero new
weights, zero new license, and inherits the existing hashes and WER baseline.

**The block is one predicate, not a missing feature.** Asking the current runtime
for drafts the direct way:

```sh
nemo-speech serve --asr-model "$MODEL" --device metal --asr.salm.mtp_draft 4 ...
# [salm] path=upstream llama.cpp mtp_draft=0 slots=1 n_batch=512
```

The flag is parsed and overridden to `0`, and the "carries no NextN (MTP) layers"
warning does *not* fire — so `llama_model_n_layer_nextn() > 0` and the head is
detected. The override is `salm_engine.cpp`:

```cpp
// MTP rolls drafts back through the batched path's replay rows; on upstream
// llama.cpp the engine compacts slot ids, which cannot carry the drafter's state.
if (!fast)
    cfg.mtp_draft = 0;
```

**The speculative machinery itself is backend-agnostic in the pinned llama.cpp.**
`SALM_FAST_PATH`
requires both `NEMO_SPEECH_GGML_PATCHED` and `GGML_USE_CUDA`, so it is off on
Metal — but what it guards is NeMo's *batched serving engine*, not llama.cpp's
capability:

| Piece | Where it lives | Gated to CUDA? |
| --- | --- | --- |
| `ctx_type`, `LLAMA_CONTEXT_TYPE_MTP` | `llama.cpp/include/llama.h`, `src/llama-context.cpp` | no |
| `n_outputs_max_per_seq` | public `llama_context_params` field | no |
| `common_speculative` MTP drafter | `llama.cpp/common/speculative.cpp` | **no CUDA reference in the file** |
| `serving.mtp_replay` | NeMo `patches/`, inside `#if SALM_FAST_PATH` | yes, and it is the only gated piece |

**Consequence for the plan:** the first real experiment is not "download 22 GB
and try MLX". It is *drive the pinned llama.cpp's own MTP on Metal with the
weights we already have*, which needs no new assets and answers the accept-rate
question directly.

## 3. Design

### 3.1 The boundary stays a process

Keep the helper-process boundary for every backend. A trait is a compile-time
interface, not a licence to link MLX, Python, or 30 B of Metal state into the GUI
process. Cancellation-by-reap and crash isolation are properties PR #9 is built
on; a pluggable backend must not quietly remove them.

### 3.2 Two crates

```text
crates/
  nemo-inference/          # trait, audio/transcript types, backend registry, identity
    src/lib.rs
    src/id.rs              # BackendId, EngineKind, WeightSet, Options, Identity
    src/serve.rs           # the HTTP contract as a reusable type
  nemo-inference/backends/
    nemo_speech.rs         # current: spawn `nemo-speech serve`, HTTP
    nemo_native.rs         # the C ABI from the spike, in-process helper
    mlx_audio.rs           # Python helper process
    llama_cpp.rs           # patched GGUF runtime with MTP
```

`nemo-inference` has no I/O of its own and no heavy dependencies: it defines the
trait, the value types, and the identity record.

### 3.3 The trait

Deliberately narrow, synchronous, and cancellation-shaped like the existing
`Backend`, because the worker thread already works that way:

```rust
pub struct Audio { pub wav: Vec<u8>, pub seconds: f64 }
pub struct Transcript { pub text: String, pub request_ms: f64, pub reused_model: bool,
                        pub model_load_ms: Option<f64> }

pub trait InferenceBackend: Send {
    fn identity(&self) -> Identity;
    fn loaded(&self) -> bool;
    fn prepare(&mut self, canceled: &AtomicBool) -> Result<()>;   // load + health
    fn transcribe(&mut self, audio: Audio, canceled: &AtomicBool) -> Result<Transcript>;
    fn unload(&mut self) -> bool;
}
```

Two rules that keep this from becoming an abstraction tax:

- **No generic parameter threading.** `Box<dyn InferenceBackend>` chosen once at
  worker start. The event loop must not learn what a backend is.
- **No backend-specific methods on the trait.** Anything only one backend can do
  (word timestamps, streaming partials) stays behind its own type until two
  backends need it.

### 3.4 Identity is mandatory, not optional

Every backend must return an `Identity` that covers: engine kind and revision,
weight files with hashes, encoder with hash, and the experiment options actually
in force. `scripts/benchmark-corpus.py` already fingerprints runtime + decoder +
colocated encoder; a backend that cannot produce that record cannot participate
in an A/B, because a paired comparison with unknown identities is not a
comparison.

This is the single most important requirement in this document. It is also the
one most likely to be dropped for convenience.

### 3.5 The shim is the testing lever

The harness spawns a `--candidate-runtime` executable that speaks
`serve`/`/health`/`/v1/audio/transcriptions`. Rather than teach the harness about
backends, ship one binary that satisfies that contract and dispatches on config:

```sh
nemo-inference-shim serve --asr-model "$MODEL" --backend nemo-speech
nemo-inference-shim serve --asr-model "$MODEL" --backend llama-cpp --mtp-draft 4
nemo-inference-shim serve --asr-model "$MODEL" --backend mlx-audio
```

Then every existing measurement works on every backend unchanged:

```sh
uv run --no-project scripts/benchmark-corpus.py \
  --candidate baseline:8:512:1:0 --candidate mtp4:8:512:1:0 \
  --candidate-runtime "mtp4=/abs/path/nemo-inference-shim" \
  --suite typical --rounds 5 --repeats 3 --output ...
```

The same binary is what the app spawns, so "measured" and "shipped" cannot drift
apart — which is the failure this whole document exists to prevent.

## 4. Plan of action

Sequenced so each step ends in something verifiable, and each step is worth
stopping after.

### Milestone 0 — measure MTP on Metal, no download (~half a day)

Not a refactor. The point is to find out whether the rest of the plan is worth
doing.

- Reconfigure the existing Metal build tree
  (`vendor/NeMo-Speech.cpp/build/metal-server`, already `GGML_METAL=ON`) with
  `NEMO_SPEECH_BUILD_EXAMPLES=ON` and build `llama-cli`.
- Run our current GGUF on the frozen 5/20/60 s WAVs, greedy, on Metal, twice:
  with the MTP drafter off and on, for `--draft-max` 2/4/8.
- Record: tokens/s, tokens per verification step, accept rate, transcript.
- Gate: **≥1.5x on decode with an unchanged transcript**, else stop and report
  that speculation is not worth a port. That number decides milestones 1–4.

If `llama-cli` cannot drive this model's MTP, fall back to the same measurement
through a ~50 line driver against `common_speculative`. Either way the artifact
is a script in `scripts/` with its output under `artifacts/`, not a one-off shell
history.

### Milestone 1 — extract the crate, change no behaviour

Pure move. `src/backend.rs` becomes `crates/nemo-inference` plus the
`nemo_speech` backend, with `main.rs` holding `Box<dyn InferenceBackend>`.

Verification: `cargo xtask check`; the existing lifecycle tests unchanged; one
real dictation through the installed path. **No behaviour change is the
acceptance criterion** — if any test needs editing to pass, the extraction
changed behaviour.

### Milestone 2 — the shim and the second backend

`nemo-inference-shim` with `nemo-speech` and `llama-cpp` backends. The second one
is the point: it is the first time two backends sit behind one contract.

Verification: `benchmark-corpus.py` A/Bs both behind the shim and reproduces the
baseline within noise (an A/A through two code paths). This is where identity
enforcement gets tested for real.

### Milestone 3 — the MTP port, only if Milestone 0 passed

Two options, and the order matters:

1. **Patch the pinned llama.cpp** to carry single-sequence drafter state on the
   non-batched path, using `llama_state_seq_get_data` / `llama_state_seq_set_data`
   and `llama_memory_seq_rm` (all already public in `llama.h`). Feasible because
   Nemo runs `max_sequences = 1`: no slot compaction, and the ~48 MB recurrent
   state save/restore is ~0.1 ms at 530 GB/s. Adopts with **no** new weights, no
   new boundary, no accuracy baseline reset.
2. **MLX-Audio**, which already implements MTP, at the cost of 21.6 GB of new
   NVFP4 weights, a Python engine, ~24.4 GB resident, and a WER re-gate.

Option 1 first is the strongly preferred reading of the evidence: it is the same
speedup mechanism, and it keeps the GGUF, the license, the hashes, and the
process boundary. Option 2 stays available as the independent cross-check if
option 1 stalls, and both are reachable through the same shim.

### Milestone 4 — model and quantization comparisons

Only now is it cheap: the corpus A/B, the WER gates, and the identity record
already work for any backend. A model swap becomes a config line rather than a
branch.

## 5. Decisions to make explicitly

- **HTTP stays.** The C ABI is worth doing for cancellation and crash isolation,
  not latency, and now we have the number (≤4.31 ms) to say so.
- **No streaming in the near term.** This SALM model refuses streamed audio, so
  `stop → visible` cannot be attacked by overlapping decode with recording. That
  door needs a different recognition model, not a different backend.
- **The 1 s ceiling is the metric.** `(935 ms − fixed) / slope` is the number to
  move: 26.9 s of dictation today. At decode 1.9x it becomes 43.6 s, at 2.8x
  56.1 s.
- **One backend ships at a time.** The registry may hold several; only one is
  selected by default and covered by the installed verification.

## 6. What has not been done

Stated plainly, because the earlier write-up drifted toward "plausible":

- **No candidate engine has been executed.** No MLX-Audio install, no weight
  download, no `transcribe-cpp` build. What exists is the physics (530 GB/s peak,
  283 GB/s achieved), the transport floor, and the source-level gate analysis.
- **The MTP accept rate on this model is unmeasured on any backend.** It is the
  one number the whole plan hinges on, and Milestone 0 exists to get it.
- **MLX-Audio's batch-1 MoE efficiency is unmeasured.** It faces the same
  bandwidth wall; if its kernels sit at 53% of peak like ours, NVFP4 alone buys
  11% and only speculation matters.
- **`llama-cli` has not been built** from the vendored tree, so "it can drive
  MTP on Metal" is an inference from the public API surface, not a run.

## 10. Multi-device delivery

The constraint that decides the engine choice is not speed, it is provisioning.

Nemo provisions in exactly three places: the model manifest is **embedded at
compile time** (`include_str!("../models/manifest.json")`, one pinned revision,
`Store::decoder()` = `files[0]`), `nemo model download` fetches it with per-file
SHA-256 and resume, and `cargo xtask install` builds, signs, installs and confirms
the app. One app bundle, one model cache, one installer, verifiable on any machine.

**Anything that adds a second provisioning path breaks that**, and a second laptop
is where it breaks first:

| | current runtime | MLX sidecar |
| --- | --- | --- |
| new download | none | 21.6 GB into a cache the model store does not own |
| toolchain on the device | none (bundled) | uv + CPython + PyPI resolution |
| verifiable by `nemo model verify` | yes | no |
| new license in the tree | no | OpenMDW-1.1 |
| works on a locked-down work laptop | yes | only if PyPI and git are reachable |
| cold-load regression | none | 42% of dictations, median +2.16 s |

**Route A adds nothing.** The 1.55x arrives as a new binary inside the existing
bundle: same GGUF, same manifest, same download, same installer, same cache, no
config migration. Its setup story on the second laptop is *unchanged*, which is the
only version of this that stays easy in a year.

### Configuration shape

One schema, with backend-specific values validated by the backend that owns them:

```toml
# config.toml — identical meaning on every device
runtime = ".../Contents/Resources/runtime/nemo-speech"
model = ".../NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-AD-IQ4_NL.gguf"
mtp_draft_tokens = 8      # 0 disables; 1-8 is the model's supported range
idle_unload_seconds = 300
preload_model = true
```

Three rules, each learned from something that already went wrong here:

1. **Never silently ignore an unsupported option.** `--asr.salm.mtp_draft 4` was
   parsed and overridden to 0 with no error, which is how this whole investigation
   started. An unsupported value must fail loudly at load.
2. **Every non-default option appears in the identity record.** Otherwise a config
   difference becomes an unrecorded variable in an A/B.
3. **`nemo doctor` must report the engine identity and capabilities** — revision,
   weight hashes, and whether speculative decoding is active — so each device can
   verify itself instead of being diagnosed by bench mark.

### Per-device checklist

Both machines: Apple Silicon, macOS 26+, same app build. Then

```sh
cargo xtask install          # or the packaged app for a non-developer machine
nemo model verify            # existing; unchanged by route A
nemo doctor                  # engine identity + capabilities
```

The model downloads once per machine (21.35 GB, hash-checked, resumable). No
further setup differs between the two laptops.

### What to expect, cumulatively

| | today | cheap wins (§11) | + route A |
| --- | ---: | ---: | ---: |
| fixed per request | 135 ms | 105–110 ms | 105–110 ms |
| slope | 29.70 ms/s | ~29.7 | **~19.3 ms/s** |
| 1 s ceiling | 26.9 s | ~28 s | **~43 s** |
| p95 (25.8 s) | 901 ms | ~875 ms | **~605 ms** |

Route A's row uses the measured 1.55x; the middle column is a target, not a result.

## 11. Cheap wins that need no new provisioning

Ordered by payoff per unit of risk, all inside the current runtime and manifest.

1. **The 135 ms fixed cost.** It is 39% of a 5 s dictation and 7% of a 60 s one,
   and only ~0.5–4.3 ms of it is transport. The remaining ~130 ms is inside the
   request and has never been broken down. This is the largest non-decode lever in
   the product and it needs no new dependency at all.
2. **The endpoint/prompt difference.** `/v1/chat/completions` measured consistently
   ~23–25 ms faster than `/v1/audio/transcriptions` on identical audio (5 s 285 vs
   310, 20 s 735 vs 757, 60 s 1891 vs 1914). That is ~8% of a short dictation for a
   change in `src/backend.rs`. Treat it as a hypothesis from two datasets, not a
   result: it needs a paired check, and the two paths use different prompts, so
   token counts and transcripts must be compared, not just timings.
3. **A different quantization of the same model.** The only weight-side lever that
   is not speculation, and it can only reach maybe 5–15% because our non-speculative
   decode already matches MLX's. It is *not* a config line today: the manifest
   embeds one revision and `Store::decoder()` is `files[0]`, so multiple variants
   need `Manifest`/`Store` keyed by model id first. Worth doing only if (1) and (2)
   are exhausted, and it costs a download plus a full WER re-gate.
