# Setting up the MLX-Audio backend

The MLX-Audio engine with MTP speculative decoding measured **1.50–1.65x** faster than the included runtime on 5–100 second clips through the app's own path, with identical word errors. See [the engine comparison](engine-comparison-2026-11-09.md) for the measurements and their limits.

It is opt-in. The default stays `nemo-speech`; MLX-Audio runs only when `config.toml` names it, which `nemo backend use mlx-audio` does. MLX-Audio requires Apple silicon, macOS 26+, user-installed `uv`, Python 3.12+, and about 22 GB for its pinned NVFP4 weights plus about 27 GB of memory while loaded (28.5 GB peak). Nemo detects missing prerequisites and prints the remedy; it never installs tooling.

## Install and provision

```sh
nemo quit
cargo xtask install
nemo backend setup mlx-audio
```

Quit Nemo before installing: `cargo xtask install` refuses to replace a running app. The app bundle includes `mlx-serve.py`, its lockfile, and `mlx-runner.sh` in `Contents/Resources/runtime/`; `mlx_runtime` can remain unset. `nemo backend setup mlx-audio` downloads missing weights from the pinned manifest into the managed profile directory (`models/main`) and checks every recorded SHA-256 digest. Setup reports `verified` on success. Its dedicated downloader has a scoped network grant and is retired after the fetch; inference remains under the local-only sandbox. A second setup with usable weights performs local verification without starting a downloader. The bundled helper loads that managed profile, so deleting the development cache does not break the app.

## Select and run

```sh
nemo backend use mlx-audio
nemo quit
open -a "$HOME/Applications/Nemo.app"
nemo backend list      # * marks the selection; weights and engine should be ready
nemo warm              # load ahead of dictation
nemo status
```

`nemo backend use` writes the choice to `config.toml`; `nemo backend use nemo-speech` returns to the default. `nemo doctor` shows the engine in force and whether speculative decoding is active.

The one engine option is the draft length, 0–8 tokens per verification step. Eight is the default and the fastest measured; zero disables speculation. It is rejected for `nemo-speech`, which cannot speculate:

```toml
backend = "mlx-audio"
mtp_draft_tokens = 8
```

The bundled runner applies the wired-memory limit that keeps inference tails tight, and a 2 GB limit on MLX's buffer cache. Without the cache limit the engine grew from 30 GB after load to 38–40 GB after long clips. It `exec`s the interpreter so Nemo owns and reaps the engine process, and refuses to start a third engine. Do not bypass the runner.

## Return to the included backend

```sh
nemo backend use nemo-speech
nemo quit
open -a "$HOME/Applications/Nemo.app"
```

## One engine process, never more

Before and after any paired benchmark, check that no MLX engines leaked:

```sh
pgrep -f 'environments-v2/mlx-serve.*bin/python' | wc -l
```

The expected count after a run is zero. A/B benchmarks may use at most two engines concurrently; one GPU benchmark at a time. See the [benchmark skill](../.agents/skills/benchmark-nemo/SKILL.md) for measurement discipline.

## Verification status

HTTP corpus measurements do not establish stop-to-visible performance. An installed microphone, transcription, clipboard, and visible-insertion check is required before making an end-to-end claim. The current evidence and remaining gaps are tracked in [the experiment record](mlx-audio-experiment.md).

## Provisioning check

A fresh isolated data directory was provisioned from the public
`nvidia/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-NVFP4` repository: all 16 pinned
files downloaded, every digest verified, `backend setup` reported `verified`, and
a second run performed local verification only. The temporary 20 GB copy was then
deleted; no network-capable download helper remained.
