use anyhow::{Context, Result, ensure};
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained, sel,
};
use objc2_app_kit::{
    NSAccessibility, NSBackingStoreType, NSBezierPath, NSBitmapImageFileType, NSButton,
    NSButtonType, NSCellImagePosition, NSColor, NSEvent, NSFont, NSFontWeightMedium, NSPanel,
    NSScreen, NSStatusWindowLevel, NSTextField, NSView, NSWindowCollectionBehavior,
    NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{NSDictionary, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
use std::{
    cell::RefCell,
    path::Path,
    rc::Rc,
    time::{Duration, Instant},
};
use tao::{
    event::Event,
    event_loop::{ControlFlow, EventLoopBuilder},
    platform::{
        macos::{ActivationPolicy, EventLoopExtMacOS},
        run_return::EventLoopExtRunReturn,
    },
};

const SIZE: NSSize = NSSize::new(240.0, 44.0);
const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const WAVE_BARS: usize = 32;

struct Frame {
    phase: Phase,
    levels: [f32; WAVE_BARS],
    rotation: f64,
}

impl Frame {
    fn new(phase: Phase) -> Self {
        Self {
            phase,
            levels: [0.0; WAVE_BARS],
            rotation: 0.0,
        }
    }

    fn advance(&mut self, level: f32, elapsed: Duration) {
        match self.phase {
            Phase::Recording => {
                let target = (level * 12.0).clamp(0.0, 1.0).sqrt();
                let previous = self.levels[WAVE_BARS - 1];
                let smoothed = target.max(previous * 0.72);
                self.levels.rotate_left(1);
                self.levels[WAVE_BARS - 1] = smoothed;
            }
            Phase::Transcribing => self.rotation = (elapsed.as_secs_f64() / 0.8 * 360.0) % 360.0,
            Phase::Hidden => {}
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Hidden,
    Recording,
    Transcribing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Stop,
    Cancel,
}

impl Action {
    pub fn command(self, phase: Phase) -> Option<&'static str> {
        match (self, phase) {
            (Self::Stop, Phase::Recording) => Some("stop"),
            (Self::Cancel, Phase::Recording | Phase::Transcribing) => Some("cancel"),
            _ => None,
        }
    }
}

struct ButtonAction {
    action: Action,
    on_action: Rc<dyn Fn(Action)>,
}

define_class!(
    #[unsafe(super(NSButton))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ButtonAction]
    struct IndicatorButton;

    unsafe impl NSObjectProtocol for IndicatorButton {}

    impl IndicatorButton {
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool { true }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool { false }

        #[unsafe(method(activate:))]
        fn activate(&self, _sender: &NSButton) {
            self.setEnabled(false);
            (self.ivars().on_action)(self.ivars().action);
        }
    }
);

impl IndicatorButton {
    fn new(
        action: Action,
        on_action: Rc<dyn Fn(Action)>,
        x: f64,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let button = Self::alloc(mtm).set_ivars(ButtonAction { action, on_action });
        let button: Retained<Self> = unsafe {
            msg_send![super(button), initWithFrame: NSRect::new(NSPoint::new(x, 6.0), NSSize::new(32.0, 32.0))]
        };
        let (glyph, label) = match action {
            Action::Stop => ("■", "Stop and transcribe"),
            Action::Cancel => ("×", "Cancel"),
        };
        button.setButtonType(NSButtonType::MomentaryPushIn);
        button.setImagePosition(NSCellImagePosition::NoImage);
        button.setTitle(&NSString::from_str(glyph));
        button.setFont(Some(&NSFont::systemFontOfSize(18.0)));
        button.setBordered(false);
        button.setContentTintColor(Some(&NSColor::whiteColor()));
        button.setAccessibilityLabel(Some(&NSString::from_str(label)));
        button.setToolTip(Some(&NSString::from_str(label)));
        unsafe {
            button.setTarget(Some(&button));
            button.setAction(Some(sel!(activate:)));
        }
        button
    }
}

define_class!(
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    struct IndicatorPanel;

    unsafe impl NSObjectProtocol for IndicatorPanel {}

    impl IndicatorPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool { false }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool { false }
    }
);

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = RefCell<Frame>]
    struct IndicatorView;

    unsafe impl NSObjectProtocol for IndicatorView {}

    impl IndicatorView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let bounds = self.bounds();
            let pill = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                NSRect::new(NSPoint::new(0.5, 0.5), NSSize::new(bounds.size.width - 1.0, bounds.size.height - 1.0)),
                21.5, 21.5,
            );
            NSColor::colorWithSRGBRed_green_blue_alpha(0.09, 0.10, 0.12, 0.96).setFill();
            pill.fill();
            NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.16).setStroke();
            pill.setLineWidth(1.0);
            pill.stroke();
            let frame = self.ivars().borrow();
            match frame.phase {
                Phase::Recording => {
                    NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 0.29, 0.32, 1.0).setFill();
                    NSBezierPath::bezierPathWithOvalInRect(NSRect::new(NSPoint::new(19.0, 18.0), NSSize::new(8.0, 8.0))).fill();
                    for (i, level) in frame.levels.iter().enumerate() {
                        let height = 3.0 + f64::from(*level) * 21.0;
                        NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.35 + i as f64 / (WAVE_BARS - 1) as f64 * 0.65).setFill();
                        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                            NSRect::new(NSPoint::new(38.0 + i as f64 * 3.5, 22.0 - height / 2.0), NSSize::new(2.0, height)),
                            1.0, 1.0,
                        ).fill();
                    }
                }
                Phase::Transcribing => {
                    NSColor::colorWithSRGBRed_green_blue_alpha(0.70, 0.78, 1.0, 1.0).setStroke();
                    let arc = NSBezierPath::bezierPath();
                    arc.setLineWidth(2.0);
                    arc.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle(NSPoint::new(23.0, 22.0), 5.0, 40.0 - frame.rotation, 320.0 - frame.rotation);
                    arc.stroke();
                }
                Phase::Hidden => {}
            }
        }
    }
);

