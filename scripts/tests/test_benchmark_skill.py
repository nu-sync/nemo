import copy
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[2] / ".agents/skills/benchmark-nemo/scripts/audit-run.py"
spec = importlib.util.spec_from_file_location("benchmark_skill_audit", SCRIPT)
audit_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit_module)


def report():
    candidates = [{"name": name, "runtime": {"sha256": "a" * 64}, "model": {"sha256": "b" * 64}}
                  for name in ("baseline", "candidate")]
    return {
        "schema_version": 1, "state": "complete", "rounds": 1, "repeats": 1, "max_wer": .05,
        "candidates": candidates,
        "corpus": {"selected": ["one"], "manifest": {"fixtures": [{"id": "one"}]}},
        "visits": [{"candidate": name, "round": 1, "status": "ok", "pid": pid,
                    "load_to_health_ms": 2500} for name, pid in (("baseline", 123), ("candidate", 124))],
        "records": [{"candidate": name, "round": 1, "fixture": "one", "stage": stage,
                     "repeat": repeat, "request_ms": 300, "status": "ok", "text": "Sounds good.",
                     "accuracy": {"reference_words": 2, "errors": 0, "wer": 0}}
                    for name in ("baseline", "candidate") for stage, repeat in (("first", 0), ("warm", 1))],
        "summary": {name: {"one": {"latency_budget_pass": None}} for name in ("baseline", "candidate")},
    }


class BenchmarkSkillAuditTests(unittest.TestCase):
    def test_audits_complete_paired_real_runner_shape_without_claiming_a_winner(self):
        result = audit_module.audit(report())
        self.assertEqual({key: result[key] for key in ("verdict", "visits", "first_requests", "warm_requests",
                                                       "warm_samples_per_fixture_per_candidate", "cleanup_checked")},
                         {"verdict": "complete_accurate_run", "visits": 2, "first_requests": 2,
                          "warm_requests": 2, "warm_samples_per_fixture_per_candidate": 1,
                          "cleanup_checked": False})

    def test_missing_or_duplicate_samples_cannot_pass(self):
        missing = report()
        missing["records"].pop()
        duplicate = report()
        duplicate["records"].append(copy.deepcopy(duplicate["records"][-1]))
        for data in (missing, duplicate):
            with self.assertRaisesRegex(ValueError, "missing/duplicate warm request pairs"):
                audit_module.audit(data)

    def test_reproducing_accuracy_failure_is_not_an_accurate_run(self):
        data = report()
        data["records"][-1].update(status="accuracy_failed", text="Sounds bad.",
                                   accuracy={"reference_words": 2, "errors": 1, "wer": .5})
        with self.assertRaisesRegex(ValueError, "accuracy failures were not expected"):
            audit_module.audit(data)
        result = audit_module.audit(data, expect_accuracy_failures=True)
        self.assertEqual(result["verdict"], "complete_accuracy_failure_reproduced")
        self.assertEqual(result["accuracy_failures"], 1)
        with self.assertRaisesRegex(ValueError, "expected accuracy failure was not reproduced"):
            audit_module.audit(report(), expect_accuracy_failures=True)

    def test_health_transport_and_budget_failures_are_not_expected_quality_failures(self):
        for kind, message in (("health", "health/readiness"), ("transport", "missing real response"),
                              ("budget", "HTTP latency budget failed")):
            data = report()
            if kind == "health":
                del data["visits"][0]["load_to_health_ms"]
            elif kind == "transport":
                del data["records"][0]["accuracy"]
            else:
                data["summary"]["candidate"]["one"]["latency_budget_pass"] = False
            with self.subTest(kind=kind), self.assertRaisesRegex(ValueError, message):
                audit_module.audit(data)

    def test_cleanup_probe_uses_only_signal_zero_and_never_kills_a_live_pid(self):
        with patch.object(audit_module.os, "kill", return_value=None) as probe:
            with self.assertRaisesRegex(ValueError, "cleanup inconclusive"):
                audit_module.audit(report(), check_cleanup=True)
            self.assertIn(probe.call_args.args, [(123, 0), (124, 0)])
        with patch.object(audit_module.os, "kill", side_effect=ProcessLookupError) as probe:
            result = audit_module.audit(report(), check_cleanup=True)
            self.assertEqual(result["cleanup_checked"], True)
            self.assertEqual(sorted(call.args for call in probe.call_args_list), [(123, 0), (124, 0)])


if __name__ == "__main__":
    unittest.main()
