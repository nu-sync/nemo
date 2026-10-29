use anyhow::{Context, Result, ensure};
use std::{collections::BTreeSet, fs, path::Path, process::Command};

pub fn check(root: &Path, scanner: &Path) -> Result<()> {
    let scanner = if scanner.components().count() > 1 {
        scanner.canonicalize()?
    } else {
        scanner.to_owned()
    };
    let files = crate::output(Command::new("git").current_dir(root).args([
        "ls-files",
        "--cached",
        "--others",
        "--exclude-standard",
        "-z",
    ]))?;
    let snapshot = tempfile::tempdir()?;
    let mut count = 0;
    for relative in files
        .split('\0')
        .filter(|name| !name.is_empty())
        .collect::<BTreeSet<_>>()
    {
        let source = root.join(relative);
        if !source.exists() && !source.is_symlink() {
            continue;
        }
        validate_path(Path::new(relative))?;
        ensure!(
            !source.is_symlink(),
            "Review source symlink before release: {relative}"
        );
        ensure!(
            fs::metadata(&source)?.len() <= 1024 * 1024,
            "Review large source file before release: {relative}"
        );
        let target = snapshot.path().join(relative);
        fs::create_dir_all(target.parent().context("Source file has no parent")?)?;
        fs::copy(source, target)?;
        count += 1;
    }
    let reports = root.join("artifacts/privacy-review");
    fs::create_dir_all(&reports)?;
    let version = crate::output(Command::new(&scanner).arg("version"))?;
    for (mode, source, report) in [
        ("git", root, "history.json"),
        ("dir", snapshot.path(), "source.json"),
    ] {
        let mut command = Command::new(&scanner);
        command
            .current_dir(root)
            .arg(mode)
            .arg(source)
            .args([
                "--redact",
                "--no-banner",
                "--no-color",
                "--ignore-gitleaks-allow",
                "--report-format",
                "json",
            ])
            .arg("--report-path")
            .arg(reports.join(report))
            .env_remove("GITLEAKS_CONFIG")
            .env_remove("GITLEAKS_CONFIG_TOML");
        if mode == "git" {
            command.arg("--log-opts=--all --full-history");
        }
        crate::run(&mut command)?;
    }
    let summary = serde_json::json!({
        "scanner": format!("gitleaks {}",version.trim()),
        "head": crate::output(Command::new("git").current_dir(root).args(["rev-parse", "HEAD"]))?.trim(),
        "source_files": count,
        "scope": "all reachable Git history and current non-ignored source files",
        "limitations": "Automated secret detection and source-path checks; manual privacy and license review required",
    });
    fs::write(
        reports.join("summary.json"),
        serde_json::to_vec_pretty(&summary)?,
    )?;
    println!(
        "Privacy checks passed: {count} source files and all reachable history; {}",
        version.trim()
    );
    Ok(())
}

fn validate_path(path: &Path) -> Result<()> {
    let name = path
        .file_name()
        .context("Source path has no filename")?
        .to_string_lossy()
        .to_lowercase();
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_lowercase();
    let generated = path.components().any(|part| {
        matches!(
            part.as_os_str().to_str(),
            Some("artifacts" | "target" | "vendor" | "dist" | ".venv" | "__pycache__")
        )
    });
    let credentials = name == ".env"
        || name.starts_with(".env.")
        || matches!(
            extension.as_str(),
            "p12" | "pfx" | "key" | "pem" | "mobileprovision"
        );
    let licensed_cue = matches!(
        path.to_str(),
        Some("assets/sounds/handy-marimba-start.wav" | "assets/sounds/handy-marimba-stop.wav")
    );
    let captured = matches!(
        extension.as_str(),
        "gguf" | "part" | "aiff" | "aif" | "caf" | "mp3" | "m4a" | "wav"
    ) && !licensed_cue;
    ensure!(
        !generated && !credentials && !captured,
        "Review excluded/private source path before release: {}",
        path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_source_rejects_local_data_and_keeps_licensed_audio() {
        for path in [
            "artifacts/transcript.txt",
            "target/release/nemo",
            "vendor/runtime.c",
            ".env.local",
            "signing/private.p12",
            "recording.wav",
            "models/model.gguf",
        ] {
            assert!(validate_path(Path::new(path)).is_err(), "{path}");
        }
        for path in [
            "src/main.rs",
            "docs/performance.md",
            "assets/sounds/handy-marimba-start.wav",
            "assets/sounds/handy-marimba-stop.wav",
        ] {
            validate_path(Path::new(path)).unwrap();
        }
    }
}
