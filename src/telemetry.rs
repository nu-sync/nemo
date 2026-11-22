//! Structured events: one JSON object per line on stderr, which the daemon appends to
//! `daemon.log`.
//!
//! Every event says when it happened, which build wrote it and which dictation it belongs
//! to, so lines can be joined and compared across releases. Each dictation ends with one
//! `dictation` event naming its outcome. Events never carry audio or transcript text.

use serde_json::{Map, Value, json};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static TRACKER: Mutex<Tracker> = Mutex::new(Tracker {
    last_id: 0,
    current: None,
});

/// The daemon's executable digest, shortened. The version alone cannot tell two
/// development builds apart.
static BUILD: OnceLock<String> = OnceLock::new();

pub fn set_build(executable_sha256: &str) {
    let _ = BUILD.set(executable_sha256.chars().take(12).collect());
}

/// How far a dictation got. The event loop advances it alongside its own state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Microphone,
    Recording,
    Audio,
    Transcription,
    Delivery,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::Microphone => "microphone",
            Self::Recording => "recording",
            Self::Audio => "audio",
            Self::Transcription => "transcription",
            Self::Delivery => "delivery",
        }
    }
}

pub enum Ending {
    Delivered,
    Canceled,
    Failed(String),
}

struct Dictation {
    id: u64,
    started: Instant,
    phase: Phase,
}

struct Tracker {
    last_id: u64,
    current: Option<Dictation>,
}

impl Tracker {
    /// The ID is the start time in Unix milliseconds, so it stays unique across daemon
    /// restarts that append to the same log.
    fn begin(&mut self, unix_ms: u64) {
        self.last_id = unix_ms.max(self.last_id + 1);
        self.current = Some(Dictation {
            id: self.last_id,
            started: Instant::now(),
            phase: Phase::Microphone,
        });
    }

    fn advance(&mut self, phase: Phase) {
        if let Some(dictation) = &mut self.current {
            dictation.phase = phase;
        }
    }

    /// The closing event's ID and fields, or nothing when no dictation is under way: the
    /// event loop reports model loads and downloads through the same paths.
    fn finish(&mut self, ending: Ending) -> Option<(u64, Value)> {
        let dictation = self.current.take()?;
        let (outcome, error) = match ending {
            Ending::Delivered => ("delivered", None),
            Ending::Canceled => ("canceled", None),
            Ending::Failed(error) => ("failed", Some(error)),
        };
        let mut fields = json!({
            "outcome": outcome,
            "phase": dictation.phase.label(),
            "duration_ms": dictation.started.elapsed().as_secs_f64() * 1000.0,
        });
        if let Some(error) = error {
            fields["error"] = error.into();
        }
        Some((dictation.id, fields))
    }
}

pub fn begin_dictation() {
    TRACKER.lock().unwrap().begin(unix_ms());
}

pub fn advance_dictation(phase: Phase) {
    TRACKER.lock().unwrap().advance(phase);
}

pub fn finish_dictation(ending: Ending) {
    let finished = TRACKER.lock().unwrap().finish(ending);
    if let Some((id, fields)) = finished {
        write(Some(id), "dictation", fields);
    }
}

/// Write one event, attributed to the dictation under way, if any.
pub fn emit(event: &str, fields: Value) {
    let dictation = TRACKER
        .lock()
        .unwrap()
        .current
        .as_ref()
        .map(|dictation| dictation.id);
    write(dictation, event, fields);
}

fn write(dictation: Option<u64>, event: &str, fields: Value) {
    let build = BUILD.get().map(String::as_str);
    eprintln!("{}", envelope(unix_ms(), build, dictation, event, fields));
}

/// Write one event for a step that can fail, with `success` and the failure's reason.
pub fn emit_result<T>(event: &str, result: &anyhow::Result<T>, fields: Value) {
    emit(event, with_result(result, fields));
}

fn with_result<T>(result: &anyhow::Result<T>, mut fields: Value) -> Value {
    fields["success"] = result.is_ok().into();
    if let Err(error) = result {
        fields["error"] = format!("{error:#}").into();
    }
    fields
}

