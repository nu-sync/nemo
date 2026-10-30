use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub microphone: Option<crate::audio::Microphone>,
    pub hotkey: crate::shortcut::Shortcut,
    pub model: PathBuf,
    pub runtime: PathBuf,
    pub idle_unload_seconds: u64,
    #[serde(skip_serializing_if = "is_default")]
    pub preload_model: bool,
    #[serde(skip_serializing_if = "is_default")]
    pub model_warmup: bool,
    #[serde(skip_serializing_if = "is_default")]
    pub inference_threads: u32,
    #[serde(skip_serializing_if = "is_one")]
    pub encoder_batch_size: u32,
    pub max_recording_seconds: u64,
    pub paste: bool,
    #[serde(skip_serializing_if = "is_default")]
    pub remove_trailing_period: bool,
    #[serde(skip_serializing_if = "is_default")]
    pub sound_pack: crate::sounds::SoundPack,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

fn is_one(value: &u32) -> bool {
    *value == 1
}

impl Default for Config {
    fn default() -> Self {
        Self {
            microphone: None,
            hotkey: crate::shortcut::Shortcut::RightOption,
            model: crate::model::Store::managed().decoder(),
            runtime: default_runtime(),
            idle_unload_seconds: 60,
            preload_model: false,
            model_warmup: false,
            inference_threads: 0,
            encoder_batch_size: 1,
            max_recording_seconds: 120,
            paste: true,
            remove_trailing_period: false,
            sound_pack: crate::sounds::SoundPack::default(),
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
    if let Some(app) = std::env::var_os("NEMO_APP_PATH") {
        return PathBuf::from(app).join("Contents/Resources/runtime/nemo-speech");
    }
    let adjacent = executable.parent().unwrap().join("nemo-speech");
    if let Some(home) = std::env::var_os("HOME") {
        let installed = PathBuf::from(home)
            .join("Applications/Nemo.app/Contents/Resources/runtime/nemo-speech");
        if installed.is_file() || !adjacent.is_file() {
            return installed;
        }
    }
    adjacent
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
        ensure!(
            self.inference_threads <= 256,
            "inference_threads must be 0–256"
        );
        ensure!(
            (1..=16).contains(&self.encoder_batch_size),
            "encoder_batch_size must be 1–16"
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
        self.check_model_assets()
    }

    pub fn check_model_assets(&self) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_configuration_keeps_inference_defaults() {
        let config: Config = toml::from_str(
            "hotkey = 'RightOption'\nmodel = '/models/decoder.gguf'\nruntime = '/runtime/nemo-speech'\nidle_unload_seconds = 60\nmax_recording_seconds = 120\npaste = true\n",
        )
        .unwrap();
        config.validate().unwrap();
        assert!(!config.remove_trailing_period);
        assert_eq!(config.sound_pack, crate::sounds::SoundPack::HandyMarimba);
        assert!(!config.preload_model);
        assert!(!config.model_warmup);
        assert_eq!(config.inference_threads, 0);
        assert_eq!(config.encoder_batch_size, 1);
        assert_eq!(config.idle_unload_seconds, 60);
    }

    #[test]
    fn five_minute_performance_configuration_round_trips() {
        let config = Config {
            idle_unload_seconds: 300,
            preload_model: true,
            model_warmup: true,
            inference_threads: 18,
            encoder_batch_size: 4,
            ..Config::default()
        };
        config.validate().unwrap();
        let serialized = toml::to_string_pretty(&config).unwrap();
        let restored: Config = toml::from_str(&serialized).unwrap();
        restored.validate().unwrap();
        assert_eq!(toml::to_string_pretty(&restored).unwrap(), serialized);
    }

    #[test]
    fn sound_pack_configuration_round_trips_and_rejects_unknown_packs() {
        for (value, expected) in [
            ("handy_marimba", crate::sounds::SoundPack::HandyMarimba),
            ("macos_recording", crate::sounds::SoundPack::MacosRecording),
            ("minimal", crate::sounds::SoundPack::Minimal),
            ("chime", crate::sounds::SoundPack::Chime),
        ] {
            let config: Config = toml::from_str(&format!("sound_pack = '{value}'")).unwrap();
            assert_eq!(config.sound_pack, expected);
            let restored: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
            assert_eq!(restored.sound_pack, expected);
        }
        assert!(toml::from_str::<Config>("sound_pack = 'unknown'").is_err());
    }

    #[test]
    fn invalid_inference_tuning_is_rejected() {
        assert!(
            Config {
                idle_unload_seconds: 0,
                ..Config::default()
            }
            .validate()
            .unwrap_err()
            .to_string()
            .contains("idle_unload_seconds")
        );
        for threads in [257, u32::MAX] {
            let config = Config {
                inference_threads: threads,
                ..Config::default()
            };
            assert!(
                config
                    .validate()
                    .unwrap_err()
                    .to_string()
                    .contains("inference_threads")
            );
        }
        for batch_size in [0, 17, u32::MAX] {
            let config = Config {
                encoder_batch_size: batch_size,
                ..Config::default()
            };
            assert!(
                config
                    .validate()
                    .unwrap_err()
                    .to_string()
                    .contains("encoder_batch_size")
            );
        }
        assert!(toml::from_str::<Config>("inference_threads = -1").is_err());
    }
}
