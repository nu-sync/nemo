use anyhow::{Context, Result, bail, ensure};
use block2::RcBlock;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use objc2::{rc::Retained, runtime::AnyObject};
use objc2_app_kit::{NSEvent, NSEventMask, NSEventType};
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, fmt, ptr::NonNull, rc::Rc, str::FromStr, sync::Arc};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Shortcut {
    RightOption,
    Combination(HotKey),
}

impl FromStr for Shortcut {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        if value.eq_ignore_ascii_case("RightOption") || value.eq_ignore_ascii_case("RightAlt") {
            Ok(Self::RightOption)
        } else {
            Ok(Self::Combination(value.parse()?))
        }
    }
}

impl TryFrom<String> for Shortcut {
    type Error = anyhow::Error;
    fn try_from(value: String) -> Result<Self> {
        value.parse()
    }
}

impl From<Shortcut> for String {
    fn from(value: Shortcut) -> Self {
        match value {
            Shortcut::RightOption => "RightOption".into(),
            Shortcut::Combination(key) => key.into_string(),
        }
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RightOption => formatter.write_str("Right Option"),
            Self::Combination(key) => write!(formatter, "{key}"),
        }
    }
}

pub enum Binding {
    Combination {
        _manager: GlobalHotKeyManager,
    },
    RightOption {
        global: Retained<AnyObject>,
        local: Retained<AnyObject>,
    },
}

impl Binding {
    pub fn register(
        shortcut: &Shortcut,
        toggle: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self> {
        let toggle = Arc::new(toggle);
        match shortcut {
            Shortcut::Combination(key) => {
                let key = *key;
                let manager = GlobalHotKeyManager::new()?;
                manager
                    .register(key)
                    .with_context(|| format!("Cannot register {key}; choose another shortcut"))?;
                GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
                    if event.id == key.id() && event.state == HotKeyState::Pressed {
                        toggle();
                    }
                }));
                Ok(Self::Combination { _manager: manager })
            }
            Shortcut::RightOption => {
                ensure!(
                    crate::output::trusted(),
                    "Right Option needs Nemo's Accessibility permission. Enable Nemo in System Settings → Privacy & Security → Accessibility, then reopen Nemo"
                );
                let state = Rc::new(RefCell::new(RightOptionTap::default()));
                let mask = NSEventMask::FlagsChanged
                    | NSEventMask::KeyDown
                    | NSEventMask::LeftMouseDown
                    | NSEventMask::RightMouseDown
                    | NSEventMask::OtherMouseDown;
                let global_state = state.clone();
                let global_toggle = toggle.clone();
                let global_handler = RcBlock::new(move |event: NonNull<NSEvent>| {
                    if observe(&mut global_state.borrow_mut(), unsafe { event.as_ref() }) {
                        global_toggle();
                    }
                });
                let global =
                    NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global_handler)
                        .context(
                        "Cannot observe Right Option; check Nemo's Accessibility permission",
                    )?;
                let local_handler = RcBlock::new(move |event: NonNull<NSEvent>| {
                    if observe(&mut state.borrow_mut(), unsafe { event.as_ref() }) {
                        toggle();
                    }
                    event.as_ptr()
                });
                let local = unsafe {
                    NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local_handler)
                };
                let Some(local) = local else {
                    unsafe {
                        NSEvent::removeMonitor(&global);
                    }
                    bail!("Cannot observe Right Option inside Nemo");
                };
                Ok(Self::RightOption { global, local })
            }
        }
    }
}

impl Drop for Binding {
    fn drop(&mut self) {
        if let Self::RightOption { global, local } = self {
            unsafe {
                NSEvent::removeMonitor(global);
                NSEvent::removeMonitor(local);
            }
        }
    }
}

