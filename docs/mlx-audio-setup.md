# Setting up the MLX-Audio backend

What this is: an **experimental** second engine that measured **1.58-1.70x** faster
than the included runtime on the frozen corpus, with unchanged word error rate. See
[the MLX-Audio experiment](mlx-audio-experiment.md) for the numbers and their limits.

What this is not: a shipped feature. There is no installer, no download command and
no fallback yet, so the steps below are manual and the state they leave on disk is
machine-specific. The included engine stays the default and keeps working throughout.

## Requirements

- Apple silicon, macOS 26 or newer.
- **`uv` on PATH** — a hard requirement, installed by you (`brew install uv`).
  Check with `nemo backend list`; the alternative row reports `tools ready` or the
  exact command that fixes it.
- Python 3.12 or newer, which `uv` can fetch itself.
- About **22 GB** free for the NVFP4 weights, plus ~25.5 GB of memory while loaded.

## 1. Install the app

```sh
nemo quit                       # the installer refuses to replace a running app
cargo xtask install
cargo install --path . --locked --force   # optional: refresh `nemo` on PATH
```

Quitting first matters: `cargo xtask install` reports
"The responding daemon does not match the installed build" and exits without
replacing anything if a daemon is running.

`~/.cargo/bin/nemo` and `~/Applications/Nemo.app/Contents/MacOS/nemo` are separate
binaries that can drift. `nemo backend list` failing with
`unrecognized subcommand 'backend'` means you are running a stale one.

## 2. Put the engine and the weights in place

The helper ships as scripts, so keep them somewhere durable — not in a worktree:

```sh
mkdir -p ~/Library/Caches/nemo-mlx
cp scripts/mlx-serve.py scripts/mlx-serve.py.lock scripts/mlx-runner.sh \
   ~/Library/Caches/nemo-mlx/
chmod +x ~/Library/Caches/nemo-mlx/mlx-runner.sh
~/Library/Caches/nemo-mlx/mlx-runner.sh serve --port 8099 &
```

The first start resolves the pinned Python environment and needs network; later
starts do not. `curl localhost:8099/health` should report the engine and draft depth.

Weights — the profile is pinned in `models/manifest.json` (16 files, 21.6 GB, every
digest recorded) but **nothing can download through the app yet**. Fetch them and
link them into the profile's revision directory:

```sh
hf download nvidia/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-NVFP4 \
   --local-dir ~/Library/Caches/nemo-mlx/NVFP4
mkdir -p ~/Library/Application\ Support/Nemo/models/main
for f in ~/Library/Caches/nemo-mlx/NVFP4/*; do
  ln -sf "$f" ~/Library/Application\ Support/Nemo/models/main/"$(basename "$f")"
done
nemo backend setup mlx-audio    # verifies prerequisites, then hashes the weights
```

## 3. Select it

`nemo backend use` writes the selection but not the helper path, so add it once:

```toml
# ~/Library/Application Support/Nemo/config.toml
backend = "mlx-audio"
mlx_runtime = "/Users/<you>/Library/Caches/nemo-mlx/mlx-runner.sh"
```

```sh
nemo quit; open -a "$HOME/Applications/Nemo.app"
nemo backend list      # * marks the selection; both rows should read ready
nemo warm              # loads the engine (~2.1 s measured)
nemo status            # "Ready - Right Option - Model loaded"
```

Then dictate. `mlx-runner.sh` applies the wired limit that keeps the tail tight; do
not launch the helper without it.

## 4. Going back

```sh
nemo backend use nemo-speech
nemo quit; open -a "$HOME/Applications/Nemo.app"
```

The included engine needs no weights beyond the ones already present and is
unaffected by anything above.

## Known gaps in this setup

- **No fallback.** If `uv` disappears or the engine fails to load, dictation is not
  ready rather than slower; nothing drops back to the included engine.
- **Nothing is verified end to end.** No installed recording or visible-insertion
  check has run on this artifact, so no stop-to-visible claim exists.
- **The engine is outside the app bundle**, so an app update does not carry it and a
  cache cleanup deletes it.
- **Accuracy evidence is 4 of 12 fixtures** at five visits and one repeat.
