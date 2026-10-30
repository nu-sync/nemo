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
    Minimal,
    Chime,
}

impl SoundPack {
    pub const ALL: [Self; 4] = [
        Self::HandyMarimba,
        Self::MacosRecording,
        Self::Minimal,
        Self::Chime,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::HandyMarimba => "Handy Marimba",
            Self::MacosRecording => "macOS Recording",
            Self::Minimal => "Minimal",
            Self::Chime => "Chime",
        }
    }
}

#[derive(Clone, Copy)]
pub enum Cue {
    Start,
    Stop,
    Complete,
}

struct Cues {
    start: Retained<NSSound>,
    stop: Retained<NSSound>,
    complete: Retained<NSSound>,
}

impl Cues {
    fn bundled(start: &[u8], stop: &[u8], complete: &[u8]) -> Result<Self> {
        let load = |bytes: &[u8]| {
            NSSound::initWithData(NSSound::alloc(), &NSData::with_bytes(bytes))
                .context("Cannot decode bundled sound")
        };
        Ok(Self {
            start: load(start)?,
            stop: load(stop)?,
            complete: load(complete)?,
        })
    }

    fn sound(&self, cue: Cue) -> &NSSound {
        match cue {
            Cue::Start => &self.start,
            Cue::Stop => &self.stop,
            Cue::Complete => &self.complete,
        }
    }

    fn stop(&self) {
        self.start.stop();
        self.stop.stop();
        self.complete.stop();
    }
}

pub struct Player {
    marimba: Cues,
    macos: Option<Cues>,
    minimal: Cues,
    chime: Cues,
}

impl Player {
    pub fn new() -> Result<Self> {
        let marimba = Cues::bundled(
            include_bytes!("../assets/sounds/handy-marimba-start.wav"),
            include_bytes!("../assets/sounds/handy-marimba-stop.wav"),
            include_bytes!("../assets/sounds/handy-marimba-complete.wav"),
        )
        .context("Cannot load Handy Marimba")?;
        let minimal = Cues::bundled(
            include_bytes!("../assets/sounds/minimal-start.wav"),
            include_bytes!("../assets/sounds/minimal-stop.wav"),
            include_bytes!("../assets/sounds/minimal-complete.wav"),
        )
        .context("Cannot load Minimal")?;
        let chime = Cues::bundled(
            include_bytes!("../assets/sounds/chime-start.wav"),
            include_bytes!("../assets/sounds/chime-stop.wav"),
            include_bytes!("../assets/sounds/chime-complete.wav"),
        )
        .context("Cannot load Chime")?;
        let load_system = |path: &str| {
            NSSound::initWithContentsOfFile_byReference(
                NSSound::alloc(),
                &NSString::from_str(path),
                false,
            )
        };
        let system = "/System/Library/Components/CoreAudio.component/Contents/SharedSupport/SystemSounds/system";
        let macos = load_system(&format!("{system}/begin_record.caf"))
            .zip(load_system(&format!("{system}/end_record.caf")))
            .zip(load_system("/System/Library/Sounds/Glass.aiff"))
            .map(|((start, stop), complete)| Cues {
                start,
                stop,
                complete,
            });
        if macos.is_none() {
            eprintln!("macOS Recording sounds unavailable; using Handy Marimba as fallback");
        }
        Ok(Self {
            marimba,
            macos,
            minimal,
            chime,
        })
    }

    pub fn play(&self, pack: SoundPack, cue: Cue) {
        for cues in [&self.marimba, &self.minimal, &self.chime]
            .into_iter()
            .chain(self.macos.iter())
        {
            cues.stop();
        }
        if !self.cues(pack).sound(cue).play() {
            eprintln!("Could not play {} sound", pack.label());
        }
    }

    fn cues(&self, pack: SoundPack) -> &Cues {
        match pack {
            SoundPack::HandyMarimba => &self.marimba,
            SoundPack::MacosRecording => self.macos.as_ref().unwrap_or(&self.marimba),
            SoundPack::Minimal => &self.minimal,
            SoundPack::Chime => &self.chime,
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

    #[test]
    fn bundled_packs_decode_all_three_auditioned_cues() {
        let player = Player::new().unwrap();
        for (pack, durations) in [
            (SoundPack::HandyMarimba, [0.580, 0.788, 0.520]),
            (SoundPack::Minimal, [0.170, 0.170, 0.210]),
            (SoundPack::Chime, [0.430, 0.430, 0.650]),
        ] {
            for (cue, duration) in [Cue::Start, Cue::Stop, Cue::Complete]
                .into_iter()
                .zip(durations)
            {
                assert!((player.cues(pack).sound(cue).duration() - duration).abs() < 0.005);
            }
        }
    }

    #[test]
    fn unavailable_macos_pack_falls_back_for_every_cue() {
        let mut player = Player::new().unwrap();
        player.macos = None;
        for cue in [Cue::Start, Cue::Stop, Cue::Complete] {
            assert!(std::ptr::eq(
                player.cues(SoundPack::MacosRecording).sound(cue),
                player.cues(SoundPack::HandyMarimba).sound(cue),
            ));
        }
    }
}
