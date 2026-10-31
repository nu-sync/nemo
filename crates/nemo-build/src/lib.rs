pub mod install;
pub mod native;
pub mod package;

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::Path,
    process::{Command, Stdio},
};

pub fn run(command: &mut Command) -> Result<String> {
    let result = command
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("Cannot execute {command:?}"))?;
    ensure!(
        result.status.success(),
        "Command failed ({:?}): {}{}",
        command,
        String::from_utf8_lossy(&result.stderr),
        String::from_utf8_lossy(&result.stdout)
    );
    Ok(String::from_utf8(result.stdout)?)
}

pub fn sha256(path: &Path) -> Result<String> {
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

/// Hash the helper files packaged into `Contents/Resources/runtime`.
///
/// Installers compare this alongside the executable so a helper-only change is not
/// mistaken for an already-current installation.
pub fn runtime_sha256(root: &Path) -> Result<String> {
    let mut digest = Sha256::new();
    for name in ["mlx-serve.py", "mlx-serve.py.lock", "mlx-runner.sh"] {
        let path = root.join("scripts").join(name);
        let bytes =
            std::fs::read(&path).with_context(|| format!("Cannot read {}", path.display()))?;
        digest.update(name.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(&bytes);
    }
    Ok(format!("{:x}", digest.finalize()))
}
