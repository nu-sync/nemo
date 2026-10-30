use crate::{backend::Process, model};
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
}

impl Downloader {
    pub fn spawn() -> Result<Self> {
        let mut child = Command::new(std::env::current_exe()?)
            .arg("download-worker")
            .args(["--parent-pid", &std::process::id().to_string()])
            .env_clear()
            .env("NEMO_DATA_DIR", crate::config::data_dir())
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
        Ok(downloader)
    }

    pub fn download(&mut self, canceled: &AtomicBool) -> Result<()> {
        ensure!(!canceled.load(Ordering::Acquire), "Canceled");
        self.input.write_all(b"download\n")?;
        let mut response = Vec::new();
        let mut interrupted = false;
        let mut started = false;
        loop {
            if started && canceled.load(Ordering::Acquire) && !interrupted {
                unsafe { libc::kill(self.child.id() as i32, libc::SIGINT) };
                interrupted = true;
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
            if !started && response.starts_with(b"started\n") {
                response.drain(..8);
                started = true;
            }
            if response.ends_with(b"\n") {
                let error: Option<String> = serde_json::from_slice(&response)?;
                ensure!(!canceled.load(Ordering::Acquire), "Canceled");
                if let Some(error) = error {
                    bail!("{error}");
                }
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Downloader {
    fn drop(&mut self) {
        unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
        let _ = self.child.wait();
    }
}

pub fn worker(parent: u32) -> Result<()> {
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
    let process = Process::default();
    let child = process.clone();
    ctrlc::set_handler(move || {
        stopping.store(true, Ordering::Release);
        child.stop();
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
        let error = model::Store::managed()
            .download(&process, &canceled)
            .err()
            .map(|error| format!("{error:#}"));
        println!("{}", serde_json::to_string(&error)?);
        std::io::stdout().flush()?;
    }
    Ok(())
}
