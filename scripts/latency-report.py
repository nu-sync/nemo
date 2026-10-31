#!/usr/bin/env python3
"""Summarize actual daemon timings without estimating insertion latency or joining unkeyed events."""

import argparse
import json
import math
from pathlib import Path

from speech_corpus import distribution


def valid_number(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) and value >= 0


def latency_report(lines):
    transcriptions, deliveries = [], []
    failures = {"transcription": 0, "delivery": 0}
    for line in lines:
        try:
            event = json.loads(line)
        except (ValueError, TypeError):
            continue
        if not isinstance(event, dict) or event.get("event") not in failures:
            continue
        kind = event["event"]
        if event.get("success") is False:
            failures[kind] += 1
            continue
        if event.get("success") is not True:
            continue
        if kind == "transcription" and all(valid_number(event.get(field)) for field in ("audio_seconds", "request_ms")):
            transcriptions.append(event)
        if kind == "delivery" and valid_number(event.get("stop_to_dispatch_ms")):
            deliveries.append(event["stop_to_dispatch_ms"])
    return {
        "scope": "request_ms is HTTP inference; stop_to_dispatch_ms is observed stop-to-dispatch, NOT visible insertion; events have no correlation IDs and are not joined",
        "failures": failures,
        "transcriptions": {"n": len(transcriptions),
                           "audio_seconds": distribution([event["audio_seconds"] for event in transcriptions]),
                           "request_ms": distribution([event["request_ms"] for event in transcriptions]),
                           "reused_model_request_ms": distribution([event["request_ms"] for event in transcriptions if event.get("reused_model") is True]),
                           "new_model_request_ms": distribution([event["request_ms"] for event in transcriptions if event.get("reused_model") is False])},
        "stop_to_dispatch_ms": distribution(deliveries),
        "duration_buckets": [{"min_seconds": low, "max_seconds_exclusive": high,
                              "n": len(bucket), "request_ms": distribution([event["request_ms"] for event in bucket])}
                             for low, high in ((0, 2), (2, 5), (5, 10), (10, 20), (20, 40), (40, 61), (61, None))
                             for bucket in [[event for event in transcriptions if event["audio_seconds"] >= low
                                             and (high is None or event["audio_seconds"] < high)]]],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--log", type=Path, default=Path.home() / "Library/Application Support/Nemo/daemon.log")
    parser.add_argument("--output", type=Path, help="optional new JSON report; refuses to overwrite")
    args = parser.parse_args()
    with args.log.open(errors="replace") as source:
        report = latency_report(source)
    print(json.dumps(report, indent=2))
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        with args.output.open("x") as destination:
            destination.write(json.dumps(report, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
