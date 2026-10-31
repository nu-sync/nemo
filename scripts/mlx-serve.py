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
"""MLX-Audio behind Nemo's runtime contract, for paired measurement only.

Speaks just enough of `nemo-speech serve` for `scripts/benchmark-corpus.py`:
`GET /health`, `POST /v1/audio/transcriptions` (multipart `file`, `response_format=json`)
returning {"text": ...}. Unknown nemo-speech flags are ignored.

    NEMO_MLX_DRAFT=8 uv run --locked scripts/mlx-serve.py serve --port 8080 --api-key k

This is an experiment harness. It is not a shipped backend: it needs 21.6 GB of
NVFP4 weights, loads in ~5.5 s, and holds ~25.5 GB resident.
"""
import argparse
import json
import os
import sys
import tempfile
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

MANAGED = Path.home() / "Library/Application Support/Nemo/models/main"
DRAFT = int(os.environ.get("NEMO_MLX_DRAFT", "8"))
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
    args, _unknown = parser.parse_known_args(argv)
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


class Handler(BaseHTTPRequestHandler):
    model = None
    key = ""

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
            self.reply(200, {"status": "ok", "engine": "mlx-audio", "draft": DRAFT})
        else:
            self.reply(404, {"error": "not found"})

    def do_POST(self):
        if not self.path.rstrip("/").endswith("/v1/audio/transcriptions"):
            self.reply(404, {"error": "not found"})
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
            result = self.model.generate(
                path,
                speculative=DRAFT > 0,
                num_draft_tokens=DRAFT or 4,
                temperature=0,
                max_tokens=2048,
            )
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
    print(f"[mlx-serve] loading model={path} draft={DRAFT} wired={WIRED} cache={CACHE_LIMIT} warmup={WARMUP} compile={COMPILE}", flush=True)
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
            Handler.model.generate(probe, speculative=DRAFT > 0, num_draft_tokens=DRAFT or 4, temperature=0, max_tokens=32)
        finally:
            os.unlink(probe)
    Handler.key = args.api_key
    server = HTTPServer((args.host, args.port), Handler)
    print(f"HTTP API listening on http://{args.host}:{args.port}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
