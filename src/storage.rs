use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

pub struct Lock {
    file: File,
}

impl Lock {
    pub fn acquire(path: &Path) -> Result<Self> {
        fs::create_dir_all(path.parent().context("Lock has no parent")?)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        ensure!(
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
            "Another operation is using {}. Try again when it finishes",
            path.display()
        );
        Ok(Self { file })
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

pub fn temporary(path: &Path) -> std::path::PathBuf {
    path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    ))
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(path.parent().context("File has no parent")?)?;
    let temporary = temporary(path);
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lock_is_exclusive_and_released_without_deleting_the_lockfile() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("operation.lock");
        let lock = Lock::acquire(&path).unwrap();
        assert!(Lock::acquire(&path).is_err());
        drop(lock);
        assert!(Lock::acquire(&path).is_ok());
    }

    #[test]
    fn dropping_a_lock_releases_it_while_a_child_retains_the_descriptor() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("inherited.lock");
        let lock = Lock::acquire(&path).unwrap();
        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            loop {
                unsafe { libc::pause() };
            }
        }
        drop(lock);
        let reacquired = Lock::acquire(&path);
        let killed = unsafe { libc::kill(child, libc::SIGKILL) };
        let waited = loop {
            let waited = unsafe { libc::waitpid(child, std::ptr::null_mut(), 0) };
            if waited >= 0
                || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                break waited;
            }
        };
        assert_eq!(killed, 0);
        assert_eq!(waited, child);
        assert!(reacquired.is_ok(), "{:?}", reacquired.err());
    }
}