const RIGHT_OPTION_KEYCODE: u16 = 61;
// Apple IOLLEvent.h: NX_DEVICERALTKEYMASK, NX_DEVICELALTKEYMASK.
const RIGHT_OPTION_FLAG: u64 = 0x40;
const LEFT_OPTION_FLAG: u64 = 0x20;
const OTHER_MODIFIERS: u64 = LEFT_OPTION_FLAG | (1 << 17) | (1 << 18) | (1 << 20) | (1 << 23);

#[derive(Default)]
struct RightOptionTap {
    down: bool,
    solo: bool,
}

impl RightOptionTap {
    fn flags_changed(&mut self, keycode: u16, flags: u64) -> bool {
        if keycode != RIGHT_OPTION_KEYCODE {
            self.solo = false;
            return false;
        }
        let other = flags & OTHER_MODIFIERS != 0;
        if flags & RIGHT_OPTION_FLAG != 0 {
            if !self.down {
                self.solo = !other;
            }
            self.down = true;
            if other {
                self.solo = false;
            }
            false
        } else {
            let toggle = self.down && self.solo && !other;
            self.down = false;
            self.solo = false;
            toggle
        }
    }

    fn chord_used(&mut self) {
        self.solo = false;
    }
}

fn observe(state: &mut RightOptionTap, event: &NSEvent) -> bool {
    if event.r#type() == NSEventType::FlagsChanged {
        state.flags_changed(event.keyCode(), event.modifierFlags().bits() as u64)
    } else {
        state.chord_used();
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const RIGHT_DOWN: u64 = RIGHT_OPTION_FLAG | (1 << 19);

    #[test]
    fn right_option_toggles_once_on_release() {
        let mut tap = RightOptionTap::default();
        assert!(!tap.flags_changed(61, RIGHT_DOWN));
        assert!(!tap.flags_changed(61, RIGHT_DOWN));
        assert!(tap.flags_changed(61, 0));
        assert!(!tap.flags_changed(61, 0));
        assert!(!tap.flags_changed(61, RIGHT_DOWN));
        assert!(tap.flags_changed(61, 0));
    }

    #[test]
    fn left_option_never_toggles() {
        let mut tap = RightOptionTap::default();
        assert!(!tap.flags_changed(58, LEFT_OPTION_FLAG | (1 << 19)));
        assert!(!tap.flags_changed(58, 0));
    }

    #[test]
    fn typing_or_clicking_with_option_does_not_toggle() {
        let mut tap = RightOptionTap::default();
        tap.flags_changed(61, RIGHT_DOWN);
        tap.chord_used();
        assert!(!tap.flags_changed(61, 0));
        tap.flags_changed(61, RIGHT_DOWN);
        assert!(tap.flags_changed(61, 0));
    }

    #[test]
    fn another_modifier_cancels_the_tap_even_when_released_first() {
        let mut tap = RightOptionTap::default();
        tap.flags_changed(61, RIGHT_DOWN);
        tap.flags_changed(58, RIGHT_DOWN | LEFT_OPTION_FLAG);
        tap.flags_changed(58, RIGHT_DOWN);
        assert!(!tap.flags_changed(61, 0));
        tap.flags_changed(61, RIGHT_DOWN | (1 << 20));
        assert!(!tap.flags_changed(61, 0));
    }

    #[test]
    fn caps_lock_does_not_prevent_dictation() {
        let mut tap = RightOptionTap::default();
        tap.flags_changed(61, RIGHT_DOWN | (1 << 16));
        assert!(tap.flags_changed(61, 1 << 16));
    }

    #[test]
    fn configuration_accepts_right_option_and_existing_combinations() {
        let right: Shortcut = "RightOption".parse().unwrap();
        assert!(matches!(right, Shortcut::RightOption));
        assert_eq!(String::from(right), "RightOption");
        assert!(matches!(
            "Control+Alt+Space".parse::<Shortcut>().unwrap(),
            Shortcut::Combination(_)
        ));
        assert!("not-a-key".parse::<Shortcut>().is_err());
    }
}
