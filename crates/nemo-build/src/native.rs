use anyhow::{Context, Result, ensure};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

pub const REVISION: &str = "6a62501107e1894723880f1094a286ea996a0ec7";
const OPTIONS: [&str; 4] = [
    "-DNEMO_SPEECH_BUILD_TTS=OFF",
    "-DNEMO_SPEECH_BUILD_NMT=OFF",
    "-DNEMO_SPEECH_BUILD_DIAR=OFF",
    "-DNEMO_SPEECH_BUILD_MIC_CAPTURE=OFF",
];

fn command(program: &str) -> Command {
    let mut command = Command::new(program);
    let mut paths = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    if let Ok(path) = env::join_paths(paths) {
        command.env("PATH", path);
    }
    command
}

pub fn prepare_tools() -> Result<()> {
    ensure!(
        cfg!(target_os = "macos") && cfg!(target_arch = "aarch64"),
        "Nemo requires an Apple Silicon Mac"
    );
    let version = crate::run(command("/usr/bin/sw_vers").arg("-productVersion"))?;
    ensure!(
        version
            .trim()
            .split('.')
            .next()
            .and_then(|n| n.parse::<u32>().ok())
            .is_some_and(|n| n >= 26),
        "Nemo requires macOS 26 or newer"
    );
    crate::run(command("/usr/bin/xcode-select").arg("-p"))
        .context("Install Xcode Command Line Tools with xcode-select --install")?;
    crate::run(command("brew").arg("--prefix")).context("Install Homebrew from https://brew.sh")?;
    let missing: Vec<_> = ["cmake", "ninja", "sentencepiece", "abseil"]
        .into_iter()
        .filter(|formula| {
            crate::run(command("brew").args(["list", "--versions", formula])).is_err()
        })
        .collect();
    if !missing.is_empty() {
        crate::run(
            command("brew")
                .env("HOMEBREW_NO_AUTO_UPDATE", "1")
                .arg("install")
                .args(missing),
        )?;
    }
    Ok(())
}

pub fn build(root: &Path) -> Result<PathBuf> {
    ensure!(
        cfg!(target_os = "macos") && cfg!(target_arch = "aarch64"),
        "The Metal runtime requires an Apple Silicon Mac"
    );
    build_with(root, crate::run)
}

fn build_with(root: &Path, mut run: impl FnMut(&mut Command) -> Result<String>) -> Result<PathBuf> {
    ensure!(root.is_absolute(), "Native build root must be absolute");
    let runtime = root.join("vendor/NeMo-Speech.cpp");
    ensure!(!runtime.is_symlink(), "Runtime source cannot be a symlink");
    if !runtime.join(".git").exists() {
        ensure!(
            !runtime.exists(),
            "Runtime source path is not a Git checkout: {}",
            runtime.display()
        );
        let parent = runtime.parent().context("Runtime source has no parent")?;
        fs::create_dir_all(parent)?;
        let staging = tempfile::Builder::new()
            .prefix(".NeMo-Speech.clone-")
            .tempdir_in(parent)?;
        let source = staging.path().join("source");
        run(command("git")
            .args(["clone", "https://github.com/NVIDIA/NeMo-Speech.cpp"])
            .arg(&source))?;
        run(command("git")
            .arg("-C")
            .arg(&source)
            .args(["checkout", REVISION]))?;
        verify_revision(&source, &mut run)?;
        fs::rename(&source, &runtime)?;
    }
    verify_revision(&runtime, &mut run)?;
    run(command("git").arg("-C").arg(&runtime).args([
        "submodule",
        "update",
        "--init",
        "--depth",
        "1",
        "llama.cpp",
        "third_party/cpp-httplib",
    ]))?;
    ensure!(
        runtime.join("llama.cpp/ggml/CMakeLists.txt").is_file(),
        "llama.cpp submodule is incomplete"
    );
    ensure!(
        runtime.join("third_party/cpp-httplib/httplib.h").is_file(),
        "cpp-httplib submodule is incomplete"
    );
    let prefix = run(command("brew").arg("--prefix"))?;
    run(command("cmake")
        .current_dir(&runtime)
        .args(["--preset", "metal-server"])
        .args(OPTIONS)
        .arg(format!("-DCMAKE_PREFIX_PATH={}", prefix.trim())))?;
    run(command("cmake")
        .current_dir(&runtime)
        .args(["--build", "--preset", "metal-server", "--parallel"])
        .arg(std::thread::available_parallelism()?.get().to_string()))?;
    let bin = runtime.join("build/metal-server/bin");
    ensure!(
        bin.join("nemo-speech").is_file(),
        "Native build did not produce nemo-speech"
    );
    Ok(bin)
}

