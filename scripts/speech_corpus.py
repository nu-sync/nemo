#!/usr/bin/env python3
"""Validate the versioned speech corpus and score transcripts without extra dependencies."""

import argparse
import hashlib
import json
import math
from pathlib import Path
import re
import subprocess
import unicodedata
import wave

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CORPUS = ROOT / "benchmarks/speech"
SAMPLE_RATE = 16000


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def words(text):
    text = unicodedata.normalize("NFKC", text).lower().replace("’", "'")
    return re.findall(r"[^\W_]+(?:'[^\W_]+)*", text, flags=re.UNICODE)


def word_errors(reference, hypothesis):
    """Minimum word edit distance; punctuation/case ignored, no spelling/number aliases."""
    expected, actual = words(reference), words(hypothesis)
    if not expected:
        raise ValueError("reference must contain words")
    # Cells hold (distance, substitutions, deletions, insertions).
    previous = [(j, 0, 0, j) for j in range(len(actual) + 1)]
    for i, left in enumerate(expected, 1):
        current = [(i, 0, i, 0)]
        for j, right in enumerate(actual, 1):
            distance, subs, dels, ins = previous[j - 1]
            substitution = (distance + (left != right), subs + (left != right), dels, ins)
            distance, subs, dels, ins = previous[j]
            deletion = (distance + 1, subs, dels + 1, ins)
            distance, subs, dels, ins = current[-1]
            insertion = (distance + 1, subs, dels, ins + 1)
            current.append(min((substitution, deletion, insertion), key=lambda cell: cell[0]))
        previous = current
    distance, subs, dels, ins = previous[-1]
    return {"reference_words": len(expected), "hypothesis_words": len(actual),
            "substitutions": subs, "deletions": dels, "insertions": ins,
            "errors": distance, "wer": distance / len(expected)}


def distribution(values):
    values = sorted(values)
    if not values:
        return None
    def percentile(fraction):
        return values[math.ceil(fraction * len(values)) - 1]
    return {"n": len(values), "p50": percentile(.5), "p95": percentile(.95),
            "p99": percentile(.99), "min": values[0], "max": values[-1],
            "p95_sample_warning": len(values) < 100}


def local_file(root, name):
    path = (root / name).resolve()
    if not path.is_relative_to(root.resolve()) or not path.is_file():
        raise ValueError(f"missing or unsafe corpus path: {name}")
    return path


def decode_flac(source, destination):
    subprocess.run(["/usr/bin/afconvert", str(source), str(destination),
                    "-f", "WAVE", "-d", "LEI16@16000", "-c", "1"],
                   check=True, capture_output=True, timeout=60)


def pcm_info(path):
    with wave.open(str(path), "rb") as audio:
        if (audio.getnchannels(), audio.getsampwidth(), audio.getframerate(), audio.getcomptype()) != (1, 2, SAMPLE_RATE, "NONE"):
            raise ValueError(f"expected mono 16 kHz PCM16: {path}")
        data = audio.readframes(audio.getnframes())
        return {"frames": audio.getnframes(), "seconds": audio.getnframes() / SAMPLE_RATE,
                "pcm_sha256": hashlib.sha256(data).hexdigest()}


def load_corpus(root):
    root = Path(root)
    manifest = json.loads((root / "manifest.json").read_text())
    if manifest.get("schema_version") != 1 or not manifest.get("fixtures"):
        raise ValueError("unsupported or empty speech corpus")
    ids = set()
    for fixture in manifest["fixtures"]:
        name = fixture["id"]
        if not re.fullmatch(r"[a-z0-9_-]+", name) or name in ids:
            raise ValueError(f"invalid or duplicate fixture id: {name}")
        ids.add(name)
        target = fixture["target_seconds"]
        if not isinstance(target, (int, float)) or not 0 < target <= 300:
            raise ValueError(f"invalid duration: {name}")
        for kind in ("audio", "text"):
            path = local_file(root, fixture[kind])
            if sha256(path) != fixture[kind + "_sha256"]:
                raise ValueError(f"{kind} checksum mismatch: {name}")
        if not words(local_file(root, fixture["text"]).read_text()):
            raise ValueError(f"empty reference: {name}")
    return manifest


def prepare_audio(root, fixtures, destination):
    result = {}
    for fixture in fixtures:
        path = destination / (fixture["id"] + ".wav")
        decode_flac(local_file(root, fixture["audio"]), path)
        info = pcm_info(path)
        if info["frames"] != round(fixture["target_seconds"] * SAMPLE_RATE):
            raise ValueError(f"decoded duration mismatch: {fixture['id']}")
        if info["pcm_sha256"] != fixture["pcm_sha256"]:
            raise ValueError(f"decoded PCM checksum mismatch: {fixture['id']}")
        if path.stat().st_size > 64 * 1024 * 1024:
            raise ValueError(f"fixture exceeds runtime upload limit: {path}")
        result[fixture["id"]] = path
    return result


def main():
    import tempfile
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, default=DEFAULT_CORPUS)
    args = parser.parse_args()
    manifest = load_corpus(args.corpus)
    with tempfile.TemporaryDirectory(prefix="nemo-corpus-check-") as directory:
        prepare_audio(args.corpus, manifest["fixtures"], Path(directory))
    for fixture in manifest["fixtures"]:
        print(f"{fixture['id']}: {fixture['target_seconds']}s, audio/text/PCM hashes verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
