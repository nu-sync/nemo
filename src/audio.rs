use anyhow::{Context, Result, bail, ensure};
use cpal::{
    FromSample, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use std::{
    io::Cursor,
    sync::{Arc, Mutex},
};

pub struct Recording {
    stream: cpal::Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    error: Arc<Mutex<Option<String>>>,
    sample_rate: u32,
}

impl Recording {
    pub fn start(max_seconds: u64) -> Result<Self> {
        let device = cpal::default_host()
            .default_input_device()
            .context("No microphone found")?;
        let supported = device.default_input_config().context(
            "Cannot open microphone. Check System Settings → Privacy & Security → Microphone",
        )?;
        let sample_rate = supported.sample_rate();
        let config: cpal::StreamConfig = supported.clone().into();
        let samples = Arc::new(Mutex::new(Vec::new()));
        let error = Arc::new(Mutex::new(None));
        let cap = sample_rate as usize * max_seconds as usize;
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(&device, &config, &samples, &error, cap)?,
            cpal::SampleFormat::I16 => build::<i16>(&device, &config, &samples, &error, cap)?,
            cpal::SampleFormat::U16 => build::<u16>(&device, &config, &samples, &error, cap)?,
            format => bail!("Unsupported microphone sample format: {format}"),
        };
        stream.play()?;
        Ok(Self {
            stream,
            samples,
            error,
            sample_rate,
        })
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

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    samples: &Arc<Mutex<Vec<f32>>>,
    error: &Arc<Mutex<Option<String>>>,
    cap: usize,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let samples = samples.clone();
    let error = error.clone();
    let channels = config.channels as usize;
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _| {
            let mut samples = samples.lock().unwrap();
            let remaining = cap.saturating_sub(samples.len());
            samples.extend(data.chunks_exact(channels).take(remaining).map(|frame| {
                frame.iter().map(|s| s.to_sample::<f32>()).sum::<f32>() / channels as f32
            }));
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
