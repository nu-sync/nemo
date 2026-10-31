#!/bin/sh
# Launch the MLX-Audio helper as ONE process that a supervisor can reap.
#
# Two rules here, both learned from a leak that cost 100+ GB:
#
# 1. `exec` the interpreter. Going through a shell or through `uv run` makes the engine a
#    grandchild, and killing the process the supervisor owns then leaves the engine alive
#    with ~25 GB resident - and a wired limit means it cannot be paged out.
# 2. Refuse to start a third engine. An A/B needs at most two (the control and the
#    candidate), so anything beyond that is a leak, not work. Failing loudly is better
#    than silently competing for memory.
#
# NEMO_MLX_WIRED pins the weights in wired memory: identical p50, and the intermittent
# 1.8x tail disappears (30 s p95 1186 -> 655 ms).
set -eu

CAP="${NEMO_ENGINE_CAP:-2}"
# Match the interpreter running the helper, not any shell that merely mentions the path:
# a `pgrep` on the bare filename counts the terminal command that started the comparison.
running() { pgrep -f "bin/python .*mlx-serve\.py" 2>/dev/null | wc -l | tr -d ' '; }

if [ "$(running)" -ge "$CAP" ]; then
    echo "mlx-runner: refusing to start - $CAP engine process(es) already running (cap $CAP)." >&2
    echo "mlx-runner: reap them first: pkill -f mlx-serve.py" >&2
    exit 75
fi

CACHE="${NEMO_MLX_CACHE:-$HOME/Library/Caches/nemo-mlx}"
SCRIPT="${NEMO_MLX_SCRIPT:-$(cd "$(dirname "$0")" && pwd)/mlx-serve.py}"
export NEMO_MLX_WIRED="${NEMO_MLX_WIRED:-25000000000}"

# The uv-managed environment for this script's lock, so the engine process is the one the
# supervisor spawns. Falls back to `uv run` only if that environment is missing.
PY=""
for candidate in "$HOME"/.cache/uv/environments-v2/mlx-serve-*/bin/python; do
    [ -x "$candidate" ] && PY="$candidate" && break
done

if [ -n "$PY" ]; then
    exec "$PY" "$SCRIPT" "$@"
fi
echo "mlx-runner: no prepared environment for $SCRIPT; falling back to 'uv run' (engine becomes a child of uv)" >&2
exec "$(command -v uv)" run --locked "$SCRIPT" "$@"
