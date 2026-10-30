use crate::audio::{Inputs, Microphone};
use crate::sounds::{Cue, SoundPack};
use anyhow::Result;
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained, sel,
};
use objc2_app_kit::{NSButton, NSPopUpButton, NSTextField, NSView};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
use std::cell::RefCell;
use tao::{
    dpi::LogicalSize,
    event_loop::EventLoopWindowTarget,
    platform::macos::WindowExtMacOS,
    window::{Window, WindowBuilder},
};

#[derive(Clone)]
pub enum Change {
    Microphone(Option<Microphone>),
    RemoveTrailingPeriod(bool),
    LaunchOnStartup(bool),
    SoundPack(SoundPack),
    PreviewSound(Cue),
}

struct Actions {
    on_change: Box<dyn Fn(Change)>,
    microphones: RefCell<Vec<Option<Microphone>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Actions]
    struct SettingsAction;

    unsafe impl NSObjectProtocol for SettingsAction {}

    impl SettingsAction {
        #[unsafe(method(selectMicrophone:))]
        fn select_microphone(&self, sender: &NSPopUpButton) {
            if let Some(microphone) = self.ivars().microphones.borrow().get(sender.indexOfSelectedItem() as usize) {
                (self.ivars().on_change)(Change::Microphone(microphone.clone()));
            }
        }

        #[unsafe(method(togglePeriod:))]
        fn toggle_period(&self, sender: &NSButton) {
            (self.ivars().on_change)(Change::RemoveTrailingPeriod(sender.state() == 1));
        }
        #[unsafe(method(selectSoundPack:))]
        fn select_sound_pack(&self, sender: &NSPopUpButton) {
            if let Some(pack) = SoundPack::ALL.get(sender.indexOfSelectedItem() as usize) {
                (self.ivars().on_change)(Change::SoundPack(*pack));
            }
        }
        #[unsafe(method(previewStart:))]
        fn preview_start(&self, _sender: &NSButton) {
            (self.ivars().on_change)(Change::PreviewSound(Cue::Start));
        }
        #[unsafe(method(previewStop:))]
        fn preview_stop(&self, _sender: &NSButton) {
            (self.ivars().on_change)(Change::PreviewSound(Cue::Stop));
        }
        #[unsafe(method(toggleStartup:))]
        fn toggle_startup(&self, sender: &NSButton) {
            (self.ivars().on_change)(Change::LaunchOnStartup(sender.state() == 1));
        }
    }
);

pub struct Settings {
    pub window: Window,
    checkbox: Retained<NSButton>,
    startup: Retained<NSButton>,
    sound_pack: Retained<NSPopUpButton>,
    microphone: Retained<NSPopUpButton>,
    input_status: Retained<NSTextField>,
    error: Retained<NSTextField>,
    action: Retained<SettingsAction>,
}

