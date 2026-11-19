use anyhow::{Context, Result, bail, ensure};
use cpal::{
    FromSample, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use std::{
    io::Cursor,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicU32, AtomicUsize, Ordering},
    },
};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Microphone {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    pub devices: Vec<Microphone>,
    pub default: Option<Microphone>,
}

impl Inputs {
    pub fn current() -> Result<Self> {
        let host = cpal::default_host();
        let describe = |device: cpal::Device| -> Result<Microphone> {
            Ok(Microphone {
                id: device.id()?.to_string(),
                name: device.description()?.name().to_owned(),
            })
        };
        let devices = host
            .input_devices()?
            .map(describe)
            .collect::<Result<Vec<_>>>()?;
        let default = host.default_input_device().map(describe).transpose()?;
        Ok(Self { devices, default })
    }

    pub fn resolve(&self, selected: Option<&Microphone>) -> Option<&Microphone> {
        selected
            .and_then(|selected| self.devices.iter().find(|d| d.id == selected.id))
            .or(self.default.as_ref())
    }
}

#[derive(Default)]
struct LevelMeter(AtomicU32);

impl LevelMeter {
    fn publish(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        let power = samples.iter().map(|sample| sample * sample).sum::<f32>();
        let rms = (power / samples.len() as f32).sqrt().clamp(0.0, 1.0);
        self.0.fetch_max(rms.to_bits(), Ordering::Relaxed);
    }

    fn take(&self) -> f32 {
        f32::from_bits(self.0.swap(0, Ordering::Relaxed))
    }
}

pub struct Recording {
    stream: cpal::Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    error: Arc<Mutex<Option<String>>>,
    sample_rate: u32,
    level: Arc<LevelMeter>,
    pub microphone: Microphone,
}

const PREVIEW_SECONDS: [usize; 4] = [3, 6, 12, 24];

/// Only the live recording owns this result. A late preview cannot reach the next recording.
pub struct RecordingPreview {
    samples: Weak<Mutex<Vec<f32>>>,
    sample_rate: u32,
    submitted: AtomicUsize,
    tokens: [OnceLock<Vec<u32>>; PREVIEW_SECONDS.len()],
}

impl RecordingPreview {
    pub fn new(samples: &Arc<Mutex<Vec<f32>>>, sample_rate: u32) -> Arc<Self> {
        Arc::new(Self {
            samples: Arc::downgrade(samples),
            sample_rate,
            submitted: AtomicUsize::new(0),
            tokens: Default::default(),
        })
    }

    pub fn take_prefix(&self) -> Option<(usize, Vec<f32>, u32)> {
        let submitted = self.submitted.load(Ordering::Relaxed);
        if submitted == PREVIEW_SECONDS.len() {
            return None;
        }
        let samples = self.samples.upgrade()?;
        let samples = samples.try_lock().ok()?;
        let rate = self.sample_rate as usize;
        let available = PREVIEW_SECONDS.partition_point(|s| s * rate <= samples.len());
        if available <= submitted
            || self.submitted.fetch_max(available, Ordering::Relaxed) >= available
        {
            return None;
        }
        let index = available - 1;
        Some((
            index,
            samples[..PREVIEW_SECONDS[index] * rate].to_vec(),
            self.sample_rate,
        ))
    }

    pub fn publish(&self, index: usize, tokens: Vec<u32>) {
        if !tokens.is_empty() {
            let _ = self.tokens[index].set(tokens);
        }
    }

    pub fn ready(&self) -> Option<&Vec<u32>> {
        self.tokens.iter().rev().find_map(OnceLock::get)
    }
}

impl Recording {
    pub fn start(max_seconds: u64, selected: Option<&Microphone>) -> Result<Self> {
        let inputs = Inputs::current()?;
        let microphone = inputs
            .resolve(selected)
            .context("No microphone available. Connect one or choose an input in Nemo Settings")?;
        match Self::open(max_seconds, microphone) {
            Ok(recording) => Ok(recording),
            Err(error) => {
                // macOS may have changed the default while the first input was opening.
                let refreshed = Inputs::current().ok();
                let fallback = refreshed
                    .as_ref()
                    .and_then(|inputs| inputs.resolve(None))
                    .filter(|device| device.id != microphone.id);
                if let Some(fallback) = fallback {
                    eprintln!("Could not open {}: {error:#}", microphone.name);
                    return Self::open(max_seconds, fallback)
                        .with_context(|| format!("Could not open {}", fallback.name));
                }
                Err(error).with_context(|| {
                    format!(
                        "Could not open {}. Choose another input in Nemo Settings",
                        microphone.name
                    )
                })
            }
        }
    }

