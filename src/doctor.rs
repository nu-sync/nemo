use crate::{
    audio,
    config::{self, Config},
    control, permissions,
    readiness::{Activity, Readiness},
};
use clap::ValueEnum;
use std::{io::IsTerminal, sync::mpsc, thread, time::Duration};

#[derive(Clone, Copy, ValueEnum)]
pub enum Color {
    Auto,
    Always,
    Never,
}

impl Color {
    fn enabled(self) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Auto => {
                std::io::stdout().is_terminal()
                    && std::env::var_os("NO_COLOR").is_none()
                    && std::env::var("TERM").as_deref() != Ok("dumb")
            }
        }
    }
}

fn row(color: bool, code: u8, label: &str, text: &str) -> String {
    if color {
        format!("\x1b[{code}m{label}\x1b[0m  {text}\n")
    } else {
        format!("{label}  {text}\n")
    }
}

pub fn report(readiness: &Readiness, color: bool) -> String {
    let mut output = String::from("Nemo doctor\n\n");
    if readiness.blockers.is_empty() && readiness.activity != Activity::Checking {
        output.push_str(&row(color, 32, "OK", &readiness.summary()));
    } else {
        output.push_str(&row(color, 33, "ATTENTION", &readiness.summary()));
    }
    output.push_str(&format!(
        "Activity: {}\nSource: running Nemo app\n",
        readiness.activity.label()
    ));
    for blocker in &readiness.blockers {
        output.push('\n');
        output.push_str(&row(
            color,
            33,
            "ACTION",
            &blocker.label(&readiness.shortcut),
        ));
        output.push_str(blocker.remedy());
        output.push('\n');
        if let Some(url) = blocker.settings_url() {
            output.push_str(&format!("Open settings: open '{url}'\n"));
        }
    }
    output
}

pub fn run(config: &Config, color: Color) {
    let color = color.enabled();
    match control::send("readiness") {
        Ok(json) => match serde_json::from_str::<Readiness>(&json) {
            Ok(readiness) => print!("{}", report(&readiness, color)),
            Err(_) => {
                println!("Nemo doctor\n");
                print!(
                    "{}",
                    row(
                        color,
                        33,
                        "UNKNOWN",
                        "Running app does not provide readiness. Reopen the updated Nemo app."
                    )
                );
                println!(
                    "Daemon status: {}",
                    control::send("status").unwrap_or_else(|_| "unavailable".into())
                );
                println!(
                    "Daemon permissions: {}",
                    control::send("permissions").unwrap_or_else(|_| "unavailable".into())
                );
            }
        },
        Err(error) => {
            println!("Nemo doctor\n");
            print!(
                "{}",
                row(
                    color,
                    31,
                    "STOPPED",
                    &format!("Cannot inspect the Nemo app: {error}")
                )
            );
            println!("Open Nemo.app, then run doctor again.");
        }
    }
    println!();
    let runtime_path = config.runtime_path();
    let runtime = runtime_path.is_file();
    print!(
        "{}",
        row(
            color,
            if runtime { 32 } else { 31 },
            if runtime { "OK" } else { "MISSING" },
            &format!(
                "Runtime ({}): {}",
                config.backend.label(),
                runtime_path.display()
            )
        )
    );
    let model = config.check_model_assets().is_ok();
    print!(
        "{}",
        row(
            color,
            if model { 32 } else { 31 },
            if model { "OK" } else { "MISSING" },
            &format!("Model assets: {}", config.model.display())
        )
    );
    println!(
        "Config: {}\nConfigured shortcut: {}",
        config::path().display(),
        config.hotkey
    );
    let local = permissions::Snapshot::current();
    println!("Foreground app: {}", crate::output::frontmost_app());
    println!(
        "CLI permissions (this process only): Accessibility={}, Microphone={:?}",
        local.accessibility, local.microphone
    );
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(audio::Inputs::current());
    });
    match receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(inputs)) => {
            match inputs.resolve(config.microphone.as_ref()) {
                Ok((device, false)) => print!(
                    "{}",
                    row(color, 32, "OK", &format!("Microphone: {}", device.name))
                ),
                Ok((device, true)) => print!(
                    "{}",
                    row(
                        color,
                        33,
                        "ACTION",
                        &format!(
                            "Saved microphone unavailable; confirm replacement: {}",
                            device.name
                        )
                    )
                ),
                Err(error) => print!(
                    "{}",
                    row(color, 31, "BLOCKED", &format!("Microphone: {error}"))
                ),
            }
            for device in inputs.devices {
                println!("Input device: {} ({})", device.name, device.id);
            }
        }
        Ok(Err(error)) => print!(
            "{}",
            row(color, 31, "ERROR", &format!("Microphones: {error:#}"))
        ),
        Err(_) => print!(
            "{}",
            row(
                color,
                33,
                "WAITING",
                "Microphone discovery timed out; check the macOS permission dialog."
            )
        ),
    }
    println!("Log: {}", config::data_dir().join("daemon.log").display());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_explains_accessibility_from_the_daemon_and_prints_the_fix() {
        let readiness = Readiness::observe(
            Activity::Idle,
            &crate::shortcut::Shortcut::RightOption,
            permissions::Snapshot {
                accessibility: false,
                microphone: permissions::Microphone::Authorized,
            },
            false,
            &crate::readiness::Input::Available,
            true,
            true,
        );
        let plain = report(&readiness, false);
        assert!(
            plain.starts_with(
                "Nemo doctor\n\nATTENTION  Right Option blocked: enable Accessibility\n"
            )
        );
        assert!(plain.contains("Source: running Nemo app"));
        assert!(plain.contains("Open settings: open 'x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility'"));
        assert!(!plain.contains('\x1b'));
        let colored = report(&readiness, true);
        assert!(
            colored
                .contains("\x1b[33mATTENTION\x1b[0m  Right Option blocked: enable Accessibility")
        );
    }
}
