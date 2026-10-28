#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
scripts/check.sh
cargo build --release --locked
temporary=$(mktemp -d /tmp/nemo-inference-check.XXXXXX)
trap 'rm -rf "$temporary"' EXIT
target/release/nemo config > "$temporary/config.toml"
export NEMO_DATA_DIR="$temporary"
mkdir -p artifacts
phrase='The quick brown fox jumps over the lazy dog. This is a local speech recognition test on my laptop.'
say -v Samantha -o artifacts/sample.aiff "$phrase"
afconvert -f WAVE -d LEI16@16000 -c 1 artifacts/sample.aiff artifacts/sample.wav
target/release/nemo transcribe artifacts/sample.wav > artifacts/sample-transcript.txt
python3 - <<'PY'
import pathlib
import re
expected = 'The quick brown fox jumps over the lazy dog. This is a local speech recognition test on my laptop.'
actual = pathlib.Path('artifacts/sample-transcript.txt').read_text().strip()
normalize = lambda text: re.findall(r"[a-z0-9]+", text.lower())
assert normalize(actual) == normalize(expected), f'Transcript mismatch: {actual!r}'
print('Verified real Nemotron transcription:', actual)
PY
cargo test --release --locked real_model_transcribes_repeated_audio -- --ignored --nocapture