    fn open(max_seconds: u64, microphone: &Microphone) -> Result<Self> {
        let device = cpal::default_host()
            .device_by_id(&microphone.id.parse()?)
            .context("Microphone disconnected before recording started")?;
        let supported = device.default_input_config().context(
            "Cannot open microphone. Check System Settings → Privacy & Security → Microphone",
        )?;
        let sample_rate = supported.sample_rate();
        let config: cpal::StreamConfig = supported.clone().into();
        let samples = Arc::new(Mutex::new(Vec::new()));
        let error = Arc::new(Mutex::new(None));
        let level = Arc::new(LevelMeter::default());
        let cap = sample_rate as usize * max_seconds as usize;
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => {
                build::<f32>(&device, &config, &samples, &error, &level, cap)?
            }
            cpal::SampleFormat::I16 => {
                build::<i16>(&device, &config, &samples, &error, &level, cap)?
            }
            cpal::SampleFormat::U16 => {
                build::<u16>(&device, &config, &samples, &error, &level, cap)?
            }
            format => bail!("Unsupported microphone sample format: {format}"),
        };
        stream.play()?;
        Ok(Self {
            microphone: microphone.clone(),
            stream,
            samples,
            error,
            sample_rate,
            level,
        })
    }

    pub fn preview(&self) -> Arc<RecordingPreview> {
        RecordingPreview::new(&self.samples, self.sample_rate)
    }

    pub fn take_level(&self) -> f32 {
        self.level.take()
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }

    pub fn finish(self) -> Result<Vec<u8>> {
        drop(self.stream);
        if let Some(error) = self.error.lock().unwrap().take() {
            bail!("Microphone failed: {error}");
        }
        let samples = self.samples.lock().unwrap();
        ensure!(
            samples.len() >= self.sample_rate as usize / 4,
            "Recording was shorter than 250 ms"
        );
        ensure!(
            samples.iter().any(|s| s.abs() > 0.001),
            "No audio detected. Check the microphone and its permission"
        );
        wav(&samples, self.sample_rate)
    }
}

pub fn probe(microphone: &Microphone) -> Result<String> {
    let device = cpal::default_host()
        .device_by_id(&microphone.id.parse()?)
        .context("Microphone disconnected before its stream could be checked")?;
    let supported = device
        .default_input_config()
        .context("Cannot read the microphone's default input configuration")?;
    let config: cpal::StreamConfig = supported.clone().into();
    let _stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => probe_stream::<f32>(&device, &config),
        cpal::SampleFormat::I16 => probe_stream::<i16>(&device, &config),
        cpal::SampleFormat::U16 => probe_stream::<u16>(&device, &config),
        format => bail!("Unsupported microphone sample format: {format}"),
    }
    .context("Cannot build the microphone input stream")?;
    Ok(format!(
        "{} Hz, {} channel(s), {:?}",
        config.sample_rate,
        config.channels,
        supported.sample_format()
    ))
}

fn probe_stream<T>(device: &cpal::Device, config: &cpal::StreamConfig) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    Ok(device.build_input_stream(config, |_: &[T], _| {}, |_| {}, None)?)
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    samples: &Arc<Mutex<Vec<f32>>>,
    error: &Arc<Mutex<Option<String>>>,
    level: &Arc<LevelMeter>,
    cap: usize,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let samples = samples.clone();
    let error = error.clone();
    let level = level.clone();
    let channels = config.channels as usize;
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _| {
            let mut samples = samples.lock().unwrap();
            let start = samples.len();
            let remaining = cap.saturating_sub(start);
            samples.extend(data.chunks_exact(channels).take(remaining).map(|frame| {
                frame.iter().map(|s| s.to_sample::<f32>()).sum::<f32>() / channels as f32
            }));
            level.publish(&samples[start..]);
        },
        move |err| {
            *error.lock().unwrap() = Some(err.to_string());
        },
        None,
    )?)
}

