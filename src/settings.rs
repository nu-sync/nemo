use crate::sounds::{Cue, SoundPack};
use anyhow::Result;
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained, sel,
};
use objc2_app_kit::{NSButton, NSPopUpButton, NSTextField, NSView};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
use tao::{
    dpi::LogicalSize,
    event_loop::EventLoopWindowTarget,
    platform::macos::WindowExtMacOS,
    window::{Window, WindowBuilder},
};

#[derive(Clone, Copy)]
pub enum Change {
    RemoveTrailingPeriod(bool),
    LaunchOnStartup(bool),
    SoundPack(SoundPack),
    PreviewSound(Cue),
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Box<dyn Fn(Change)>]
    struct SettingsAction;

    unsafe impl NSObjectProtocol for SettingsAction {}

    impl SettingsAction {
        #[unsafe(method(togglePeriod:))]
        fn toggle_period(&self, sender: &NSButton) {
            (self.ivars())(Change::RemoveTrailingPeriod(sender.state() == 1));
        }
        #[unsafe(method(selectSoundPack:))]
        fn select_sound_pack(&self, sender: &NSPopUpButton) {
            if let Some(pack) = SoundPack::ALL.get(sender.indexOfSelectedItem() as usize) {
                (self.ivars())(Change::SoundPack(*pack));
            }
        }
        #[unsafe(method(previewStart:))]
        fn preview_start(&self, _sender: &NSButton) {
            (self.ivars())(Change::PreviewSound(Cue::Start));
        }
        #[unsafe(method(previewStop:))]
        fn preview_stop(&self, _sender: &NSButton) {
            (self.ivars())(Change::PreviewSound(Cue::Stop));
        }
        #[unsafe(method(toggleStartup:))]
        fn toggle_startup(&self, sender: &NSButton) {
            (self.ivars())(Change::LaunchOnStartup(sender.state() == 1));
        }
    }
);

pub struct Settings {
    pub window: Window,
    checkbox: Retained<NSButton>,
    startup: Retained<NSButton>,
    sound_pack: Retained<NSPopUpButton>,
    error: Retained<NSTextField>,
    _action: Retained<SettingsAction>,
}

impl Settings {
    pub fn new<T>(
        event_loop: &EventLoopWindowTarget<T>,
        config: &crate::config::Config,
        on_change: impl Fn(Change) + 'static,
    ) -> Result<Self> {
        let window = WindowBuilder::new()
            .with_title("Nemo Settings")
            .with_inner_size(LogicalSize::new(360.0, 226.0))
            .with_resizable(false)
            .with_maximizable(false)
            .with_minimizable(false)
            .with_visible(false)
            .with_focused(false)
            .build(event_loop)?;
        let mtm = MainThreadMarker::new().expect("Settings are created on the main thread");
        let action =
            SettingsAction::alloc(mtm).set_ivars(Box::new(on_change) as Box<dyn Fn(Change)>);
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
            NSPoint::new(24.0, 174.0),
            NSSize::new(312.0, 28.0),
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
            NSPoint::new(24.0, 138.0),
            NSSize::new(312.0, 28.0),
        ));
        startup.setEnabled(crate::startup::available());
        if !crate::startup::available() {
            startup.setToolTip(Some(&NSString::from_str(
                "Open Nemo.app to change launch on startup.",
            )));
        }
        let sound_label = NSTextField::labelWithString(&NSString::from_str("Sound pack"), mtm);
        sound_label.setFrame(NSRect::new(
            NSPoint::new(24.0, 101.0),
            NSSize::new(96.0, 22.0),
        ));
        let sound_pack = NSPopUpButton::new(mtm);
        sound_pack.setFrame(NSRect::new(
            NSPoint::new(122.0, 96.0),
            NSSize::new(216.0, 30.0),
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
            NSPoint::new(122.0, 56.0),
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
            NSPoint::new(232.0, 56.0),
            NSSize::new(106.0, 30.0),
        ));
        let error = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        error.setFrame(NSRect::new(
            NSPoint::new(24.0, 20.0),
            NSSize::new(312.0, 22.0),
        ));
        // Tao owns this NSView for the lifetime of the window.
        let view = unsafe { &*window.ns_view().cast::<NSView>() };
        view.addSubview(&checkbox);
        view.addSubview(&startup);
        view.addSubview(&sound_label);
        view.addSubview(&sound_pack);
        view.addSubview(&preview_start);
        view.addSubview(&preview_stop);
        view.addSubview(&error);
        let settings = Self {
            window,
            checkbox,
            startup,
            sound_pack,
            error,
            _action: action,
        };
        settings.set_sound_pack(config.sound_pack);
        settings.refresh_startup();
        Ok(settings)
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
