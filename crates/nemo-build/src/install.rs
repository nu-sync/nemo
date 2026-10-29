use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, OpenOptions},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

pub struct Options {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub allow_permission_reset: bool,
    pub launch: bool,
}

type Executor<'a> = dyn Fn(&mut Command) -> Result<String> + 'a;

pub fn verify(app: &Path) -> Result<()> {
    verify_with(app, &crate::run)
}

fn verify_with(app: &Path, execute: &Executor<'_>) -> Result<()> {
    let info = plist::Value::from_file(app.join("Contents/Info.plist"))?;
    ensure!(
        info.as_dictionary()
            .and_then(|info| info.get("CFBundleIdentifier"))
            .and_then(plist::Value::as_string)
            == Some("computer.nemo.dictate"),
        "Refusing an unrelated application: {}",
        app.display()
    );
    ensure!(
        app.join("Contents/MacOS/nemo").is_file(),
        "Application executable is missing: {}",
        app.display()
    );
    execute(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(app),
    )?;
    Ok(())
}

pub fn open(app: &Path) -> Result<()> {
    open_with(app, &crate::run)
}

fn open_with(app: &Path, execute: &Executor<'_>) -> Result<()> {
    let mut command = Command::new("/usr/bin/open");
    let overrides: Vec<_> = ["NEMO_DATA_DIR", "NEMO_RUNTIME"]
        .into_iter()
        .filter_map(|name| {
            std::env::var_os(name).map(|value| {
                let mut assignment = std::ffi::OsString::from(name);
                assignment.push("=");
                assignment.push(value);
                assignment
            })
        })
        .collect();
    if !overrides.is_empty() {
        command.arg("-n");
        for value in overrides {
            command.arg("--env").arg(value);
        }
    }
    execute(command.arg(app))?;
    Ok(())
}

pub fn same_identity(existing: &Path, candidate: &Path) -> Result<bool> {
    same_identity_with(existing, candidate, &crate::run)
}

