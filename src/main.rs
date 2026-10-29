mod audio;
mod backend;
mod config;
mod control;
mod model;
mod output;
mod permissions;
mod settings;
mod shortcut;
mod sounds;
mod startup;
mod storage;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use config::Config;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use tao::{
    event::Event as TaoEvent,
    event_loop::{ControlFlow, EventLoopBuilder},
    platform::macos::{ActivationPolicy, EventLoopExtMacOS},
};
use tray_icon::{
    Icon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

#[derive(Parser)]
#[command(name = "nemo", version, about = "Local Nemotron dictation")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Manage pinned model weights in Application Support.
    Model {
        #[command(subcommand)]
        command: ModelCommand,
    },
    /// Run the menu bar daemon.
    Run,
    /// Start recording, or stop and transcribe.
    Toggle,
    /// Discard the current recording or transcription.
    Cancel,
    /// Open the running daemon's settings.
    Settings,
    /// Print the daemon's current state.
    Status,
    /// Load the model ahead of the next dictation.
    Warm,
    /// Stop the daemon and release the model.
    Quit {
        /// Refuse to interrupt recording, microphone setup, or model work.
        #[arg(long)]
        if_idle: bool,
    },
    /// Transcribe a WAV file without changing the clipboard unless requested.
    Transcribe {
        file: PathBuf,
        #[arg(long)]
        copy: bool,
        #[arg(long)]
        paste: bool,
    },
    /// Inspect paths, devices, and permissions.
    Doctor,
    /// Report this process's permission state without opening the microphone.
    Permissions,
    /// Print configuration, or set a shortcut. Restart the daemon after changing it.
    Config {
        #[arg(long)]
        hotkey: Option<String>,
    },
}

#[derive(Subcommand)]
enum ModelCommand {
    Path,
    Download,
    Verify,
    /// Import existing weights and leave compatibility links for the running app.
    Migrate {
        #[arg(long)]
        from: PathBuf,
    },
}

enum Event {
    Action(String),
    SettingsChanged(settings::Change),
    QuitIfIdle(mpsc::Sender<bool>),
    RecordingStarted(u64, Result<audio::Recording>),
    Finished(u64, Result<Outcome>),
    WorkerUnloaded,
}
enum Job {
    Prepare(Arc<AtomicBool>),
    Download(u64, Arc<AtomicBool>),
    Warm(u64, Arc<AtomicBool>),
    Transcribe(u64, Vec<u8>, Arc<AtomicBool>),
}

enum Outcome {
    ModelLoaded,
    ModelDownloaded,
    Transcript(String),
}
enum State {
    Idle,
    Starting {
        id: u64,
        canceled: Arc<AtomicBool>,
    },
    Recording {
        audio: audio::Recording,
        started: Instant,
        canceled: Arc<AtomicBool>,
    },
    Working {
        id: u64,
        canceled: Arc<AtomicBool>,
    },
}

