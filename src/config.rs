use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

/// The engine that runs transcription. Nemo speaks the local HTTP contract that
/// `Backend` and the benchmark harness both drive; any other engine has to speak
/// the same contract to be selectable here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    /// The bundled `nemo-speech` runtime. The default and the only shipped engine.
    #[default]
    NemoSpeech,
    /// An out-of-tree engine helper (for example an MLX-Audio script) sitting beside
    /// the bundled runtime. Experimental: it needs its own weights and toolchain.
    MlxAudio,
}

impl Backend {
    /// Every backend, in the order a settings list would show them.
    pub fn all() -> impl Iterator<Item = Self> {
        [Self::NemoSpeech, Self::MlxAudio].into_iter()
    }

    /// The weight profile this backend runs, which is how the store and the CLI
    /// name it.
    pub fn profile(self) -> &'static str {
        match self {
            Self::NemoSpeech => crate::model::DEFAULT_PROFILE,
            Self::MlxAudio => "mlx-audio",
        }
    }

    /// The backend for a profile name, when one exists.
    pub fn from_profile(profile: &str) -> Option<Self> {
        Self::all().find(|backend| backend.profile() == profile)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::NemoSpeech => "nemo-speech",
            Self::MlxAudio => "mlx-audio",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub microphone: Option<crate::audio::Microphone>,
    pub hotkey: crate::shortcut::Shortcut,
    pub model: PathBuf,
    pub runtime: PathBuf,
    #[serde(skip_serializing_if = "is_default")]
    pub backend: Backend,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mlx_runtime: Option<PathBuf>,
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
            backend: Backend::default(),
            mlx_runtime: None,
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
        if let Some(path) = &self.mlx_runtime {
            ensure!(path.is_absolute(), "mlx_runtime must be an absolute path");
        }
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

    /// The engine executable the configured backend runs. An alternative backend
    /// defaults to a helper sitting beside the bundled runtime, so one install can
    /// carry both without a second configuration key.
    pub fn runtime_path(&self) -> PathBuf {
        match self.backend {
            Backend::NemoSpeech => self.runtime.clone(),
            Backend::MlxAudio => self.mlx_runtime.clone().unwrap_or_else(|| {
                self.runtime
                    .parent()
                    .map(|directory| directory.join("mlx-serve.py"))
                    .unwrap_or_else(|| PathBuf::from("mlx-serve.py"))
            }),
        }
    }

    /// The configuration that runs the included engine, for when the selected engine
    /// fails.
    ///
    /// The selected engine owns `runtime` and `model`, so falling back rebuilds those two
    /// from the shipped defaults while keeping every setting that has nothing to do with
    /// the engine. Returns `None` when the included engine is already selected, because
    /// there is nothing behind it.
    pub fn included(&self) -> Option<Self> {
        if self.backend == Backend::NemoSpeech {
            return None;
        }
        Some(Self {
            backend: Backend::NemoSpeech,
            runtime: default_runtime(),
            model: crate::model::Store::managed().decoder(),
            ..self.clone()
        })
    }

    pub fn check_assets(&self) -> Result<()> {
        ensure!(
            self.runtime_path().is_file(),
            "Inference runtime missing for backend '{}'. Reinstall Nemo or set the runtime path in configuration",
            self.backend.label()
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

    fn sample_runtime() -> std::path::PathBuf {
        std::path::PathBuf::from("/Applications/Nemo.app/Contents/Resources/runtime/nemo-speech")
    }

    #[test]
    fn the_default_backend_is_the_bundled_runtime() {
        let config = Config::default();
        assert_eq!(config.backend, Backend::NemoSpeech);
        assert_eq!(config.runtime_path(), config.runtime);
    }

    #[test]
    fn the_default_configuration_omits_the_backend_key() {
        let toml = toml::to_string_pretty(&Config::default()).unwrap();
        assert!(
            !toml.contains("backend"),
            "default config must not pin a backend: {toml}"
        );
        assert!(!toml.contains("mlx_runtime"));
    }

    #[test]
    fn an_alternative_backend_defaults_beside_the_bundled_runtime() {
        let toml = format!(
            "runtime = {:?}\nbackend = \"mlx-audio\"\n",
            sample_runtime().display().to_string()
        );
        let config: Config = toml::from_str(&toml).unwrap();
        assert_eq!(config.backend, Backend::MlxAudio);
        assert_eq!(
            config.runtime_path(),
            std::path::PathBuf::from(
                "/Applications/Nemo.app/Contents/Resources/runtime/mlx-serve.py"
            )
        );
    }

    #[test]
    fn an_explicit_helper_path_wins_and_must_be_absolute() {
        let toml = "runtime = \"/opt/nemo/nemo-speech\"\nbackend = \"mlx-audio\"\nmlx_runtime = \"/opt/nemo/helper.py\"\n";
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(
            config.runtime_path(),
            std::path::PathBuf::from("/opt/nemo/helper.py")
        );

        let relative = "runtime = \"/opt/nemo/nemo-speech\"\nbackend = \"mlx-audio\"\nmlx_runtime = \"helper.py\"\n";
        let config: Config = toml::from_str(relative).unwrap();
        assert!(
            config.validate().is_err(),
            "relative helper path must be rejected"
        );
    }

    #[test]
    fn an_unknown_backend_is_rejected_rather_than_silently_ignored() {
        let toml = "backend = \"whisper-cpp\"\n";
        assert!(toml::from_str::<Config>(toml).is_err());
    }
}
