"""Compare warm MLX requests on the same frozen 48 kHz speech in two Nemo bundles."""

import argparse
import importlib.util
import json
import os
from pathlib import Path
from statistics import median
import subprocess
import sys
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / "scripts"))
from nemo_status import control
from speech_corpus import DEFAULT_CORPUS, load_corpus, local_file, sha256, word_errors

spec = importlib.util.spec_from_file_location("benchmark_app", ROOT / "scripts/benchmark-app.py")
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


def command(*args):
    return subprocess.check_output(args, text=True).strip()


def idle_snapshot(data):
    power = command("/usr/bin/pmset", "-g", "batt").splitlines()[0]
    if "AC Power" not in power:
        raise RuntimeError("Reconnect AC power before comparing laptop timings")
    if (data / "run/control.sock").exists():
        identity = json.loads(control(data, "identity"))
        if json.loads(control(data, "readiness"))["activity"] != "idle":
            raise RuntimeError("Dictation is busy. Leave it running and retry later.")
    else:
        identity = None
    log = data / "daemon.log"
    return power, identity, log.stat().st_mtime_ns if log.exists() else None


def bundle_id(bundle):
    # CDHash covers the executable and its signed resource seal, including the helper.
    info = subprocess.check_output(["/usr/bin/codesign", "-dv", "--verbose=4", bundle],
                                   stderr=subprocess.STDOUT, text=True)
    return next(line for line in info.splitlines() if line.startswith("CDHash="))


def paired_reduction(pair, expected_text):
    for row in pair.values():
        if any(text != expected_text for text in row["texts"]):
            raise ValueError("Raw transcripts differ. Inspect the saved responses.")
    baseline_ms = pair["baseline"]["warm"]["request_ms"]
    candidate_ms = pair["candidate"]["warm"]["request_ms"]
    return 100 * (1 - candidate_ms / baseline_ms)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", required=True, type=Path, help="baseline Nemo.app")
    parser.add_argument("--candidate", required=True, type=Path, help="candidate Nemo.app")
    parser.add_argument("--seconds", type=int, default=30, help="frozen fixture duration")
    parser.add_argument("--rounds", type=int, default=6, help="alternating paired visits")
    parser.add_argument("--output", required=True, type=Path, help="new evidence directory")
    args = parser.parse_args()
    if args.rounds < 1:
        parser.error("--rounds must be positive")
    data = Path(os.environ.get("NEMO_DATA_DIR", Path.home() / "Library/Application Support/Nemo")).resolve()
    manifest = load_corpus(DEFAULT_CORPUS)
    fixture = next((f for f in manifest["fixtures"] if f["target_seconds"] == args.seconds), None)
    if fixture is None:
        parser.error("--seconds must select a duration in the frozen corpus")
    model = data / "models/main"
    if not (model / "model.safetensors.index.json").is_file():
        parser.error(f"expected a provisioned MLX model at {model}")
    idle_snapshot(data)
    run = args.output.resolve()
    run.mkdir(parents=True, exist_ok=False)
    wav = run / (fixture["id"] + ".wav")
    subprocess.run(["/usr/bin/afconvert", local_file(DEFAULT_CORPUS, fixture["audio"]), wav,
                    "-f", "WAVE", "-d", "LEI16@48000", "-c", "1"], check=True)
    reference = local_file(DEFAULT_CORPUS, fixture["text"]).read_text()
    bundles = {"baseline": args.baseline.expanduser().resolve(),
               "candidate": args.candidate.expanduser().resolve()}
    profiles = {}
    identities = {}
    for name, bundle in bundles.items():
        subprocess.run(["/usr/bin/codesign", "--verify", "--deep", "--strict", bundle], check=True)
        identities[name] = bundle_id(bundle)
        exe = bundle / "Contents/MacOS/nemo"
        profiles[name] = SimpleNamespace(output=run / name, nemo=exe, models=data / "models",
                                         model=Path(command(exe, "model", "path")))
        bench.write_config(profiles[name], "mlx-audio")
    report = {"state": "running", "fixture": fixture["id"], "wav_sha256": sha256(wav),
              "chip": command("sysctl", "-n", "machdep.cpu.brand_string"),
              "memory_bytes": int(command("sysctl", "-n", "hw.memsize")),
              "macos": command("sw_vers", "-productVersion"), "model_store": str(model),
              "model_index_sha256": sha256(model / "model.safetensors.index.json"),
              "bundles": {n: {"path": str(b), "cdhash": identities[n]} for n, b in bundles.items()},
              "visits": []}
    path = run / "results.json"

    def save():
        path.write_text(json.dumps(report, indent=2) + "\n")

    save()
    reductions = []
    try:
        for index in range(args.rounds):
            pair = {}
            before = idle_snapshot(data)
            for name in (("baseline", "candidate") if index % 2 == 0 else ("candidate", "baseline")):
                if idle_snapshot(data) != before or bundle_id(bundles[name]) != identities[name]:
                    raise RuntimeError("Daemon, power or bundle changed during the pair")
                texts, timings, load, memory, _ = bench.run_nemo(profiles[name], "mlx-audio", [wav, wav])
                if not timings[1]["reused_model"]:
                    raise RuntimeError("Second request did not reuse the loaded model")
                row = {"round": index + 1, "name": name, "texts": texts, "load": load,
                       "first": timings[0], "warm": timings[1], "memory": memory,
                       "accuracy": word_errors(reference, texts[1])}
                report["visits"].append(row)
                save()
                if idle_snapshot(data) != before or bundle_id(bundles[name]) != identities[name]:
                    raise RuntimeError("Daemon, power or bundle changed during the pair")
                pair[name] = row
            reductions.append(paired_reduction(pair, report["visits"][0]["texts"][0]))
        for bundle in bundles.values():
            subprocess.run(["/usr/bin/codesign", "--verify", "--deep", "--strict", bundle], check=True)
        report.update(state="complete", median_paired_reduction_percent=median(reductions))
    except Exception as error:
        report.update(state="incomplete", reason=str(error))
        raise
    finally:
        save()
    kind = "A/A" if identities["candidate"] == identities["baseline"] else "A/B"
    print(f"{kind} completed with exact transcripts across {len(reductions)} paired visits "
          f"({path})")


if __name__ == "__main__":
    main()
