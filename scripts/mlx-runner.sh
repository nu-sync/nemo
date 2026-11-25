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

SCRIPT="${NEMO_MLX_SCRIPT:-$(cd "$(dirname "$0")" && pwd)/mlx-serve.py}"
export NEMO_MLX_WIRED="${NEMO_MLX_WIRED:-25000000000}"
# NEMO_MLX_CACHE bounds MLX's buffer cache. Unbounded, freed buffers pile up with every new
# audio length. A 1 GB limit kept the short-recording helper near 26 GB on the M5 Max.
export NEMO_MLX_CACHE="${NEMO_MLX_CACHE:-1000000000}"

# The sandbox has a fixed PATH. Also support uv's standalone installer and Nix.
UV="$(command -v uv || true)"
if [ -z "$UV" ]; then
    for candidate in "$HOME/.local/bin/uv" "$HOME/.nix-profile/bin/uv"; do
        if [ -x "$candidate" ]; then
            UV="$candidate"
            break
        fi
    done
fi
if [ -z "$UV" ]; then
    echo "mlx-runner: uv is required to locate the locked helper environment" >&2
    exit 1
fi

# The cache can contain several helpers with incompatible dependency versions.
if ! "$UV" sync --quiet --offline --locked --script "$SCRIPT"; then
    printf 'mlx-runner: prepare the locked environment with uv sync --locked --script "%s"\n' "$SCRIPT" >&2
    exit 1
fi
PY="$("$UV" python find --offline --script "$SCRIPT")"
exec "$PY" "$SCRIPT" "$@"
