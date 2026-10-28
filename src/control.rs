use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
        },
    },
    path::PathBuf,
    time::Duration,
};

pub fn dir() -> PathBuf {
    crate::config::data_dir().join("run")
}
pub fn socket() -> PathBuf {
    dir().join("control.sock")
}

pub struct Server {
    _lock: File,
    pub listener: UnixListener,
}

impl Server {
    pub fn bind() -> Result<Self> {
        fs::create_dir_all(dir())?;
        fs::set_permissions(dir(), fs::Permissions::from_mode(0o700))?;
        let lock = File::create(dir().join("daemon.lock"))?;
        ensure!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
            "Nemo is already running"
        );
        let _ = fs::remove_file(socket());
        let listener = UnixListener::bind(socket())?;
        Ok(Self {
            _lock: lock,
            listener,
        })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = fs::remove_file(socket());
    }
}

pub fn send(command: &str) -> Result<String> {
    let mut stream = UnixStream::connect(socket())
        .context("Nemo is not running. Open Nemo.app or run `nemo run`")?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    writeln!(stream, "{command}")?;
    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response)?;
    ensure!(!response.is_empty(), "Daemon disconnected");
    Ok(response.trim().into())
}
