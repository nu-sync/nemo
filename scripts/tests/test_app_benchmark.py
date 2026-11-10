import importlib.util
from pathlib import Path
import sys
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
spec = importlib.util.spec_from_file_location("benchmark_app", SCRIPTS / "benchmark-app.py")
app = importlib.util.module_from_spec(spec)
spec.loader.exec_module(app)

FIXTURES = [{"id": "short", "target_seconds": 5}, {"id": "long", "target_seconds": 60}]


def records(engine, fixture, warm_ms, errors, cold_ms=None):
    accuracy = {"errors": errors, "reference_words": 20, "wer": errors / 20}
    rows = [{"engine": engine, "fixture": fixture, "stage": "warm", "request_ms": value,
             "total_ms": value, "text": f"{engine}-{errors}", "accuracy": accuracy} for value in warm_ms]
    if cold_ms:
        rows.append({"engine": engine, "fixture": fixture, "stage": "cold", "request_ms": 300,
                     "total_ms": cold_ms, "load_ms": cold_ms - 300, "text": f"{engine}-{errors}",
                     "accuracy": accuracy})
    return rows


def decision(mlx_short, mlx_long, mlx_long_errors=1):
    rows = (records("nemo-speech", "short", [300, 310, 290], 0, cold_ms=2900)
            + records("nemo-speech", "long", [1900, 1920, 1880], 1)
            + records("mlx-audio", "short", mlx_short, 0, cold_ms=4300)
            + records("mlx-audio", "long", mlx_long, mlx_long_errors))
    summary = app.summarize(rows, [], FIXTURES, "nemo-speech", "mlx-audio")
    return summary, app.decide(summary, 1.4)


class GateTests(unittest.TestCase):
    def test_a_faster_engine_with_the_same_word_errors_is_promoted(self):
        summary, verdict = decision([180, 185, 175], [1200, 1210, 1190])
        self.assertTrue(verdict["promote"])
        self.assertAlmostEqual(summary["buckets"][0]["warm_speedup"], 300 / 180)
        self.assertEqual(summary["buckets"][0]["engines"]["mlx-audio"]["warm_request_ms"],
                         {"n": 3, "median": 180, "min": 175, "max": 185})

    def test_cold_requests_are_reported_apart_from_warm_ones(self):
        summary, _ = decision([180, 185, 175], [1200, 1210, 1190])
        short = summary["buckets"][0]
        self.assertEqual(short["engines"]["mlx-audio"]["cold_total_ms"]["median"], 4300)
        self.assertLess(short["cold_speedup"], 1)
        self.assertGreater(short["warm_speedup"], 1.4)
        self.assertNotIn("cold_speedup", summary["buckets"][1])

    def test_a_lost_word_on_one_bucket_blocks_promotion_however_fast(self):
        _, verdict = decision([100, 100, 100], [600, 600, 600], mlx_long_errors=2)
        self.assertTrue(verdict["speed_gate"])
        self.assertEqual(verdict["buckets_with_lost_accuracy"], ["long"])
        self.assertFalse(verdict["promote"])

    def test_a_gained_word_is_reported_as_changed_not_silently_accepted(self):
        _, verdict = decision([180, 185, 175], [1200, 1210, 1190], mlx_long_errors=0)
        self.assertEqual(verdict["buckets_with_changed_accuracy"], ["long"])
        self.assertTrue(verdict["accuracy_not_worse_anywhere"])
        self.assertFalse(verdict["promote"])

    def test_an_unchanged_but_insufficiently_faster_engine_is_not_promoted(self):
        _, verdict = decision([230, 235, 225], [1500, 1510, 1490])
        self.assertFalse(verdict["speed_gate"])
        self.assertFalse(verdict["promote"])

    def test_a_run_whose_repeats_disagree_is_not_evidence_for_promotion(self):
        steady = [180 + step % 3 for step in range(10)]
        contended = steady[:-1] + [290]
        _, verdict = decision(steady, [1200] * 3)
        self.assertTrue(verdict["quiet_machine"] and verdict["promote"])
        _, verdict = decision(contended, [1200] * 3)
        self.assertTrue(verdict["speed_gate"])
        self.assertEqual(verdict["widest_repeat_spread"]["case"], "short/mlx-audio")
        self.assertFalse(verdict["quiet_machine"])
        self.assertFalse(verdict["promote"])

if __name__ == "__main__":
    unittest.main()
