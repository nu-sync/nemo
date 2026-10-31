#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12,<3.14"
# dependencies = ["faster-whisper==1.2.1", "huggingface-hub==0.36.0", "av==16.0.1"]
# ///
"""Independently check TTS/reference agreement with pinned Whisper base.en, without playback."""

import argparse
import importlib.metadata
import json
from pathlib import Path
import tempfile
import time

from speech_corpus import DEFAULT_CORPUS, ROOT, load_corpus, local_file, prepare_audio, sha256, word_errors

REPOSITORY = "Systran/faster-whisper-base.en"
REVISION = "3d3d5dee26484f91867d81cb899cfcf72b96be6c"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, default=DEFAULT_CORPUS)
    parser.add_argument("--durations", type=int, nargs="+")
    parser.add_argument("--cache", type=Path, default=ROOT / "artifacts/reference-asr/whisper-base.en")
    parser.add_argument("--max-wer", type=float, default=.05)
    parser.add_argument("--output", type=Path, required=True, help="new report directory")
    args = parser.parse_args()
    if not 0 <= args.max_wer <= 1:
        parser.error("max-wer must be between zero and one")
    corpus = args.corpus.expanduser().resolve()
    manifest = load_corpus(corpus)
    fixtures = [fixture for fixture in manifest["fixtures"] if not args.durations or fixture["target_seconds"] in args.durations]
    if not fixtures or (args.durations and set(args.durations) != {f["target_seconds"] for f in fixtures}):
        parser.error("requested duration missing from corpus")
    args.output.mkdir(parents=True, exist_ok=False)
    from faster_whisper import WhisperModel
    from huggingface_hub import snapshot_download
    model_path = Path(snapshot_download(REPOSITORY, revision=REVISION, local_dir=str(args.cache),
                                       allow_patterns=["config.json", "model.bin", "tokenizer.json", "vocabulary.*"]))
    model = WhisperModel(str(model_path), device="cpu", compute_type="int8", cpu_threads=4, num_workers=1)
    report = {
        "created_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "scope": "independent ASR consistency, not human ground-truth certification; no reference prompting or playback",
        "corpus_manifest_sha256": sha256(corpus / "manifest.json"),
        "script_sha256": sha256(Path(__file__)), "scoring_sha256": sha256(Path(__file__).with_name("speech_corpus.py")),
        "lock_sha256": sha256(Path(str(Path(__file__)) + ".lock")),
        "packages": {name: importlib.metadata.version(name) for name in ("faster-whisper", "ctranslate2", "huggingface-hub")},
        "model": {"repository": REPOSITORY, "revision": REVISION,
                  "files": {path.name: sha256(path) for path in model_path.iterdir() if path.is_file()}},
        "options": {"language": "en", "beam_size": 5, "vad_filter": False,
                    "condition_on_previous_text": False, "initial_prompt": None,
                    "device": "cpu", "compute_type": "int8", "cpu_threads": 4},
        "max_wer": args.max_wer, "state": "running", "records": [],
    }
    destination = args.output / "results.json"
    def save():
        temporary = destination.with_suffix(".tmp")
        temporary.write_text(json.dumps(report, indent=2) + "\n")
        temporary.replace(destination)
    save()
    try:
        with tempfile.TemporaryDirectory(prefix="nemo-reference-asr-") as directory:
            audio = prepare_audio(corpus, fixtures, Path(directory))
            for fixture in fixtures:
                segments, _ = model.transcribe(str(audio[fixture["id"]]), language="en", beam_size=5,
                                               vad_filter=False, condition_on_previous_text=False)
                text = " ".join(segment.text.strip() for segment in segments)
                score = word_errors(local_file(corpus, fixture["text"]).read_text(), text)
                record = {"fixture": fixture["id"], "audio_sha256": fixture["audio_sha256"],
                          "text_sha256": fixture["text_sha256"], "text": text, "accuracy": score,
                          "status": "ok" if score["wer"] <= args.max_wer else "review_required"}
                report["records"].append(record)
                save()
                print(f"{fixture['id']}: WER={100 * score['wer']:.2f}% {record['status']}", flush=True)
        report["state"] = "complete"
    except KeyboardInterrupt:
        report["state"] = "interrupted"
    except Exception as error:
        report["state"] = "error"
        report["error"] = f"{type(error).__name__}: {error}"
    finally:
        save()
    print(f"Evidence: {destination}")
    return 130 if report["state"] == "interrupted" else int(report["state"] != "complete" or any(row["status"] != "ok" for row in report["records"]))


if __name__ == "__main__":
    raise SystemExit(main())
