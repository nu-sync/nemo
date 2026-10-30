use crate::{
    control,
    readiness::{Activity, Blocker, Readiness},
};
use anyhow::{Context, Result, ensure};
use std::{
    io::{self, Write},
    process::{Command, Stdio},
};

#[derive(Debug, PartialEq, Eq)]
enum Destination {
    SystemSettings(&'static str),
    NemoSettings,
}

struct Guidance {
    message: String,
    destination: Option<Destination>,
}

impl Guidance {
    fn from_readiness(readiness: &Readiness) -> Self {
        if let Some(blocker) = readiness.blockers.first() {
            let destination = if let Some(url) = blocker.settings_url() {
                Some(Destination::SystemSettings(url))
            } else {
                match blocker {
                    Blocker::MicrophoneUnavailable
                    | Blocker::MicrophoneConfirmation
                    | Blocker::MicrophoneScanFailed(_) => Some(Destination::NemoSettings),
                    Blocker::Accessibility
                    | Blocker::ShortcutUnavailable
                    | Blocker::MicrophonePermission
                    | Blocker::MicrophoneNotDetermined
                    | Blocker::CheckingMicrophones
                    | Blocker::ModelMissing
                    | Blocker::RuntimeMissing => None,
                }
            };
            Self {
                message: format!(
                    "{}. {}",
                    blocker.label(&readiness.shortcut),
                    blocker.remedy()
                ),
                destination,
            }
        } else {
            Self {
                message: if readiness.activity == Activity::Idle {
                    format!(
                        "Nemo is ready. Tap {} to start recording.",
                        readiness.shortcut
                    )
                } else {
                    format!(
                        "Nemo: {}. No readiness blockers reported.",
                        readiness.activity.label()
                    )
                },
                destination: None,
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Effect<'a> {
    Speak(&'a str),
    Open(&'a Destination),
}

fn perform(
    guidance: &Guidance,
    silent: bool,
    mut execute: impl FnMut(Effect<'_>) -> Result<()>,
) -> Vec<anyhow::Error> {
    let mut errors = Vec::new();
    if !silent && let Err(error) = execute(Effect::Speak(&guidance.message)) {
        errors.push(error);
    }
    if let Some(destination) = &guidance.destination
        && let Err(error) = execute(Effect::Open(destination))
    {
        errors.push(error);
    }
    errors
}

fn execute(effect: Effect<'_>) -> Result<()> {
    match effect {
        Effect::Speak(message) => {
            let mut child = Command::new("/usr/bin/say")
                .args(["-f", "-"])
                .stdin(Stdio::piped())
                .spawn()
                .context("Cannot start macOS speech")?;
            let written = child
                .stdin
                .take()
                .context("Missing speech input")?
                .write_all(message.as_bytes());
            let status = child.wait().context("Cannot wait for macOS speech")?;
            written.context("Cannot send the explanation to macOS speech")?;
            ensure!(status.success(), "macOS speech failed ({status})");
        }
        Effect::Open(Destination::SystemSettings(url)) => {
            let status = Command::new("/usr/bin/open")
                .arg(url)
                .status()
                .context("Cannot open System Settings")?;
            ensure!(
                status.success(),
                "System Settings could not be opened ({status})"
            );
        }
        Effect::Open(Destination::NemoSettings) => {
            let response = control::send("settings")?;
            ensure!(
                response == "accepted",
                "Nemo could not open its settings: {response}"
            );
        }
    }
    Ok(())
}

pub fn run(silent: bool, dry_run: bool) -> Result<()> {
    let guidance = match control::send("readiness") {
        Ok(json) => match serde_json::from_str::<Readiness>(&json) {
            Ok(readiness) => Guidance::from_readiness(&readiness),
            Err(_) => Guidance { message: "The running Nemo app does not provide readiness. Reopen the updated Nemo app, then run nemo assist again.".into(), destination: None },
        },
        Err(error) => Guidance { message: format!("Cannot inspect the Nemo app: {error}. Open Nemo.app, then run nemo assist again."), destination: None },
    };
    println!("{}", guidance.message);
    match &guidance.destination {
        Some(Destination::SystemSettings(url)) => println!(
            "{}: {url}",
            if dry_run {
                "Would open"
            } else {
                "Opening settings"
            }
        ),
        Some(Destination::NemoSettings) => println!(
            "{} Nemo Settings",
            if dry_run { "Would open" } else { "Opening" }
        ),
        None => {}
    }
    io::stdout().flush()?;
    if dry_run {
        return Ok(());
    }
    let errors = perform(&guidance, silent, execute);
    for error in &errors {
        eprintln!("{error:#}");
    }
    ensure!(
        errors.is_empty(),
        "Assistance could not complete; follow the printed instructions above"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked(blocker: Blocker) -> Guidance {
        Guidance::from_readiness(&Readiness {
            activity: Activity::Idle,
            shortcut: "Right Option".into(),
            blockers: vec![blocker, Blocker::RuntimeMissing],
        })
    }

    #[test]
    fn accessibility_is_explained_and_spoken_before_opening_its_settings() {
        let guidance = blocked(Blocker::Accessibility);
        assert!(guidance.message.starts_with(
            "Right Option blocked: enable Accessibility. Enable Nemo in System Settings"
        ));
        let mut events = Vec::new();
        let errors = perform(&guidance, false, |effect| {
            events.push(format!("{effect:?}"));
            Ok(())
        });
        assert!(errors.is_empty());
        assert_eq!(events, vec![format!("Speak({:?})", guidance.message), "Open(SystemSettings(\"x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility\"))".into()]);
    }

    #[test]
    fn speech_failure_still_opens_the_correct_microphone_pane() {
        let guidance = blocked(Blocker::MicrophonePermission);
        let mut opened = None;
        let errors = perform(&guidance, false, |effect| match effect {
            Effect::Speak(_) => anyhow::bail!("Speech unavailable"),
            Effect::Open(destination) => {
                opened = Some(format!("{destination:?}"));
                Ok(())
            }
        });
        assert_eq!(errors[0].to_string(), "Speech unavailable");
        assert_eq!(
            opened.as_deref(),
            Some(
                "SystemSettings(\"x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone\")"
            )
        );
    }

    #[test]
    fn silent_assistance_opens_settings_without_speaking() {
        let guidance = blocked(Blocker::MicrophoneUnavailable);
        let mut events = Vec::new();
        perform(&guidance, true, |effect| {
            events.push(format!("{effect:?}"));
            Ok(())
        });
        assert_eq!(events, vec!["Open(NemoSettings)"]);
    }

    #[test]
    fn missing_model_is_described_without_starting_a_download() {
        let guidance = blocked(Blocker::ModelMissing);
        assert_eq!(
            guidance.message,
            "Transcription blocked: model missing. Choose Download model in the tray."
        );
        let mut events = Vec::new();
        perform(&guidance, true, |effect| {
            events.push(format!("{effect:?}"));
            Ok(())
        });
        assert!(events.is_empty());
    }
}
