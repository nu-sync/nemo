#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Read-only completeness/accuracy/cleanup audit; not a latency-winner or SLO judge."""

import argparse
from collections import Counter
import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import re
import sys


def require(condition, message):
    if not condition:
        raise ValueError(message)


def positive_int(value):
    return isinstance(value, int) and not isinstance(value, bool) and value > 0


def finite(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def audit(report, expect_accuracy_failures=False, check_cleanup=False):
    require(report.get("schema_version") == 1 and report.get("state") == "complete",
            "report is unsupported or incomplete")
    rounds, repeats = report["rounds"], report["repeats"]
    require(positive_int(rounds) and positive_int(repeats), "invalid visit/repeat counts")
    names = [config["name"] for config in report["candidates"]]
    selected = report["corpus"]["selected"]
    require(names and len(names) == len(set(names)), "missing/duplicate candidate names")
    require(selected and len(selected) == len(set(selected)), "missing/duplicate selected fixtures")
    fixture_ids = {fixture["id"] for fixture in report["corpus"]["manifest"]["fixtures"]}
    require(set(selected) <= fixture_ids, "selected fixture absent from manifest")
    for config in report["candidates"]:
        for kind in ("runtime", "model"):
            require(re.fullmatch(r"[a-f0-9]{64}", config[kind]["sha256"]) is not None,
                    f"missing {kind} identity")
    threshold = report["max_wer"]
    require(finite(threshold) and 0 <= threshold <= 1, "invalid WER threshold")
    expected_visits = [(number, name) for number in range(1, rounds + 1)
                       for name in (names if number % 2 else list(reversed(names)))]
    visits = report["visits"]
    require([(visit["round"], visit["candidate"]) for visit in visits] == expected_visits,
            "missing visits or incorrect alternating candidate order")
    for visit in visits:
        require(visit["status"] == "ok" and finite(visit.get("load_to_health_ms"))
                and visit["load_to_health_ms"] > 0, "visit lacks successful health/readiness evidence")
        require(positive_int(visit.get("pid")), "visit lacks an owned runtime PID")
    records = report["records"]
    failures = 0
    for row in records:
        require((row["round"], row["candidate"]) in expected_visits and row["fixture"] in selected,
                "record is outside the selected visit/fixture set")
        require(row["stage"] in ("first", "warm"), "unknown request stage")
        require(isinstance(row.get("text"), str) and "accuracy" in row, "missing real response/accuracy")
        require(finite(row.get("request_ms")) and row["request_ms"] > 0, "invalid request latency")
        score = row["accuracy"]
        require(positive_int(score["reference_words"]) and finite(score["wer"])
                and score["wer"] >= 0 and finite(score["errors"]) and score["errors"] >= 0,
                "invalid word-error evidence")
        require(math.isclose(score["wer"], score["errors"] / score["reference_words"], abs_tol=1e-12),
                "WER does not agree with error/reference counts")
        inaccurate = score["wer"] > threshold
        require(row["status"] == ("accuracy_failed" if inaccurate else "ok"),
                "transport error or inconsistent accuracy status")
        failures += inaccurate
    for number in range(1, rounds + 1):
        schedules = []
        for name in names:
            rows = [row for row in records if row["round"] == number and row["candidate"] == name]
            first = [row for row in rows if row["stage"] == "first"]
            require(len(first) == 1 and first[0]["repeat"] == 0, "missing/duplicate first request")
            warm = [(row["fixture"], row["repeat"]) for row in rows if row["stage"] == "warm"]
            expected = [(fixture, repeat) for fixture in selected for repeat in range(1, repeats + 1)]
            require(Counter(warm) == Counter(expected), "missing/duplicate warm request pairs")
            schedules.append(warm)
        require(all(schedule == schedules[0] for schedule in schedules), "candidate schedules differ")
    require(bool(failures) == expect_accuracy_failures,
            "accuracy failures were not expected" if failures else "expected accuracy failure was not reproduced")
    require(all(stats["latency_budget_pass"] is not False
                for fixtures in report["summary"].values() for stats in fixtures.values()),
            "a declared HTTP latency budget failed")
    if check_cleanup:
        for pid in {visit["pid"] for visit in visits}:
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                continue
            except PermissionError as error:
                raise ValueError(f"cleanup inconclusive for PID {pid}: {error}") from error
            raise ValueError(f"PID {pid} exists: cleanup inconclusive (possibly PID reuse); inspect identity, do not kill blindly")
    return {"verdict": "complete_accuracy_failure_reproduced" if failures else "complete_accurate_run",
            "visits": len(visits), "first_requests": len(visits),
            "warm_requests": len(visits) * len(selected) * repeats,
            "warm_samples_per_fixture_per_candidate": rounds * repeats,
            "accuracy_failures": failures, "cleanup_checked": check_cleanup,
            "scope": "report structure, recorded readiness/accuracy, and optional immediate PID absence; not an optimization, independent ASR, or end-to-end SLO verdict"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    parser.add_argument("--expect-accuracy-failures", action="store_true")
    parser.add_argument("--check-cleanup", action="store_true", help="check recorded PIDs immediately after a new run; old PIDs can be reused")
    parser.add_argument("--output", type=Path, help="optional new audit JSON; never overwrites the benchmark")
    args = parser.parse_args()
    try:
        raw = args.report.read_bytes()
        result = audit(json.loads(raw), args.expect_accuracy_failures, args.check_cleanup)
        result.update(report_sha256=hashlib.sha256(raw).hexdigest(),
                      audited_utc=datetime.datetime.now(datetime.timezone.utc).isoformat())
        text = json.dumps(result, indent=2) + "\n"
        if args.output:
            with args.output.open("x") as destination:
                destination.write(text)
        print(text, end="")
        return 0
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"Audit failed or inconclusive: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
