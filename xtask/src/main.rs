mod privacy;

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use nemo_build::{install, native, package};
use serde_json::Value;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(about = "Build, verify, and install Nemo")]
struct Cli {
    #[command(subcommand)]
    command: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Run formatting, workspace Clippy, and tests without microphone or model access.
    Check,
    /// Build the release executable.
    Build,
    /// Prepare the pinned native speech runtime.
    Runtime,
    /// Build a signed application bundle without installing it.
    Package {
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Build and safely update the installed application, then confirm the running build.
    Install {
        #[arg(long)]
        destination: Option<PathBuf>,
        #[arg(long)]
        allow_permission_reset: bool,
        #[arg(long)]
        no_launch: bool,
    },
    /// Prepare native tools, download the model, and install Nemo.
    Setup,
    /// Download or resume the pinned model files.
    DownloadModel,
    /// Run checks and transcribe a generated speech file; no speaker playback or microphone.
    Verify,
    /// Check public-release metadata and documentation without publishing.
    ReleaseCheck,
    /// Scan reachable history and a clean source snapshot with Gitleaks.
    PrivacyCheck {
        #[arg(long, default_value = "gitleaks")]
        gitleaks: PathBuf,
    },
}

fn main() {
    if let Err(error) = execute() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

fn execute() -> Result<()> {
    let cli = Cli::parse();
    let root = workspace_root()?;
    env::set_current_dir(&root)?;
    match cli.command {
        Task::Check => check(),
        Task::Build => {
            println!("{}", timed("Build", build)?.display());
            Ok(())
        }
        Task::Runtime => {
            native::prepare_tools()?;
            println!("{}", timed("Runtime", || native::build(&root))?.display());
            Ok(())
        }
        Task::Package { output } => {
            let binary = timed("Build", build)?;
            let destination = destination(None)?;
            let options = package_options(&root, binary, output, &destination)?;
            println!(
                "{}",
                timed("Package", || package::build(&options))?.display()
            );
            Ok(())
        }
        Task::Install {
            destination: requested,
            allow_permission_reset,
            no_launch,
        } => install_checkout(
            &root,
            destination(requested)?,
            allow_permission_reset,
            !no_launch,
        ),
        Task::Setup => {
            native::prepare_tools()?;
            timed("Runtime", || native::build(&root))?;
            let binary = timed("Build", build)?;
            let manifest: Value =
                serde_json::from_slice(&fs::read(root.join("models/manifest.json"))?)?;
            if manifest["files"]
                .as_array()
                .context("Model manifest has no files")?
                .iter()
                .any(|file| {
                    file["name"]
                        .as_str()
                        .is_some_and(|name| root.join("models").join(name).is_file())
                })
            {
                run(Command::new(&binary)
                    .args(["model", "migrate", "--from"])
                    .arg(root.join("models")))?;
            }
            run(Command::new(&binary).args(["model", "download"]))?;
            install_checkout(&root, destination(None)?, false, true)
        }
        Task::DownloadModel => {
            run(Command::new(timed("Build", build)?).args(["model", "download"]))
        }
        Task::Verify => verify(&root),
        Task::ReleaseCheck => release_check(&root),
        Task::PrivacyCheck { gitleaks } => privacy::check(&root, &gitleaks),
    }
}

fn cargo() -> Command {
    Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

fn run(command: &mut Command) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("Cannot start {command:?}"))?;
    ensure!(status.success(), "{command:?} failed ({status})");
    Ok(())
}

fn output(command: &mut Command) -> Result<String> {
    let result = command
        .output()
        .with_context(|| format!("Cannot start {command:?}"))?;
    ensure!(
        result.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&result.stderr).trim()
    );
    Ok(String::from_utf8(result.stdout)?)
}

fn timed<T>(label: &str, action: impl FnOnce() -> Result<T>) -> Result<T> {
    let started = Instant::now();
    let result = action();
    eprintln!("{label}: {:.2}s", started.elapsed().as_secs_f64());
    result
}

