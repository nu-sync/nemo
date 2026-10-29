use anyhow::{Context, Result};
use objc2::{AllocAnyThread, rc::Retained};
use objc2_app_kit::NSSound;
use objc2_foundation::{NSData, NSString};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoundPack {
    #[default]
    HandyMarimba,
    MacosRecording,
}

impl SoundPack {
    pub const ALL: [Self; 2] = [Self::HandyMarimba, Self::MacosRecording];

    pub fn label(self) -> &'static str {
        match self {
            Self::HandyMarimba => "Handy Marimba",
            Self::MacosRecording => "macOS Recording",
        }
    }
}

#[derive(Clone, Copy)]
pub enum Cue {
    Start,
    Stop,
}

struct Pair {
    start: Retained<NSSound>,
    stop: Retained<NSSound>,
}

pub struct Player {
    marimba: Pair,
    macos: Option<Pair>,
}

impl Player {
    pub fn new() -> Result<Self> {
        let load = |bytes: &[u8]| {
            NSSound::initWithData(NSSound::alloc(), &NSData::with_bytes(bytes))
                .context("Cannot decode bundled Handy Marimba sound")
        };
        let marimba = Pair {
            start: load(include_bytes!("../assets/sounds/handy-marimba-start.wav"))?,
            stop: load(include_bytes!("../assets/sounds/handy-marimba-stop.wav"))?,
        };
        let load_system = |name| {
            NSSound::initWithContentsOfFile_byReference(
                NSSound::alloc(),
                &NSString::from_str(&format!(
                    "/System/Library/Components/CoreAudio.component/Contents/SharedSupport/SystemSounds/system/{name}.caf"
                )),
                false,
            )
        };
        let macos = load_system("begin_record")
            .zip(load_system("end_record"))
            .map(|(start, stop)| Pair { start, stop });
        if macos.is_none() {
            eprintln!("macOS Recording sounds unavailable; using Handy Marimba as fallback");
        }
        Ok(Self { marimba, macos })
    }

    pub fn play(&self, pack: SoundPack, cue: Cue) {
        for pair in std::iter::once(&self.marimba).chain(self.macos.iter()) {
            pair.start.stop();
            pair.stop.stop();
        }
        let pair = match pack {
            SoundPack::HandyMarimba => &self.marimba,
            SoundPack::MacosRecording => self.macos.as_ref().unwrap_or(&self.marimba),
        };
        let sound = match cue {
            Cue::Start => &pair.start,
            Cue::Stop => &pair.stop,
        };
        if !sound.play() {
            eprintln!("Could not play {} sound", pack.label());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_marimba_decodes_with_the_expected_recording_cues() {
        let player = Player::new().unwrap();
        assert!((player.marimba.start.duration() - 0.580).abs() < 0.005);
        assert!((player.marimba.stop.duration() - 0.788).abs() < 0.005);
    }
}
