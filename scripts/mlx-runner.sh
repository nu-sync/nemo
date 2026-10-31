#!/bin/sh
# Launch the MLX helper the way Nemo does: a fixed environment and no network.
#
# NEMO_MLX_WIRED pins the ~25 GB of weights in wired memory. Measured on this
# machine, that is the whole difference between a fast median and a shippable one:
# identical p50, and the occasional 1.8x tail disappears (30 s p95 1186 -> 655 ms,
# 20 s 701 -> 456 ms). Without it macOS can evict pages between two dictations and
# the next request faults them back.
set -eu
UV="$(command -v uv)"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
export NEMO_MLX_WIRED="${NEMO_MLX_WIRED:-25000000000}"
exec "$UV" run --locked "$SCRIPT_DIR/mlx-serve.py" "$@"
