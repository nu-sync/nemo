use crate::config::{self, Config};
use anyhow::{Context, Result, bail, ensure};
use reqwest::blocking::{Client, multipart};
use serde::Deserialize;
use std::{
    fs::{self, File},
    io::Read,
    net::TcpListener,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
struct ProcessState {
    child: Option<Child>,
    shutdown: bool,
}

#[derive(Clone, Default)]
pub struct Process(Arc<Mutex<ProcessState>>);

impl Process {
    pub fn run(&self, command: &mut Command, canceled: &AtomicBool) -> Result<()> {
        let mut child = command.spawn().context("Cannot start model download")?;
        {
            let mut slot = self.0.lock().unwrap();
            if slot.shutdown || canceled.load(Ordering::Acquire) {
                let _ = child.kill();
                let _ = child.wait();
                bail!("Canceled");
            }
            slot.child = Some(child);
        }
        loop {
            if canceled.load(Ordering::Acquire) {
                self.stop();
                bail!("Canceled");
            }
            let mut slot = self.0.lock().unwrap();
            let child = slot.child.as_mut().context("Download stopped")?;
            if let Some(status) = child.try_wait()? {
                slot.child = None;
                ensure!(
                    status.success(),
                    "Model download failed ({status}); retry to resume"
                );
                return Ok(());
            }
            drop(slot);
            thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn stop(&self) {
        if let Some(mut child) = self.0.lock().unwrap().child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    pub fn shutdown(&self) {
        self.0.lock().unwrap().shutdown = true;
        self.stop();
    }

    fn running(&self) -> Result<bool> {
        let mut slot = self.0.lock().unwrap();
        if let Some(child) = slot.child.as_mut() {
            if child.try_wait()?.is_none() {
                return Ok(true);
            }
            slot.child = None;
        }
        Ok(false)
    }
}

pub struct Backend {
    config: Config,
    pub process: Process,
    base: Option<String>,
    key: String,
    client: Client,
}

impl Backend {
    pub fn new(config: Config, process: Process) -> Result<Self> {
        let mut random = [0_u8; 24];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        let key = random.iter().map(|byte| format!("{byte:02x}")).collect();
        Ok(Self {
            config,
            process,
            base: None,
            key,
            client: Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(300))
                .build()?,
        })
    }

    pub fn loaded(&self) -> bool {
        self.base.is_some()
    }

    pub fn unload(&mut self) -> bool {
        self.process.stop();
        self.base.take().is_some()
    }

    pub fn ready(&mut self, canceled: &AtomicBool) -> Result<bool> {
        ensure!(!canceled.load(Ordering::Acquire), "Canceled");
        if self.base.is_some() && self.process.running()? {
            return Ok(true);
        }
        let started = Instant::now();
        let result = self.load(canceled);
        eprintln!(
            "{}",
            serde_json::json!({
                "event": "model_load",
                "load_ms": started.elapsed().as_secs_f64() * 1000.0,
                "success": result.is_ok(),
            })
        );
        result.map(|()| false)
    }

    fn load(&mut self, canceled: &AtomicBool) -> Result<()> {
        self.unload();
        self.config.check_assets()?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        drop(listener);
        fs::create_dir_all(config::data_dir())?;
        let log = File::create(config::data_dir().join("inference.log"))?;
        let mut command = Command::new(&self.config.runtime);
        command
            .args(["serve", "--asr-model"])
            .arg(&self.config.model)
            .args([
                "--host",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--api-key",
                &self.key,
                "--device",
                "metal",
                "--threads",
                "4",
                "--no-ui",
                "--asr.backend.threads",
                &self.config.inference_threads.to_string(),
                "--asr.salm.max_sequences",
                "1",
                "--asr.salm.n_ctx_per_sequence",
                "8192",
                "--asr.salm.max_context",
                "8192",
                "--asr.salm.n_batch",
                "512",
                "--asr.salm.encoder_max_batch",
                &self.config.encoder_batch_size.to_string(),
                "--asr.salm.max_new_tokens",
                "4096",
                "--asr.salm.mtp_draft",
                "0",
                "--write-timeout",
                "300",
                "--max-upload-mb",
                "64",
            ]);
        if !self.config.model_warmup {
            command.arg("--no-warmup");
        }
        let mut child = command
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()
            .context("Cannot start inference runtime")?;
        {
            let mut slot = self.process.0.lock().unwrap();
            if slot.shutdown || canceled.load(Ordering::Acquire) {
                let _ = child.kill();
                let _ = child.wait();
                bail!("Canceled");
            }
            slot.child = Some(child);
        }
        let base = format!("http://127.0.0.1:{port}");
        let start = Instant::now();
        loop {
            if canceled.load(Ordering::Acquire) {
                self.unload();
                bail!("Canceled");
            }
            ensure!(
                self.process.running()?,
                "Inference runtime exited. See {}",
                config::data_dir().join("inference.log").display()
            );
            if let Ok(response) = self
                .client
                .get(format!("{base}/health"))
                .timeout(Duration::from_millis(500))
                .send()
                && response.status().is_success()
            {
                self.base = Some(base);
                return Ok(());
            }
            if start.elapsed() > Duration::from_secs(120) {
                self.unload();
                bail!("Model loading timed out. See inference.log");
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn transcribe(&mut self, audio: Vec<u8>, canceled: &AtomicBool) -> Result<String> {
        let started = Instant::now();
        let mut audio_seconds = None;
        let mut model_load_ms = None;
        let mut reused_model = false;
        let mut request_started = None;
        let result = (|| {
            ensure!(audio.len() <= 64 * 1024 * 1024, "WAV input exceeds 64 MiB");
            let wav = hound::WavReader::new(std::io::Cursor::new(&audio))
                .context("Input must be a readable WAV file")?;
            ensure!(
                wav.spec().sample_rate > 0 && wav.duration() > 0,
                "WAV input is empty"
            );
            let seconds = wav.duration() as f64 / wav.spec().sample_rate as f64;
            audio_seconds = Some(seconds);
            ensure!(
                seconds <= 300.0,
                "WAV input exceeds the five-minute dictation limit"
            );
            let loading = Instant::now();
            let ready = self.ready(canceled);
            model_load_ms = Some(loading.elapsed().as_secs_f64() * 1000.0);
            reused_model = ready?;
            if reused_model {
                model_load_ms = Some(0.0);
            }
            ensure!(!canceled.load(Ordering::Acquire), "Canceled");
            request_started = Some(Instant::now());
            let part = multipart::Part::bytes(audio)
                .file_name("recording.wav")
                .mime_str("audio/wav")?;
            let form = multipart::Form::new()
                .part("file", part)
                .text("response_format", "json");
            let response = self
                .client
                .post(format!(
                    "{}/v1/audio/transcriptions",
                    self.base.as_ref().unwrap()
                ))
                .bearer_auth(&self.key)
                .multipart(form)
                .send()
                .context("Transcription request failed")?;
            let status = response.status();
            let body = response.text()?;
            ensure!(
                status.is_success(),
                "Transcription failed ({status}): {}",
                body.chars().take(1000).collect::<String>()
            );
            ensure!(!canceled.load(Ordering::Acquire), "Canceled");
            #[derive(Deserialize)]
            struct Transcript {
                text: String,
            }
            let result: Transcript =
                serde_json::from_str(&body).context("Invalid response from inference runtime")?;
            let text = result.text.trim().to_string();
            ensure!(!text.is_empty(), "No speech recognized");
            Ok(text)
        })();
        let request_ms =
            request_started.map(|start: Instant| start.elapsed().as_secs_f64() * 1000.0);
        let real_time_factor = if result.is_ok() {
            request_ms
                .zip(audio_seconds)
                .map(|(ms, seconds)| ms / 1000.0 / seconds)
        } else {
            None
        };
        eprintln!(
            "{}",
            serde_json::json!({
                "event": "transcription",
                "audio_seconds": audio_seconds,
                "model_load_ms": model_load_ms,
                "request_ms": request_ms,
                "total_ms": started.elapsed().as_secs_f64() * 1000.0,
                "reused_model": reused_model,
                "real_time_factor": real_time_factor,
                "success": result.is_ok(),
            })
        );
        result
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.unload();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stop_reaps_the_owned_child_and_is_repeatable() {
        let process = Process::default();
        let child = Command::new("/bin/sleep").arg("60").spawn().unwrap();
        let pid = child.id();
        process.0.lock().unwrap().child = Some(child);
        assert!(process.running().unwrap());
        process.stop();
        process.stop();
        assert!(!process.running().unwrap());
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
    }

    #[test]
    fn invalid_audio_is_rejected_before_loading_weights() {
        let mut backend = Backend::new(Config::default(), Process::default()).unwrap();
        let error = backend
            .transcribe(vec![0; 44], &AtomicBool::new(false))
            .unwrap_err();
        assert!(error.to_string().contains("readable WAV"));
        assert!(!backend.process.running().unwrap());
    }

    #[test]
    #[ignore = "requires downloaded model and artifacts/sample.wav; run scripts/verify.sh"]
    fn real_model_transcribes_repeated_audio() {
        let mut backend = Backend::new(Config::load().unwrap(), Process::default()).unwrap();
        let audio = fs::read("artifacts/sample.wav").unwrap();
        let canceled = AtomicBool::new(false);
        let started = Instant::now();
        backend.ready(&canceled).unwrap();
        println!("model load: {:.3}s", started.elapsed().as_secs_f64());
        for run in 1..=3 {
            let started = Instant::now();
            let text = backend.transcribe(audio.clone(), &canceled).unwrap();
            assert_eq!(
                text,
                "The quick brown fox jumps over the lazy dog. This is a local speech recognition test on my laptop."
            );
            println!("warm run {run}: {:.3}s", started.elapsed().as_secs_f64());
        }
    }
}
