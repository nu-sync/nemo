use anyhow::{Context, Result, ensure};
use plist::{Dictionary, Value};
use std::{
    collections::HashSet,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

pub struct Options {
    pub root: PathBuf,
    pub binary: PathBuf,
    pub native_dir: PathBuf,
    pub output: PathBuf,
    pub version: String,
    pub sign_identity: Option<String>,
}

pub fn signing_identity(existing: &Path) -> Result<Option<String>> {
    if !existing.exists() {
        return Ok(None);
    }
    let details = Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=4"])
        .arg(existing)
        .output()?;
    ensure!(
        details.status.success(),
        "Cannot inspect installed signing identity"
    );
    let details = String::from_utf8_lossy(&details.stderr);
    let Some(authority) = details
        .lines()
        .find_map(|line| line.strip_prefix("Authority="))
    else {
        return Ok(None);
    };
    let identities = crate::run(Command::new("/usr/bin/security").args([
        "find-identity",
        "-v",
        "-p",
        "codesigning",
    ]))?;
    for line in identities.lines() {
        if line.contains(&format!("\"{authority}\""))
            && let Some(hash) = line
                .split_whitespace()
                .nth(1)
                .filter(|hash| hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Ok(Some(hash.to_owned()));
        }
    }
    anyhow::bail!(
        "The installed app's signing certificate is unavailable. Set NEMO_SIGN_IDENTITY to a usable identity"
    )
}

pub fn build(options: &Options) -> Result<PathBuf> {
    let output = std::path::absolute(&options.output)?;
    ensure!(
        output
            .extension()
            .is_some_and(|extension| extension == "app"),
        "Output must end in .app"
    );
    ensure!(!output.is_symlink(), "Package output cannot be a symlink");
    if let Some(home) = std::env::var_os("HOME") {
        let installed = PathBuf::from(home).join("Applications/Nemo.app");
        ensure!(
            output != installed
                && (!output.exists()
                    || output.canonicalize()? != installed.canonicalize().unwrap_or(installed)),
            "Packaging cannot overwrite the installed app. Use the installer"
        );
    }
    let parent = output.parent().context("Output has no parent")?;
    fs::create_dir_all(parent)?;
    let workspace = tempfile::Builder::new()
        .prefix(".nemo-package-")
        .tempdir_in(parent)?;
    let app = workspace.path().join("Nemo.app");
    let contents = app.join("Contents");
    let runtime = contents.join("Resources/runtime");
    fs::create_dir_all(&runtime)?;
    for name in ["mlx-serve.py", "mlx-serve.py.lock", "mlx-runner.sh"] {
        let source = options.root.join("scripts").join(name);
        let destination = runtime.join(name);
        fs::copy(&source, &destination)
            .with_context(|| format!("Cannot bundle {}", source.display()))?;
        let mode = fs::metadata(&destination)?.permissions().mode();
        let mode = if name.ends_with(".sh") {
            mode | 0o111
        } else {
            mode
        };
        fs::set_permissions(&destination, fs::Permissions::from_mode(mode))?;
    }
    fs::create_dir_all(contents.join("MacOS"))?;
    let binary = contents.join("MacOS/nemo");
    fs::copy(&options.binary, &binary)?;
    let source_sha256 = crate::sha256(&binary)?;
    let runtime_sha256 = crate::runtime_sha256(&options.root)?;
    let identity = options
        .sign_identity
        .clone()
        .or_else(|| std::env::var("NEMO_SIGN_IDENTITY").ok())
        .unwrap_or_else(|| "-".into());
    let native = std::path::absolute(&options.native_dir)?;
    let mut queue = vec![native.join("nemo-speech")];
    let mut copied = HashSet::new();
    while let Some(source) = queue.pop() {
        let name = source
            .file_name()
            .context("Runtime dependency has no filename")?
            .to_owned();
        if !copied.insert(name.clone()) {
            continue;
        }
        let destination = runtime.join(&name);
        fs::copy(&source, &destination)
            .with_context(|| format!("Cannot copy {}", source.display()))?;
        let mode = fs::metadata(&destination)?.permissions().mode();
        fs::set_permissions(&destination, fs::Permissions::from_mode(mode | 0o200))?;
        let dependencies = crate::run(Command::new("/usr/bin/otool").arg("-L").arg(&source))?;
        let mut changes = Command::new("/usr/bin/install_name_tool");
        let mut changed = false;
        for line in dependencies.lines().skip(1) {
            let dependency = line.trim().split(" (").next().unwrap_or_default();
            if dependency.starts_with("/System/") || dependency.starts_with("/usr/lib/") {
                continue;
            }
            let dependency_name = Path::new(dependency)
                .file_name()
                .context("Runtime dependency has no filename")?;
            if dependency_name == name {
                continue;
            }
            let candidates = [
                PathBuf::from(dependency),
                source.parent().unwrap().join(dependency_name),
                native.join(dependency_name),
                Path::new("/opt/homebrew/lib").join(dependency_name),
            ];
            let resolved = candidates
                .into_iter()
                .find(|path| path.is_file())
                .with_context(|| {
                    format!("Cannot resolve {dependency} used by {}", source.display())
                })?;
            queue.push(resolved);
            changes.arg("-change").arg(dependency).arg(format!(
                "@loader_path/{}",
                dependency_name.to_string_lossy()
            ));
            changed = true;
        }
        if source
            .extension()
            .is_some_and(|extension| extension == "dylib")
        {
            changes
                .arg("-id")
                .arg(format!("@rpath/{}", name.to_string_lossy()));
            changed = true;
        }
        if changed {
            crate::run(changes.arg(&destination))?;
        }
    }
    for entry in fs::read_dir(&runtime)? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "dylib")
        {
            sign(&path, &identity, None)?;
        }
    }
    sign(&runtime.join("nemo-speech"), &identity, None)?;
    let mut info = Dictionary::new();
    for (key, value) in [
        ("CFBundleExecutable", "nemo"),
        ("CFBundleIdentifier", "computer.nemo.dictate"),
        ("CFBundleName", "Nemo"),
        ("CFBundleDisplayName", "Nemo"),
        ("CFBundlePackageType", "APPL"),
        ("CFBundleShortVersionString", &options.version),
        ("CFBundleVersion", &options.version),
        ("LSMinimumSystemVersion", "26.0"),
        (
            "NSMicrophoneUsageDescription",
            "Nemo records your voice when you toggle dictation. Audio stays on this Mac.",
        ),
    ] {
        info.insert(key.into(), Value::String(value.into()));
    }
    info.insert("LSUIElement".into(), Value::Boolean(true));
    info.insert("NSHighResolutionCapable".into(), Value::Boolean(true));
    Value::Dictionary(info).to_file_xml(contents.join("Info.plist"))?;
    let licenses = contents.join("Resources/licenses");
    fs::create_dir_all(&licenses)?;
    for (source, destination) in [
        ("LICENSE", "Nemo-MIT.txt"),
        ("assets/sounds/Handy-MIT.txt", "Handy-MIT.txt"),
    ] {
        fs::copy(options.root.join(source), licenses.join(destination))?;
    }
    if options.root.join("models/LICENSE").is_file() {
        fs::copy(
            options.root.join("models/LICENSE"),
            licenses.join("Model-OpenMDW-1.1.txt"),
        )?;
    }
    for name in ["LICENSE", "NOTICE", "THIRD_PARTY_NOTICES.md"] {
        fs::copy(
            options.root.join("vendor/NeMo-Speech.cpp").join(name),
            licenses.join(format!("NeMo-Speech-{name}")),
        )?;
    }
    fs::copy(
        options.root.join("models/manifest.json"),
        contents.join("Resources/model-manifest.json"),
    )?;
    fs::write(
        contents.join("Resources/install-receipt.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"source_sha256":source_sha256,"runtime_sha256":runtime_sha256,"version":options.version}),
        )?,
    )?;
    sign(
        &app,
        &identity,
        Some(&options.root.join("scripts/entitlements.plist")),
    )?;
    crate::install::verify(&app)?;
    let previous = workspace.path().join("previous.app");
    if output.exists() {
        fs::rename(&output, &previous)?;
    }
    if let Err(error) = fs::rename(&app, &output) {
        if previous.exists() {
            fs::rename(&previous, &output)
                .context("Cannot restore previous package after packaging failure")?;
        }
        return Err(error.into());
    }
    Ok(output)
}

