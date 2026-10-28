use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub hotkey: crate::shortcut::Shortcut,
    pub model: PathBuf,
    pub runtime: PathBuf,
    pub idle_unload_seconds: u64,
    pub max_recording_seconds: u64,
    pub paste: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: crate::shortcut::Shortcut::RightOption,
            model: crate::model::Store::managed().decoder(),
            runtime: default_runtime(),
            idle_unload_seconds: 60,
            max_recording_seconds: 120,
            paste: true,
        }
    }
}

pub fn data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("NEMO_DATA_DIR") {
        return PathBuf::from(path);
    }
    PathBuf::from(std::env::var_os("HOME").expect("HOME must be set"))
        .join("Library/Application Support/Nemo")
}

fn default_runtime() -> PathBuf {
    if let Some(path) = std::env::var_os("NEMO_RUNTIME") {
        return path.into();
    }
    let executable = std::env::current_exe().expect("executable path is available");
    let bundled = executable
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("Resources/runtime/nemo-speech");
    if bundled.is_file() {
        return bundled;
    }
    if let Some(home) = std::env::var_os("HOME") {
        let installed = PathBuf::from(home)
            .join("Applications/Nemo.app/Contents/Resources/runtime/nemo-speech");
        if installed.is_file() {
            return installed;
        }
    }
    executable.parent().unwrap().join("nemo-speech")
}

pub fn path() -> PathBuf {
    data_dir().join("config.toml")
}

impl Config {
    pub fn load() -> Result<Self> {
        ensure!(
            data_dir().is_absolute(),
            "NEMO_DATA_DIR must be an absolute path"
        );
        let file = path();
        let config: Self = if file.exists() {
            toml::from_str(&fs::read_to_string(&file)?)
                .with_context(|| format!("Invalid configuration: {}", file.display()))?
        } else {
            Self::default()
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=300).contains(&self.max_recording_seconds),
            "max_recording_seconds must be 1–300"
        );
        ensure!(
            self.idle_unload_seconds > 0,
            "idle_unload_seconds must be positive"
        );
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        let _lock = crate::storage::Lock::acquire(&data_dir().join("config.lock"))?;
        self.write()
    }

    pub fn update(edit: impl FnOnce(&mut Self) -> Result<()>) -> Result<Self> {
        let _lock = crate::storage::Lock::acquire(&data_dir().join("config.lock"))?;
        let mut config = Self::load()?;
        edit(&mut config)?;
        config.write()?;
        Ok(config)
    }

    fn write(&self) -> Result<()> {
        self.validate()?;
        crate::storage::atomic_write(&path(), toml::to_string_pretty(self)?.as_bytes())
    }

    pub fn check_assets(&self) -> Result<()> {
        ensure!(
            self.runtime.is_file(),
            "Inference runtime missing. Reinstall Nemo or set the runtime path in configuration"
        );
        ensure!(
            self.model.is_file(),
            "Model missing. Choose Download model in Nemo, or run `nemo model download`"
        );
        let manifest = crate::model::manifest();
        if self.model.file_name().and_then(|s| s.to_str())
            == Some("NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-AD-IQ4_NL.gguf")
        {
            for asset in &manifest.files {
                let path = self.model.parent().unwrap().join(&asset.name);
                let size = fs::metadata(&path)
                    .with_context(|| {
                        format!("Missing {}. Download the model again", path.display())
                    })?
                    .len();
                ensure!(
                    size == asset.bytes,
                    "Incomplete model: {} has {size} of {} bytes. Download the model again",
                    path.display(),
                    asset.bytes
                );
            }
        }
        Ok(())
    }
}
