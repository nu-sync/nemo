use crate::model;
use anyhow::{Context, Result, bail, ensure};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::fd::AsRawFd,
    os::unix::process::CommandExt,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub struct Downloader {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
    state: HelperState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HelperState {
    Starting,
    Ready,
    AwaitingStart,
    Downloading,
    CancelRequested,
    Retired,
}

impl Downloader {
    pub fn spawn(profile: &str) -> Result<Self> {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg("download-worker")
            .args([
                "--parent-pid",
                &std::process::id().to_string(),
                "--profile",
                profile,
            ])
            .env_clear()
            .env("NEMO_DATA_DIR", crate::config::data_dir());
        Self::start(&mut command)
    }

    fn start(command: &mut Command) -> Result<Self> {
        let mut child = command
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .context("Cannot start the model download helper")?;
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let flags = unsafe { libc::fcntl(output.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(output.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                < 0
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::last_os_error().into());
        }
        let mut downloader = Self {
            child,
            input,
            output,
            state: HelperState::Starting,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut ready = [0; 6];
        let mut received = 0;
        while received < ready.len() {
            ensure!(
                Instant::now() < deadline,
                "Model download helper did not become ready"
            );
            match downloader.output.read(&mut ready[received..]) {
                Ok(0) => bail!("Model download helper exited during startup"),
                Ok(count) => received += count,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50))
                }
                Err(error) => return Err(error.into()),
            }
        }
        ensure!(
            &ready == b"ready\n",
            "Invalid model download helper greeting"
        );
        downloader.state = HelperState::Ready;
        Ok(downloader)
    }

    pub fn retired(&self) -> bool {
        self.state == HelperState::Retired
    }

    fn retire(&mut self) -> Result<()> {
        if !self.retired() {
            if let Err(error) = self.child.kill()
                && self.child.try_wait()?.is_none()
            {
                return Err(error.into());
            }
            self.child.wait()?;
            self.state = HelperState::Retired;
        }
        Ok(())
    }

    pub fn download(&mut self, canceled: &AtomicBool) -> Result<()> {
        ensure!(
            self.state == HelperState::Ready,
            "Download helper stopped; reopen Nemo to retry"
        );
        ensure!(!canceled.load(Ordering::Acquire), "Canceled");
        match self.request(canceled) {
            Ok(Some(error)) => {
                self.state = HelperState::Ready;
                bail!("{error}")
            }
            Ok(None) => {
                self.retire()?;
                ensure!(!canceled.load(Ordering::Acquire), "Canceled");
                Ok(())
            }
            Err(error) => {
                self.retire()?;
                Err(error)
            }
        }
    }

    fn request(&mut self, canceled: &AtomicBool) -> Result<Option<String>> {
        self.input.write_all(b"download\n")?;
        let mut response = Vec::new();
        self.state = HelperState::AwaitingStart;
        loop {
            if self.state == HelperState::Downloading && canceled.load(Ordering::Acquire) {
                unsafe { libc::kill(self.child.id() as i32, libc::SIGINT) };
                self.state = HelperState::CancelRequested;
            }
            let mut buffer = [0; 1024];
            match self.output.read(&mut buffer) {
                Ok(0) => bail!("Model download helper exited"),
                Ok(count) => response.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
            ensure!(
                response.len() <= 64 * 1024,
                "Invalid download helper response"
            );
            if self.state == HelperState::AwaitingStart && response.starts_with(b"started\n") {
                response.drain(..8);
                self.state = HelperState::Downloading;
            }
            if response.ends_with(b"\n") {
                return Ok(serde_json::from_slice(&response)?);
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Downloader {
    fn drop(&mut self) {
        let _ = self.retire();
    }
}

pub fn worker(parent: u32, profile: &str) -> Result<()> {
    ensure!(
        unsafe { libc::getppid() } == parent as i32,
        "Download helper parent changed"
    );
    ensure!(
        unsafe { libc::setpgid(0, 0) } == 0,
        "Cannot isolate download helper process group"
    );
    let canceled = Arc::new(AtomicBool::new(false));
    let stopping = canceled.clone();
    ctrlc::set_handler(move || {
        stopping.store(true, Ordering::Release);
    })?;
    thread::spawn(move || {
        loop {
            if unsafe { libc::getppid() } != parent as i32 {
                unsafe { libc::kill(-libc::getpid(), libc::SIGKILL) };
            }
            thread::sleep(Duration::from_millis(100));
        }
    });
    crate::network::download_only()?;
    println!("ready");
    std::io::stdout().flush()?;
    for line in BufReader::new(std::io::stdin()).lines() {
        ensure!(line? == "download", "Invalid model download command");
        canceled.store(false, Ordering::Release);
        println!("started");
        std::io::stdout().flush()?;
        let error = model::Store::profile_at(&crate::config::data_dir(), profile)
            .and_then(|store| store.download(&canceled))
            .err()
            .map(|error| format!("{error:#}"));
        println!("{}", serde_json::to_string(&error)?);
        std::io::stdout().flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(script: &str, directory: &std::path::Path) -> Downloader {
        Downloader::start(Command::new("/bin/sh").args([
            "-c",
            script,
            "download-fixture",
            directory.to_str().unwrap(),
        ]))
        .unwrap()
    }

    #[test]
    fn late_cancellation_reaps_the_successful_helper_before_returning() {
        let directory = tempfile::tempdir().unwrap();
        let mut downloader = peer(
            "trap '' INT; printf 'ready\\n'; read -r request; printf 'started\\n'; touch \"$1/published\"; while ! test -e \"$1/accept\"; do sleep 0.01; done; printf 'null\\n'; read -r request",
            directory.path(),
        );
        let canceled = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !directory.path().join("published").exists() {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(5));
                }
                canceled.store(true, Ordering::Release);
                std::fs::write(directory.path().join("accept"), b"").unwrap();
            });
            let error = downloader.download(&canceled).unwrap_err();
            assert_eq!(error.to_string(), "Canceled");
        });
        assert!(downloader.retired());
        assert!(downloader.child.try_wait().unwrap().is_some());
    }

    #[test]
    fn helper_death_is_reaped_before_reporting_failure() {
        let directory = tempfile::tempdir().unwrap();
        let mut downloader = peer(
            "printf 'ready\\n'; read -r request; kill -KILL $$",
            directory.path(),
        );
        let error = downloader.download(&AtomicBool::new(false)).unwrap_err();
        assert!(downloader.retired(), "{error:#}");
        assert!(downloader.child.try_wait().unwrap().is_some());
    }

    #[test]
    fn interrupted_transfer_can_retry_and_success_retires_the_helper() {
        let directory = tempfile::tempdir().unwrap();
        let mut downloader = peer(
            "printf 'ready\\n'; read -r request; printf 'started\\n\"Canceled\"\\n'; read -r request; printf 'started\\nnull\\n'; read -r request",
            directory.path(),
        );
        let canceled = AtomicBool::new(false);
        assert_eq!(
            downloader.download(&canceled).unwrap_err().to_string(),
            "Canceled"
        );
        assert!(!downloader.retired());
        downloader.download(&canceled).unwrap();
        assert!(downloader.retired());
        assert!(downloader.child.try_wait().unwrap().is_some());
    }

    #[test]
    fn cancellation_waits_for_start_acknowledgement() {
        let directory = tempfile::tempdir().unwrap();
        let mut downloader = peer(
            "trap 'touch \"$1/interrupted\"' INT; printf 'ready\\n'; read -r request; touch \"$1/awaiting\"; attempts=0; while ! test -e \"$1/acknowledge\"; do attempts=$((attempts + 1)); test \"$attempts\" -lt 500 || exit 1; sleep 0.01; done; printf 'started\\n'; attempts=0; while ! test -e \"$1/interrupted\"; do attempts=$((attempts + 1)); test \"$attempts\" -lt 500 || exit 1; sleep 0.01; done; printf '\"Canceled\"\\n'; read -r request",
            directory.path(),
        );
        let canceled = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !directory.path().join("awaiting").exists() {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(5));
                }
                canceled.store(true, Ordering::Release);
                thread::sleep(Duration::from_millis(150));
                assert!(!directory.path().join("interrupted").exists());
                std::fs::write(directory.path().join("acknowledge"), b"").unwrap();
            });
            assert_eq!(
                downloader.download(&canceled).unwrap_err().to_string(),
                "Canceled"
            );
        });
        assert!(directory.path().join("interrupted").exists());
        assert_eq!(downloader.state, HelperState::Ready);
    }

    #[test]
    fn malformed_reply_retires_the_helper() {
        let directory = tempfile::tempdir().unwrap();
        let mut downloader = peer(
            "printf 'ready\\n'; read -r request; printf 'started\\ninvalid-json\\n'; read -r request",
            directory.path(),
        );
        let error = downloader.download(&AtomicBool::new(false)).unwrap_err();
        assert!(downloader.retired(), "{error:#}");
    }
}
