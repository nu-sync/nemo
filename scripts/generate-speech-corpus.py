#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12,<3.14"
# dependencies = ["kokoro-onnx==0.6.1"]
# ///
"""Generate frozen, text-paired FLAC fixtures locally; never opens a speaker or microphone."""

import argparse
import importlib.metadata
import json
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import time
import urllib.request
import wave

from speech_corpus import DEFAULT_CORPUS, ROOT, SAMPLE_RATE, load_corpus, pcm_info, sha256

ASSETS = {
    "kokoro-v1.0.onnx": "beb0d1848dee9a49da392cc3df26958d46cfa35d321edf434f52949153f0df3a",
    "voices-v1.0.bin": "bca610b8308e8d99f32e6fe4197e7ec01679264efed0cac9140fe9c29f1fbf7d",
}
BASE_URL = "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.1/"


def asset(cache, name, expected):
    path = cache / name
    if not path.exists():
        partial = path.with_suffix(path.suffix + ".part")
        print(f"Downloading {name} to {cache}", flush=True)
        try:
            with urllib.request.urlopen(BASE_URL + name, timeout=120) as response, partial.open("wb") as output:
                shutil.copyfileobj(response, output)
            if sha256(partial) != expected:
                raise ValueError(f"download checksum mismatch: {name}")
            partial.replace(path)
        finally:
            partial.unlink(missing_ok=True)
    if sha256(path) != expected:
        raise ValueError(f"TTS asset checksum mismatch: {path}")
    return path