fn verify_revision(
    source: &Path,
    run: &mut impl FnMut(&mut Command) -> Result<String>,
) -> Result<()> {
    let revision = run(command("git")
        .arg("-C")
        .arg(source)
        .args(["rev-parse", "HEAD"]))?;
    ensure!(
        revision.trim() == REVISION,
        "Runtime revision differs from the tested revision: {REVISION}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::bail;

    fn args(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn checkout(path: &Path) {
        for relative in [
            ".git",
            "llama.cpp/ggml",
            "third_party/cpp-httplib",
            "build/metal-server/bin",
        ] {
            fs::create_dir_all(path.join(relative)).unwrap();
        }
        for relative in [
            "llama.cpp/ggml/CMakeLists.txt",
            "third_party/cpp-httplib/httplib.h",
            "build/metal-server/bin/nemo-speech",
        ] {
            fs::write(path.join(relative), b"fixture").unwrap();
        }
    }

    #[test]
    fn failed_checkout_does_not_publish_partial_source_and_can_be_retried() {
        let root = tempfile::tempdir().unwrap();
        let runtime = root.path().join("vendor/NeMo-Speech.cpp");
        let result = build_with(root.path(), |command| {
            let args = args(command);
            if args.first().map(String::as_str) == Some("clone") {
                checkout(Path::new(args.last().unwrap()));
            }
            if args.iter().any(|arg| arg == "checkout") {
                bail!("checkout failed");
            }
            Ok(String::new())
        });
        assert!(result.unwrap_err().to_string().contains("checkout failed"));
        assert!(!runtime.exists());
        assert_eq!(fs::read_dir(root.path().join("vendor")).unwrap().count(), 0);
        let mut configured = false;
        let bin = build_with(root.path(), |command| {
            let args = args(command);
            if args.first().map(String::as_str) == Some("clone") {
                checkout(Path::new(args.last().unwrap()));
            }
            if args.iter().any(|arg| arg == "rev-parse") {
                return Ok(REVISION.into());
            }
            if command.get_program() == "brew" {
                return Ok("/opt/homebrew\n".into());
            }
            if args.first().map(String::as_str) == Some("--preset") {
                assert_eq!(command.get_current_dir(), Some(runtime.as_path()));
                assert_eq!(
                    args,
                    [
                        "--preset",
                        "metal-server",
                        OPTIONS[0],
                        OPTIONS[1],
                        OPTIONS[2],
                        OPTIONS[3],
                        "-DCMAKE_PREFIX_PATH=/opt/homebrew"
                    ]
                );
                configured = true;
            }
            Ok(String::new())
        })
        .unwrap();
        assert!(configured);
        assert_eq!(bin, runtime.join("build/metal-server/bin"));
        assert_eq!(fs::read(bin.join("nemo-speech")).unwrap(), b"fixture");
    }

    #[test]
    fn existing_wrong_revision_and_unowned_paths_are_not_modified() {
        let root = tempfile::tempdir().unwrap();
        let runtime = root.path().join("vendor/NeMo-Speech.cpp");
        checkout(&runtime);
        assert!(
            build_with(root.path(), |_| Ok("wrong revision".into()))
                .unwrap_err()
                .to_string()
                .contains("revision differs")
        );
        fs::remove_dir_all(runtime.join(".git")).unwrap();
        assert!(
            build_with(root.path(), |_| panic!("must not run commands"))
                .unwrap_err()
                .to_string()
                .contains("not a Git checkout")
        );
        assert_eq!(
            fs::read(runtime.join("build/metal-server/bin/nemo-speech")).unwrap(),
            b"fixture"
        );
        fs::remove_dir_all(&runtime).unwrap();
        std::os::unix::fs::symlink(root.path().join("missing"), &runtime).unwrap();
        assert!(
            build_with(root.path(), |_| panic!("must not run commands"))
                .unwrap_err()
                .to_string()
                .contains("symlink")
        );
    }
}
