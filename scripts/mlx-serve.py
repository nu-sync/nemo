#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12,<3.14"
# dependencies = [
#     "mlx==0.32.4",
#     "mlx-audio @ git+https://github.com/Blaizzy/mlx-audio.git@7a0986280fe295f4cf40f64164bb23abed416a22",
#     "numpy",
#     "huggingface-hub",
# ]
# ///
"""MLX-Audio with MTP speculative decoding behind Nemo's engine contract.

Speaks the loopback contract Nemo's `Backend` and `scripts/benchmark-corpus.py` drive:
`GET /health`, `POST /v1/audio/transcriptions` (multipart `file`, `response_format=json`)
returning {"text": ...}, authorized by the bearer key the launcher passes.

    uv run --locked scripts/mlx-serve.py serve --port 8080 --api-key k --mtp-draft 8

`--mtp-draft` (0-8, 0 disables speculation) is the one engine option, and an
unsupported value fails at startup instead of being ignored. The benchmark harness
also passes `nemo-speech` tuning flags to every candidate; those are skipped.

Needs the pinned NVFP4 profile (`nemo backend setup mlx-audio`). Bundled with the app
and selected with `backend = "mlx-audio"`; launch it through `mlx-runner.sh`.
"""
import argparse
import hmac
import json
import os
import sys
import tempfile
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

MANAGED = Path.home() / "Library/Application Support/Nemo/models/main"
MAX_DRAFT = 8  # the model's MTP head drafts at most 8 tokens per verification step
WIRED = int(os.environ.get("NEMO_MLX_WIRED", "0") or 0)
CACHE_LIMIT = int(os.environ.get("NEMO_MLX_CACHE", "0") or 0)
WARMUP = os.environ.get("NEMO_MLX_WARMUP") == "1"
COMPILE = os.environ.get("NEMO_MLX_COMPILE") == "1"


def weights(argument):
    """The profile directory to load, preferring what the caller pinned.

    The installed app passes the managed profile directory. A benchmark passes the
    decoder GGUF because that is the shared runtime contract, so fall back to the
    pinned managed profile. An unprovisioned profile fails rather than silently
    loading some other copy of the weights.
    """
    for candidate in (argument, os.environ.get("NEMO_MLX_MODEL"), MANAGED):
        if candidate and (Path(candidate) / "model.safetensors.index.json").is_file():
            return Path(candidate)
    raise SystemExit(f"mlx-serve: no NVFP4 weights at {MANAGED}; run: nemo backend setup mlx-audio")


def parse(argv):
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8080)
    parser.add_argument("--api-key", default="")
    parser.add_argument("--asr-model", default="")
    parser.add_argument("--mtp-draft", type=int, default=int(os.environ.get("NEMO_MLX_DRAFT", MAX_DRAFT)))
    args, _unknown = parser.parse_known_args(argv)
    if not 0 <= args.mtp_draft <= MAX_DRAFT:
        raise SystemExit(f"mlx-serve: --mtp-draft must be 0-{MAX_DRAFT}, got {args.mtp_draft}")
    return args


def extract_file(body: bytes, content_type: str):
    boundary = content_type.split("boundary=", 1)[1].strip().strip('"').encode()
    marker = b"--" + boundary
    for part in body.split(marker):
        if b'name="file"' not in part:
            continue
        head, _, rest = part.partition(b"\r\n\r\n")
        if b'name="file"' not in head:
            continue
        return rest[: rest.rfind(b"\r\n")] if rest.endswith(b"\r\n") else rest
    raise ValueError("no file part")


def transcribe(path, max_tokens=2048):
    """Greedy decoding: speculation is only exact at temperature 0."""
    return Handler.model.generate(
        path,
        speculative=Handler.draft > 0,
        num_draft_tokens=Handler.draft or 1,
        temperature=0,
        max_tokens=max_tokens,
    )


class Handler(BaseHTTPRequestHandler):
    model = None
    key = ""
    draft = MAX_DRAFT

    def log_message(self, *a):
        pass

    def reply(self, code, payload):
        body = json.dumps(payload).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path.rstrip("/").endswith("/health"):
            import mlx.core as mx
            self.reply(200, {"status": "ok", "engine": "mlx-audio", "draft": self.draft,
                             "memory": {"active": mx.get_active_memory(), "cache": mx.get_cache_memory(),
                                        "peak": mx.get_peak_memory()}})
        else:
            self.reply(404, {"error": "not found"})

    def do_POST(self):
        if not self.path.rstrip("/").endswith("/v1/audio/transcriptions"):
            self.reply(404, {"error": "not found"})
            return
        offered = self.headers.get("Authorization", "")
        if self.key and not hmac.compare_digest(offered, f"Bearer {self.key}"):
            self.reply(401, {"error": "unauthorized"})
            return
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        try:
            wav = extract_file(body, self.headers.get("Content-Type", ""))
        except Exception as error:
            self.reply(400, {"error": f"bad multipart: {error}"})
            return
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as handle:
            handle.write(wav)
            path = handle.name
        try:
            result = transcribe(path)
            self.reply(200, {"text": (result.text or "").strip()})
        except Exception as error:
            self.reply(500, {"error": f"{type(error).__name__}: {error}"})
        finally:
            os.unlink(path)


def main():
    args = parse(sys.argv[1:])
    from mlx_audio.stt import load

    import mlx.core as mx
    if WIRED:
        mx.set_wired_limit(WIRED)
    if CACHE_LIMIT:
        mx.set_cache_limit(CACHE_LIMIT)
    if COMPILE:
        mx.enable_compile()
    path = weights(args.asr_model)
    print(f"[mlx-serve] loading model={path} draft={args.mtp_draft} wired={WIRED} cache={CACHE_LIMIT} warmup={WARMUP} compile={COMPILE}", flush=True)
    Handler.draft = args.mtp_draft
    Handler.key = args.api_key
    Handler.model = load(str(path), strict=True)
    if WARMUP:
        # pay Metal kernel compilation on a throwaway request, not the user's first
        import wave
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as handle:
            probe = handle.name
        try:
            with wave.open(probe, "wb") as w:
                w.setnchannels(1); w.setsampwidth(2); w.setframerate(16000)
                w.writeframes(b"\x00\x00" * 16000)
            transcribe(probe, max_tokens=32)
        finally:
            os.unlink(probe)
    server = HTTPServer((args.host, args.port), Handler)
    print(f"HTTP API listening on http://{args.host}:{args.port}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