pub struct Indicator {
    panel: Retained<IndicatorPanel>,
    view: Retained<IndicatorView>,
    label: Retained<NSTextField>,
    stop: Retained<IndicatorButton>,
    cancel: Retained<IndicatorButton>,
    phase: Phase,
    started: Instant,
    next_frame: Instant,
}

impl Indicator {
    pub fn new(on_action: impl Fn(Action) + 'static) -> Self {
        let mtm = MainThreadMarker::new().expect("Indicator is created on the main thread");
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), SIZE);
        let panel = IndicatorPanel::alloc(mtm).set_ivars(());
        let panel: Retained<IndicatorPanel> = unsafe {
            msg_send![super(panel), initWithContentRect: bounds,
                styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                backing: NSBackingStoreType::Buffered, defer: false]
        };
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(true);
        panel.setFloatingPanel(true);
        panel.setHidesOnDeactivate(false);
        panel.setIgnoresMouseEvents(false);
        panel.setBecomesKeyOnlyIfNeeded(true);
        panel.setLevel(NSStatusWindowLevel);
        panel.setCollectionBehavior(fullscreen_collection_behavior());
        let view = IndicatorView::alloc(mtm).set_ivars(RefCell::new(Frame::new(Phase::Hidden)));
        let view: Retained<IndicatorView> =
            unsafe { msg_send![super(view), initWithFrame: bounds] };
        let label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        label.setFrame(NSRect::new(
            NSPoint::new(38.0, 13.0),
            NSSize::new(114.0, 18.0),
        ));
        label.setFont(Some(&NSFont::systemFontOfSize_weight(13.0, unsafe {
            NSFontWeightMedium
        })));
        label.setTextColor(Some(&NSColor::whiteColor()));
        let on_action: Rc<dyn Fn(Action)> = Rc::new(on_action);
        let stop = IndicatorButton::new(Action::Stop, on_action.clone(), 160.0, mtm);
        let cancel = IndicatorButton::new(Action::Cancel, on_action, 196.0, mtm);
        stop.setHidden(true);
        cancel.setHidden(true);
        view.addSubview(&label);
        view.addSubview(&stop);
        view.addSubview(&cancel);
        panel.setContentView(Some(&view));
        Self {
            panel,
            view,
            label,
            stop,
            cancel,
            phase: Phase::Hidden,
            started: Instant::now(),
            next_frame: Instant::now(),
        }
    }

    pub fn set_phase(&mut self, phase: Phase) {
        if self.phase == phase {
            return;
        }
        self.started = Instant::now();
        self.next_frame = self.started;
        *self.view.ivars().borrow_mut() = Frame::new(phase);
        self.label.setHidden(phase != Phase::Transcribing);
        self.stop.setHidden(phase != Phase::Recording);
        self.stop.setEnabled(phase == Phase::Recording);
        self.cancel.setHidden(phase == Phase::Hidden);
        self.cancel.setEnabled(phase != Phase::Hidden);
        if phase == Phase::Hidden {
            self.panel.orderOut(None);
        } else {
            if self.phase == Phase::Hidden {
                let mtm = MainThreadMarker::new().expect("Indicator is updated on the main thread");
                let pointer = NSEvent::mouseLocation();
                let screen = NSScreen::screens(mtm)
                    .iter()
                    .find(|screen| contains(screen.frame(), pointer))
                    .or_else(|| NSScreen::mainScreen(mtm));
                if let Some(screen) = screen {
                    self.panel
                        .setFrameOrigin(bottom_center(screen.visibleFrame()));
                }
            }
            self.label.setStringValue(&NSString::from_str(match phase {
                Phase::Recording => "Recording",
                Phase::Transcribing => "Transcribing",
                Phase::Hidden => unreachable!(),
            }));
            self.view.setNeedsDisplay(true);
            self.panel.orderFrontRegardless();
        }
        self.phase = phase;
    }

    pub fn animate(&mut self, level: impl FnOnce() -> f32) -> Option<Instant> {
        if self.phase == Phase::Hidden {
            return None;
        }
        let now = Instant::now();
        if now >= self.next_frame {
            self.view
                .ivars()
                .borrow_mut()
                .advance(level(), now - self.started);
            self.view.setNeedsDisplay(true);
            self.next_frame = now + FRAME_INTERVAL;
        }
        Some(self.next_frame)
    }

    fn verify(&self, phase: Phase) -> Result<()> {
        ensure!(
            self.panel.isVisible() == (phase != Phase::Hidden),
            "Indicator visibility disagrees with {phase:?}"
        );
        ensure!(
            !self.panel.isKeyWindow() && !self.panel.isMainWindow(),
            "Indicator took keyboard focus"
        );
        ensure!(
            !self.panel.canBecomeKeyWindow() && !self.panel.canBecomeMainWindow(),
            "Indicator can take keyboard focus"
        );
        ensure!(
            !self.panel.ignoresMouseEvents(),
            "Indicator ignores its controls"
        );
        ensure!(
            self.stop.isHidden() == (phase != Phase::Recording),
            "Stop visibility disagrees with phase"
        );
        ensure!(
            self.cancel.isHidden() == (phase == Phase::Hidden),
            "Cancel visibility disagrees with phase"
        );
        ensure!(
            !self.stop.acceptsFirstResponder() && !self.cancel.acceptsFirstResponder(),
            "Indicator controls accept keyboard focus"
        );
        ensure!(
            self.stop.acceptsFirstMouse(None) && self.cancel.acceptsFirstMouse(None),
            "Indicator controls require activation before clicking"
        );
        match phase {
            Phase::Recording => ensure!(
                self.label.isHidden() && self.view.ivars().borrow().phase == Phase::Recording,
                "Recording waveform missing"
            ),
            Phase::Transcribing => ensure!(
                self.label.stringValue().to_string() == "Transcribing",
                "Transcribing label missing"
            ),
            Phase::Hidden => {}
        }
        Ok(())
    }

    fn snapshot(&self, path: &Path) -> Result<()> {
        self.view.display();
        let bounds = self.view.bounds();
        let bitmap = self
            .view
            .bitmapImageRepForCachingDisplayInRect(bounds)
            .context("Cannot create indicator bitmap")?;
        self.view
            .cacheDisplayInRect_toBitmapImageRep(bounds, &bitmap);
        let png = unsafe {
            bitmap.representationUsingType_properties(
                NSBitmapImageFileType::PNG,
                &NSDictionary::new(),
            )
        }
        .context("Cannot encode indicator PNG")?;
        std::fs::write(path, png.to_vec())?;
        Ok(())
    }
}

