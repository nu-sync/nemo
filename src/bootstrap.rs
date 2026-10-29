use crate::{config, control, storage};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs::{self, File},
    io::Read,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const BUILD_SCRIPT: &[u8] = include_bytes!("../scripts/build-runtime.sh");
const PAYLOAD: &[(&str, &[u8])] = &[
    ("scripts/build-runtime.sh", BUILD_SCRIPT),
    ("scripts/bundle.py", include_bytes!("../scripts/bundle.py")),
    (
        "scripts/install_app.py",
        include_bytes!("../scripts/install_app.py"),
    ),
    (
        "scripts/entitlements.plist",
        include_bytes!("../scripts/entitlements.plist"),
    ),
    ("LICENSE", include_bytes!("../LICENSE")),
    ("models/LICENSE", include_bytes!("../models/LICENSE")),
    (
        "models/manifest.json",
        include_bytes!("../models/manifest.json"),
    ),
    (
        "assets/sounds/Handy-MIT.txt",
        include_bytes!("../assets/sounds/Handy-MIT.txt"),
    ),
];

pub fn bundled() -> bool {
    std::env::current_exe().is_ok_and(|path| bundled_executable(&path))
}

fn bundled_executable(path: &Path) -> bool {
    path.file_name() == Some(OsStr::new("nemo"))
        && path.parent().and_then(Path::file_name) == Some(OsStr::new("MacOS"))
        && path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            == Some(OsStr::new("Contents"))
        && path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .and_then(Path::extension)
            == Some(OsStr::new("app"))
}

struct Paths {
    data: PathBuf,
    app: PathBuf,
    cache: PathBuf,
}

impl Paths {
    fn new(data: PathBuf, app: PathBuf) -> Result<Self> {
        ensure!(data.is_absolute(), "NEMO_DATA_DIR must be an absolute path");
        ensure!(
            app.is_absolute() && app.extension() == Some(OsStr::new("app")),
            "NEMO_APP_PATH must be an absolute path ending in .app"
        );
        ensure!(!app.is_symlink(), "The app destination cannot be a symlink");
        let build = format!("{:x}", Sha256::digest(BUILD_SCRIPT));
        let cache = data.join("runtime-builds").join(&build[..16]);
        Ok(Self { data, app, cache })
    }

    fn current() -> Result<Self> {
        let app = match std::env::var_os("NEMO_APP_PATH") {
            Some(path) => path.into(),
            None => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?)
                .join("Applications/Nemo.app"),
        };
        Self::new(config::data_dir(), app)
    }
}

#[derive(Deserialize)]
struct Receipt {
    source_sha256: String,
    version: String,
}

fn receipt_matches(bytes: &[u8], source_sha256: &str) -> bool {
    serde_json::from_slice::<Receipt>(bytes).is_ok_and(|receipt| {
        receipt.source_sha256 == source_sha256 && receipt.version == env!("CARGO_PKG_VERSION")
    })
}

fn sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut bytes = [0; 64 * 1024];
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            return Ok(format!("{:x}", digest.finalize()));
        }
        digest.update(&bytes[..count]);
    }
}

fn output(command: &mut Command) -> Result<String> {
    let result = command.stdin(Stdio::null()).output()?;
    ensure!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr).trim()
    );
    Ok(String::from_utf8(result.stdout)?)
}

fn verify_app(app: &Path) -> Result<()> {
    let identifier = output(
        Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", "Print :CFBundleIdentifier"])
            .arg(app.join("Contents/Info.plist")),
    )?;
    ensure!(
        identifier.trim() == "computer.nemo.dictate",
        "Refusing an unrelated application: {}",
        app.display()
    );
    output(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(app),
    )?;
    Ok(())
}

#[derive(Clone, Default)]
struct Runner {
    canceled: Arc<AtomicBool>,
    group: Arc<Mutex<Option<u32>>>,
}

impl Runner {
    fn with_signals() -> Result<Self> {
        let runner = Self::default();
        let stopping = runner.clone();
        ctrlc::set_handler(move || stopping.cancel())?;
        Ok(runner)
    }

    fn cancel(&self) {
        self.canceled.store(true, Ordering::Release);
        if let Some(group) = *self.group.lock().unwrap() {
            unsafe { libc::kill(-(group as i32), libc::SIGINT) };
        }
    }