fn envelope(
    unix_ms: u64,
    build: Option<&str>,
    dictation: Option<u64>,
    event: &str,
    fields: Value,
) -> Value {
    let mut line = match fields {
        Value::Object(fields) => fields,
        _ => Map::new(),
    };
    line.insert("event".into(), event.into());
    line.insert("ts".into(), timestamp(unix_ms).into());
    line.insert("version".into(), env!("CARGO_PKG_VERSION").into());
    if let Some(build) = build {
        line.insert("build".into(), build.into());
    }
    if let Some(dictation) = dictation {
        line.insert("dictation".into(), dictation.into());
    }
    Value::Object(line)
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// RFC 3339 in UTC with milliseconds.
fn timestamp(unix_ms: u64) -> String {
    let (days, ms) = (unix_ms / 86_400_000, unix_ms % 86_400_000);
    // Days since 1970-01-01 to a calendar date, counted in 400-year eras that start on
    // March 1 so the leap day falls at the end of a year.
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1_000 % 60,
        ms % 1_000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker() -> Tracker {
        Tracker {
            last_id: 0,
            current: None,
        }
    }

    #[test]
    fn every_event_says_when_which_build_and_which_dictation() {
        let line = envelope(
            1_795_374_603_123,
            Some("c6b7219a3184"),
            Some(7),
            "delivery",
            json!({"delivery_ms": 4.5}),
        );
        assert_eq!(
            line,
            json!({
                "event": "delivery",
                "ts": "2026-11-22T19:10:03.123Z",
                "version": env!("CARGO_PKG_VERSION"),
                "build": "c6b7219a3184",
                "dictation": 7,
                "delivery_ms": 4.5,
            })
        );
        let outside = envelope(0, None, None, "model_load", json!({}));
        assert_eq!(outside.get("dictation"), None);
        assert_eq!(outside.get("build"), None);
    }

    #[test]
    fn timestamps_follow_the_calendar_through_leap_days() {
        for (unix_ms, expected) in [
            (0, "1970-01-01T00:00:00.000Z"),
            (951_782_399_999, "2000-02-28T23:59:59.999Z"),
            (951_782_400_000, "2000-02-29T00:00:00.000Z"),
            (951_868_800_000, "2000-03-01T00:00:00.000Z"),
            (1_798_761_599_000, "2026-12-31T23:59:59.000Z"),
            (4_107_542_400_000, "2100-03-01T00:00:00.000Z"),
        ] {
            assert_eq!(timestamp(unix_ms), expected);
        }
    }

    #[test]
    fn a_failed_step_records_its_reason() {
        let failed: anyhow::Result<()> =
            Err(anyhow::anyhow!("device busy").context("Could not open microphone"));
        assert_eq!(
            with_result(&failed, json!({"microphone_start_ms": 3.0})),
            json!({
                "microphone_start_ms": 3.0,
                "success": false,
                "error": "Could not open microphone: device busy",
            })
        );
        assert_eq!(with_result(&Ok(()), json!({})), json!({"success": true}));
    }

    #[test]
    fn a_dictation_ends_once_with_its_outcome_and_the_phase_it_reached() {
        let mut tracker = tracker();
        tracker.begin(1_000);
        tracker.advance(Phase::Recording);
        tracker.advance(Phase::Audio);
        let (id, fields) = tracker
            .finish(Ending::Failed("Recording was shorter than 250 ms".into()))
            .unwrap();
        assert_eq!(id, 1_000);
        assert_eq!(fields["outcome"], "failed");
        assert_eq!(fields["phase"], "audio");
        assert_eq!(fields["error"], "Recording was shorter than 250 ms");
        assert!(fields["duration_ms"].as_f64().unwrap() >= 0.0);
        assert!(tracker.finish(Ending::Canceled).is_none());

        tracker.begin(2_000);
        let (_, fields) = tracker.finish(Ending::Delivered).unwrap();
        assert_eq!(fields["outcome"], "delivered");
        assert_eq!(fields["phase"], "microphone");
        assert_eq!(fields.get("error"), None);
    }

    #[test]
    fn work_outside_a_dictation_ends_nothing() {
        let mut tracker = tracker();
        tracker.advance(Phase::Delivery);
        assert!(tracker.finish(Ending::Delivered).is_none());
    }

    #[test]
    fn dictation_ids_stay_unique_when_the_clock_repeats() {
        let mut tracker = tracker();
        tracker.begin(5_000);
        tracker.begin(5_000);
        assert_eq!(tracker.current.as_ref().unwrap().id, 5_001);
        tracker.begin(4_000);
        assert_eq!(tracker.current.as_ref().unwrap().id, 5_002);
    }
}