pub fn wav(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>> {
    let mut output = Cursor::new(Vec::new());
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::new(&mut output, spec)?;
    for sample in samples {
        writer.write_sample((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    Ok(output.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn microphone(id: &str, name: &str) -> Microphone {
        Microphone {
            id: id.into(),
            name: name.into(),
        }
    }

    #[test]
    fn selection_uses_identity_even_when_names_match_and_devices_reorder() {
        let first = microphone("first", "USB microphone");
        let second = microphone("second", "USB microphone");
        let inputs = Inputs {
            devices: vec![second.clone(), first.clone()],
            default: Some(first),
        };
        assert_eq!(inputs.resolve(Some(&second)), Some(&second));
    }

    #[test]
    fn disconnected_selection_resolves_to_default_and_reconnect_restores_it() {
        let selected = microphone("headset", "Headset");
        let built_in = microphone("built-in", "Built-in microphone");
        let mut inputs = Inputs {
            devices: vec![built_in.clone()],
            default: Some(built_in.clone()),
        };
        assert_eq!(inputs.resolve(Some(&selected)), Some(&built_in));
        inputs.devices.push(selected.clone());
        assert_eq!(inputs.resolve(Some(&selected)), Some(&selected));
        assert_eq!(inputs.resolve(None), Some(&built_in));
    }

    #[test]
    fn no_microphone_or_missing_default_never_selects_an_arbitrary_input() {
        let selected = microphone("headset", "Headset");
        let mut inputs = Inputs::default();
        assert_eq!(inputs.resolve(None), None);
        assert_eq!(inputs.resolve(Some(&selected)), None);
        inputs.devices.push(selected.clone());
        assert_eq!(inputs.resolve(Some(&selected)), Some(&selected));
        assert_eq!(inputs.resolve(None), None);
    }

    #[test]
    fn system_default_is_resolved_again_without_changing_explicit_selection() {
        let built_in = microphone("built-in", "Built-in microphone");
        let headset = microphone("headset", "Headset");
        let mut inputs = Inputs {
            devices: vec![built_in.clone(), headset.clone()],
            default: Some(built_in.clone()),
        };
        assert_eq!(inputs.resolve(None), Some(&built_in));
        inputs.default = Some(headset.clone());
        assert_eq!(inputs.resolve(None), Some(&headset));
        assert_eq!(inputs.resolve(Some(&built_in)), Some(&built_in));
    }

    #[test]
    #[ignore = "requires an available microphone and macOS microphone permission"]
    fn real_microphone_opens_by_id_and_automatically_falls_back() {
        let inputs = Inputs::current().unwrap();
        let default = inputs
            .default
            .as_ref()
            .expect("System default microphone required");
        let missing = microphone(
            "coreaudio:nemo-test-disconnected-input",
            "Disconnected test input",
        );
        for selection in [Some(&missing), None, Some(default)] {
            let recording = Recording::start(1, selection).unwrap();
            assert_eq!(recording.microphone.id, default.id);
            std::thread::sleep(std::time::Duration::from_millis(400));
            assert!(recording.error().is_none(), "{:?}", recording.error());
            {
                let samples = recording.samples.lock().unwrap();
                assert!(!samples.is_empty(), "Input stream produced no samples");
                let level = recording.take_level();
                assert!(level.is_finite() && (0.0..=1.0).contains(&level));
                if samples.iter().any(|sample| sample.abs() > 0.0) {
                    assert!(
                        level > 0.0,
                        "Captured audio never reached the display meter"
                    );
                }
            }
            drop(recording);
        }
    }

    #[test]
    fn meter_reports_captured_power_and_retains_activity_between_display_frames() {
        let meter = LevelMeter::default();
        meter.publish(&[0.0; 8]);
        assert_eq!(meter.take(), 0.0);
        meter.publish(&[-0.25, 0.25, -0.25, 0.25]);
        meter.publish(&[0.0; 8]);
        assert_eq!(meter.take(), 0.25);
        assert_eq!(meter.take(), 0.0);
        meter.publish(&[]);
        assert_eq!(meter.take(), 0.0);
        meter.publish(&[-2.0, 2.0]);
        assert_eq!(meter.take(), 1.0);
    }

    #[test]
    fn recording_offers_the_latest_available_prefix_and_keeps_the_newest_result() {
        let samples = Arc::new(Mutex::new(vec![1.0; 5]));
        let preview = RecordingPreview::new(&samples, 2);
        assert!(preview.take_prefix().is_none());
        samples.lock().unwrap().push(1.0);
        let (first, prefix, rate) = preview.take_prefix().unwrap();
        assert_eq!((first, prefix.len(), rate), (0, 6, 2));
        assert!(preview.take_prefix().is_none());
        samples.lock().unwrap().resize(26, 2.0);
        let (latest, prefix, _) = preview.take_prefix().unwrap();
        assert_eq!((latest, prefix.len()), (2, 24));
        preview.publish(latest, vec![3]);
        preview.publish(first, vec![1]);
        assert_eq!(preview.ready(), Some(&vec![3]));
        samples.lock().unwrap().resize(50, 3.0);
        assert_eq!(preview.take_prefix().unwrap().1.len(), 48);
        preview.publish(3, vec![]);
        assert_eq!(preview.ready(), Some(&vec![3]));
        assert!(preview.take_prefix().is_none());
        assert!(RecordingPreview::new(&samples, 2).ready().is_none());
    }

    #[test]
    fn wav_preserves_duration_and_clips_pcm() {
        let bytes = wav(&[-2.0, -0.5, 0.0, 0.5, 2.0], 16000).unwrap();
        let mut reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.spec().sample_rate, 16000);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(
            reader
                .samples::<i16>()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            vec![-32767, -16383, 0, 16383, 32767]
        );
    }
}