fn same_identity_with(existing: &Path, candidate: &Path, execute: &Executor<'_>) -> Result<bool> {
    for (app, requirement_app) in [(candidate, existing), (existing, candidate)] {
        let result = Command::new("/usr/bin/codesign")
            .args(["-d", "-r-"])
            .arg(requirement_app)
            .output()?;
        ensure!(
            result.status.success(),
            "Cannot read code identity: {}",
            requirement_app.display()
        );
        let details = format!(
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let requirement = details
            .lines()
            .find_map(|line| line.trim_start_matches("# ").strip_prefix("designated => "))
            .context("Cannot read designated requirement")?;
        if execute(
            Command::new("/usr/bin/codesign")
                .args(["--verify", "--strict", "-R"])
                .arg(format!("={requirement}"))
                .arg(app),
        )
        .is_err()
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn running(app: &Path, execute: &Executor<'_>) -> Result<bool> {
    let executable = app.join("Contents/MacOS/nemo");
    let processes = execute(Command::new("/bin/ps").args(["-A", "-o", "pid=,comm="]))?;
    Ok(processes.lines().any(|line| {
        line.trim()
            .split_once(char::is_whitespace)
            .is_some_and(|(_, path)| path.trim() == executable.to_string_lossy())
    }))
}

fn stop_idle(destination: &Path, staged: &Path, execute: &Executor<'_>) -> Result<bool> {
    if !running(destination, execute)? {
        return Ok(false);
    }
    let executable = staged.join("Contents/MacOS/nemo");
    let state = execute(Command::new(&executable).arg("status"))?;
    ensure!(
        !matches!(
            state.trim(),
            "Recording"
                | "Opening microphone"
                | "Transcribing"
                | "Loading model"
                | "Downloading model"
                | "Canceling"
                | "Canceling microphone request"
        ),
        "Nemo is busy ({}). Finish or cancel before upgrading",
        state.trim()
    );
    ensure!(
        execute(Command::new(&executable).args(["quit", "--if-idle"]))?.trim() == "accepted",
        "Nemo refused a safe quit. Finish the operation or quit the older app manually before upgrading"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while running(destination, execute)? {
        ensure!(
            Instant::now() < deadline,
            "Nemo did not exit. The installed app has not been replaced"
        );
        thread::sleep(Duration::from_millis(50));
    }
    Ok(true)
}

struct InstallLock(std::fs::File);
impl Drop for InstallLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

pub fn install(options: &Options) -> Result<Option<PathBuf>> {
    install_with(options, |_| Ok(()))
}

pub fn install_with(
    options: &Options,
    confirm: impl FnOnce(&Path) -> Result<()>,
) -> Result<Option<PathBuf>> {
    install_execute(options, &crate::run, confirm)
}

fn install_execute(
    options: &Options,
    execute: &Executor<'_>,
    confirm: impl FnOnce(&Path) -> Result<()>,
) -> Result<Option<PathBuf>> {
    let source = std::path::absolute(&options.source)?;
    let destination = std::path::absolute(&options.destination)?;
    ensure!(
        destination
            .extension()
            .is_some_and(|extension| extension == "app"),
        "Destination must end in .app"
    );
    ensure!(
        !destination.is_symlink(),
        "Destination cannot be an application symlink"
    );
    ensure!(
        source != destination
            && (!destination.exists() || source.canonicalize()? != destination.canonicalize()?),
        "Source and destination must differ"
    );
    verify_with(&source, execute)?;
    let parent = destination.parent().context("Destination has no parent")?;
    fs::create_dir_all(parent)?;
    let stem = destination
        .file_stem()
        .context("Destination has no name")?
        .to_string_lossy();
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(parent.join(format!(".{stem}.install.lock")))?;
    ensure!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "Another installer is using this destination"
    );
    let _lock = InstallLock(lock);
    if destination.exists() {
        verify_with(&destination, execute)?;
    }
    let temporary = tempfile::Builder::new()
        .prefix(&format!(".{stem}.install-"))
        .tempdir_in(parent)?;
    let staged = temporary.path().join(destination.file_name().unwrap());
    execute(Command::new("/usr/bin/ditto").arg(&source).arg(&staged))?;
    verify_with(&staged, execute)?;
    ensure!(
        !destination.exists()
            || options.allow_permission_reset
            || same_identity_with(&destination, &staged, execute)?,
        "The update changes code identity and may reset macOS permissions. Use a stable signing identity or explicitly allow a permission reset. The installed app is unchanged"
    );
    let backup = if destination.exists() {
        let backups = parent.join(format!(".{stem}.backups"));
        fs::create_dir_all(&backups)?;
        let reserved = tempfile::Builder::new()
            .prefix("build-")
            .suffix(".app")
            .tempdir_in(backups)?;
        let path = reserved.path().to_owned();
        reserved.close()?;
        Some(path)
    } else {
        None
    };
    let was_running = stop_idle(&destination, &staged, execute)?;
    let result = (|| {
        if let Some(backup) = &backup {
            fs::rename(&destination, backup)?;
        }
        fs::rename(&staged, &destination)?;
        verify_with(&destination, execute)?;
        if was_running && options.launch {
            open_with(&destination, execute)?;
        }
        confirm(&destination)?;
        Ok(())
    })();
    if let Err(error) = result {
        let rollback = (|| -> Result<()> {
            if let Some(backup) = &backup {
                if backup.exists() {
                    if destination.exists() {
                        fs::rename(&destination, temporary.path().join("failed.app"))?;
                    }
                    fs::rename(backup, &destination)?;
                }
            } else if destination.exists() {
                fs::rename(&destination, temporary.path().join("failed.app"))?;
            }
            if was_running && destination.exists() {
                open_with(&destination, execute)?;
            }
            Ok(())
        })();
        rollback.with_context(|| {
            format!("Installation failed ({error:#}); automatic rollback also failed")
        })?;
        return Err(error);
    }
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn app(path: &Path, version: &str, identifier: &str) {
        fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
        fs::copy("/usr/bin/true", path.join("Contents/MacOS/nemo")).unwrap();
        let mut info = plist::Dictionary::new();
        for (key, value) in [
            ("CFBundleIdentifier", identifier),
            ("CFBundleExecutable", "nemo"),
            ("CFBundleVersion", version),
        ] {
            info.insert(key.into(), plist::Value::String(value.into()));
        }
        plist::Value::Dictionary(info)
            .to_file_xml(path.join("Contents/Info.plist"))
            .unwrap();
        crate::run(
            Command::new("/usr/bin/codesign")
                .args(["--force", "--sign", "-"])
                .arg(path),
        )
        .unwrap();
    }
    fn version(path: &Path) -> String {
        plist::Value::from_file(path.join("Contents/Info.plist"))
            .unwrap()
            .as_dictionary()
            .unwrap()["CFBundleVersion"]
            .as_string()
            .unwrap()
            .into()
    }
    fn options(root: &Path) -> Options {
        Options {
            source: root.join("source/Nemo.app"),
            destination: root.join("installed/Nemo.app"),
            allow_permission_reset: true,
            launch: false,
        }
    }

    #[test]
    fn fresh_install_retains_the_source_and_upgrades_retain_a_verified_backup() {
        let root = tempfile::tempdir().unwrap();
        let mut options = options(root.path());
        app(&options.source, "1", "computer.nemo.dictate");
        assert!(install(&options).unwrap().is_none());
        assert_eq!(version(&options.destination), "1");
        assert_eq!(version(&options.source), "1");
        options.allow_permission_reset = false;
        let backup = install(&options).unwrap().unwrap();
        assert_eq!(version(&backup), "1");
        assert!(same_identity(&options.source, &options.destination).unwrap());
        verify(&backup).unwrap();
    }

    #[test]
    fn changed_identity_and_unrelated_app_fail_before_quitting() {
        let root = tempfile::tempdir().unwrap();
        let mut options = options(root.path());
        app(&options.source, "2", "computer.nemo.dictate");
        app(&options.destination, "1", "computer.nemo.dictate");
        options.allow_permission_reset = false;
        assert!(
            install(&options)
                .unwrap_err()
                .to_string()
                .contains("code identity")
        );
        assert_eq!(version(&options.destination), "1");
        options.allow_permission_reset = true;
        fs::remove_dir_all(&options.destination).unwrap();
        app(&options.destination, "1", "example.unrelated");
        assert!(
            install(&options)
                .unwrap_err()
                .to_string()
                .contains("unrelated")
        );
        assert_eq!(version(&options.destination), "1");
    }

    #[test]
    fn failed_build_confirmation_restores_the_previous_bundle() {
        let root = tempfile::tempdir().unwrap();
        let options = options(root.path());
        app(&options.source, "2", "computer.nemo.dictate");
        app(&options.destination, "1", "computer.nemo.dictate");
        let error = install_with(&options, |installed| {
            assert_eq!(version(installed), "2");
            anyhow::bail!("running build does not match");
        })
        .unwrap_err();
        assert!(error.to_string().contains("does not match"));
        assert_eq!(version(&options.destination), "1");
        verify(&options.destination).unwrap();
    }

    #[test]
    fn a_recording_started_after_preflight_prevents_replacement() {
        let root = tempfile::tempdir().unwrap();
        let options = options(root.path());
        app(&options.source, "2", "computer.nemo.dictate");
        app(&options.destination, "1", "computer.nemo.dictate");
        let execute = |command: &mut Command| {
            let args: Vec<_> = command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            if command.get_program() == "/bin/ps" {
                return Ok(format!(
                    "123 {}\n",
                    options.destination.join("Contents/MacOS/nemo").display()
                ));
            }
            if args == ["status"] {
                return Ok("Ready".into());
            }
            if args == ["quit", "--if-idle"] {
                anyhow::bail!("Safe quit refused (busy)");
            }
            crate::run(command)
        };
        assert!(
            install_execute(&options, &execute, |_| Ok(()))
                .unwrap_err()
                .to_string()
                .contains("busy")
        );
        assert_eq!(version(&options.destination), "1");
    }

    #[test]
    fn failed_post_install_verification_restores_and_reopens_the_old_app() {
        let root = tempfile::tempdir().unwrap();
        let options = options(root.path());
        app(&options.source, "2", "computer.nemo.dictate");
        app(&options.destination, "1", "computer.nemo.dictate");
        let stopped = Cell::new(false);
        let opened = Cell::new(false);
        let execute = |command: &mut Command| {
            let args: Vec<_> = command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            if command.get_program() == "/bin/ps" {
                return Ok(if stopped.get() {
                    String::new()
                } else {
                    format!(
                        "123 {}\n",
                        options.destination.join("Contents/MacOS/nemo").display()
                    )
                });
            }
            if args == ["status"] {
                return Ok("Ready".into());
            }
            if args == ["quit", "--if-idle"] {
                stopped.set(true);
                return Ok("accepted".into());
            }
            if command.get_program() == "/usr/bin/open" {
                assert_eq!(version(&options.destination), "1");
                opened.set(true);
                return Ok(String::new());
            }
            if args.first().is_some_and(|arg| arg == "--verify")
                && args.last() == Some(&options.destination.to_string_lossy().into_owned())
                && version(&options.destination) == "2"
            {
                anyhow::bail!("post-install failure");
            }
            crate::run(command)
        };
        assert!(
            install_execute(&options, &execute, |_| Ok(()))
                .unwrap_err()
                .to_string()
                .contains("post-install failure")
        );
        assert_eq!(version(&options.destination), "1");
        assert!(opened.get());
        verify(&options.destination).unwrap();
    }
}
