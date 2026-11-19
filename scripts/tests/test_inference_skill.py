import importlib.util
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / ".agents/skills/optimize-nemo-inference/scripts/compare-bundles.py"
spec = importlib.util.spec_from_file_location("compare_bundles", SCRIPT)
compare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compare)


def visit(text, cold_ms, warm_ms):
    return {"texts": [text, text], "first": {"request_ms": cold_ms},
            "warm": {"request_ms": warm_ms}}


class ComparisonTests(unittest.TestCase):
    def test_paired_timing_uses_the_reused_model_request(self):
        pair = {"baseline": visit("Lunch.", 2500, 200), "candidate": visit("Lunch.", 3000, 150)}
        self.assertEqual(compare.paired_reduction(pair, "Lunch."), 25.0)

    def test_even_a_punctuation_change_disqualifies_a_speed_sample(self):
        pair = {"baseline": visit("Lunch.", 2500, 200), "candidate": visit("Lunch.", 2500, 150)}
        pair["candidate"]["texts"][1] = "Lunch"
        with self.assertRaisesRegex(ValueError, "Raw transcripts differ"):
            compare.paired_reduction(pair, "Lunch.")

    def test_both_bundles_drifting_from_an_earlier_round_is_detected(self):
        pair = {"baseline": visit("Lunch", 2500, 200), "candidate": visit("Lunch", 2500, 150)}
        with self.assertRaisesRegex(ValueError, "Raw transcripts differ"):
            compare.paired_reduction(pair, "Lunch.")
