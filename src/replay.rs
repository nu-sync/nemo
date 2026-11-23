use crate::{
    audio::{self, RecordingPreview},
    config::Config,
    engines::Engines,
    output, telemetry,
};
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    io::BufReader,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const SAMPLE_RATE: u32 = 48_000;
const CHUNK_FRAMES: usize = SAMPLE_RATE as usize / 100;

#[derive(serde::Serialize)]
pub struct ReplayResult {
    pub text: String,
    pub formatted: String,
    pub stop_to_text_ms: f64,
    pub preview_tokens: usize,
}

pub fn transcribe(
    path: &Path,
    engines: &mut Engines,
    config: &Config,
    canceled: &AtomicBool,
) -> Result<ReplayResult> {
    let pcm = read_pcm(path)?;
    engines.ready(canceled)?;

    let samples = Arc::new(Mutex::new(Vec::with_capacity(pcm.len())));
    let preview = config
        .recording_previews()
        .then(|| RecordingPreview::new(&samples, SAMPLE_RATE));
    let started = Instant::now();
    let mut end = 0;
    let mut triggers = audio::RecordingTriggers::default();
    let mut speech_boundary_submissions = 0;
    for chunk in pcm.chunks(CHUNK_FRAMES) {
        let start = end;
        end += chunk.len();
        let deadline = started + Duration::from_secs_f64(end as f64 / f64::from(SAMPLE_RATE));
        thread::sleep(deadline.saturating_duration_since(Instant::now()));
        ensure!(!canceled.load(Ordering::Acquire), "Canceled");
        let mut captured = samples.lock().unwrap();
        captured.extend(chunk.iter().map(|value| f32::from(*value) / 32_768.0));
        let rms = audio::block_rms(&captured[start..]);
        drop(captured);
        if end < pcm.len()
            && let Some(preview) = &preview
            && let Some(work) = triggers.observe(start, end, SAMPLE_RATE, rms)
        {
            engines.recording_work(preview, work);
            speech_boundary_submissions +=
                usize::from(work == audio::RecordingWork::SpeechBoundary);
        }
    }

    let stopped = Instant::now();
    let hint = preview
        .and_then(|preview| preview.ready().cloned())
        .unwrap_or_default();
    let wav = audio::finish_samples(&samples.lock().unwrap(), SAMPLE_RATE)?;
    let raw = engines.transcribe(wav, canceled, &hint)?;
    let formatted = output::format_transcript(&raw, config.remove_trailing_period).to_owned();
    let elapsed_ms = stopped.elapsed().as_secs_f64() * 1_000.0;
    telemetry::emit(
        "recording_replay",
        serde_json::json!({"stop_to_text_ms": elapsed_ms, "preview_tokens": hint.len(), "speech_boundary_submissions": speech_boundary_submissions}),
    );
    Ok(ReplayResult {
        text: raw,
        formatted,
        stop_to_text_ms: elapsed_ms,
        preview_tokens: hint.len(),
    })
}

fn read_pcm(path: &Path) -> Result<Vec<i16>> {
    let file = File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
    let mut reader = hound::WavReader::new(BufReader::new(file))?;
    let spec = reader.spec();
    ensure!(
        spec.sample_rate == SAMPLE_RATE
            && spec.channels == 1
            && spec.bits_per_sample == 16
            && spec.sample_format == hound::SampleFormat::Int,
        "Recording replay requires a 48 kHz mono PCM16 WAV"
    );
    ensure!(
        reader.duration() <= 300 * SAMPLE_RATE,
        "WAV input exceeds the five-minute dictation limit"
    );
    Ok(reader.samples::<i16>().collect::<Result<_, _>>()?)
}
