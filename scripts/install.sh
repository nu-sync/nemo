#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --locked
python3 scripts/bundle.py
exec python3 scripts/install_app.py "$@"
