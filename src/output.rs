use anyhow::{Result, ensure};
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;
use std::{
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn CGPreflightPostEventAccess() -> bool;
    fn CGEventCreateKeyboardEvent(
        source: *const std::ffi::c_void,
        key: u16,
        down: bool,
    ) -> *mut std::ffi::c_void;
    fn CGEventSetFlags(event: *mut std::ffi::c_void, flags: u64);
    fn CGEventPost(tap: u32, event: *mut std::ffi::c_void);
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: *const std::ffi::c_void);
}

pub fn trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

pub fn can_post_events() -> bool {
    unsafe { CGPreflightPostEventAccess() }
}

pub fn frontmost_app() -> String {
    objc2_app_kit::NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .and_then(|app| app.localizedName())
        .map(|name| name.to_string())
        .unwrap_or_else(|| "unknown".into())
}

pub fn deliver(text: &str, paste: bool, canceled: &AtomicBool) -> Result<()> {
    objc2::rc::autoreleasepool(|_| {
        ensure!(!canceled.load(Ordering::Acquire), "Canceled");
        let clipboard = NSPasteboard::generalPasteboard();
        clipboard.clearContents();
        ensure!(
            clipboard
                .setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString }),
            "Copy to clipboard failed"
        );
        post_paste(paste, canceled)
    })
}

fn post_paste(paste: bool, canceled: &AtomicBool) -> Result<()> {
    if paste {
        ensure!(
            trusted() && can_post_events(),
            "Text copied. Enable Nemo in System Settings → Privacy & Security → Accessibility to paste automatically"
        );
        ensure!(!canceled.load(Ordering::Acquire), "Canceled");
        eprintln!("Pasting into {}", frontmost_app());
        unsafe {
            let down = CGEventCreateKeyboardEvent(std::ptr::null(), 9, true);
            let up = CGEventCreateKeyboardEvent(std::ptr::null(), 9, false);
            if down.is_null() || up.is_null() {
                if !down.is_null() {
                    CFRelease(down);
                }
                if !up.is_null() {
                    CFRelease(up);
                }
                anyhow::bail!("Text copied, but macOS could not create the paste keystroke");
            }
            CGEventSetFlags(down, 1 << 20);
            CGEventSetFlags(up, 1 << 20);
            CGEventPost(0, down);
            CGEventPost(0, up);
            CFRelease(down);
            CFRelease(up);
        }
    }
    Ok(())
}

pub fn sound(name: &str) {
    if let Ok(mut child) = Command::new("/usr/bin/afplay")
        .arg(format!("/System/Library/Sounds/{name}.aiff"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

pub fn format_transcript(text: &str, remove_trailing_period: bool) -> &str {
    if remove_trailing_period {
        text.strip_suffix('.').unwrap_or(text)
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::format_transcript;

    #[test]
    fn removes_only_the_final_period_when_enabled() {
        for (text, expected) in [
            ("Hello world.", "Hello world"),
            (
                "First sentence. Second sentence.",
                "First sentence. Second sentence",
            ),
            ("Version 1.2.", "Version 1.2"),
            ("Hello world", "Hello world"),
            ("Really?", "Really?"),
            ("Hello!", "Hello!"),
            ("", ""),
        ] {
            assert_eq!(format_transcript(text, true), expected);
            assert_eq!(format_transcript(text, false), text);
        }
    }
}
