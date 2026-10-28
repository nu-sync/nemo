use crate::shortcut::Shortcut;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Microphone {
    NotDetermined,
    Authorized,
    Denied,
    Restricted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub accessibility: bool,
    pub microphone: Microphone,
}

impl Snapshot {
    pub fn current() -> Self {
        let status = unsafe {
            AVCaptureDevice::authorizationStatusForMediaType(
                AVMediaTypeAudio.expect("audio media type"),
            )
        };
        let microphone = match status {
            AVAuthorizationStatus::Authorized => Microphone::Authorized,
            AVAuthorizationStatus::NotDetermined => Microphone::NotDetermined,
            AVAuthorizationStatus::Denied => Microphone::Denied,
            _ => Microphone::Restricted,
        };
        Self {
            accessibility: crate::output::trusted(),
            microphone,
        }
    }

    pub fn shortcut_allowed(self, shortcut: &Shortcut) -> bool {
        !matches!(shortcut, Shortcut::RightOption) || self.accessibility
    }

    pub fn recording_allowed(self) -> bool {
        matches!(
            self.microphone,
            Microphone::Authorized | Microphone::NotDetermined
        )
    }

    pub fn issue(self, shortcut: &Shortcut) -> Option<&'static str> {
        if !self.shortcut_allowed(shortcut) {
            Some("Enable Accessibility")
        } else if !self.recording_allowed() {
            Some("Enable Microphone")
        } else if self.microphone == Microphone::NotDetermined {
            Some("Microphone permission required")
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_run_can_request_microphone_without_accessibility() {
        let first = Snapshot {
            accessibility: false,
            microphone: Microphone::NotDetermined,
        };
        assert!(!first.shortcut_allowed(&Shortcut::RightOption));
        assert!(first.recording_allowed());
        assert_eq!(
            first.issue(&Shortcut::RightOption),
            Some("Enable Accessibility")
        );
    }
    #[test]
    fn granting_permissions_enables_the_shortcut_and_clears_the_issue() {
        let granted = Snapshot {
            accessibility: true,
            microphone: Microphone::Authorized,
        };
        assert!(granted.shortcut_allowed(&Shortcut::RightOption));
        assert_eq!(granted.issue(&Shortcut::RightOption), None);
    }
    #[test]
    fn revoked_microphone_access_blocks_capture_but_not_settings_or_shortcuts() {
        for microphone in [Microphone::Denied, Microphone::Restricted] {
            let permissions = Snapshot {
                accessibility: true,
                microphone,
            };
            assert!(!permissions.recording_allowed());
            assert_eq!(
                permissions.issue(&Shortcut::RightOption),
                Some("Enable Microphone")
            );
        }
    }
    #[test]
    fn combinations_remain_available_for_clipboard_only_use() {
        let permissions = Snapshot {
            accessibility: false,
            microphone: Microphone::Authorized,
        };
        let key = "Control+Shift+Space".parse().unwrap();
        assert!(permissions.shortcut_allowed(&key));
    }
}
