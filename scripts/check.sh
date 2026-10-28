#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
[[ "$(uname -s)" == Darwin ]] || { echo 'Checks require macOS (AppKit and CoreAudio).' >&2; exit 1; }
temporary=$(mktemp -d /tmp/nemo-check.XXXXXX)
trap 'rm -rf "$temporary"' EXIT
export NEMO_DATA_DIR="$temporary/data"
unset NEMO_RUNTIME
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
python3 -m unittest discover -s scripts/tests -v
for script in scripts/*.sh; do bash -n "$script"; done
