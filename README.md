# Nemo

Local voice dictation for Apple Silicon Macs. Tap **right Option** to record, then tap it again to transcribe and paste into the focused app.

Audio is processed on your Mac. No account or API key is required.

After installation and model download, dictation works offline. The controller and
inference runtime deny external networking; a constrained helper fetches only the
pinned model. See the [local-only audit](docs/local-only-audit.md) for controls,
license checks, verification, and the separate network requirements of source installation.

## Engines

Nemo runs one of two local inference engines, chosen in `config.toml` without a rebuild:

| `backend` | What it is | Needs | Memory while loaded |
| --- | --- | --- | --- |
| `nemo-speech` | The default: the bundled NeMo-Speech.cpp runtime. Runs after a standard install. | Nothing extra | about 20 GB |
| `mlx-audio` | Opt-in: MLX-Audio with speculative decoding. **1.50–1.65x faster** on 5–100 second dictations, same word errors. | `uv`, Python 3.12+, 21.6 GB of MLX weights | about 27 GB |

`nemo-speech` is the default. `mlx-audio` is opt-in:
[set it up](docs/mlx-audio-setup.md) with `nemo backend setup mlx-audio`, then select it
with `nemo backend use mlx-audio`. If it fails to load, Nemo falls back to `nemo-speech`.
Numbers are from the
[November 9 engine comparison](docs/engine-comparison-2026-11-09.md) on an M5 Max.

## Install

You need **macOS 26 or newer**, an Apple Silicon Mac, [Rust](https://rustup.rs), and [Homebrew](https://brew.sh). Allow about **22 GB** for either model download, plus space for build files.

Install Apple’s Command Line Tools if you haven’t already:

```sh
xcode-select --install
```

Finish that installer, then open Terminal and run:

```sh
cargo install --locked --git https://github.com/nu-sync/nemo.git nemo-dictate
```

Choose one of the following setup paths before opening Nemo.

### Default engine

```sh
nemo
```

The first `nemo` run installs the build dependencies through Homebrew, builds the inference runtime, and creates `~/Applications/Nemo.app`. Nemo appears as a **dot in the menu bar** and automatically downloads the Nemotron model. Open its menu to check the download.

Setup happens once. Later, run `nemo` or open **Nemo** from `~/Applications`.

### MLX-Audio only

Install [`uv`](https://docs.astral.sh/uv/) and make sure `python3 --version` reports 3.12 or newer. Run the following before opening Nemo for the first time:

```sh
nemo install
uv sync --locked --script "$HOME/Applications/Nemo.app/Contents/Resources/runtime/mlx-serve.py"
nemo backend setup mlx-audio
nemo backend use mlx-audio
```

Open `~/Library/Application Support/Nemo/config.toml` and replace the existing `model` line with:

```toml
model = "/dev/null"
```

This setting skips the automatic download of the default GGUF weights. MLX-Audio loads the weights downloaded by `nemo backend setup mlx-audio`.

```sh
open -a "$HOME/Applications/Nemo.app"
nemo backend list
```

Look for `* mlx-audio: weights ready`. The app bundle includes both runtimes. On a fresh install with these steps, the list shows the default weights as `not provisioned`. The menu bar may report **Transcription blocked: model missing** because its readiness check looks for GGUF weights. Use `nemo backend list` to check the MLX weights.

If you later switch to `nemo-speech`, remove the `model` line to restore its default download path.

## Dictate

Wait for the model download to finish, then:

1. Enable **Nemo** in **System Settings → Privacy & Security → Accessibility**. This enables right Option and automatic paste.
2. Click where you want the text to go.
3. Tap **right Option**. Allow **Microphone** access when prompted. Speak when **Recording** appears at the bottom of the screen.
4. Tap **right Option** again. The indicator changes to **Transcribing**, then disappears when Nemo finishes copying and pasting your words.

Keep the destination app focused until paste finishes. **Cancel** in Nemo’s menu discards the current operation. **Quit** closes Nemo and releases the model.

The menu bar dot is gray when idle, red when recording, and amber during setup or transcription. Left Option and Option used with another key keep their normal behavior.

The indicator appears above the Dock on the display containing your pointer. It lets clicks pass through and keeps your destination app focused.

## Settings

Choose **Settings…** from Nemo’s menu, or run `nemo settings`.

| Setting | What it does |
| --- | --- |
| Remove trailing period | Removes the final `.` from each transcript. Off by default. |
| Launch on startup | Opens Nemo when you log in. macOS may ask you to approve the login item. |
| Sound pack | Choose Handy Marimba or macOS Recording. Preview the start and stop sounds. |

These settings save immediately. For other options, choose **Open configuration** from the menu. The file is `~/Library/Application Support/Nemo/config.toml`; restart Nemo after editing it.

The model loads while you record and unloads after **60 idle seconds** by default. To keep it ready for five minutes, change this value in your configuration:

```toml
idle_unload_seconds = 300
```

See [advanced configuration](docs/development.md#configure) for shortcuts, recording limits, custom models, and inference tuning.

## Update

Finish your current dictation, then run:

```sh
cargo install --locked --force --git https://github.com/nu-sync/nemo.git nemo-dictate
nemo
```

Nemo keeps your model and settings and retains the previous app in `~/Applications/.Nemo.backups/`. It refuses to replace an app that is recording or transcribing.

If an update changes the app’s signing identity, the installer stops before replacing it. To accept that update:

```sh
nemo install --allow-permission-reset
nemo
```

You may need to grant Microphone and Accessibility access again.

## Troubleshooting

```sh
nemo doctor
nemo status
```

`doctor` reports model and runtime paths, the input device, and permissions for both the command and the running app. Logs are in `~/Library/Application Support/Nemo/daemon.log` and `inference.log`.

- **No recording or paste:** check Nemo’s Microphone and Accessibility entries in System Settings.
- **Interrupted model download:** choose **Download model** in the menu, or quit Nemo and run `nemo` again. Partial downloads are retained.
- **Build failed:** fix the reported dependency or build error and run `nemo` again. Completed build files are retained.

## Development

Build and install the current checkout with Rust:

```sh
cargo xtask check
cargo xtask install
```

The update command retains the previous app and refuses to interrupt dictation. Python and Bash are not required for building or installing Nemo.

See [development and advanced configuration](docs/development.md) for source builds, CLI commands, packaging, tests, and the application design. See [contributing](CONTRIBUTING.md) and [release preparation](docs/releasing.md) for development and publication requirements. The project’s [verify-nemo skill](.agents/skills/verify-nemo/SKILL.md) exercises real speaker playback, microphone capture, transcription, paste, and idle unloading.

## Licenses

Nemo is [MIT licensed](LICENSE). It uses [NVIDIA Nemotron 3.5 Transcribe](https://huggingface.co/nvidia/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B), the [AtomicChat IQ4_NL conversion](https://huggingface.co/AtomicChat/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-GGUF), and [NeMo-Speech.cpp](https://github.com/NVIDIA/NeMo-Speech.cpp). Model, runtime, and sound licenses are included in `Nemo.app/Contents/Resources/licenses`.
