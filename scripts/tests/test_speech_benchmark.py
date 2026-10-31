import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
import wave

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
from speech_corpus import DEFAULT_CORPUS, distribution, load_corpus, sha256, word_errors, words


def module(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), SCRIPTS / (name + ".py"))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


class AccuracyTests(unittest.TestCase):
    def test_scores_substitution_and_insertion_in_the_returned_transcript(self):
        self.assertEqual(word_errors("The quick brown fox.", "the fast brown fox jumps"),
                         {"reference_words": 4, "hypothesis_words": 5, "substitutions": 1,
                          "deletions": 0, "insertions": 1, "errors": 2, "wer": .5})

    def test_missing_all_words_is_not_a_fast_success(self):
        self.assertEqual(word_errors("Sounds good.", ""),
                         {"reference_words": 2, "hypothesis_words": 0, "substitutions": 0,
                          "deletions": 2, "insertions": 0, "errors": 2, "wer": 1.0})

    def test_only_case_and_punctuation_are_normalized(self):
        self.assertEqual(words("DON’T leave; twenty-five café cups!"),
                         ["don't", "leave", "twenty", "five", "café", "cups"])
        self.assertEqual(word_errors("The centre.", "the center")["wer"], .5)

    def test_percentiles_use_nearest_rank_not_a_median_labelled_p95(self):
        self.assertEqual(distribution(list(range(1, 101))),
                         {"n": 100, "p50": 50, "p95": 95, "p99": 99, "min": 1, "max": 100,
                          "p95_sample_warning": False})
        self.assertEqual(distribution([10, 30, 20])["p95"], 30)
        self.assertIsNone(distribution([]))

    def test_duration_fitting_brackets_rates_instead_of_oscillating(self):
        class Engine:
            def create(self, text, voice, speed, lang):
                # A nonlinear duration curve that makes proportional updates overshoot.
                seconds = 50 * (.82 / speed) ** 2
                return bytes(round(seconds * 1000)), 1000
        audio, rate, attempts = module("generate-speech-corpus").fit_speech(
            Engine(), "Original complete passage.", "voice", "en-us", 50)
        self.assertEqual(rate, 1000)
        self.assertTrue(48500 <= len(audio) <= 50000, attempts)
        self.assertEqual(attempts[0], {"speed": 1.0, "seconds": 33.62})

    def test_actual_log_reports_dispatch_without_adding_guessed_overhead(self):
        report = module("latency-report").latency_report([
            'Ready', '{bad json', '[]',
            json.dumps(dict(event="transcription", success=True, audio_seconds=5,
                            request_ms=350, reused_model=True)),
            json.dumps(dict(event="transcription", success=False)),
            json.dumps(dict(event="delivery", success=True, stop_to_dispatch_ms=390)),
            json.dumps(dict(event="transcription", success=True, audio_seconds=5, request_ms=float('nan'))),
        ])
        self.assertEqual(report["transcriptions"]["request_ms"]["p95"], 350)
        self.assertEqual(report["transcriptions"]["n"], 1)
        self.assertEqual(report["stop_to_dispatch_ms"]["p95"], 390)
        self.assertEqual(report["failures"], {"transcription": 1, "delivery": 0})
        self.assertEqual([bucket["n"] for bucket in report["duration_buckets"]], [0, 0, 1, 0, 0, 0, 0])


class FrozenCorpusTests(unittest.TestCase):
    def test_frozen_asr_evidence_is_bound_to_the_actual_audio_and_references(self):
        manifest = load_corpus(DEFAULT_CORPUS)
        fixtures = {fixture["id"]: fixture for fixture in manifest["fixtures"]}
        for name in ("reference-check.json", "nemo-check.json"):
            report = json.loads((DEFAULT_CORPUS / name).read_text())
            self.assertEqual(report["state"], "complete")
            self.assertEqual(report["corpus_manifest_sha256"], sha256(DEFAULT_CORPUS / "manifest.json"))
            self.assertEqual({row["fixture"] for row in report["records"]}, set(fixtures))
            for row in report["records"]:
                fixture = fixtures[row["fixture"]]
                reference = (DEFAULT_CORPUS / fixture["text"]).read_text()
                self.assertEqual(word_errors(reference, row["text"]), row["accuracy"])
                if name == "reference-check.json":
                    self.assertEqual(row["audio_sha256"], fixture["audio_sha256"])
                    self.assertEqual(row["text_sha256"], fixture["text_sha256"])
                    self.assertEqual(row["status"], "ok")