    fn run(&self, command: &mut Command, step: &str) -> Result<()> {
        let mut group = self.group.lock().unwrap();
        ensure!(!self.canceled.load(Ordering::Acquire), "Canceled");
        let child = command
            .process_group(0)
            .stdin(Stdio::null())
            .spawn()
            .with_context(|| format!("Cannot start {step}"))?;
        *group = Some(child.id());
        drop(group);
        let mut running = Running {
            child,
            group: &self.group,
        };
        let mut canceled_at = None;
        loop {
            let mut group = self.group.lock().unwrap();
            if let Some(status) = running.child.try_wait()? {
                if !status.success() || self.canceled.load(Ordering::Acquire) {
                    unsafe { libc::kill(-(running.child.id() as i32), libc::SIGKILL) };
                }
                *group = None;
                ensure!(!self.canceled.load(Ordering::Acquire), "Canceled");
                ensure!(
                    status.success(),
                    "{step} failed ({status}); run nemo again to retry"
                );
                return Ok(());
            }
            if self.canceled.load(Ordering::Acquire) {
                let started = canceled_at.get_or_insert_with(Instant::now);
                if started.elapsed() >= Duration::from_secs(2) {
                    unsafe { libc::kill(-(running.child.id() as i32), libc::SIGKILL) };
                }
            }
            drop(group);
            thread::sleep(Duration::from_millis(50));
        }
    }
}

struct Running<'a> {
    child: Child,
    group: &'a Mutex<Option<u32>>,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        let mut group = self.group.lock().unwrap();
        if *group == Some(self.child.id()) {
            unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
            *group = None;
        }
        drop(group);
        let _ = self.child.wait();
    }
}

fn prepare_tools(runner: &Runner) -> Result<PathBuf> {
    ensure!(
        cfg!(target_os = "macos") && cfg!(target_arch = "aarch64"),
        "Nemo requires an Apple Silicon Mac"
    );
    let version = output(Command::new("/usr/bin/sw_vers").arg("-productVersion"))?;
    ensure!(
        version
            .trim()
            .split('.')
            .next()
            .and_then(|n| n.parse::<u32>().ok())
            .is_some_and(|major| major >= 26),
        "Nemo requires macOS 26 or newer"
    );
    output(Command::new("/usr/bin/xcode-select").arg("-p")).context(
        "Install Xcode Command Line Tools with xcode-select --install, then run nemo again",
    )?;
    let brew = [
        PathBuf::from("brew"),
        PathBuf::from("/opt/homebrew/bin/brew"),
    ]
    .into_iter()
    .find(|path| output(Command::new(path).arg("--prefix")).is_ok())
    .context("Install Homebrew from https://brew.sh, then run nemo again")?;
    let prefix = PathBuf::from(output(Command::new(&brew).arg("--prefix"))?.trim());
    let missing: Vec<_> = ["cmake", "ninja", "sentencepiece", "abseil"]
        .into_iter()
        .filter(|formula| {
            Command::new(&brew)
                .args(["list", "--versions", formula])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_or(true, |status| !status.success())
        })
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "Installing build tools with Homebrew: {}",
            missing.join(", ")
        );
        runner.run(
            Command::new(&brew)
                .env("HOMEBREW_NO_AUTO_UPDATE", "1")
                .arg("install")
                .args(&missing),
            "Homebrew build dependency installation",
        )?;
    }
    let python = [
        PathBuf::from("python3"),
        prefix.join("bin/python3"),
        PathBuf::from("/usr/bin/python3"),
    ];
    if let Some(path) = python.iter().find(|path| usable_python(path)) {
        return Ok(path.clone());
    }
    eprintln!("Installing Python for one-time app setup");
    runner.run(
        Command::new(&brew)
            .env("HOMEBREW_NO_AUTO_UPDATE", "1")
            .args(["install", "python"]),
        "Python installation",
    )?;
    python
        .into_iter()
        .find(|path| usable_python(path))
        .context("Python 3.9 or newer is required for app setup")
}

