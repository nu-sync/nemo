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
enum ProcessState {
    #[default]
    Unloaded,
    Starting(Child),
    Ready {
        child: Child,
        base: String,
    },
    Shutdown,
}

impl ProcessState {
    fn running(&mut self) -> Result<bool> {
        let child = match self {
            Self::Starting(child) | Self::Ready { child, .. } => child,
            Self::Unloaded | Self::Shutdown => return Ok(false),
        };
        if child.try_wait()?.is_none() {
            return Ok(true);
        }
        *self = Self::Unloaded;
        Ok(false)
    }

    fn retire(&mut self, shutdown: bool) -> bool {
        let next = if shutdown || matches!(self, Self::Shutdown) {
            Self::Shutdown
        } else {
            Self::Unloaded
        };
        let previous = std::mem::replace(self, next);
        let loaded = matches!(previous, Self::Ready { .. });
        if let Self::Starting(mut child) | Self::Ready { mut child, .. } = previous {
            let _ = child.kill();
            let _ = child.wait();
        }
        loaded
    }
}

#[derive(Clone, Default)]
pub struct Process(Arc<Mutex<ProcessState>>);

impl Process {
    pub fn stop(&self) {
        self.0.lock().unwrap().retire(false);
    }

    pub fn shutdown(&self) {
        self.0.lock().unwrap().retire(true);
    }

    fn start(&self, command: &mut Command, canceled: &AtomicBool) -> Result<()> {
        let mut state = self.0.lock().unwrap();
        ensure!(
            matches!(*state, ProcessState::Unloaded) && !canceled.load(Ordering::Acquire),
            "Canceled"
        );
        let child = command.spawn().context("Cannot start inference runtime")?;
        *state = ProcessState::Starting(child);
        Ok(())
    }

    fn running(&self) -> Result<bool> {
        self.0.lock().unwrap().running()
    }

    fn mark_ready(&self, base: String, canceled: &AtomicBool) -> Result<()> {
        let mut state = self.0.lock().unwrap();
        ensure!(
            !canceled.load(Ordering::Acquire) && state.running()?,
            "Canceled"
        );
        ensure!(
            matches!(*state, ProcessState::Starting(_)),
            "Inference runtime is not starting"
        );
        let ProcessState::Starting(child) = std::mem::take(&mut *state) else {
            unreachable!();
        };
        *state = ProcessState::Ready { child, base };
        Ok(())
    }

    fn endpoint(&self) -> Result<Option<String>> {
        let mut state = self.0.lock().unwrap();
        if state.running()?
            && let ProcessState::Ready { base, .. } = &*state
        {
            return Ok(Some(base.clone()));
        }
        Ok(None)
    }
}

pub struct Backend {
    config: Config,
    pub process: Process,
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
            key,
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(300))
                .build()?,
        })
    }

    pub fn loaded(&self) -> bool {
        self.process.endpoint().ok().flatten().is_some()
    }

    /// The engine this process runs, for status and for telemetry that has to attribute a
    /// latency to a specific engine.
    pub fn backend(&self) -> crate::config::Backend {
        self.config.backend
    }

    pub fn unload(&mut self) -> bool {
        self.process.0.lock().unwrap().retire(false)
    }

    pub fn ready(&mut self, canceled: &AtomicBool) -> Result<bool> {
        ensure!(!canceled.load(Ordering::Acquire), "Canceled");
        if self.process.endpoint()?.is_some() {
            return Ok(true);
        }
        let started = Instant::now();
        let result = self.load(canceled);
        eprintln!(
            "{}",
            serde_json::json!({
                "event": "model_load",
                "engine": self.backend().label(),
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
        use std::os::unix::fs::PermissionsExt;
        log.set_permissions(fs::Permissions::from_mode(0o600))?;
        let runtime = self.config.runtime_path();
        let mut command = if self.config.backend == crate::config::Backend::NemoSpeech {
            crate::network::inference_command(&runtime)
        } else {
            crate::network::helper_inference_command(&runtime)
        };
        command
            .args(["serve", "--asr-model"])
            .arg(self.config.model_path())
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
        command
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        if self.config.backend != crate::config::Backend::NemoSpeech {
            command.env("NEMO_MLX_MODEL", self.config.model_path());
        }
        self.process.start(&mut command, canceled)?;
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
                self.process.mark_ready(base, canceled)?;
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
                    self.process
                        .endpoint()?
                        .context("Inference runtime is not ready")?
                ))
                .bearer_auth(&self.key)
                .multipart(form)
                .send()
                .context("Transcription request failed")?;
            let status = response.status();
            let body = response.text()?;
            ensure!(status.is_success(), "Transcription failed ({status})");
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
                "engine": self.backend().label(),
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
        process
            .start(
                Command::new("/bin/sleep").arg("60"),
                &AtomicBool::new(false),
            )
            .unwrap();
        let pid = match &*process.0.lock().unwrap() {
            ProcessState::Starting(child) => child.id(),
            _ => unreachable!(),
        };
        assert!(process.running().unwrap());
        process.stop();
        process.stop();
        assert!(!process.running().unwrap());
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
    }

    #[test]
    fn stopped_runtime_can_restart_but_shutdown_is_terminal() {
        let process = Process::default();
        let canceled = AtomicBool::new(false);
        process
            .start(Command::new("/bin/sleep").arg("60"), &canceled)
            .unwrap();
        assert!(process.endpoint().unwrap().is_none());
        process
            .mark_ready("http://127.0.0.1:1".into(), &canceled)
            .unwrap();
        assert_eq!(
            process.endpoint().unwrap().as_deref(),
            Some("http://127.0.0.1:1")
        );
        process.stop();
        assert!(process.endpoint().unwrap().is_none());
        process
            .start(Command::new("/bin/sleep").arg("60"), &canceled)
            .unwrap();
        process.shutdown();
        process.stop();
        assert!(
            process
                .start(Command::new("/bin/sleep").arg("60"), &canceled)
                .is_err()
        );
        assert!(!process.running().unwrap());
        assert!(matches!(*process.0.lock().unwrap(), ProcessState::Shutdown));
    }

    #[test]
    fn cancellation_prevents_start_and_readiness_publication() {
        let process = Process::default();
        let canceled = AtomicBool::new(true);
        assert!(
            process
                .start(Command::new("/bin/sleep").arg("60"), &canceled)
                .is_err()
        );
        canceled.store(false, Ordering::Release);
        process
            .start(Command::new("/bin/sleep").arg("60"), &canceled)
            .unwrap();
        canceled.store(true, Ordering::Release);
        assert!(
            process
                .mark_ready("http://127.0.0.1:1".into(), &canceled)
                .is_err()
        );
        assert!(process.endpoint().unwrap().is_none());
        process.stop();
    }

    #[test]
    fn exited_ready_runtime_loses_its_endpoint_and_can_restart() {
        let process = Process::default();
        let canceled = AtomicBool::new(false);
        process
            .start(Command::new("/bin/sleep").arg("60"), &canceled)
            .unwrap();
        process
            .mark_ready("http://127.0.0.1:1".into(), &canceled)
            .unwrap();
        {
            let mut state = process.0.lock().unwrap();
            let ProcessState::Ready { child, .. } = &mut *state else {
                unreachable!()
            };
            child.kill().unwrap();
            child.wait().unwrap();
        }
        assert!(process.endpoint().unwrap().is_none());
        process
            .start(Command::new("/bin/sleep").arg("60"), &canceled)
            .unwrap();
        assert!(process.running().unwrap());
        process.stop();
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
    #[ignore = "requires downloaded model and artifacts/sample.wav; run cargo xtask verify"]
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
