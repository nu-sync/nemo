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
import select
import sys
import tempfile
import wave
from functools import cache
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


# Pinned MLX-Audio Kaiser FIR parameters for 48 kHz to 16 kHz.
FILTER_RADIUS = 64 * 3
FFT_SIZE = 8192


@cache
def capture_filter_spectrum():
    import numpy as np

    offsets = np.arange(-FILTER_RADIUS, FILTER_RADIUS + 1)
    taps = np.sinc(offsets * (0.9475937167399596 / 3))
    taps *= np.kaiser(taps.size, 14.769656459379492)
    return np.fft.rfft(taps / taps.sum(), n=FFT_SIZE)


def resample_capture(path, target_rate):
    if target_rate != 16000:
        return path

    try:
        with wave.open(path, "rb") as audio:
            if (audio.getframerate(), audio.getnchannels(), audio.getsampwidth()) != (48000, 1, 2):
                return path
            frames = audio.getnframes()
            if not 0 < frames <= os.path.getsize(path) // 2:
                return path
            pcm = audio.readframes(frames)
    except (EOFError, wave.Error):
        return path
    if len(pcm) != frames * 2:
        return path

    import numpy as np

    samples = np.frombuffer(pcm, dtype="<i2").astype(np.float64) / 32768.0
    padded = np.pad(samples, FILTER_RADIUS, mode="edge")
    spectrum = capture_filter_spectrum()
    overlap = 2 * FILTER_RADIUS
    # Each overlap-save block advances a multiple of three input frames so
    # the decimation phase stays continuous across block boundaries.
    step = (FFT_SIZE - overlap) // 3 * 3
    result = np.empty((frames + 2) // 3, dtype=np.float32)
    for start in range(0, frames, step):
        block = np.fft.rfft(padded[start:start + FFT_SIZE], n=FFT_SIZE)
        filtered = np.fft.irfft(block * spectrum)
        output = result[start // 3:(start + step) // 3]
        output[:] = filtered[overlap:overlap + 3 * output.size:3]
    return result


def transcribe(path, max_tokens=2048, hint=()):
    """Return the greedy ASR transcript for one request."""
    audio = resample_capture(path, Handler.model.sample_rate)
    if Handler.draft_head is not None:
        from nemo_mtp import transcribe as transcribe_with_shortlist

        return transcribe_with_shortlist(
            Handler.model, audio, Handler.draft_head, Handler.draft, max_tokens, hint
        )
    result = Handler.model.generate(
        audio,
        speculative=Handler.draft > 0,
        num_draft_tokens=Handler.draft or 1,
        temperature=0,
        max_tokens=max_tokens,
    )
    return (result.text or "").strip()


class PreviewInterrupted(Exception):
    pass


class Handler(BaseHTTPRequestHandler):
    model = None
    key = ""
    draft = MAX_DRAFT
    draft_head = None

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

    def preview_checkpoint(self):
        # A queued final request or a disconnected preview client takes priority.
        if select.select([self.server.socket, self.connection], [], [], 0)[0]:
            raise PreviewInterrupted

    def preview(self, path):
        from nemo_mtp import PREVIEW_TOKENS, audio_tokens

        print("[mlx-serve] preview started", flush=True)
        self.preview_checkpoint()
        audio = resample_capture(path, self.model.sample_rate)
        self.preview_checkpoint()
        tokens = []
        for token in audio_tokens(self.model, audio, self.draft_head, self.draft, 2048,
                                  checkpoint=self.preview_checkpoint):
            if token in self.model.config.eos_token_id:
                break
            tokens.append(token)
            if len(tokens) == PREVIEW_TOKENS:
                break
        return {"tokens": tokens}

    def do_POST(self):
        wake_gpu = self.path.rstrip("/") == "/v1/gpu/wake"
        preview = self.path.rstrip("/") == "/v1/audio/preview"
        if not (wake_gpu or preview) and not self.path.rstrip("/").endswith("/v1/audio/transcriptions"):
            self.reply(404, {"error": "not found"})
            return
        offered = self.headers.get("Authorization", "")
        if self.key and not hmac.compare_digest(offered, f"Bearer {self.key}"):
            self.reply(401, {"error": "unauthorized"})
            return
        if wake_gpu:
            import mlx.core as mx

            mx.async_eval(mx.sum(mx.square(mx.arange(32, dtype=mx.float32))))
            self.reply(200, {})
            return
        hint = []
        try:
            size = int(self.headers.get("Content-Length", "0"))
            if not 0 < size <= 65 * 1024 * 1024:
                raise ValueError("invalid audio size")
            if preview and self.draft_head is None:
                self.reply(200, {"tokens": []})
                return
            if self.draft_head is not None and self.headers.get("X-Nemo-Preview"):
                from nemo_mtp import PREVIEW_TOKENS
                hint = json.loads(self.headers["X-Nemo-Preview"])
                vocab = self.model.llm.lm_head.weight.shape[0]
                if (not isinstance(hint, list) or len(hint) > PREVIEW_TOKENS or
                        any(type(t) is not int or not 0 <= t < vocab or
                            t in self.model.config.eos_token_id for t in hint)):
                    raise ValueError("invalid preview tokens")
            body = self.rfile.read(size)
            wav = body if preview else extract_file(body, self.headers.get("Content-Type", ""))
        except (IndexError, ValueError) as error:
            self.reply(400, {"error": str(error)})
            return
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as handle:
            handle.write(wav)
            path = handle.name
        try:
            result = self.preview(path) if preview else {"text": transcribe(path, hint=hint)}
            self.reply(200, result)
        except PreviewInterrupted:
            print("[mlx-serve] preview interrupted", flush=True)
            self.reply(200, {"tokens": []})
        except Exception as error:
            self.reply(500, {"error": f"{type(error).__name__}: {error}"})
        finally:
            os.unlink(path)


def main():
    # The runtime directory is part of a signed app bundle.
    sys.dont_write_bytecode = True
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
    if Handler.draft:
        from nemo_mtp import prepare_draft_head

        Handler.draft_head = prepare_draft_head(Handler.model)
    if WARMUP:
        # pay Metal kernel compilation on a throwaway request, not the user's first
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as handle:
            probe = handle.name
        try:
            with wave.open(probe, "wb") as w:
                w.setnchannels(1); w.setsampwidth(2); w.setframerate(16000)
                w.writeframes(b"\x00\x00" * 16000)
            transcribe(probe, max_tokens=32)
        finally:
            os.unlink(probe)
    with HTTPServer((args.host, args.port), Handler) as server:
        print(f"HTTP API listening on http://{args.host}:{args.port}", flush=True)
        # Nemo stops this process directly. Block on the socket between requests.
        while True:
            server.handle_request()


if __name__ == "__main__":
    main()