fn fullscreen_collection_behavior() -> NSWindowCollectionBehavior {
    NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::CanJoinAllApplications
        | NSWindowCollectionBehavior::FullScreenAuxiliary
}

fn contains(frame: NSRect, point: NSPoint) -> bool {
    point.x >= frame.origin.x
        && point.x < frame.origin.x + frame.size.width
        && point.y >= frame.origin.y
        && point.y < frame.origin.y + frame.size.height
}

fn bottom_center(visible: NSRect) -> NSPoint {
    NSPoint::new(
        visible.origin.x + (visible.size.width - SIZE.width) / 2.0,
        visible.origin.y + 16.0,
    )
}

pub fn preview(output: Option<&Path>) -> Result<()> {
    let frontmost = || {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|app| app.processIdentifier())
    };
    let original_app = frontmost().context("No foreground app to verify focus preservation")?;
    if let Some(output) = output {
        std::fs::create_dir_all(output)?;
    }
    let mut event_loop = EventLoopBuilder::<()>::new().build();
    event_loop.set_activation_policy(ActivationPolicy::Accessory);
    event_loop.set_activate_ignoring_other_apps(false);
    let actions = Rc::new(RefCell::new(Vec::new()));
    let observed = actions.clone();
    let mut indicator = Indicator::new(move |action| observed.borrow_mut().push(action));
    indicator.verify(Phase::Hidden)?;
    let mut next = Instant::now();
    let mut frame_step = 0;
    let phases = [Phase::Recording, Phase::Transcribing, Phase::Hidden];
    let mut step = 0;
    let mut result = Ok(());
    event_loop.run_return(|event, _, flow| {
        if matches!(event, Event::LoopDestroyed) {
            return;
        }
        if result.is_err() || step == phases.len() {
            *flow = ControlFlow::Exit;
            return;
        }
        if Instant::now() < next {
            let elapsed = indicator.started.elapsed().as_secs_f32();
            let level = if elapsed < 1.0 {
                (elapsed * 18.0).sin().abs() * 0.06
            } else {
                0.0
            };
            let animation = indicator.animate(|| level);
            if frame_step < 2 && elapsed >= 0.5 + frame_step as f32 * 0.6 {
                if let Some(output) = output {
                    let name = match indicator.phase {
                        Phase::Recording => "recording",
                        Phase::Transcribing => "transcribing",
                        Phase::Hidden => unreachable!(),
                    };
                    if let Err(error) =
                        indicator.snapshot(&output.join(format!("{name}-{}.png", frame_step + 1)))
                    {
                        result = Err(error);
                    }
                }
                frame_step += 1;
            }
            *flow = ControlFlow::WaitUntil(animation.map_or(next, |frame| frame.min(next)));
            return;
        }
        frame_step = 0;
        let phase = phases[step];
        step += 1;
        result = (|| {
            ensure!(
                frontmost() == Some(original_app),
                "Foreground app changed during preview"
            );
            if let Some(output) = output
                && step > 1
            {
                let previous = if phase == Phase::Transcribing {
                    "recording"
                } else {
                    "transcribing"
                };
                ensure!(
                    std::fs::read(output.join(format!("{previous}-1.png")))?
                        != std::fs::read(output.join(format!("{previous}-2.png")))?,
                    "{previous} animation produced identical frames"
                );
            }
            indicator.set_phase(phase);
            indicator.set_phase(phase);
            indicator.verify(phase)?;
            actions.borrow_mut().clear();
            if phase == Phase::Recording {
                unsafe { indicator.stop.performClick(None) };
                unsafe { indicator.stop.performClick(None) };
                unsafe { indicator.cancel.performClick(None) };
                ensure!(
                    *actions.borrow() == [Action::Stop, Action::Cancel],
                    "Recording buttons dispatched incorrect actions"
                );
                indicator.stop.setEnabled(true);
                indicator.cancel.setEnabled(true);
            } else if phase == Phase::Transcribing {
                unsafe { indicator.stop.performClick(None) };
                unsafe { indicator.cancel.performClick(None) };
                ensure!(
                    *actions.borrow() == [Action::Cancel],
                    "Transcribing controls dispatched incorrect actions"
                );
                indicator.cancel.setEnabled(true);
            } else {
                unsafe { indicator.stop.performClick(None) };
                unsafe { indicator.cancel.performClick(None) };
                ensure!(
                    actions.borrow().is_empty(),
                    "Hidden controls dispatched an action"
                );
            }
            ensure!(
                frontmost() == Some(original_app),
                "Indicator control changed the foreground app"
            );
            if let Some(output) = output {
                match phase {
                    Phase::Recording => indicator.snapshot(&output.join("recording.png"))?,
                    Phase::Transcribing => indicator.snapshot(&output.join("transcribing.png"))?,
                    Phase::Hidden => {}
                }
            }
            println!(
                "Verified {phase:?}: window={} frame={:?}",
                indicator.panel.windowNumber(),
                indicator.panel.frame()
            );
            Ok(())
        })();
        if result.is_err() || step == phases.len() {
            *flow = ControlFlow::Exit;
        } else {
            next = Instant::now() + Duration::from_secs(2);
            let frame = indicator.animate(|| 0.0);
            *flow = ControlFlow::WaitUntil(frame.map_or(next, |frame| frame.min(next)));
        }
    });
    indicator.set_phase(Phase::Hidden);
    ensure!(
        indicator
            .animate(|| panic!("Hidden indicator sampled audio"))
            .is_none(),
        "Hidden indicator scheduled animation"
    );
    ensure!(
        frontmost() == Some(original_app),
        "Foreground app changed after hiding the indicator"
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_stop_cannot_start_a_recording_and_cancel_only_applies_to_visible_work() {
        assert_eq!(Action::Stop.command(Phase::Recording), Some("stop"));
        for phase in [Phase::Hidden, Phase::Transcribing] {
            assert_eq!(Action::Stop.command(phase), None);
        }
        assert_eq!(Action::Cancel.command(Phase::Hidden), None);
        for phase in [Phase::Recording, Phase::Transcribing] {
            assert_eq!(Action::Cancel.command(phase), Some("cancel"));
        }
    }

    #[test]
    fn waveform_shows_audio_history_then_settles_to_silence() {
        let mut frame = Frame::new(Phase::Recording);
        frame.advance(0.04, Duration::ZERO);
        let peak = frame.levels[WAVE_BARS - 1];
        assert!(peak > 0.5);
        frame.advance(0.0, FRAME_INTERVAL);
        assert_eq!(frame.levels[WAVE_BARS - 2], peak);
        assert!(frame.levels[WAVE_BARS - 1] < peak);
        for _ in 0..100 {
            frame.advance(0.0, FRAME_INTERVAL);
        }
        assert!(frame.levels.iter().all(|level| *level < 0.001));
        assert_eq!(Frame::new(Phase::Recording).levels, [0.0; WAVE_BARS]);
    }

    #[test]
    fn spinner_rotates_with_elapsed_time_and_wraps_each_cycle() {
        let mut frame = Frame::new(Phase::Transcribing);
        frame.advance(0.0, Duration::from_millis(200));
        assert_eq!(frame.rotation, 90.0);
        frame.advance(0.0, Duration::from_millis(1000));
        assert_eq!(frame.rotation, 90.0);
    }

    #[test]
    fn overlay_joins_other_applications_full_screen_spaces() {
        let behavior = fullscreen_collection_behavior();
        assert!(behavior.contains(NSWindowCollectionBehavior::CanJoinAllSpaces));
        assert!(behavior.contains(NSWindowCollectionBehavior::CanJoinAllApplications));
        assert!(behavior.contains(NSWindowCollectionBehavior::FullScreenAuxiliary));
        assert!(!behavior.contains(NSWindowCollectionBehavior::Stationary));
    }

    #[test]
    fn indicator_sits_above_the_dock_on_the_visible_screen() {
        assert_eq!(
            bottom_center(NSRect::new(
                NSPoint::new(0.0, 80.0),
                NSSize::new(1440.0, 798.0)
            )),
            NSPoint::new(600.0, 96.0)
        );
    }

    #[test]
    fn indicator_centers_on_a_display_left_of_and_below_the_primary_display() {
        assert_eq!(
            bottom_center(NSRect::new(
                NSPoint::new(-1920.0, -1080.0),
                NSSize::new(1920.0, 1056.0)
            )),
            NSPoint::new(-1080.0, -1064.0)
        );
    }

    #[test]
    fn display_selection_uses_global_coordinates_and_excludes_shared_edges() {
        let left = NSRect::new(NSPoint::new(-1920.0, -100.0), NSSize::new(1920.0, 1080.0));
        assert!(contains(left, NSPoint::new(-960.0, 400.0)));
        assert!(!contains(left, NSPoint::new(0.0, 400.0)));
        assert!(!contains(left, NSPoint::new(-960.0, -101.0)));
    }
}
