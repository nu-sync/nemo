#!/usr/bin/env python3
"""Join daemon.log events by dictation ID and summarize outcomes, failures and timings.

Reads only stamped events (those with `ts`); older lines are counted and skipped.
Events carry no audio or transcript text, but `error` can name a microphone or a path.
"""

import argparse
import json
from collections import Counter
from pathlib import Path

from speech_corpus import distribution

# Step event -> the timing fields a dictation row takes from it.
STEP_FIELDS = {
    "capture_start": ("microphone_start_ms",),
    "audio_finalized": ("finalization_ms",),
    "inference_started": ("stop_to_inference_ms",),
    "transcription": ("audio_seconds", "request_ms", "reused_model", "engine"),
    "delivery": ("delivery_ms", "stop_to_dispatch_ms"),
}
TIMINGS = ("microphone_start_ms", "finalization_ms", "stop_to_inference_ms", "request_ms",
           "stop_to_dispatch_ms", "duration_ms")


def stamped_events(lines):
    """Parsed events that have a timestamp, and how many JSON events lacked one."""
    events, unstamped = [], 0
    for line in lines:
        try:
            event = json.loads(line)
        except (ValueError, TypeError):
            continue
        if not isinstance(event, dict) or "event" not in event:
            continue
        if isinstance(event.get("ts"), str):
            events.append(event)
        else:
            unstamped += 1
    return events, unstamped


def dictation_rows(events):
    """One row per dictation ID, in log order. `outcome` is None until the closing event."""
    rows = {}
    for event in events:
        identifier = event.get("dictation")
        if identifier is None:
            continue
        row = rows.setdefault(identifier, {
            "dictation": identifier, "ts": event["ts"], "build": event.get("build"),
            "version": event.get("version"), "outcome": None, "phase": None, "error": None,
            "failed_steps": []})
        kind = event["event"]
        if kind == "dictation":
            row.update({key: event.get(key) for key in ("outcome", "phase", "error", "duration_ms")})
        elif kind in STEP_FIELDS and event.get("success", True):
            row.update({key: event[key] for key in STEP_FIELDS[kind] if key in event})
        elif event.get("success") is False:
            row["failed_steps"].append(f"{kind}: {event.get('error')}")
    return list(rows.values())


def summarize(rows, events):
    finished = [row for row in rows if row["outcome"]]
    delivered = [row for row in finished if row["outcome"] == "delivered"]
    loads = [event for event in events if event["event"] == "model_load"]
    return {
        "dictations": len(rows),
        "unfinished": len(rows) - len(finished),
        "delivered_rate": round(len(delivered) / len(finished), 4) if finished else None,
        "outcomes": dict(Counter(f"{row['outcome']} at {row['phase']}" for row in finished).most_common()),
        "failures": dict(Counter(row["error"] for row in finished if row["outcome"] == "failed").most_common()),
        "failed_steps": dict(Counter(step for row in rows for step in row["failed_steps"]).most_common()),
        "cold_dictations": sum(row.get("reused_model") is False for row in rows),
        "audio_seconds": distribution([row["audio_seconds"] for row in delivered if "audio_seconds" in row]),
        "delivered_timings": {field: distribution([row[field] for row in delivered if row.get(field) is not None])
                              for field in TIMINGS},
        "model_loads": {"n": len(loads), "failed": sum(event.get("success") is False for event in loads),
                        "load_ms": distribution([event["load_ms"] for event in loads
                                                 if event.get("success") and "load_ms" in event])},
    }


def group_key(record, by):
    return {"all": "all", "build": record.get("build") or "no build",
            "version": record.get("version") or "no version", "day": record["ts"][:10]}[by]


def report(lines, by="all", since=None, until=None):
    events, unstamped = stamped_events(lines)
    events = [event for event in events
              if (since is None or event["ts"] >= since) and (until is None or event["ts"] < until)]
    rows = dictation_rows(events)
    keys = sorted({group_key(record, by) for record in rows + events})
    return {
        "scope": "stop_to_dispatch_ms is stop to paste dispatch, NOT visible insertion; timings cover delivered dictations only; ts is UTC",
        "unstamped_events_skipped": unstamped,
        "first_ts": events[0]["ts"] if events else None,
        "last_ts": events[-1]["ts"] if events else None,
        "groups": {key: summarize([row for row in rows if group_key(row, by) == key],
                                  [event for event in events if group_key(event, by) == key])
                   for key in keys},
    }


def trace(lines, which):
    """Every event of one dictation: an ID, `last`, or `slowest` by stop_to_dispatch_ms."""
    events, _ = stamped_events(lines)
    rows = dictation_rows(events)
    if not rows:
        raise ValueError("no stamped dictations in the log")
    if which == "last":
        identifier = rows[-1]["dictation"]
    elif which == "slowest":
        identifier = max(rows, key=lambda row: row.get("stop_to_dispatch_ms") or 0)["dictation"]
    else:
        identifier = int(which)
    found = [event for event in events if event.get("dictation") == identifier]
    if not found:
        raise ValueError(f"no events for dictation {identifier}")
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--log", type=Path, default=Path.home() / "Library/Application Support/Nemo/daemon.log")
    parser.add_argument("--by", choices=("all", "build", "version", "day"), default="all")
    parser.add_argument("--since", help="keep events at or after this UTC timestamp prefix, e.g. 2026-11-22T01:24")
    parser.add_argument("--until", help="keep events before this UTC timestamp prefix")
    parser.add_argument("--dictation", help="print one dictation's events: an ID, 'last' or 'slowest'")
    parser.add_argument("--rows", action="store_true", help="print one JSON row per dictation instead of a summary")
    parser.add_argument("--output", type=Path, help="optional new JSON report; refuses to overwrite")
    args = parser.parse_args()
    with args.log.open(errors="replace") as source:
        lines = list(source)
    if args.dictation:
        result = trace(lines, args.dictation)
    elif args.rows:
        events, _ = stamped_events(lines)
        result = dictation_rows([event for event in events
                                 if (args.since is None or event["ts"] >= args.since)
                                 and (args.until is None or event["ts"] < args.until)])
    else:
        result = report(lines, args.by, args.since, args.until)
    text = "\n".join(json.dumps(item) for item in result) if isinstance(result, list) else json.dumps(result, indent=2)
    print(text)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        with args.output.open("x") as destination:
            destination.write(text + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
