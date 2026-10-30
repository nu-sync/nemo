use crate::{
    permissions::{Microphone, Snapshot},
    shortcut::Shortcut,
};
use serde::{Deserialize, Serialize};

pub const ACCESSIBILITY_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
pub const MICROPHONE_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Activity {
    Checking,
    Idle,
    OpeningMicrophone,
    Recording,
    LoadingModel,
    DownloadingModel,
    Transcribing,
    Canceling,
}

impl Activity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Checking => "Checking readiness",
            Self::Idle => "Idle",
            Self::OpeningMicrophone => "Opening microphone",
            Self::Recording => "Recording",
            Self::LoadingModel => "Loading model",
            Self::DownloadingModel => "Downloading model",
            Self::Transcribing => "Transcribing",
            Self::Canceling => "Canceling",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Input {
    Checking,
    Available,
    NeedsConfirmation,
    Unavailable,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Blocker {
    Accessibility,
    ShortcutUnavailable,
    MicrophonePermission,
    MicrophoneNotDetermined,
    CheckingMicrophones,
    MicrophoneUnavailable,
    MicrophoneConfirmation,
    MicrophoneScanFailed(String),
    ModelMissing,
    RuntimeMissing,
}

impl Blocker {
    pub fn settings_url(&self) -> Option<&'static str> {
        match self {
            Self::Accessibility => Some(ACCESSIBILITY_URL),
            Self::MicrophonePermission | Self::MicrophoneNotDetermined => Some(MICROPHONE_URL),
            Self::ShortcutUnavailable
            | Self::CheckingMicrophones
            | Self::MicrophoneUnavailable
            | Self::MicrophoneConfirmation
            | Self::MicrophoneScanFailed(_)
            | Self::ModelMissing
            | Self::RuntimeMissing => None,
        }
    }

    pub fn label(&self, shortcut: &str) -> String {
        match self {
            Self::Accessibility => format!("{shortcut} blocked: enable Accessibility"),
            Self::ShortcutUnavailable => format!("{shortcut} unavailable: retrying registration"),
            Self::MicrophonePermission => "Recording blocked: enable Microphone".into(),
            Self::MicrophoneNotDetermined => {
                "Microphone permission needed on first recording".into()
            }
            Self::CheckingMicrophones => "Checking microphones".into(),
            Self::MicrophoneUnavailable => "Recording blocked: no microphone available".into(),
            Self::MicrophoneConfirmation => {
                "Selected microphone unavailable: confirm replacement when recording".into()
            }
            Self::MicrophoneScanFailed(error) => format!("Cannot check microphones: {error}"),
            Self::ModelMissing => "Transcription blocked: model missing".into(),
            Self::RuntimeMissing => "Transcription blocked: runtime missing".into(),
        }
    }

    pub fn action(&self) -> Option<&'static str> {
        match self {
            Self::Accessibility => Some("permissions"),
            Self::MicrophonePermission | Self::MicrophoneNotDetermined => Some("microphone"),
            Self::ModelMissing => Some("download"),
            Self::MicrophoneUnavailable
            | Self::MicrophoneConfirmation
            | Self::MicrophoneScanFailed(_) => Some("settings"),
            Self::ShortcutUnavailable | Self::CheckingMicrophones | Self::RuntimeMissing => None,
        }
    }

    pub fn remedy(&self) -> &'static str {
        match self {
            Self::Accessibility => {
                "Enable Nemo in System Settings → Privacy & Security → Accessibility. If already enabled, toggle it off and on, then reopen Nemo."
            }
            Self::ShortcutUnavailable => {
                "Nemo retries registration every two seconds. Check daemon.log if it remains unavailable."
            }
            Self::MicrophonePermission => {
                "Enable Nemo in System Settings → Privacy & Security → Microphone."
            }
            Self::MicrophoneNotDetermined => {
                "Choose Record in the tray and allow Nemo's microphone request."
            }
            Self::CheckingMicrophones => "Wait for microphone discovery.",
            Self::MicrophoneUnavailable | Self::MicrophoneScanFailed(_) => {
                "Connect a microphone and select it in Nemo Settings."
            }
            Self::MicrophoneConfirmation => {
                "Choose Record and confirm the replacement microphone, or select another in Nemo Settings."
            }
            Self::ModelMissing => "Choose Download model in the tray.",
            Self::RuntimeMissing => "Reinstall Nemo to restore the bundled runtime.",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Readiness {
    pub activity: Activity,
    pub shortcut: String,
    pub blockers: Vec<Blocker>,
}

impl Readiness {
    pub fn checking(shortcut: &Shortcut) -> Self {
        Self {
            activity: Activity::Checking,
            shortcut: shortcut.to_string(),
            blockers: vec![],
        }
    }

    pub fn observe(
        activity: Activity,
        shortcut: &Shortcut,
        permissions: Snapshot,
        registered: bool,
        input: &Input,
        model: bool,
        runtime: bool,
    ) -> Self {
        let mut blockers = Vec::new();
        if !permissions.shortcut_allowed(shortcut) {
            blockers.push(Blocker::Accessibility);
        } else if !registered {
            blockers.push(Blocker::ShortcutUnavailable);
        }
        match permissions.microphone {
            Microphone::Denied | Microphone::Restricted => {
                blockers.push(Blocker::MicrophonePermission)
            }
            Microphone::NotDetermined => blockers.push(Blocker::MicrophoneNotDetermined),
            Microphone::Authorized => {}
        }
        match input {
            Input::Checking => blockers.push(Blocker::CheckingMicrophones),
            Input::Available => {}
            Input::NeedsConfirmation => blockers.push(Blocker::MicrophoneConfirmation),
            Input::Unavailable => blockers.push(Blocker::MicrophoneUnavailable),
            Input::Failed(error) => blockers.push(Blocker::MicrophoneScanFailed(error.clone())),
        }
        if !model {
            blockers.push(Blocker::ModelMissing);
        }
        if !runtime {
            blockers.push(Blocker::RuntimeMissing);
        }
        Self {
            activity,
            shortcut: shortcut.to_string(),
            blockers,
        }
    }

    pub fn summary(&self) -> String {
        if let Some(blocker) = self.blockers.first() {
            blocker.label(&self.shortcut)
        } else if self.activity == Activity::Idle {
            format!("Ready — {}", self.shortcut)
        } else {
            self.activity.label().into()
        }
    }

    pub fn color(&self) -> [u8; 3] {
        if self.activity == Activity::Recording {
            [230, 55, 55]
        } else if !self.blockers.is_empty() || self.activity != Activity::Idle {
            [230, 170, 30]
        } else {
            [70, 180, 100]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessibility_grant_registration_and_revocation_change_readiness() {
        let observe = |accessibility, registered| {
            Readiness::observe(
                Activity::Idle,
                &Shortcut::RightOption,
                Snapshot {
                    accessibility,
                    microphone: Microphone::Authorized,
                },
                registered,
                &Input::Available,
                true,
                true,
            )
        };
        let blocked = observe(false, false);
        assert_eq!(
            blocked.summary(),
            "Right Option blocked: enable Accessibility"
        );
        assert_eq!(blocked.blockers[0].action(), Some("permissions"));
        assert_eq!(
            observe(true, false).summary(),
            "Right Option unavailable: retrying registration"
        );
        assert_eq!(observe(true, true).summary(), "Ready — Right Option");
        assert_eq!(
            observe(false, true).summary(),
            "Right Option blocked: enable Accessibility"
        );
    }

    #[test]
    fn menu_recording_does_not_hide_the_shortcut_blocker() {
        let readiness = Readiness::observe(
            Activity::Recording,
            &Shortcut::RightOption,
            Snapshot {
                accessibility: false,
                microphone: Microphone::Authorized,
            },
            false,
            &Input::Available,
            true,
            true,
        );
        assert_eq!(
            readiness.summary(),
            "Right Option blocked: enable Accessibility"
        );
        assert_eq!(readiness.activity.label(), "Recording");
        assert_eq!(readiness.color(), [230, 55, 55]);
    }

    #[test]
    fn combinations_do_not_require_accessibility_and_all_other_issues_remain_visible() {
        let readiness = Readiness::observe(
            Activity::Idle,
            &"Control+Shift+Space".parse().unwrap(),
            Snapshot {
                accessibility: false,
                microphone: Microphone::Denied,
            },
            true,
            &Input::Unavailable,
            false,
            false,
        );
        assert_eq!(
            readiness.blockers,
            vec![
                Blocker::MicrophonePermission,
                Blocker::MicrophoneUnavailable,
                Blocker::ModelMissing,
                Blocker::RuntimeMissing
            ]
        );
        assert_eq!(readiness.summary(), "Recording blocked: enable Microphone");
    }
}