impl Settings {
    pub fn new<T>(
        event_loop: &EventLoopWindowTarget<T>,
        config: &crate::config::Config,
        on_change: impl Fn(Change) + 'static,
    ) -> Result<Self> {
        let window = WindowBuilder::new()
            .with_title("Nemo Settings")
            .with_inner_size(LogicalSize::new(460.0, 330.0))
            .with_resizable(false)
            .with_maximizable(false)
            .with_minimizable(false)
            .with_visible(false)
            .with_focused(false)
            .build(event_loop)?;
        let mtm = MainThreadMarker::new().expect("Settings are created on the main thread");
        let action = SettingsAction::alloc(mtm).set_ivars(Actions {
            on_change: Box::new(on_change),
            microphones: RefCell::new(vec![None]),
        });
        let action: Retained<SettingsAction> = unsafe { msg_send![super(action), init] };
        // The target is retained for the window's lifetime and implements this selector.
        let checkbox = unsafe {
            NSButton::checkboxWithTitle_target_action(
                &NSString::from_str("Remove trailing period"),
                Some(&action),
                Some(sel!(togglePeriod:)),
                mtm,
            )
        };
        checkbox.setState(isize::from(config.remove_trailing_period));
        checkbox.setFrame(NSRect::new(
            NSPoint::new(24.0, 278.0),
            NSSize::new(412.0, 28.0),
        ));
        let startup = unsafe {
            NSButton::checkboxWithTitle_target_action(
                &NSString::from_str("Launch on startup"),
                Some(&action),
                Some(sel!(toggleStartup:)),
                mtm,
            )
        };
        startup.setFrame(NSRect::new(
            NSPoint::new(24.0, 242.0),
            NSSize::new(412.0, 28.0),
        ));
        startup.setEnabled(crate::startup::available());
        if !crate::startup::available() {
            startup.setToolTip(Some(&NSString::from_str(
                "Open Nemo.app to change launch on startup.",
            )));
        }
        let sound_label = NSTextField::labelWithString(&NSString::from_str("Sound pack"), mtm);
        sound_label.setFrame(NSRect::new(
            NSPoint::new(24.0, 205.0),
            NSSize::new(96.0, 22.0),
        ));
        let sound_pack = NSPopUpButton::new(mtm);
        sound_pack.setFrame(NSRect::new(
            NSPoint::new(122.0, 200.0),
            NSSize::new(316.0, 30.0),
        ));
        for pack in SoundPack::ALL {
            sound_pack.addItemWithTitle(&NSString::from_str(pack.label()));
        }
        unsafe {
            sound_pack.setTarget(Some(&action));
            sound_pack.setAction(Some(sel!(selectSoundPack:)));
        }
        let preview_start = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Play start"),
                Some(&action),
                Some(sel!(previewStart:)),
                mtm,
            )
        };
        preview_start.setFrame(NSRect::new(
            NSPoint::new(122.0, 160.0),
            NSSize::new(106.0, 30.0),
        ));
        let preview_stop = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Play stop"),
                Some(&action),
                Some(sel!(previewStop:)),
                mtm,
            )
        };
        preview_stop.setFrame(NSRect::new(
            NSPoint::new(232.0, 160.0),
            NSSize::new(106.0, 30.0),
        ));
        let microphone_label = NSTextField::labelWithString(&NSString::from_str("Microphone"), mtm);
        microphone_label.setFrame(NSRect::new(
            NSPoint::new(24.0, 118.0),
            NSSize::new(96.0, 22.0),
        ));
        let microphone = NSPopUpButton::new(mtm);
        microphone.setFrame(NSRect::new(
            NSPoint::new(122.0, 113.0),
            NSSize::new(316.0, 30.0),
        ));
        unsafe {
            microphone.setTarget(Some(&action));
            microphone.setAction(Some(sel!(selectMicrophone:)));
        }
        let input_status =
            NSTextField::wrappingLabelWithString(&NSString::from_str("Checking microphones…"), mtm);
        input_status.setFrame(NSRect::new(
            NSPoint::new(24.0, 54.0),
            NSSize::new(412.0, 48.0),
        ));
        let error = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        error.setFrame(NSRect::new(
            NSPoint::new(24.0, 20.0),
            NSSize::new(412.0, 22.0),
        ));
        // Tao owns this NSView for the lifetime of the window.
        let view = unsafe { &*window.ns_view().cast::<NSView>() };
        view.addSubview(&checkbox);
        view.addSubview(&startup);
        view.addSubview(&sound_label);
        view.addSubview(&sound_pack);
        view.addSubview(&preview_start);
        view.addSubview(&preview_stop);
        view.addSubview(&microphone_label);
        view.addSubview(&microphone);
        view.addSubview(&input_status);
        view.addSubview(&error);
        let settings = Self {
            window,
            checkbox,
            startup,
            sound_pack,
            microphone,
            input_status,
            error,
            action,
        };
        settings.set_sound_pack(config.sound_pack);
        settings.refresh_startup();
        Ok(settings)
    }

    pub fn set_inputs(
        &self,
        inputs: &Inputs,
        selected: Option<&Microphone>,
        active: Option<&Microphone>,
        busy: bool,
    ) {
        let mut choices = vec![None];
        choices.extend(inputs.devices.iter().cloned().map(Some));
        if let Some(selected) = selected
            && !inputs.devices.iter().any(|d| d.id == selected.id)
        {
            choices.push(Some(selected.clone()));
        }
        if self.microphone.numberOfItems() == 0
            || *self.action.ivars().microphones.borrow() != choices
        {
            self.microphone.removeAllItems();
            let mut titles = std::collections::HashSet::new();
            for choice in &choices {
                let title = match choice {
                    None => "System default".to_owned(),
                    Some(device) if !inputs.devices.iter().any(|d| d.id == device.id) => {
                        format!("{} (unavailable)", device.name)
                    }
                    Some(device) => device.name.clone(),
                };
                let mut unique = title.clone();
                let mut suffix = 2;
                while !titles.insert(unique.clone()) {
                    unique = format!("{title} ({suffix})");
                    suffix += 1;
                }
                self.microphone
                    .addItemWithTitle(&NSString::from_str(&unique));
            }
            *self.action.ivars().microphones.borrow_mut() = choices.clone();
        }
        let index = choices
            .iter()
            .position(|d| d.as_ref().map(|d| &d.id) == selected.map(|d| &d.id))
            .unwrap_or(0);
        self.microphone.selectItemAtIndex(index as isize);
        self.microphone.setEnabled(!busy);
        let message = if let Some(active) = active {
            format!("Recording: {}", active.name)
        } else {
            match inputs.resolve(selected) {
                Ok((device, false)) => format!("Input: {}", device.name),
                Ok((device, true)) => format!(
                    "{} unavailable. System default: {}. Nemo will ask before recording.",
                    selected.unwrap().name,
                    device.name
                ),
                Err(error) => error.to_string(),
            }
        };
        self.input_status
            .setStringValue(&NSString::from_str(&message));
    }

    pub fn confirm_fallback(&self, selected: &str, fallback: &Microphone) -> bool {
        use objc2_app_kit::NSAlert;
        let alert = NSAlert::new(MainThreadMarker::new().unwrap());
        alert.setMessageText(&NSString::from_str(&format!("{} is unavailable", selected)));
        alert.setInformativeText(&NSString::from_str(&format!(
            "Record with {} for this recording? Your saved microphone will stay selected.",
            fallback.name
        )));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        alert.addButtonWithTitle(&NSString::from_str("Use system default"));
        alert.runModal() == 1001
    }

    pub fn show(&self) {
        self.set_error("");
        self.refresh_startup();
        self.window.set_visible(true);
        self.window.set_focus();
    }

    pub fn set_remove_trailing_period(&self, enabled: bool) {
        self.checkbox.setState(isize::from(enabled));
    }

    pub fn set_sound_pack(&self, pack: SoundPack) {
        let index = SoundPack::ALL
            .iter()
            .position(|item| *item == pack)
            .unwrap();
        self.sound_pack.selectItemAtIndex(index as isize);
    }

    pub fn refresh_startup(&self) {
        use objc2_service_management::SMAppServiceStatus;
        let status = crate::startup::status();
        self.startup.setState(match status {
            SMAppServiceStatus::Enabled => 1,
            SMAppServiceStatus::RequiresApproval => -1,
            _ => 0,
        });
        let message = self.error.stringValue().to_string();
        if message.is_empty() || message == "Approve Nemo in Login Items." {
            self.set_error(if status == SMAppServiceStatus::RequiresApproval {
                "Approve Nemo in Login Items."
            } else {
                ""
            });
        }
    }

    pub fn set_error(&self, message: &str) {
        self.error.setStringValue(&NSString::from_str(message));
    }
}
