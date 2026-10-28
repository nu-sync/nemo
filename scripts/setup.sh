#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'Requires an Apple Silicon Mac.' >&2; exit 1; }
command -v cargo >/dev/null || { echo 'Install Rust: https://rustup.rs' >&2; exit 1; }
command -v brew >/dev/null || { echo 'Install Homebrew: https://brew.sh' >&2; exit 1; }
missing=()
for formula in cmake ninja sentencepiece abseil; do
    brew list --versions "$formula" >/dev/null 2>&1 || missing+=("$formula")
done
if (( ${#missing[@]} )); then HOMEBREW_NO_AUTO_UPDATE=1 brew install "${missing[@]}"; fi
scripts/build-runtime.sh
cargo build --release --locked
if python3 -c 'import json,pathlib,sys; m=json.loads(pathlib.Path("models/manifest.json").read_text()); sys.exit(not any((pathlib.Path("models") / f["name"]).is_file() for f in m["files"]))'; then
    target/release/nemo model migrate --from "$PWD/models"
fi
target/release/nemo model download
scripts/install.sh