fn sign(path: &Path, identity: &str, entitlements: Option<&Path>) -> Result<()> {
    let mut command = Command::new("/usr/bin/codesign");
    command.args(["--force", "--sign", identity]);
    if identity != "-" {
        command.args(["--timestamp", "--options", "runtime"]);
    }
    if let Some(entitlements) = entitlements {
        command.arg("--entitlements").arg(entitlements);
    }
    crate::run(command.arg(path))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_preserves_original_payload_receipt_and_rejects_tampering() {
        let root = tempfile::tempdir().unwrap();
        for (path, bytes) in [
            ("LICENSE", &b"MIT"[..]), ("assets/sounds/Handy-MIT.txt", b"Handy MIT"),
            ("models/manifest.json", b"{}"), ("vendor/NeMo-Speech.cpp/LICENSE", b"Apache"),
            ("vendor/NeMo-Speech.cpp/NOTICE", b"notice"), ("vendor/NeMo-Speech.cpp/THIRD_PARTY_NOTICES.md", b"third party"),
            ("scripts/entitlements.plist", b"<?xml version=\"1.0\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict/></plist>"),
            ("scripts/mlx-serve.py", b"print('helper')"),
            ("scripts/mlx-serve.py.lock", b"lock"),
            ("scripts/mlx-runner.sh", b"#!/bin/sh\nexec python3\n"),
        ] {
            let path = root.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        let binary = root.path().join("binary");
        let native = root.path().join("native");
        fs::create_dir(&native).unwrap();
        fs::copy("/usr/bin/true", &binary).unwrap();
        fs::copy("/usr/bin/true", native.join("nemo-speech")).unwrap();
        let expected = crate::sha256(&binary).unwrap();
        let output = build(&Options {
            root: root.path().to_owned(),
            binary,
            native_dir: native,
            output: root.path().join("Nemo.app"),
            version: "9.8.7".into(),
            sign_identity: Some("-".into()),
        })
        .unwrap();
        let receipt: serde_json::Value = serde_json::from_slice(
            &fs::read(output.join("Contents/Resources/install-receipt.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(receipt["source_sha256"], expected);
        assert_eq!(
            receipt["runtime_sha256"],
            crate::runtime_sha256(root.path()).unwrap()
        );
        assert_eq!(receipt["version"], "9.8.7");
        crate::install::verify(&output).unwrap();
        let runtime = output.join("Contents/Resources/runtime");
        for name in ["mlx-serve.py", "mlx-serve.py.lock", "mlx-runner.sh"] {
            assert!(runtime.join(name).is_file(), "missing bundled {name}");
        }
        assert_ne!(
            fs::metadata(runtime.join("mlx-runner.sh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
        fs::write(
            output.join("Contents/Resources/model-manifest.json"),
            b"tampered",
        )
        .unwrap();
        assert!(crate::install::verify(&output).is_err());
    }
}