fn main() {
    if let Err(error) = execute() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

fn execute() -> Result<()> {
    let cli = Cli::parse();
    let mut config = Config::load()?;
    match cli.command.unwrap_or(Command::Run) {
        Command::Run => run(config),
        Command::Toggle => remote("toggle"),
        Command::Cancel => remote("cancel"),
        Command::Status => remote("status"),
        Command::Settings => remote("settings"),
        Command::Warm => remote("warm"),
        Command::Quit { if_idle: false } => remote("quit"),
        Command::Quit { if_idle: true } => {
            let response = control::send("quit-idle")?;
            anyhow::ensure!(
                response == "accepted",
                "Safe quit refused ({response}). Finish or cancel the current operation; quit older Nemo versions manually before upgrading"
            );
            println!("accepted");
            Ok(())
        }
        Command::Permissions => {
            println!(
                "{}",
                serde_json::to_string(&permissions::Snapshot::current())?
            );
            Ok(())
        }
        Command::Config { hotkey } => {
            if let Some(hotkey) = hotkey {
                config = Config::update(|config| {
                    config.hotkey = hotkey.parse()?;
                    Ok(())
                })?;
            }
            print!("{}", toml::to_string_pretty(&config)?);
            Ok(())
        }
        Command::Model { command } => {
            let store = model::Store::managed();
            if matches!(command, ModelCommand::Path) {
                println!("{}", store.decoder().display());
                return Ok(());
            }
            let canceled = Arc::new(AtomicBool::new(false));
            let stopping = canceled.clone();
            let process = backend::Process::default();
            let child = process.clone();
            ctrlc::set_handler(move || {
                stopping.store(true, Ordering::Release);
                child.shutdown();
            })?;
            match command {
                ModelCommand::Download => store.download(&process, &canceled)?,
                ModelCommand::Verify => store.verify(&canceled)?,
                ModelCommand::Migrate { from } => {
                    store.migrate(&from, &canceled)?;
                    Config::update(|config| {
                        config.model = store.decoder();
                        Ok(())
                    })?;
                }
                ModelCommand::Path => unreachable!(),
            }
            println!("Model verified: {}", store.decoder().display());
            Ok(())
        }
        Command::Transcribe { file, copy, paste } => {
            anyhow::ensure!(
                fs::metadata(&file)?.len() <= 64 * 1024 * 1024,
                "WAV input exceeds 64 MiB"
            );
            let process = backend::Process::default();
            let stopping = process.clone();
            ctrlc::set_handler(move || stopping.shutdown())?;
            let remove_trailing_period = config.remove_trailing_period;
            let mut backend = backend::Backend::new(config, process)?;
            let text = backend.transcribe(fs::read(file)?, &AtomicBool::new(false))?;
            let text = output::format_transcript(&text, remove_trailing_period);
            if copy || paste {
                output::deliver(text, paste)?;
            }
            println!("{text}");
            Ok(())
        }
        Command::Doctor => {
            use cpal::traits::{DeviceTrait, HostTrait};
            println!("Config: {}", config::path().display());
            println!("Shortcut: {}", config.hotkey);
            println!(
                "Runtime: {} ({})",
                config.runtime.display(),
                if config.runtime.is_file() {
                    "found"
                } else {
                    "MISSING"
                }
            );
            println!(
                "Model: {} ({} bytes)",
                config.model.display(),
                fs::metadata(&config.model).map(|m| m.len()).unwrap_or(0)
            );
            println!(
                "Accessibility: {}",
                if output::trusted() {
                    "granted"
                } else {
                    "grant Nemo.app access in System Settings → Privacy & Security → Accessibility"
                }
            );
            println!("Post keyboard events: {}", output::can_post_events());
            println!(
                "CLI permissions: {}",
                serde_json::to_string(&permissions::Snapshot::current())?
            );
            println!(
                "Daemon permissions: {}",
                control::send("permissions").unwrap_or_else(|_| "stopped".into())
            );
            println!("Foreground app: {}", output::frontmost_app());
            let (sender, receiver) = mpsc::channel();
            thread::spawn(move || {
                let result = cpal::default_host()
                    .default_input_device()
                    .context("No microphone")
                    .and_then(|device| Ok(format!("{:?}", device.description()?)));
                let _ = sender.send(result);
            });
            match receiver.recv_timeout(Duration::from_secs(2)) {
                Ok(result) => println!("Microphone: {}", result?),
                Err(_) => println!("Microphone: macOS is waiting; check its permission dialog"),
            }
            println!(
                "Daemon: {}",
                control::send("status").unwrap_or_else(|_| "stopped".into())
            );
            Ok(())
        }
    }
}

fn remote(command: &str) -> Result<()> {
    println!("{}", control::send(command)?);
    Ok(())
}

fn icon(color: [u8; 3]) -> Icon {
    let mut rgba = vec![0; 22 * 22 * 4];
    for y in 0_i32..22 {
        for x in 0_i32..22 {
            if (x - 11).pow(2) + (y - 11).pow(2) < 64 {
                let i = ((y * 22 + x) * 4) as usize;
                rgba[i..i + 3].copy_from_slice(&color);
                rgba[i + 3] = 255;
            }
        }
    }
    Icon::from_rgba(rgba, 22, 22).expect("valid icon")
}

fn run(mut config: Config) -> Result<()> {
    let server = control::Server::bind()?;
    fs::create_dir_all(config::data_dir())?;
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(config::data_dir().join("daemon.log"))?;
    use std::os::fd::AsRawFd;
    unsafe {
        libc::dup2(log.as_raw_fd(), libc::STDERR_FILENO);
    }
    let mut event_loop = EventLoopBuilder::<Event>::with_user_event().build();
    event_loop.set_activation_policy(ActivationPolicy::Accessory);
    event_loop.set_activate_ignoring_other_apps(false);
    let proxy = event_loop.create_proxy();
    ctrlc::set_handler(move || {
        let _ = proxy.send_event(Event::Action("quit".into()));
    })?;
    let shortcut_proxy = event_loop.create_proxy();
    let mut hotkeys = None;
    eprintln!("Shortcut: {}", config.hotkey);

    let menu = Menu::new();
    let status_item = MenuItem::new("Ready", false, None);
    let toggle_item = MenuItem::new(format!("Record ({})", config.hotkey), true, None);
    let cancel_item = MenuItem::new("Cancel", false, None);
    let warm_item = MenuItem::new("Load model", true, None);
    let download_item = MenuItem::new("Download model (21.35 GB)", true, None);
    let settings_item = MenuItem::new("Settings…", true, None);
    let config_item = MenuItem::new("Open configuration", true, None);
    let permissions_item = MenuItem::new("Accessibility settings", true, None);
    let microphone_item = MenuItem::new("Microphone settings", true, None);
    let quit_item = MenuItem::new("Quit", true, None);
    menu.append_items(&[
        &status_item,
        &toggle_item,
        &cancel_item,
        &warm_item,
        &download_item,
        &settings_item,
        &config_item,
        &permissions_item,
        &microphone_item,
        &PredefinedMenuItem::separator(),
        &quit_item,
    ])?;
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icon([160, 160, 160]))
        .with_tooltip("Nemo — Ready")
        .build()?;
    let toggle_id = toggle_item.id().clone();
    let cancel_id = cancel_item.id().clone();
    let warm_id = warm_item.id().clone();
    let download_id = download_item.id().clone();
    let settings_id = settings_item.id().clone();
    let config_id = config_item.id().clone();
    let permissions_id = permissions_item.id().clone();
    let microphone_id = microphone_item.id().clone();
    let quit_id = quit_item.id().clone();
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let action = if event.id == toggle_id {
            "toggle"
        } else if event.id == cancel_id {
            "cancel"
        } else if event.id == warm_id {
            "warm"
        } else if event.id == download_id {
            "download"
        } else if event.id == settings_id {
            "settings"
        } else if event.id == config_id {
            "config"
        } else if event.id == permissions_id {
            "permissions"
        } else if event.id == microphone_id {
            "microphone"
        } else if event.id == quit_id {
            "quit"
        } else {
            return;
        };
        let _ = proxy.send_event(Event::Action(action.into()));
    }));

    let status = Arc::new(Mutex::new(String::from("ready")));
    let socket_status = status.clone();
    let proxy = event_loop.create_proxy();
    let listener = server.listener.try_clone()?;
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let mut command = String::new();
            if BufReader::new(&stream).read_line(&mut command).is_err() {
                continue;
            }
            let command = command.trim();
            let response = if command == "status" {
                socket_status.lock().unwrap().clone()
            } else if command == "permissions" {
                serde_json::to_string(&permissions::Snapshot::current()).unwrap()
            } else if command == "quit-idle" {
                let (reply, receiver) = mpsc::channel();
                if proxy.send_event(Event::QuitIfIdle(reply)).is_err() {
                    "stopping".into()
                } else {
                    match receiver.recv_timeout(Duration::from_secs(2)) {
                        Ok(true) => "accepted".into(),
                        Ok(false) => "busy".into(),
                        Err(_) => "unresponsive".into(),
                    }
                }
            } else if ["toggle", "cancel", "warm", "settings", "quit"].contains(&command) {
                if proxy.send_event(Event::Action(command.into())).is_ok() {
                    "accepted".into()
                } else {
                    "stopping".into()
                }
            } else {
                "unknown command".into()
            };
            let _ = writeln!(stream, "{response}");
        }
    });

    let process = backend::Process::default();
    let mut backend = backend::Backend::new(config.clone(), process.clone())?;
    let (jobs, receiver) = mpsc::channel();
    let proxy = event_loop.create_proxy();
    let idle_timeout = Duration::from_secs(config.idle_unload_seconds);
    thread::spawn(move || {
        loop {
            let received = if backend.loaded() {
                receiver.recv_timeout(idle_timeout)
            } else {
                receiver
                    .recv()
                    .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
            };
            match received {
                Ok(job) => {
                    let (id, result) = match job {
                        Job::Prepare(canceled) => {
                            if let Err(error) = backend.ready(&canceled) {
                                eprintln!("Model preparation failed: {error:#}");
                                backend.unload();
                            }
                            continue;
                        }
                        Job::Warm(id, canceled) => {
                            (id, backend.ready(&canceled).map(|_| Outcome::ModelLoaded))
                        }
                        Job::Download(id, canceled) => {
                            backend.unload();
                            (
                                id,
                                model::Store::managed()
                                    .download(&backend.process, &canceled)
                                    .map(|_| Outcome::ModelDownloaded),
                            )
                        }
                        Job::Transcribe(id, wav, canceled) => (
                            id,
                            backend.transcribe(wav, &canceled).map(Outcome::Transcript),
                        ),
                    };
                    if result.is_err() {
                        backend.unload();
                    }
                    if proxy.send_event(Event::Finished(id, result)).is_err() {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if backend.unload() {
                        let _ = proxy.send_event(Event::WorkerUnloaded);
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    let proxy = event_loop.create_proxy();
    let settings = settings::Settings::new(&event_loop, &config, move |change| {
        let _ = proxy.send_event(Event::SettingsChanged(change));
    })?;
    let sounds = sounds::Player::new()?;
    let mut state = State::Idle;
    let recording_proxy = event_loop.create_proxy();
    let mut next_id = 0;
    let mut last_toggle = Instant::now() - Duration::from_secs(1);
    let shortcut = config.hotkey.clone();
    let render_controls = move |state: &State| {
        let recording = matches!(state, State::Recording { .. });
        toggle_item.set_text(if recording {
            "Stop recording".into()
        } else {
            format!("Record ({shortcut})")
        });
        toggle_item.set_enabled(!matches!(
            state,
            State::Working { .. } | State::Starting { .. }
        ));
        cancel_item.set_enabled(!matches!(state, State::Idle));
        warm_item.set_enabled(matches!(state, State::Idle));
        download_item.set_enabled(matches!(state, State::Idle));
    };
    let set_status = move |message: &str, color| {
        let normalized = message.replace('\n', " ");
        let mut current = status.lock().unwrap();
        if *current == normalized {
            return;
        }
        *current = normalized;
        drop(current);
        eprintln!("{message}");
        status_item.set_text(message.chars().take(100).collect::<String>());
        let _ = tray.set_icon(Some(icon(color)));
        let _ = tray.set_tooltip(Some(format!("Nemo — {message}")));
    };

    let mut next_permissions_check = Instant::now();
    let mut last_permissions = None;
    if config.preload_model && config.check_assets().is_ok() {
        let _ = event_loop
            .create_proxy()
            .send_event(Event::Action("warm".into()));
    }
    event_loop.run(move |event, _, flow| {
        let _keep_alive = (&server, &hotkeys);
        if Instant::now() >= next_permissions_check {
            if settings.window.is_visible() { settings.refresh_startup(); }
            let observed = permissions::Snapshot::current();
            let changed = last_permissions.replace(observed) != Some(observed);
            let permissions = observed;
            if !permissions.shortcut_allowed(&config.hotkey) { hotkeys = None; }
            else if hotkeys.is_none() && changed {
                let proxy = shortcut_proxy.clone();
                match shortcut::Binding::register(&config.hotkey, move || { let _ = proxy.send_event(Event::Action("toggle".into())); }) {
                    Ok(binding) => { hotkeys = Some(binding); eprintln!("Shortcut registered: {}", config.hotkey); }
                    Err(error) => eprintln!("Shortcut unavailable: {error:#}"),
                }
            }
            let issue = permissions.issue(&config.hotkey).or_else(|| hotkeys.is_none().then_some("Shortcut unavailable"));
            if matches!(state, State::Idle) && (changed || issue.is_some()) {
                let missing_model = config.model == model::Store::managed().decoder() && !model::Store::managed().ready();
                set_status(issue.unwrap_or(if missing_model { "Download model" } else { "Ready" }), if issue.is_some() || missing_model { [230,170,30] } else { [160,160,160] });
            }
            next_permissions_check = Instant::now() + Duration::from_secs(2);
        }
        let action = match event {
            TaoEvent::UserEvent(Event::QuitIfIdle(reply)) => {
                let idle = matches!(state, State::Idle);
                let _ = reply.send(idle);
                if idle { process.shutdown(); *flow = ControlFlow::Exit; return; }
                None
            }
            TaoEvent::WindowEvent { window_id, event: tao::event::WindowEvent::CloseRequested, .. } if window_id == settings.window.id() => {
                settings.window.set_visible(false);
                None
            }
            TaoEvent::UserEvent(Event::SettingsChanged(settings::Change::RemoveTrailingPeriod(enabled))) => {
                match Config::update(|saved| { saved.remove_trailing_period = enabled; Ok(()) }) {
                    Ok(_) => { config.remove_trailing_period = enabled; settings.set_error(""); }
                    Err(error) => {
                        settings.set_remove_trailing_period(config.remove_trailing_period);
                        settings.set_error("Could not save settings.");
                        eprintln!("Cannot save settings: {error:#}");
                    }
                }
                None
            }
            TaoEvent::UserEvent(Event::SettingsChanged(settings::Change::LaunchOnStartup(enabled))) => {
                match startup::set_enabled(enabled) {
                    Ok(()) => settings.set_error(""),
                    Err(error) => { settings.set_error("Could not change launch on startup."); eprintln!("Cannot change launch on startup: {error:#}"); }
                }
                settings.refresh_startup();
                None
            }
            TaoEvent::UserEvent(Event::SettingsChanged(settings::Change::SoundPack(pack))) => {
                match Config::update(|saved| { saved.sound_pack = pack; Ok(()) }) {
                    Ok(_) => { config.sound_pack = pack; settings.set_error(""); }
                    Err(error) => {
                        settings.set_sound_pack(config.sound_pack);
                        settings.set_error("Could not save sound pack.");
                        eprintln!("Cannot save sound pack: {error:#}");
                    }
                }
                None
            }
            TaoEvent::UserEvent(Event::SettingsChanged(settings::Change::PreviewSound(cue))) => {
                sounds.play(config.sound_pack, cue);
                None
            }
            TaoEvent::UserEvent(Event::Action(action)) => Some(action),
            TaoEvent::UserEvent(Event::RecordingStarted(id, result)) => {
                if let State::Starting { id: current, canceled } = &state && *current == id {
                    let canceled = canceled.clone();
                    let was_canceled = canceled.load(Ordering::Acquire);
                    state = State::Idle;
                    if was_canceled { set_status("Canceled", [160,160,160]); }
                    else { match result {
                        Ok(audio) => { let canceled = canceled.clone(); state = State::Recording { audio, started: Instant::now(), canceled: canceled.clone() }; let _ = jobs.send(Job::Prepare(canceled)); set_status("Recording", [230,55,55]); sounds.play(config.sound_pack, sounds::Cue::Start); }
                        Err(error) => { state = State::Idle; set_status(&format!("{error:#}"), [230,80,60]); output::sound("Basso"); }
                    }}
                }
                None
            }
            TaoEvent::NewEvents(tao::event::StartCause::ResumeTimeReached { .. }) => {
                match &state { State::Recording { started, .. } if started.elapsed() >= Duration::from_secs(config.max_recording_seconds) => Some("toggle".into()), _ => None }
            },
            TaoEvent::UserEvent(Event::Finished(id, result)) => {
                if let State::Working { id: current, canceled } = &state && *current == id {
                    let was_canceled = canceled.load(Ordering::Acquire);
                    state = State::Idle;
                    if was_canceled { set_status("Canceled", [160,160,160]); }
                    else { match result {
                        Ok(Outcome::Transcript(text)) => match output::deliver(output::format_transcript(&text, config.remove_trailing_period), config.paste) {
                            Ok(()) => { set_status("Ready", [160,160,160]); output::sound("Glass"); }
                            Err(error) => { set_status(&format!("{error:#}"), [230,80,60]); output::sound("Basso"); }
                        },
                        Ok(Outcome::ModelLoaded) => set_status("Model loaded", [160,160,160]),
                        Ok(Outcome::ModelDownloaded) => set_status("Model downloaded", [160,160,160]),
                        Err(error) => { set_status(&format!("{error:#}"), [230,80,60]); output::sound("Basso"); }
                    }}
                }
                None
            }
            TaoEvent::UserEvent(Event::WorkerUnloaded) => { if matches!(state, State::Idle) { set_status("Ready · model unloaded", [160,160,160]); } None }
            _ => None,
        };
        render_controls(&state);
        let deadline = match &state {
            State::Recording { started, .. } => next_permissions_check.min(*started + Duration::from_secs(config.max_recording_seconds)),
            _ => next_permissions_check,
        };
        *flow = ControlFlow::WaitUntil(deadline);
        let Some(action) = action else { return; };
        match action.as_str() {
            "quit" => {
                if let State::Working { canceled, .. } | State::Starting { canceled, .. } | State::Recording { canceled, .. } = &state { canceled.store(true, Ordering::Release); }
                process.shutdown();
                *flow = ControlFlow::Exit;
            }
            "settings" => { settings.show(); }
            "config" => { if !config::path().exists() { let _ = config.save(); } let _ = std::process::Command::new("/usr/bin/open").arg("-e").arg(config::path()).spawn(); }
            "permissions" => { let _ = std::process::Command::new("/usr/bin/open").arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility").spawn(); }
            "microphone" => { let _ = std::process::Command::new("/usr/bin/open").arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone").spawn(); }
            "cancel" => {
                match &state {
                    State::Working { canceled, .. } => { canceled.store(true, Ordering::Release); process.stop(); set_status("Canceling", [160,160,160]); }
                    State::Recording { canceled, .. } => { canceled.store(true, Ordering::Release); state = State::Idle; output::sound("Pop"); set_status("Canceled", [160,160,160]); }
                    State::Starting { canceled, .. } => { canceled.store(true, Ordering::Release); set_status("Canceling microphone request", [160,160,160]); }
                    State::Idle => {}
                }
            }
            "warm" if matches!(state, State::Idle) => {
                next_id += 1;
                let canceled = Arc::new(AtomicBool::new(false));
                let _ = jobs.send(Job::Warm(next_id, canceled.clone()));
                state = State::Working { id: next_id, canceled };
                set_status("Loading model", [230,170,30]);
            }
            "download" if matches!(state, State::Idle) => {
                if config.model != model::Store::managed().decoder() { set_status("Set model to the managed path before downloading", [230,80,60]); return; }
                next_id += 1;
                let canceled = Arc::new(AtomicBool::new(false));
                let _ = jobs.send(Job::Download(next_id, canceled.clone()));
                state = State::Working { id: next_id, canceled };
                set_status("Downloading model", [230,170,30]);
            }
            "toggle" => {
                if last_toggle.elapsed() < Duration::from_millis(200) { return; }
                last_toggle = Instant::now();
                if matches!(state, State::Working { .. } | State::Starting { .. }) { return; }
                match std::mem::replace(&mut state, State::Idle) {
                    State::Idle => {
                        let permissions = permissions::Snapshot::current();
                        if !permissions.recording_allowed() { set_status("Enable Microphone", [230,80,60]); return; }
                        next_id += 1;
                        let id = next_id;
                        let canceled = Arc::new(AtomicBool::new(false));
                        state = State::Starting { id, canceled };
                        set_status("Opening microphone", [230,170,30]);
                        let proxy = recording_proxy.clone();
                        let seconds = config.max_recording_seconds;
                        thread::spawn(move || {
                            let result = audio::Recording::start(seconds);
                            let _ = proxy.send_event(Event::RecordingStarted(id, result));
                        });
                    },
                    State::Recording { audio, canceled, .. } => {
                        let recording = audio.finish();
                        sounds.play(config.sound_pack, sounds::Cue::Stop);
                        match recording {
                        Ok(wav) => {
                            next_id += 1;
                            let _ = jobs.send(Job::Transcribe(next_id, wav, canceled.clone()));
                            state = State::Working { id: next_id, canceled };
                            set_status("Transcribing", [230,170,30]);
                        }
                        Err(error) => { canceled.store(true, Ordering::Release); set_status(&format!("{error:#}"), [230,80,60]); output::sound("Basso"); }
                        }
                    },
                    State::Working { .. } | State::Starting { .. } => unreachable!(),
                }
            }
            _ => {}
        }
        render_controls(&state);
        if !matches!(*flow, ControlFlow::Exit) {
            *flow = ControlFlow::WaitUntil(match &state {
                State::Recording { started, .. } => next_permissions_check.min(*started + Duration::from_secs(config.max_recording_seconds)),
                _ => next_permissions_check,
            });
        }
    });
}