fn workspace_root() -> Result<PathBuf> {
    let metadata: Value = serde_json::from_str(&output(cargo().args([
        "metadata",
        "--no-deps",
        "--locked",
        "--format-version",
        "1",
    ]))?)?;
    Ok(PathBuf::from(
        metadata["workspace_root"]
            .as_str()
            .context("Cargo metadata has no workspace root")?,
    ))
}

fn build() -> Result<PathBuf> {
    let result = cargo()
        .args([
            "build",
            "--release",
            "--locked",
            "-p",
            "nemo-dictate",
            "--bin",
            "nemo",
            "--message-format=json-render-diagnostics",
        ])
        .stderr(Stdio::inherit())
        .output()?;
    ensure!(
        result.status.success(),
        "Nemo build failed ({})",
        result.status
    );
    built_executable(&String::from_utf8(result.stdout)?)
}

fn built_executable(messages: &str) -> Result<PathBuf> {
    messages
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|message| {
            (message["reason"] == "compiler-artifact" && message["target"]["name"] == "nemo")
                .then(|| message["executable"].as_str().map(PathBuf::from))
                .flatten()
        })
        .context("Cargo did not report the nemo executable")
}

fn check() -> Result<()> {
    check_notices(&workspace_root()?)?;
    let temporary = tempfile::tempdir()?;
    run(cargo().args(["fmt", "--all", "--check"]))?;
    run(cargo().args([
        "clippy",
        "--workspace",
        "--all-targets",
        "--locked",
        "--",
        "-D",
        "warnings",
    ]))?;
    run(cargo()
        .args(["test", "--workspace", "--locked"])
        .env("NEMO_DATA_DIR", temporary.path().join("data"))
        .env("NEMO_APP_PATH", temporary.path().join("Nemo.app"))
        .env_remove("NEMO_RUNTIME"))
}

fn check_notices(root: &Path) -> Result<()> {
    let lock = nemo_build::sha256(&root.join("Cargo.lock"))?;
    ensure!(
        fs::read_to_string(root.join("assets/licenses/Rust-dependencies.txt"))?
            .contains(&format!("Cargo.lock SHA256: {lock}")),
        "Rust dependency notices are stale; run python3 scripts/dependency-notices.py"
    );
    Ok(())
}

fn destination(requested: Option<PathBuf>) -> Result<PathBuf> {
    Ok(requested
        .or_else(|| env::var_os("NEMO_APP_PATH").map(PathBuf::from))
        .unwrap_or(
            PathBuf::from(env::var_os("HOME").context("HOME is not set")?)
                .join("Applications/Nemo.app"),
        ))
}

fn package_options(
    root: &Path,
    binary: PathBuf,
    requested: Option<PathBuf>,
    destination: &Path,
) -> Result<package::Options> {
    let metadata: Value = serde_json::from_str(&output(cargo().args([
        "metadata",
        "--no-deps",
        "--locked",
        "--format-version",
        "1",
    ]))?)?;
    let version = metadata["packages"]
        .as_array()
        .context("Cargo metadata has no packages")?
        .iter()
        .find(|package| package["name"] == "nemo-dictate")
        .and_then(|package| package["version"].as_str())
        .context("Cannot find Nemo package version")?
        .to_owned();
    Ok(package::Options {
        root: root.to_path_buf(),
        binary,
        native_dir: root.join("vendor/NeMo-Speech.cpp/build/metal-server/bin"),
        output: requested.unwrap_or_else(|| root.join("dist/Nemo.app")),
        version,
        sign_identity: match env::var("NEMO_SIGN_IDENTITY") {
            Ok(identity) => Some(identity),
            Err(_) => package::signing_identity(destination)?,
        },
    })
}