@unittest.skipUnless(Path("/usr/bin/afconvert").is_file(), "macOS audio conversion required")
class CorpusRunnerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="nemo-benchmark-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        corpus = self.root / "corpus"
        corpus.mkdir()
        pcm = b"\x00\x01" * 16000
        wav = self.root / "one.wav"
        with wave.open(str(wav), "wb") as audio:
            audio.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
            audio.writeframes(pcm)
        flac = corpus / "one.flac"
        subprocess.run(["/usr/bin/afconvert", str(wav), str(flac), "-f", "flac", "-d", "flac"],
                       check=True, capture_output=True)
        (corpus / "one.txt").write_text("Sounds good.\n")
        self.manifest = {"schema_version": 1, "fixtures": [
            {"id": "one", "target_seconds": 1, "audio": "one.flac", "text": "one.txt",
             "audio_sha256": hashlib.sha256(flac.read_bytes()).hexdigest(),
             "text_sha256": hashlib.sha256(b"Sounds good.\n").hexdigest(),
             "pcm_sha256": hashlib.sha256(pcm).hexdigest()}]}
        (corpus / "manifest.json").write_text(json.dumps(self.manifest))
        self.runtime = self.root / "fake-runtime"
        self.runtime.write_text('''#!/usr/bin/env python3
import json, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer
args = sys.argv
port = int(args[args.index('--port') + 1])
key = args[args.index('--api-key') + 1]
model = args[args.index('--asr-model') + 1]
class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200 if self.path == '/health' else 404)
        self.end_headers()
    def do_POST(self):
        assert self.path == '/v1/audio/transcriptions'
        assert self.headers['Authorization'] == 'Bearer ' + key
        data = self.rfile.read(int(self.headers['Content-Length']))
        assert b'RIFF' in data and b'WAVE' in data
        if 'slow' in model:
            time.sleep(20)
        self.send_response(500 if 'http-error' in model else 200)
        self.end_headers()
        self.wfile.write(json.dumps({'text': 'Sounds bad.' if 'wrong' in model else 'Sounds good.'}).encode())
HTTPServer(('127.0.0.1', port), Handler).serve_forever()
''')
        self.runtime.chmod(0o755)
        self.model = self.root / "correct.gguf"
        self.model.write_bytes(b"fake model")

    def run_benchmark(self, *extra):
        result = subprocess.run([sys.executable, str(SCRIPTS / "benchmark-corpus.py"),
                                 "--corpus", str(self.root / "corpus"),
                                 "--runtime", str(self.runtime), "--model", str(self.model),
                                 "--rounds", "2", "--repeats", "2", "--max-wer", "0",
                                 "--output", str(self.root / "results"), *extra],
                                capture_output=True, text=True, timeout=45)
        path = self.root / "results/results.json"
        report = json.loads(path.read_text()) if path.exists() else None
        return result, report

    def test_runs_real_http_and_pairs_alternating_candidate_visits(self):
        result, report = self.run_benchmark("--candidate", "a:8:512", "--candidate", "b:4:512",
                                            "--candidate-runtime", f"b={self.runtime}")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(report["state"], "complete")
        self.assertEqual([(visit["round"], visit["candidate"]) for visit in report["visits"]],
                         [(1, "a"), (1, "b"), (2, "b"), (2, "a")])
        self.assertEqual(report["summary"]["a"]["one"]["request_ms"]["n"], 4)
        self.assertEqual(report["summary"]["a"]["one"]["corpus_wer"], 0)
        self.assertEqual(report["comparisons"]["b"]["one"]["paired_samples"], 4)
        self.assertEqual([row["text"] for row in report["records"]], ["Sounds good."] * 12)
        self.assertIn("[redacted]", report["visits"][0]["command"])

    def test_bad_accuracy_fails_without_hiding_its_latency(self):
        self.model = self.root / "wrong.gguf"
        self.model.write_bytes(b"fake model")
        result, report = self.run_benchmark()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual(report["state"], "complete")
        stats = report["summary"]["baseline"]["one"]
        self.assertEqual(stats["corpus_wer"], .5)
        self.assertEqual(stats["accuracy_failures"], 4)
        self.assertEqual(stats["request_ms"]["n"], 4)
        self.assertEqual([row["status"] for row in report["records"]], ["accuracy_failed"] * 6)

    def test_latency_budget_can_fail_even_when_every_transcript_is_correct(self):
        result, report = self.run_benchmark("--request-budget-ms", "0.000001")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(report["state"], "complete")
        self.assertEqual(report["summary"]["baseline"]["one"]["latency_budget_pass"], False)
        self.assertEqual([row["status"] for row in report["records"]], ["ok"] * 6)

    def test_http_failure_saves_partial_evidence_and_fails(self):
        self.model = self.root / "http-error.gguf"
        self.model.write_bytes(b"fake model")
        result, report = self.run_benchmark()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(report["state"], "error")
        self.assertEqual(report["records"][0]["status"], "error")
        self.assertIn("HTTPError", report["records"][0]["error"])
        self.assertEqual(report["summary"]["baseline"]["one"]["request_ms"], None)

    def test_interrupt_saves_evidence_and_reaps_only_the_owned_server(self):
        model = self.root / "slow.gguf"
        model.write_bytes(b"fake model")
        process = subprocess.Popen([sys.executable, str(SCRIPTS / "benchmark-corpus.py"),
                                    "--corpus", str(self.root / "corpus"),
                                    "--runtime", str(self.runtime), "--model", str(model),
                                    "--output", str(self.root / "results")],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.addCleanup(lambda: process.kill() if process.poll() is None else None)
        path = self.root / "results/results.json"
        deadline = time.monotonic() + 20
        server_pid = None
        while time.monotonic() < deadline:
            if path.exists():
                report = json.loads(path.read_text())
                if report["visits"] and report["visits"][0].get("pid"):
                    server_pid = report["visits"][0]["pid"]
                    break
            time.sleep(.05)
        self.assertIsNotNone(server_pid)
        process.terminate()
        self.assertEqual(process.wait(timeout=15), 130)
        report = json.loads(path.read_text())
        self.assertEqual(report["state"], "interrupted")
        self.assertEqual(report["visits"][0]["status"], "interrupted")
        with self.assertRaises(ProcessLookupError):
            os.kill(server_pid, 0)

    def test_reference_drift_is_rejected_before_any_runtime_starts(self):
        (self.root / "corpus/one.txt").write_text("A different reference.")
        result, report = self.run_benchmark()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("text checksum mismatch", result.stderr)
        self.assertIsNone(report)

    def test_decoder_rejects_pcm_drift_even_with_matching_container_hash(self):
        self.manifest["fixtures"][0]["pcm_sha256"] = "0" * 64
        (self.root / "corpus/manifest.json").write_text(json.dumps(self.manifest))
        result, report = self.run_benchmark()
        self.assertEqual(result.returncode, 1)
        self.assertIn("decoded PCM checksum mismatch", report["error"])
        self.assertEqual(report["visits"], [])


if __name__ == "__main__":
    unittest.main()
