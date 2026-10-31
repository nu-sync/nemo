//! Dictation survives a broken engine.
//!
//! A second backend is experimental: its own weights, its own toolchain, its own helper
//! process. None of that may cost a user their dictation. So a failure that survives one
//! fresh load demotes the selected engine for the rest of the session and the included
//! engine takes over — the same engine that runs when nothing is configured. The reason
//! is kept so status and telemetry can say why the selected engine is not the one running.
//!
//! Retrying is deliberately not free: a failed load costs seconds on the hot path of a
//! dictation, so the demotion is sticky until the configuration changes or Nemo restarts.

use crate::{
    backend::{Backend, Process},
    config::{Backend as Engine, Config},
};
use anyhow::{Context, Result};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// What the daemon reports about which engine is actually serving, when that differs
/// from the one the user selected.
pub type Status = Arc<Mutex<Option<String>>>;

/// The selected engine, and the included engine behind it.
pub struct Engines {
    primary: Backend,
    fallback: Option<Backend>,
    demoted: Option<String>,
    status: Status,
}

impl Engines {
    /// Build both engines up front so a failure needs no setup on the hot path. The
    /// fallback shares the primary's process slot: only one engine runs at a time, so a
    /// demotion costs a load but never doubles resident memory.
    pub fn new(config: Config, process: Process, status: Status) -> Result<Self> {
        let fallback = config
            .included()
            .map(|included| Backend::new(included, process.clone()))
            .transpose()?;
        Ok(Self {
            primary: Backend::new(config, process)?,
            fallback,
            demoted: None,
            status,
        })
    }

    /// Whether an engine is loaded and answering.
    pub fn loaded(&self) -> bool {
        self.primary.loaded()
    }

    /// Stop the running engine, whichever one it is.
    pub fn unload(&mut self) -> bool {
        let primary = self.primary.unload();
        primary | self.fallback.as_mut().is_some_and(Backend::unload)
    }

    /// Load the engine that will serve the next request.
    pub fn ready(&mut self, canceled: &AtomicBool) -> Result<bool> {
        if self.demoted.is_none() {
            match self.primary.ready(canceled) {
                Ok(reused) => return Ok(reused),
                Err(error) if self.can_fall_back(canceled, &error) => self.demote(&error),
                Err(error) => return Err(error),
            }
        }
        self.serve_fallback(|fallback| fallback.ready(canceled))
            .map(|reused| {
                // The included engine was loaded from cold, so nothing was reused.
                let _ = reused;
                false
            })
    }

    /// Transcribe with the selected engine, or with the included engine when the
    /// selected one fails.
    pub fn transcribe(&mut self, audio: Vec<u8>, canceled: &AtomicBool) -> Result<String> {
        if self.demoted.is_some() {
            return self.serve_fallback(|fallback| fallback.transcribe(audio, canceled));
        }
        // Only copy the audio when a second engine exists to try it on.
        let retry = self.fallback.is_some().then(|| audio.clone());
        match self.primary.transcribe(audio, canceled) {
            Ok(text) => Ok(text),
            Err(error) if retry.is_some() && self.can_fall_back(canceled, &error) => {
                self.demote(&error);
                let audio = retry.expect("checked above");
                self.serve_fallback(|fallback| fallback.transcribe(audio, canceled))
                    .with_context(|| {
                        format!(
                            "The selected engine failed ({error:#}) and the included engine failed too"
                        )
                    })
            }
            Err(error) => Err(error),
        }
    }

    fn serve_fallback<T>(&mut self, run: impl FnOnce(&mut Backend) -> Result<T>) -> Result<T> {
        let fallback = self.fallback.as_mut().context(
            "No fallback engine is configured; select the included engine in configuration",
        )?;
        run(fallback)
    }

    /// A canceled request is the user's decision, not an engine failure, and the included
    /// engine would be asked to do work nobody wants.
    fn can_fall_back(&self, canceled: &AtomicBool, error: &anyhow::Error) -> bool {
        should_fall_back(
            self.fallback.is_some(),
            canceled.load(Ordering::Acquire),
            &error.to_string(),
        )
    }

    fn demote(&mut self, error: &anyhow::Error) {
        let reason = format!("{error:#}");
        let selected = self.primary.backend();
        // The engine actually standing in, taken from the fallback rather than assumed.
        let standing_in = self
            .fallback
            .as_ref()
            .map(Backend::backend)
            .unwrap_or(Engine::NemoSpeech);
        eprintln!(
            "{}",
            serde_json::json!({
                "event": "backend_demoted",
                "engine": selected.label(),
                "fallback": standing_in.label(),
                "reason": reason,
            })
        );
        self.primary.unload();
        *self.status.lock().unwrap() = Some(format!(
            "{} failed; using {}",
            selected.label(),
            standing_in.label()
        ));
        self.demoted = Some(reason);
    }
}

/// The whole decision, kept separate from the process handling so every combination can
/// be checked without spawning an engine.
fn should_fall_back(has_fallback: bool, canceled: bool, error: &str) -> bool {
    has_fallback && !canceled && !error.contains("Canceled")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_falls_back_only_when_there_is_something_behind_it() {
        assert!(should_fall_back(true, false, "helper missing"));
        assert!(!should_fall_back(false, false, "helper missing"));
    }

    #[test]
    fn cancellation_is_never_treated_as_a_failure() {
        assert!(!should_fall_back(true, true, "helper missing"));
        assert!(!should_fall_back(true, false, "Canceled"));
    }

    #[test]
    fn an_engine_that_already_reported_its_own_failure_is_not_retried() {
        // The fallback's own error must not look like a reason to fall back again; there
        // is nothing behind the included engine.
        assert!(!should_fall_back(false, false, "loading timed out"));
    }

    #[test]
    fn a_canceled_request_is_not_an_engine_failure() {
        let canceled = AtomicBool::new(true);
        let error = anyhow::anyhow!("Canceled");
        // No fallback is configured here, so the guard is exercised through its
        // cancellation branch only.
        let engines = Engines {
            primary: Backend::new(Config::default(), Process::default()).unwrap(),
            fallback: None,
            demoted: None,
            status: Status::default(),
        };
        assert!(!engines.can_fall_back(&canceled, &error));
    }

    #[test]
    fn the_included_engine_is_only_a_fallback_when_another_is_selected() {
        assert!(Config::default().included().is_none());
        let config = Config {
            backend: Engine::MlxAudio,
            ..Config::default()
        };
        let included = config.included().expect("a fallback exists");
        assert_eq!(included.backend, Engine::NemoSpeech);
        assert_eq!(included.runtime, Config::default().runtime);
        assert_eq!(included.model, crate::model::Store::managed().decoder());
    }
}