fn install_checkout(
    root: &Path,
    destination: PathBuf,
    allow_permission_reset: bool,
    launch: bool,
) -> Result<()> {
    let binary = timed("Build", build)?;
    let options = package_options(root, binary.clone(), None, &destination)?;
    if env::var_os("NEMO_SIGN_IDENTITY").is_none()
        && receipt_matches(
            &destination,
            &nemo_build::sha256(&binary)?,
            &options.version,
        )
    {
        install::verify(&destination)?;
        println!("Installed payload is current: {}", destination.display());
    } else {
        let source = timed("Package", || package::build(&options))?;
        timed("Install", || {
            install::install_with(
                &install::Options {
                    source,
                    destination: destination.clone(),
                    allow_permission_reset,
                    launch: false,
                },
                |_| {
                    if launch {
                        confirm_running(&binary, &destination)
                    } else {
                        Ok(())
                    }
                },
            )
        })?;
        println!("Installed {}", destination.display());
        return Ok(());
    }
    if launch {
        timed("Startup", || confirm_running(&binary, &destination))?;
    }
    println!("Installed {}", destination.display());
    Ok(())
}

fn receipt_matches(app: &Path, hash: &str, version: &str) -> bool {
    fs::read(app.join("Contents/Resources/install-receipt.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|receipt| receipt["source_sha256"] == hash && receipt["version"] == version)
}

fn confirm_running(binary: &Path, app: &Path) -> Result<()> {
    let executable = app.join("Contents/MacOS/nemo").canonicalize()?;
    let expected = nemo_build::sha256(&executable)?;
    let query = || -> Result<Value> {
        serde_json::from_str(&output(Command::new(binary).arg("identity"))?).map_err(Into::into)
    };
    if let Ok(identity) = query() {
        validate_identity(&identity, &executable, &expected)?;
        println!("Running build: {expected}");
        return Ok(());
    }
    let before = running_pids(&executable)?;
    ensure!(
        before.is_empty(),
        "Installed Nemo is running but unresponsive; finish or quit it before retrying"
    );
    install::open(app)?;
    let result = (|| {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if let Ok(identity) = query() {
                validate_identity(&identity, &executable, &expected)?;
                println!("Running build: {expected}");
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        bail!("Installed Nemo did not become ready within 15 seconds")
    })();
    if result.is_err() {
        stop_started_app(&executable)?;
    }
    result
}

fn validate_identity(identity: &Value, executable: &Path, expected: &str) -> Result<()> {
    ensure!(
        identity["pid"].as_u64().is_some_and(|pid| pid > 0),
        "Daemon identity has no process ID"
    );
    ensure!(
        identity["sha256"] == expected,
        "The responding daemon does not match the installed build"
    );
    let reported = identity["executable"]
        .as_str()
        .context("Daemon identity has no executable")?;
    ensure!(
        Path::new(reported).canonicalize()? == executable,
        "A daemon outside the installed app is responding"
    );
    Ok(())
}

fn running_pids(executable: &Path) -> Result<Vec<u32>> {
    let processes = output(Command::new("/bin/ps").args(["-A", "-o", "pid=,comm="]))?;
    Ok(processes
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (pid, name) = line.split_once(char::is_whitespace)?;
            (Path::new(name.trim()) == executable)
                .then(|| pid.parse().ok())
                .flatten()
        })
        .collect())
}

fn stop_started_app(executable: &Path) -> Result<()> {
    let owned = running_pids(executable)?;
    for pid in &owned {
        run(Command::new("/bin/kill").args(["-TERM", &pid.to_string()]))?;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining: Vec<_> = running_pids(executable)?
            .into_iter()
            .filter(|pid| owned.contains(pid))
            .collect();
        if remaining.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            for pid in remaining {
                run(Command::new("/bin/kill").args(["-KILL", &pid.to_string()]))?;
            }
            let killed_deadline = Instant::now() + Duration::from_secs(2);
            while running_pids(executable)?
                .iter()
                .any(|pid| owned.contains(pid))
            {
                ensure!(
                    Instant::now() < killed_deadline,
                    "Candidate process did not exit; restore the retained backup after stopping it"
                );
                thread::sleep(Duration::from_millis(50));
            }
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn verify(root: &Path) -> Result<()> {
    check()?;
    let binary = timed("Build", build)?;
    let temporary = tempfile::tempdir()?;
    let configuration = output(Command::new(&binary).arg("config"))?;
    fs::write(temporary.path().join("config.toml"), configuration)?;
    let phrase = "The quick brown fox jumps over the lazy dog. This is a local speech recognition test on my laptop.";
    let aiff = temporary.path().join("sample.aiff");
    let wav = temporary.path().join("sample.wav");
    run(Command::new("/usr/bin/say")
        .args(["-v", "Samantha", "-o"])
        .arg(&aiff)
        .arg(phrase))?;
    run(Command::new("/usr/bin/afconvert")
        .args(["-f", "WAVE", "-d", "LEI16@16000", "-c", "1"])
        .arg(aiff)
        .arg(&wav))?;
    fs::create_dir_all(root.join("artifacts"))?;
    fs::copy(&wav, root.join("artifacts/sample.wav"))?;
    let transcript = output(
        Command::new(&binary)
            .arg("transcribe")
            .arg(wav)
            .env("NEMO_DATA_DIR", temporary.path()),
    )?;
    ensure!(
        words(&transcript) == words(phrase),
        "Transcript mismatch: {transcript}"
    );
    fs::create_dir_all(root.join("artifacts"))?;
    fs::write(root.join("artifacts/sample-transcript.txt"), &transcript)?;
    println!("Verified file transcription: {}", transcript.trim());
    run(cargo().env("NEMO_DATA_DIR", temporary.path()).args([
        "test",
        "--release",
        "--locked",
        "-p",
        "nemo-dictate",
        "real_model_transcribes_repeated_audio",
        "--",
        "--ignored",
        "--nocapture",
    ]))
}

fn words(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn release_check(root: &Path) -> Result<()> {
    check_notices(root)?;
    for required in [
        "LICENSE",
        "README.md",
        "CONTRIBUTING.md",
        "CHANGELOG.md",
        "docs/releasing.md",
        "models/LICENSE",
        "assets/sounds/Handy-MIT.txt",
    ] {
        ensure!(
            root.join(required).is_file(),
            "Release document missing: {required}"
        );
    }
    let manifest: Value = serde_json::from_slice(&fs::read(root.join("models/manifest.json"))?)?;
    ensure!(
        manifest["revision"]
            .as_str()
            .is_some_and(|revision| revision.len() == 40),
        "Model revision must be pinned"
    );
    for file in manifest["files"]
        .as_array()
        .context("Manifest files missing")?
    {
        ensure!(file["sha256"].as_str().is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())), "Model SHA-256 must be pinned");
    }
    let _ = workspace_root()?;
    println!(
        "Release metadata checks passed. Signing, notarization, public visibility, and publishing remain separate release steps."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_discovery_uses_cargos_executable_path() {
        let messages = "not json\n{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"xtask\"},\"executable\":\"/wrong/xtask\"}\n{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"nemo\"},\"executable\":\"/custom target/release/nemo\"}\n";
        assert_eq!(
            built_executable(messages).unwrap(),
            PathBuf::from("/custom target/release/nemo")
        );
        assert!(built_executable("{\"reason\":\"build-finished\",\"success\":true}").is_err());
    }

    #[test]
    fn installed_receipt_matches_only_the_same_payload_and_version() {
        let temporary = tempfile::tempdir().unwrap();
        let receipt = temporary
            .path()
            .join("Contents/Resources/install-receipt.json");
        fs::create_dir_all(receipt.parent().unwrap()).unwrap();
        fs::write(
            receipt,
            "{\"source_sha256\":\"new-binary\",\"version\":\"0.2.0\"}",
        )
        .unwrap();
        assert!(receipt_matches(temporary.path(), "new-binary", "0.2.0"));
        assert!(!receipt_matches(temporary.path(), "old-binary", "0.2.0"));
        assert!(!receipt_matches(temporary.path(), "new-binary", "0.1.0"));
    }
}
