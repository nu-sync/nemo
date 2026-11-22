import importlib.util
import json
from pathlib import Path
import sys
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
spec = importlib.util.spec_from_file_location("telemetry_report", SCRIPTS / "telemetry-report.py")
telemetry = importlib.util.module_from_spec(spec)
spec.loader.exec_module(telemetry)


def event(kind, ts, dictation=None, build="aaa", **fields):
    line = dict(event=kind, ts=ts, version="0.2.0", build=build, **fields)
    if dictation is not None:
        line["dictation"] = dictation
    return json.dumps(line)


def delivered(identifier, ts, dispatch_ms, build="aaa"):
    return [
        event("capture_start", ts, identifier, build, success=True, microphone_start_ms=100),
        event("transcription", ts, identifier, build, success=True, audio_seconds=5, request_ms=200,
              reused_model=True, engine="mlx-audio"),
        event("delivery", ts, identifier, build, success=True, stop_to_dispatch_ms=dispatch_ms),
        event("dictation", ts, identifier, build, outcome="delivered", phase="delivery", duration_ms=6000),
    ]


LOG = [
    "Ready", "{bad json", json.dumps(dict(event="delivery", success=True, stop_to_dispatch_ms=1)),
    event("model_load", "2026-11-22T01:00:00.000Z", success=True, load_ms=2600),
    *delivered(1, "2026-11-22T01:01:00.000Z", 250),
    event("capture_start", "2026-11-22T01:02:00.000Z", 2, success=False, error="Could not open AirPods"),
    event("dictation", "2026-11-22T01:02:00.100Z", 2, outcome="failed", phase="microphone",
          error="Could not open AirPods", duration_ms=100),
    event("capture_start", "2026-11-23T01:03:00.000Z", 3, "bbb", success=True, microphone_start_ms=90),
    event("dictation", "2026-11-23T01:03:02.000Z", 3, "bbb", outcome="canceled", phase="recording", duration_ms=2000),
    *delivered(4, "2026-11-23T01:04:00.000Z", 400, "bbb"),
    event("capture_start", "2026-11-23T01:05:00.000Z", 5, "bbb", success=True, microphone_start_ms=95),
]


class TelemetryReportTests(unittest.TestCase):
    def test_joins_each_dictation_and_counts_outcomes_and_failure_reasons(self):
        summary = telemetry.report(LOG)
        group = summary["groups"]["all"]
        self.assertEqual(summary["unstamped_events_skipped"], 1)
        self.assertEqual((group["dictations"], group["unfinished"]), (5, 1))
        self.assertEqual(group["delivered_rate"], 0.5)
        self.assertEqual(group["outcomes"], {"delivered at delivery": 2, "failed at microphone": 1,
                                             "canceled at recording": 1})
        self.assertEqual(group["failures"], {"Could not open AirPods": 1})
        self.assertEqual(group["failed_steps"], {"capture_start: Could not open AirPods": 1})
        self.assertEqual(group["delivered_timings"]["stop_to_dispatch_ms"]["max"], 400)
        self.assertEqual(group["delivered_timings"]["microphone_start_ms"]["n"], 2)
        self.assertEqual(group["model_loads"]["n"], 1)

    def test_groups_by_build_and_day_so_a_change_can_be_compared(self):
        builds = telemetry.report(LOG, by="build")["groups"]
        self.assertEqual({key: group["dictations"] for key, group in builds.items()}, {"aaa": 2, "bbb": 3})
        self.assertEqual(builds["bbb"]["delivered_timings"]["stop_to_dispatch_ms"]["p50"], 400)
        self.assertEqual(sorted(telemetry.report(LOG, by="day")["groups"]), ["2026-11-22", "2026-11-23"])

    def test_a_time_window_keeps_only_its_events(self):
        late = telemetry.report(LOG, since="2026-11-23")["groups"]["all"]
        self.assertEqual((late["dictations"], late["model_loads"]["n"]), (3, 0))
        early = telemetry.report(LOG, until="2026-11-23")["groups"]["all"]
        self.assertEqual(early["dictations"], 2)

    def test_traces_one_dictation_by_id_last_or_slowest(self):
        self.assertEqual([line["event"] for line in telemetry.trace(LOG, "2")], ["capture_start", "dictation"])
        self.assertEqual({line["dictation"] for line in telemetry.trace(LOG, "last")}, {5})
        self.assertEqual({line["dictation"] for line in telemetry.trace(LOG, "slowest")}, {4})
        with self.assertRaisesRegex(ValueError, "no events for dictation 99"):
            telemetry.trace(LOG, "99")
        with self.assertRaisesRegex(ValueError, "no stamped dictations"):
            telemetry.trace(["Ready"], "last")


if __name__ == "__main__":
    unittest.main()