fn usable_python(path: &Path) -> bool {
    Command::new(path)
        .args(["-c", "import sys; sys.exit(sys.version_info < (3, 9))"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn write_payload(root: &Path) -> Result<()> {
    for (name, bytes) in PAYLOAD {
        storage::atomic_write(&root.join(name), bytes)?;
    }
    Ok(())
}

struct Setup {
    paths: Paths,
    runner: Runner,
    _lock: storage::Lock,
}

impl Setup {
    fn new() -> Result<Self> {
        let paths = Paths::current()?;
        let lock = storage::Lock::acquire(&paths.data.join("bootstrap.lock"))?;
        Ok(Self {
            paths,
            runner: Runner::with_signals()?,
            _lock: lock,
        })
    }

    fn install(&self, allow_permission_reset: bool) -> Result<PathBuf> {
        let executable = std::env::current_exe()?;
        let source = sha256(&executable)?;
        if self.paths.app.exists() {
            verify_app(&self.paths.app)
                .context("The existing Nemo app did not pass verification")?;
            if fs::read(
                self.paths
                    .app
                    .join("Contents/Resources/install-receipt.json"),
            )
            .is_ok_and(|bytes| receipt_matches(&bytes, &source))
            {
                return Ok(self.paths.app.clone());
            }
        }
        eprintln!("Preparing Nemo.app. The native speech runtime is built once and cached.");
        let python = prepare_tools(&self.runner)?;
        write_payload(&self.paths.cache)?;
        self.runner.run(
            Command::new("/bin/bash").arg(self.paths.cache.join("scripts/build-runtime.sh")),
            "native speech runtime build",
        )?;
        let candidate = self.paths.cache.join("candidate/Nemo.app");
        eprintln!("Packaging Nemo.app");
        self.runner.run(
            Command::new(&python)
                .arg(self.paths.cache.join("scripts/bundle.py"))
                .arg("--binary")
                .arg(&executable)
                .arg("--native-dir")
                .arg(
                    self.paths
                        .cache
                        .join("vendor/NeMo-Speech.cpp/build/metal-server/bin"),
                )
                .arg("--version")
                .arg(env!("CARGO_PKG_VERSION"))
                .arg("--output")
                .arg(&candidate),
            "app packaging",
        )?;
        let mut installer = Command::new(python);
        installer
            .arg(self.paths.cache.join("scripts/install_app.py"))
            .arg("--source")
            .arg(candidate)
            .arg("--destination")
            .arg(&self.paths.app)
            .arg("--no-launch");
        if allow_permission_reset {
            installer.arg("--allow-permission-reset");
        }
        self.runner
            .run(&mut installer, "app installation")
            .context(
                "For an approved code-identity change, use nemo install --allow-permission-reset",
            )?;
        Ok(self.paths.app.clone())
    }
}

pub fn install(allow_permission_reset: bool) -> Result<PathBuf> {
    ensure_cargo_launcher(&std::env::current_exe()?)?;
    Setup::new()?.install(allow_permission_reset)
}

fn ensure_cargo_launcher(executable: &Path) -> Result<()> {
    ensure!(
        !bundled_executable(executable),
        "Use the Cargo-installed nemo command to install or update Nemo.app"
    );
    Ok(())
}

#[derive(Deserialize)]
struct Identity {
    pid: u32,
    executable: Option<PathBuf>,
}

fn identity_matches(identity: &Identity, executable: &Path) -> bool {
    identity.pid != 0
        && executable.canonicalize().is_ok_and(|expected| {
            identity
                .executable
                .as_ref()
                .and_then(|path| path.canonicalize().ok())
                .is_some_and(|actual| actual == expected)
        })
}

fn app_running(app: &Path) -> Result<bool> {
    let Ok(response) = control::send("identity") else {
        return Ok(false);
    };
    let identity: Identity = serde_json::from_str(&response)
        .context("The running Nemo instance does not report its executable identity")?;
    Ok(identity_matches(
        &identity,
        &app.join("Contents/MacOS/nemo"),
    ))
}

pub fn launch() -> Result<()> {
    ensure_cargo_launcher(&std::env::current_exe()?)?;
    let setup = Setup::new()?;
    let app = setup.install(false)?;
    if control::send("status").is_ok() {
        ensure!(
            app_running(&app)?,
            "Nemo is already running outside this app. Quit that instance before starting Nemo.app"
        );
        println!("Nemo is already running");
        return Ok(());
    }
    eprintln!("Opening {}", app.display());
    let mut open = Command::new("/usr/bin/open");
    open.arg("-n");
    for name in ["NEMO_DATA_DIR", "NEMO_RUNTIME"] {
        if let Some(value) = std::env::var_os(name) {
            let mut assignment = std::ffi::OsString::from(name);
            assignment.push("=");
            assignment.push(value);
            open.arg("--env").arg(assignment);
        }
    }
    open.arg(&app);
    setup.runner.run(&mut open, "app launch")?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        ensure!(!setup.runner.canceled.load(Ordering::Acquire), "Canceled");
        if app_running(&app)?
            && let Ok(status) = control::send("status")
        {
            println!("Nemo is running: {status}");
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    bail!(
        "Nemo did not become ready. See {}",
        setup.paths.data.join("daemon.log").display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_actual_bundle_layout_bypasses_bootstrap() {
        assert!(bundled_executable(Path::new(
            "/Applications/Nemo.app/Contents/MacOS/nemo"
        )));
        for path in [
            "/Users/test/.cargo/bin/nemo",
            "/tmp/Nemo.app/nemo",
            "/tmp/Contents/MacOS/nemo",
            "/tmp/Nemo.app/Contents/MacOS/other",
        ] {
            assert!(!bundled_executable(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn the_bundled_executable_cannot_reinstall_its_signed_bytes() {
        let bundled = Path::new("/Applications/Nemo.app/Contents/MacOS/nemo");
        assert_eq!(
            ensure_cargo_launcher(bundled).unwrap_err().to_string(),
            "Use the Cargo-installed nemo command to install or update Nemo.app"
        );
        assert!(ensure_cargo_launcher(Path::new("/Users/test/.cargo/bin/nemo")).is_ok());
    }

    #[test]
    fn setup_paths_are_absolute_and_keep_isolated_data_separate() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("private-data");
        let app = root.path().join("apps/Nemo.app");
        let paths = Paths::new(data.clone(), app.clone()).unwrap();
        assert_eq!(paths.app, app);
        assert!(paths.cache.starts_with(data.join("runtime-builds")));
        assert!(Paths::new("relative".into(), app.clone()).is_err());
        assert!(Paths::new(data.clone(), "Nemo.app".into()).is_err());
        assert!(Paths::new(data, root.path().join("app")).is_err());
    }

    #[test]
    fn signed_bundle_receipts_match_the_original_cargo_payload() {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "source_sha256": "original bytes", "version": env!("CARGO_PKG_VERSION")
        }))
        .unwrap();
        assert!(receipt_matches(&bytes, "original bytes"));
        assert!(!receipt_matches(&bytes, "different build"));
        assert!(!receipt_matches(b"not a receipt", "original bytes"));
    }

    #[test]
    fn embedded_setup_does_not_need_a_repository_checkout() {
        let root = tempfile::tempdir().unwrap();
        write_payload(root.path()).unwrap();
        write_payload(root.path()).unwrap();
        for (name, bytes) in PAYLOAD {
            assert_eq!(fs::read(root.path().join(name)).unwrap(), *bytes);
        }
    }

    #[test]
    fn daemon_identity_accepts_path_aliases_but_not_another_executable() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real/Nemo.app/Contents/MacOS/nemo");
        fs::create_dir_all(real.parent().unwrap()).unwrap();
        fs::write(&real, b"app").unwrap();
        std::os::unix::fs::symlink(root.path().join("real"), root.path().join("alias")).unwrap();
        let aliased = root.path().join("alias/Nemo.app/Contents/MacOS/nemo");
        let mut identity = Identity {
            pid: 123,
            executable: Some(real),
        };
        assert!(identity_matches(&identity, &aliased));
        let other = root.path().join("other-nemo");
        fs::write(&other, b"another app").unwrap();
        identity.executable = Some(other);
        assert!(!identity_matches(&identity, &aliased));
        identity.executable = None;
        assert!(!identity_matches(&identity, &aliased));
    }

    #[test]
    fn cancellation_stops_only_the_owned_command_group() {
        let root = tempfile::tempdir().unwrap();
        let pid_file = root.path().join("child.pid");
        let runner = Runner::default();
        let running = runner.clone();
        let path = pid_file.clone();
        let task = thread::spawn(move || {
            running.run(
                Command::new("/bin/sh")
                    .args([
                        "-c",
                        "sleep 30 & echo $! > \"$1\"; wait",
                        "nemo-bootstrap-test",
                    ])
                    .arg(path),
                "test command",
            )
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut child = None;
        while child.is_none() && Instant::now() < deadline {
            child = fs::read_to_string(&pid_file)
                .ok()
                .and_then(|pid| pid.trim().parse::<u32>().ok());
            if child.is_none() {
                thread::sleep(Duration::from_millis(10));
            }
        }
        runner.cancel();
        assert!(
            task.join()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("Canceled")
        );
        let child = child.expect("child started before cancellation");
        let state = Command::new("/bin/ps")
            .args(["-p", &child.to_string(), "-o", "stat="])
            .output()
            .unwrap();
        let status = String::from_utf8_lossy(&state.stdout);
        assert!(
            status.trim().is_empty() || status.trim().starts_with('Z'),
            "child still running: {status}"
        );
        assert!(runner.group.lock().unwrap().is_none());
    }
}