def fit_speech(engine, text, voice, language, seconds):
    # Synthesis speed changes duration without pitch shifting, looping, or cutting words.
    # Stay slightly under target to allow a small, measured silence-only tail.
    speed = 1.0
    slow, fast = .5, 2.0
    seen_short = seen_long = False
    attempts = []
    for _ in range(16):
        audio, rate = engine.create(text, voice=voice, speed=speed, lang=language)
        duration = len(audio) / rate
        attempts.append({"speed": speed, "seconds": duration})
        if .97 * seconds <= duration <= seconds:
            return audio, rate, attempts
        if duration > seconds:
            slow, seen_long = speed, True
        else:
            fast, seen_short = speed, True
        proposed = speed * duration / (.985 * seconds)
        if seen_short and seen_long:
            proposed = (slow + fast) / 2
        if not .5 <= proposed <= 2.0:
            raise ValueError(f"text length unsuitable for {seconds}s at a natural rate; edit the source text (required speed {proposed:.3f})")
        speed = proposed
    raise ValueError(f"could not fit complete speech into {seconds}s: {attempts}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=DEFAULT_CORPUS,
                        help="directory containing sources.json and text/*.txt")
    parser.add_argument("--output", type=Path, default=DEFAULT_CORPUS,
                        help="corpus destination; an existing manifest is never overwritten")
    parser.add_argument("--cache", type=Path, default=ROOT / "artifacts/tts/kokoro-v1.0")
    args = parser.parse_args()
    if not shutil.which("espeak-ng") or not Path("/usr/bin/afconvert").is_file():
        parser.error("requires espeak-ng (brew install espeak-ng) and macOS afconvert")
    args.source = args.source.resolve()
    args.output = args.output.resolve()
    spec_path = args.source / "sources.json"
    spec = json.loads(spec_path.read_text())
    if (args.output / "manifest.json").exists():
        manifest = load_corpus(args.output)
        if manifest["sources_sha256"] != sha256(spec_path):
            parser.error("source specification changed; generate a new corpus with --output")
        print("Existing corpus hashes verified; no files regenerated. Use --output for a new version.")
        return 0
    args.cache.mkdir(parents=True, exist_ok=True)
    model, voices = [asset(args.cache, name, digest) for name, digest in ASSETS.items()]
    from kokoro_onnx import Kokoro
    import numpy as np
    import onnxruntime as ort
    # Keep TTS on CPU, independent of Nemo's Metal runtime; generation is not timed inference.
    options = ort.SessionOptions()
    options.intra_op_num_threads = 4
    session = ort.InferenceSession(str(model), sess_options=options, providers=["CPUExecutionProvider"])
    engine = Kokoro.from_session(session, str(voices))
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "audio").mkdir(exist_ok=True)
    if args.source != args.output:
        (args.output / "text").mkdir(exist_ok=True)
        shutil.copyfile(spec_path, args.output / "sources.json")
    manifest = {
        "schema_version": 1, "corpus_id": spec["corpus_id"],
        "created_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "sources_sha256": sha256(spec_path),
        "generator": {"script_sha256": sha256(Path(__file__)),
                      "lock_sha256": sha256(Path(str(Path(__file__)) + ".lock")),
                      "python": platform.python_version(), "os": platform.platform(),
                      "packages": {name: importlib.metadata.version(name)
                                   for name in ("kokoro-onnx", "onnxruntime", "numpy", "phonemizer", "espeakng-loader")},
                      "tts_assets": {name: {"url": BASE_URL + name, "sha256": digest}
                                     for name, digest in ASSETS.items()},
                      "sample_rate": SAMPLE_RATE, "channels": 1, "sample_width": 2},
        "reference_status": "synthesis-source; not human-listened or independently ASR-certified",
        "fixtures": [],
    }
    with tempfile.TemporaryDirectory(prefix="nemo-tts-") as directory:
        scratch = Path(directory)
        for source in spec["fixtures"]:
            text_path = args.source / source["text"]
            text = text_path.read_text()
            if args.source != args.output:
                shutil.copyfile(text_path, args.output / source["text"])
            audio, rate, attempts = fit_speech(engine, text, source["voice"], source["language"], source["target_seconds"])
            native = scratch / "native.wav"
            converted = scratch / "converted.wav"
            with wave.open(str(native), "wb") as output:
                output.setparams((1, 2, rate, 0, "NONE", "not compressed"))
                output.writeframes((np.clip(audio, -1, 1) * 32767).astype("<i2").tobytes())
            subprocess.run(["/usr/bin/afconvert", str(native), str(converted),
                            "-f", "WAVE", "-d", "LEI16@16000", "-c", "1"], check=True, capture_output=True)
            with wave.open(str(converted), "rb") as input_audio:
                pcm = input_audio.readframes(input_audio.getnframes())
            frames = round(source["target_seconds"] * SAMPLE_RATE)
            padding = frames - len(pcm) // 2
            if not 0 <= padding <= .031 * frames:
                raise ValueError("resampled speech does not fit; refusing to truncate or add excessive silence")
            with wave.open(str(converted), "wb") as output:
                output.setparams((1, 2, SAMPLE_RATE, 0, "NONE", "not compressed"))
                output.writeframes(pcm + bytes(2 * padding))
            audio_name = f"audio/{source['id']}.flac"
            destination = args.output / audio_name
            partial = scratch / "fixture.flac"
            subprocess.run(["/usr/bin/afconvert", str(converted), str(partial), "-f", "flac", "-d", "flac"],
                           check=True, capture_output=True)
            shutil.copyfile(partial, destination)
            fixture = {**source, "audio": audio_name, "audio_sha256": sha256(destination),
                       "text_sha256": sha256(text_path), **pcm_info(converted),
                       "tail_padding_seconds": padding / SAMPLE_RATE, "synthesis_attempts": attempts}
            manifest["fixtures"].append(fixture)
            print(f"{source['id']}: {source['target_seconds']}s, voice={source['voice']}, "
                  f"speed={attempts[-1]['speed']:.3f}, padding={padding / SAMPLE_RATE:.3f}s", flush=True)
    temporary = args.output / "manifest.json.tmp"
    temporary.write_text(json.dumps(manifest, indent=2) + "\n")
    temporary.replace(args.output / "manifest.json")
    load_corpus(args.output)
    print(f"Corpus: {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
