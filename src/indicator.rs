use anyhow::{Context, Result, ensure};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained};
use objc2_app_kit::{
    NSBackingStoreType, NSBezierPath, NSBitmapImageFileType, NSColor, NSEvent, NSFont,
    NSFontWeightMedium, NSPanel, NSScreen, NSStatusWindowLevel, NSTextField, NSView,
    NSWindowCollectionBehavior, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{NSDictionary, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
use std::{
    cell::Cell,
    path::Path,
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

const SIZE: NSSize = NSSize::new(168.0, 44.0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Hidden,
    Recording,
    Transcribing,
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
    #[ivars = Cell<Phase>]
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
            match self.ivars().get() {
                Phase::Recording => {
                    NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 0.29, 0.32, 1.0).setFill();
                    NSBezierPath::bezierPathWithOvalInRect(NSRect::new(NSPoint::new(19.0, 18.0), NSSize::new(8.0, 8.0))).fill();
                }
                Phase::Transcribing => {
                    NSColor::colorWithSRGBRed_green_blue_alpha(0.70, 0.78, 1.0, 1.0).setStroke();
                    let arc = NSBezierPath::bezierPath();
                    arc.setLineWidth(2.0);
                    arc.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle(NSPoint::new(23.0, 22.0), 5.0, 40.0, 320.0);
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
    phase: Phase,
}

impl Indicator {
    pub fn new() -> Self {
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
        panel.setIgnoresMouseEvents(true);
        panel.setLevel(NSStatusWindowLevel);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary,
        );
        let view = IndicatorView::alloc(mtm).set_ivars(Cell::new(Phase::Hidden));
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
        view.addSubview(&label);
        panel.setContentView(Some(&view));
        Self {
            panel,
            view,
            label,
            phase: Phase::Hidden,
        }
    }

    pub fn set_phase(&mut self, phase: Phase) {
        if self.phase == phase {
            return;
        }
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
            self.view.ivars().set(phase);
            self.view.setNeedsDisplay(true);
            self.panel.orderFrontRegardless();
        }
        self.phase = phase;
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
            self.panel.ignoresMouseEvents(),
            "Indicator intercepts clicks"
        );
        match phase {
            Phase::Recording => ensure!(
                self.label.stringValue().to_string() == "Recording",
                "Recording label missing"
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
    let mut indicator = Indicator::new();
    indicator.verify(Phase::Hidden)?;
    let mut next = Instant::now();
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
            *flow = ControlFlow::WaitUntil(next);
            return;
        }
        let phase = phases[step];
        step += 1;
        result = (|| {
            ensure!(
                frontmost() == Some(original_app),
                "Foreground app changed during preview"
            );
            indicator.set_phase(phase);
            indicator.set_phase(phase);
            indicator.verify(phase)?;
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
            *flow = ControlFlow::WaitUntil(next);
        }
    });
    indicator.set_phase(Phase::Hidden);
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
    fn indicator_sits_above_the_dock_on_the_visible_screen() {
        assert_eq!(
            bottom_center(NSRect::new(
                NSPoint::new(0.0, 80.0),
                NSSize::new(1440.0, 798.0)
            )),
            NSPoint::new(636.0, 96.0)
        );
    }

    #[test]
    fn indicator_centers_on_a_display_left_of_and_below_the_primary_display() {
        assert_eq!(
            bottom_center(NSRect::new(
                NSPoint::new(-1920.0, -1080.0),
                NSSize::new(1920.0, 1056.0)
            )),
            NSPoint::new(-1044.0, -1064.0)
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
