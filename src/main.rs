mod assist;
mod audio;
mod backend;
mod bootstrap;
mod config;
mod control;
mod doctor;
mod download;
mod indicator;
mod model;
mod network;
mod output;
mod permissions;
mod prerequisite;
mod readiness;
mod settings;
mod shortcut;
mod sounds;
mod startup;
mod storage;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::Config;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
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
    #[command(hide = true)]
    DownloadWorker {
        #[arg(long)]
        parent_pid: u32,
    },
    /// Prepare Nemo.app from this Cargo-installed executable.
    Install {
        /// Allow an update that may require granting macOS permissions again.
        #[arg(long)]
        allow_permission_reset: bool,
    },
    /// Manage pinned model weights in Application Support.
    Model {
        #[command(subcommand)]
        command: ModelCommand,
    },
    /// Run the menu bar daemon directly, without installing an app.
    Run,
    /// Preview the recording indicator without microphone access or sound.
    PreviewIndicator {
        /// Save a PNG of each visible phase in this directory.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Report the running daemon's executable and startup fingerprint.
    Identity,
    #[command(hide = true)]
    PrepareApp {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        binary: PathBuf,
        #[arg(long)]
        destination: PathBuf,
        #[arg(long)]
        allow_permission_reset: bool,
    },
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
    Doctor {
        /// Color diagnostic output. Auto respects NO_COLOR and redirected output.
        #[arg(long, value_enum, default_value = "auto")]
        color: doctor::Color,
    },
    /// Explain and speak the next blocker, then open the relevant settings.
    Assist {
        /// Print guidance without speech.
        #[arg(long)]
        silent: bool,
        /// Print guidance without speaking or opening settings.
        #[arg(long)]
        dry_run: bool,
    },
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
    IndicatorAction(indicator::Action),
    Action(String),
    SettingsChanged(settings::Change),
    QuitIfIdle(mpsc::Sender<bool>),
    RecordingStarted(u64, Result<audio::Start>),
    RecordingFinished(u64, Result<Vec<u8>>, Instant),
    InputsChanged(Instant, Result<audio::Inputs>),
    Finished(u64, Result<Outcome>),
    WorkerUnloaded,
}
enum Job {
    Prepare(Arc<AtomicBool>),
    Download(u64, Arc<AtomicBool>),
    Warm(u64, Arc<AtomicBool>),
    Transcribe(u64, Vec<u8>, Arc<AtomicBool>, Instant),
}

enum Outcome {
    ModelLoaded,
    ModelDownloaded,
    Transcript(String, Instant),
    Delivered,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkKind {
    Warm,
    Download,
    FinalizeAudio,
    Transcribe,
    Deliver,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Completion {
    Microphone,
    Audio,
    Worker,
}

enum CancelAction {
    None,
    DiscardRecording,
    WaitMicrophone,
    WaitWorker,
}

enum State {
    Idle,
    Canceling {
        id: u64,
        canceled: Arc<AtomicBool>,
        awaiting: Completion,
    },
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
        kind: WorkKind,
    },
}

impl State {
    fn request_cancel(&mut self) -> CancelAction {
        let (id, canceled, awaiting, action) = match self {
            Self::Idle | Self::Canceling { .. } => return CancelAction::None,
            Self::Recording { canceled, .. } => {
                canceled.store(true, Ordering::Release);
                *self = Self::Idle;
                return CancelAction::DiscardRecording;
            }
            Self::Starting { id, canceled } => (
                *id,
                canceled.clone(),
                Completion::Microphone,
                CancelAction::WaitMicrophone,
            ),
            Self::Working { id, canceled, kind } => (
                *id,
                canceled.clone(),
                if *kind == WorkKind::FinalizeAudio {
                    Completion::Audio
                } else {
                    Completion::Worker
                },
                CancelAction::WaitWorker,
            ),
        };
        canceled.store(true, Ordering::Release);
        *self = Self::Canceling {
            id,
            canceled,
            awaiting,
        };
        action
    }

    fn audio_finalized(&mut self, id: u64) -> Option<Arc<AtomicBool>> {
        let Self::Working {
            id: current,
            canceled,
            kind: WorkKind::FinalizeAudio,
        } = self
        else {
            return None;
        };
        if *current != id {
            return None;
        }
        let canceled = canceled.clone();
        *self = Self::Working {
            id,
            canceled: canceled.clone(),
            kind: WorkKind::Transcribe,
        };
        Some(canceled)
    }

    fn accepts_result(&self, id: u64, result: &Result<Outcome>) -> bool {
        let Self::Working {
            id: current, kind, ..
        } = self
        else {
            return false;
        };
        *current == id
            && matches!(
                (kind, result),
                (WorkKind::Warm, Ok(Outcome::ModelLoaded))
                    | (WorkKind::Download, Ok(Outcome::ModelDownloaded))
                    | (WorkKind::Transcribe, Ok(Outcome::Transcript(..)))
                    | (WorkKind::Deliver, Ok(Outcome::Delivered))
                    | (
                        WorkKind::Warm
                            | WorkKind::Download
                            | WorkKind::Transcribe
                            | WorkKind::Deliver,
                        Err(_)
                    )
            )
    }

    fn finish_cancellation(&mut self, id: u64, completion: Completion) -> bool {
        if matches!(self, Self::Canceling { id: current, awaiting, .. } if *current == id && *awaiting == completion)
        {
            *self = Self::Idle;
            true
        } else {
            false
        }
    }

    fn activity(&self) -> readiness::Activity {
        use readiness::Activity;
        match self {
            Self::Idle => Activity::Idle,
            Self::Recording { .. } => Activity::Recording,
            Self::Canceling { .. } => Activity::Canceling,
            Self::Starting { .. } => Activity::OpeningMicrophone,
            Self::Working { kind, .. } => match kind {
                WorkKind::Warm => Activity::LoadingModel,
                WorkKind::Download => Activity::DownloadingModel,
                WorkKind::FinalizeAudio | WorkKind::Transcribe | WorkKind::Deliver => {
                    Activity::Transcribing
                }
            },
        }
    }

    fn indicator_phase(&self) -> indicator::Phase {
        match self {
            Self::Recording { .. } => indicator::Phase::Recording,
            Self::Working {
                kind: WorkKind::FinalizeAudio | WorkKind::Transcribe | WorkKind::Deliver,
                ..
            } => indicator::Phase::Transcribing,
            Self::Idle | Self::Starting { .. } | Self::Working { .. } | Self::Canceling { .. } => {
                indicator::Phase::Hidden
            }
        }
    }
}

fn main() {
    if let Err(error) = execute() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

fn execute() -> Result<()> {
    let cli = Cli::parse();
    let command = match cli.command {
        Some(Command::DownloadWorker { parent_pid }) => return download::worker(parent_pid),
        None if !bootstrap::bundled() => return bootstrap::launch(),
        Some(Command::Install {
            allow_permission_reset,
        }) => {
            let app = bootstrap::install(allow_permission_reset)?;
            println!("Installed {}. Run `nemo` to open it.", app.display());
            return Ok(());
        }
        Some(Command::Assist { silent, dry_run }) => {
            network::local_only()?;
            return assist::run(silent, dry_run);
        }
        Some(Command::PreviewIndicator { output }) => {
            network::local_only()?;
            return indicator::preview(output.as_deref());
        }
        Some(Command::PrepareApp {
            root,
            binary,
            destination,
            allow_permission_reset,
        }) => return bootstrap::prepare(root, binary, destination, allow_permission_reset),
        None => Command::Run,
        Some(command) => command,
    };
    let mut downloader = if matches!(
        command,
        Command::Model {
            command: ModelCommand::Download
        }
    ) {
        Some(download::Downloader::spawn()?)
    } else {
        None
    };
    if !matches!(command, Command::Run) {
        network::local_only()?;
    }
    let mut config = Config::load()?;
    match command {
        Command::DownloadWorker { .. }
        | Command::Install { .. }
        | Command::Assist { .. }
        | Command::PreviewIndicator { .. }
        | Command::PrepareApp { .. } => {
            unreachable!()
        }
        Command::Run => run(config),
        Command::Toggle => remote("toggle"),
        Command::Cancel => remote("cancel"),
        Command::Status => remote("status"),
        Command::Identity => remote("identity"),
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
            ctrlc::set_handler(move || stopping.store(true, Ordering::Release))?;
            match command {
                ModelCommand::Download => downloader.as_mut().unwrap().download(&canceled)?,
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
                output::deliver(text, paste, &AtomicBool::new(false))?;
            }
            println!("{text}");
            Ok(())
        }
        Command::Doctor { color } => {
            doctor::run(&config, color);
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

fn startup_action(
    config: &Config,
    bundled: bool,
    managed_model: &Path,
    managed_ready: bool,
) -> Option<&'static str> {
    if bundled && config.model == managed_model && !managed_ready {
        Some("download")
    } else if config.preload_model && config.check_assets().is_ok() {
        Some("warm")
    } else {
        None
    }
}

fn start_recording(
    proxy: tao::event_loop::EventLoopProxy<Event>,
    id: u64,
    seconds: u64,
    microphone: Option<audio::Microphone>,
) {
    let requested = Instant::now();
    thread::spawn(move || {
        let result = audio::Recording::start(seconds, microphone.as_ref());
        eprintln!(
            "{}",
            serde_json::json!({
                "event": "capture_start",
                "microphone_start_ms": requested.elapsed().as_secs_f64() * 1000.0,
                "recording_started": matches!(&result, Ok(audio::Start::Recording(_))),
                "success": result.is_ok(),
            })
        );
        let _ = proxy.send_event(Event::RecordingStarted(id, result));
    });
}

fn run(mut config: Config) -> Result<()> {
    let server = control::Server::bind()?;
    let store = model::Store::managed();
    let mut downloader = if config.model != store.decoder() || store.ready() {
        None
    } else {
        Some(download::Downloader::spawn()?)
    };
    network::local_only()?;
    fs::create_dir_all(config::data_dir())?;
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(config::data_dir().join("daemon.log"))?;
    use std::os::unix::fs::PermissionsExt;
    log.set_permissions(fs::Permissions::from_mode(0o600))?;
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
    let status_item = MenuItem::new("Idle", false, None);
    let readiness_item = MenuItem::new("Checking readiness…", false, None);
    let toggle_item = MenuItem::new(format!("Record ({})", config.hotkey), true, None);
    let cancel_item = MenuItem::new("Cancel", false, None);
    let warm_item = MenuItem::new("Load model", true, None);
    let download_item = MenuItem::new("Download model (21.35 GB)", true, None);
    let settings_item = MenuItem::new("Settings…", true, None);
    let config_item = MenuItem::new("Open configuration", true, None);
    let permissions_item = MenuItem::new("Accessibility settings", true, None);
    let input_item = MenuItem::new("Checking microphones…", false, None);
    let microphone_item = MenuItem::new("Microphone settings", true, None);
    let quit_item = MenuItem::new("Quit", true, None);
    menu.append_items(&[
        &readiness_item,
        &status_item,
        &input_item,
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
        .with_tooltip("Nemo — Checking readiness")
        .build()?;
    let readiness_id = readiness_item.id().clone();
    let toggle_id = toggle_item.id().clone();
    let cancel_id = cancel_item.id().clone();
    let warm_id = warm_item.id().clone();
    let download_id = download_item.id().clone();
    let settings_id = settings_item.id().clone();
    let config_id = config_item.id().clone();
    let permissions_id = permissions_item.id().clone();
    let microphone_id = microphone_item.id().clone();
    let quit_id = quit_item.id().clone();
    let readiness = Arc::new(Mutex::new(readiness::Readiness::checking(&config.hotkey)));
    let menu_readiness = readiness.clone();
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let action = if event.id == readiness_id {
            let snapshot = menu_readiness.lock().unwrap();
            let Some(action) = snapshot
                .blockers
                .first()
                .and_then(readiness::Blocker::action)
            else {
                return;
            };
            action
        } else if event.id == toggle_id {
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

    let status = Arc::new(Mutex::new(String::from("Idle")));
    let executable_sha256 = nemo_build::sha256(&std::env::current_exe()?)?;
    let socket_status = status.clone();
    let socket_readiness = readiness.clone();
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
                let snapshot = socket_readiness.lock().unwrap();
                format!("{} · {}", snapshot.summary(), socket_status.lock().unwrap())
            } else if command == "readiness" {
                serde_json::to_string(&*socket_readiness.lock().unwrap()).unwrap()
            } else if command == "permissions" {
                serde_json::to_string(&permissions::Snapshot::current()).unwrap()
            } else if command == "identity" {
                serde_json::json!({
                    "pid": std::process::id(),
                    "executable": std::env::current_exe().ok(),
                    "sha256": executable_sha256,
                })
                .to_string()
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
                            let result = match &mut downloader {
                                Some(downloader) => downloader.download(&canceled),
                                None => model::Store::managed()
                        .verify_state(&canceled)
                        .and_then(|state| match state.is_usable() {
                            true => Ok(()),
                            false => anyhow::bail!(
                                "{}",
                                state.reason().unwrap_or_else(|| "weights unusable".into())
                            ),
                        }).map_err(|error| error.context("Repair missing or damaged weights with `nemo model download`, then reopen Nemo")),
                            };
                            if downloader
                                .as_ref()
                                .is_some_and(download::Downloader::retired)
                            {
                                downloader.take();
                            }
                            (id, result.map(|_| Outcome::ModelDownloaded))
                        }
                        Job::Transcribe(id, wav, canceled, stopped) => {
                            eprintln!(
                                "{}",
                                serde_json::json!({
                                    "event": "inference_started",
                                    "stop_to_inference_ms": stopped.elapsed().as_secs_f64() * 1000.0,
                                })
                            );
                            (
                                id,
                                backend
                                    .transcribe(wav, &canceled)
                                    .map(|text| Outcome::Transcript(text, stopped)),
                            )
                        }
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
    let indicator_proxy = event_loop.create_proxy();
    let mut indicator = indicator::Indicator::new(move |action| {
        let _ = indicator_proxy.send_event(Event::IndicatorAction(action));
    });
    let mut state = State::Idle;
    let recording_proxy = event_loop.create_proxy();
    let mut next_id = 0;
    let mut last_toggle = Instant::now() - Duration::from_secs(1);
    let shortcut = config.hotkey.clone();
    let render_controls = move |state: &State,
                                settings: &settings::Settings,
                                inputs: &audio::Inputs,
                                config: &Config| {
        let active = match state {
            State::Recording { audio, .. } => Some(&audio.microphone),
            _ => None,
        };
        settings.set_inputs(
            inputs,
            config.microphone.as_ref(),
            active,
            !matches!(state, State::Idle),
        );
        input_item.set_text(if let Some(active) = active {
            format!("Input: {}", active.name)
        } else {
            match inputs.resolve(config.microphone.as_ref()) {
                Ok((device, false)) => format!("Input: {}", device.name),
                Ok((_, true)) => {
                    format!("{} unavailable", config.microphone.as_ref().unwrap().name)
                }
                Err(_) => "No microphone available".into(),
            }
        });

        let recording = matches!(state, State::Recording { .. });
        toggle_item.set_text(if recording {
            "Stop recording".into()
        } else {
            format!("Record ({shortcut})")
        });
        toggle_item.set_enabled(!matches!(
            state,
            State::Working { .. } | State::Starting { .. } | State::Canceling { .. }
        ));
        cancel_item.set_enabled(!matches!(state, State::Idle));
        warm_item.set_enabled(matches!(state, State::Idle));
        download_item.set_enabled(matches!(state, State::Idle));
    };
    let mut animate_indicator = move |state: &State| {
        indicator.set_phase(state.indicator_phase());
        indicator.animate(|| match state {
            State::Recording { audio, .. } => audio.take_level(),
            _ => 0.0,
        })
    };
    let set_status = move |message: &str| {
        let normalized = message.replace('\n', " ");
        let mut current = status.lock().unwrap();
        if *current == normalized {
            return;
        }
        *current = normalized;
        drop(current);
        eprintln!("{message}");
        status_item.set_text(message.chars().take(100).collect::<String>());
    };

    let mut inputs = audio::Inputs::default();
    let mut scanning_inputs = false;
    let input_proxy = event_loop.create_proxy();
    let mut next_permissions_check = Instant::now();
    let mut observed_permissions = permissions::Snapshot::current();
    let mut input_readiness = readiness::Input::Checking;
    let render_readiness =
        move |state: &State, permissions, registered, input: &readiness::Input, config: &Config| {
            let model_present = config.check_model_assets().is_ok();
            let snapshot = readiness::Readiness::observe(
                state.activity(),
                &config.hotkey,
                permissions,
                registered,
                input,
                model_present,
                config.runtime_path().is_file(),
            );
            let mut previous = readiness.lock().unwrap();
            if *previous != snapshot {
                let summary = snapshot.summary();
                eprintln!("Readiness: {summary}");
                readiness_item.set_text(&summary);
                readiness_item.set_enabled(
                    snapshot
                        .blockers
                        .first()
                        .and_then(readiness::Blocker::action)
                        .is_some(),
                );
                let _ = tray.set_icon(Some(icon(snapshot.color())));
                let _ = tray.set_tooltip(Some(format!(
                    "Nemo — {} · {}",
                    snapshot.activity.label(),
                    summary
                )));
                *previous = snapshot;
            }
        };
    let store = model::Store::managed();
    if let Some(action) = startup_action(
        &config,
        bootstrap::bundled(),
        &store.decoder(),
        store.ready(),
    ) {
        let _ = event_loop
            .create_proxy()
            .send_event(Event::Action(action.into()));
    }
    event_loop.run(move |event, _, flow| {
        let _keep_alive = (&server, &hotkeys);
        if Instant::now() >= next_permissions_check {
            if !scanning_inputs {
                scanning_inputs = true;
                let proxy = input_proxy.clone();
                thread::spawn(move || {
                    let scanned_at = Instant::now();
                    let _ = proxy
                        .send_event(Event::InputsChanged(scanned_at, audio::Inputs::current()));
                });
            }
            if let State::Recording {
                audio, canceled, ..
            } = &state
                && let Some(error) = audio.error()
            {
                canceled.store(true, Ordering::Release);
                state = State::Idle;
                set_status(&format!("Microphone failed: {error}. Recording discarded"));
                output::sound("Basso");
            }

            if settings.window.is_visible() {
                settings.refresh_startup();
            }
            let observed = permissions::Snapshot::current();
            observed_permissions = observed;
            let permissions = observed;
            if !permissions.shortcut_allowed(&config.hotkey) {
                hotkeys = None;
            } else if hotkeys.is_none() {
                let proxy = shortcut_proxy.clone();
                match shortcut::Binding::register(&config.hotkey, move || {
                    let _ = proxy.send_event(Event::Action("toggle".into()));
                }) {
                    Ok(binding) => {
                        hotkeys = Some(binding);
                        eprintln!("Shortcut registered: {}", config.hotkey);
                    }
                    Err(error) => eprintln!("Shortcut unavailable: {error:#}"),
                }
            }
            next_permissions_check = Instant::now() + Duration::from_secs(2);
        }
        let action = match event {
            TaoEvent::UserEvent(Event::IndicatorAction(action)) => {
                action.command(state.indicator_phase()).map(str::to_owned)
            }
            TaoEvent::UserEvent(Event::InputsChanged(scanned_at, result)) => {
                scanning_inputs = false;
                match result {
                    Ok(observed) => {
                        if let State::Recording {
                            audio,
                            started,
                            canceled,
                        } = &state
                            && scanned_at >= *started
                            && !observed.devices.iter().any(|d| d.id == audio.microphone.id)
                        {
                            canceled.store(true, Ordering::Release);
                            let message = format!(
                                "{} disconnected. Recording discarded",
                                audio.microphone.name
                            );
                            state = State::Idle;
                            set_status(&message);
                            output::sound("Basso");
                        }
                        input_readiness = match observed.resolve(config.microphone.as_ref()) {
                            Ok((_, false)) => readiness::Input::Available,
                            Ok((_, true)) => readiness::Input::NeedsConfirmation,
                            Err(_) => readiness::Input::Unavailable,
                        };
                        inputs = observed;
                    }
                    Err(error) => {
                        input_readiness = readiness::Input::Failed(format!("{error:#}"));
                        settings.set_error("Could not list microphones.");
                        eprintln!("Cannot list microphones: {error:#}");
                    }
                }
                None
            }
            TaoEvent::UserEvent(Event::SettingsChanged(settings::Change::Microphone(
                microphone,
            ))) => {
                if matches!(state, State::Idle) {
                    match Config::update(|saved| {
                        saved.microphone = microphone.clone();
                        Ok(())
                    }) {
                        Ok(_) => {
                            config.microphone = microphone;
                            input_readiness = match inputs.resolve(config.microphone.as_ref()) {
                                Ok((_, false)) => readiness::Input::Available,
                                Ok((_, true)) => readiness::Input::NeedsConfirmation,
                                Err(_) => readiness::Input::Unavailable,
                            };
                            settings.set_error("");
                        }
                        Err(error) => {
                            settings.set_error("Could not save microphone.");
                            eprintln!("Cannot save microphone: {error:#}");
                        }
                    }
                }
                None
            }

            TaoEvent::UserEvent(Event::QuitIfIdle(reply)) => {
                let idle = matches!(state, State::Idle);
                let _ = reply.send(idle);
                if idle {
                    process.shutdown();
                    *flow = ControlFlow::Exit;
                    return;
                }
                None
            }
            TaoEvent::WindowEvent {
                window_id,
                event: tao::event::WindowEvent::CloseRequested,
                ..
            } if window_id == settings.window.id() => {
                settings.window.set_visible(false);
                None
            }
            TaoEvent::UserEvent(Event::SettingsChanged(
                settings::Change::RemoveTrailingPeriod(enabled),
            )) => {
                match Config::update(|saved| {
                    saved.remove_trailing_period = enabled;
                    Ok(())
                }) {
                    Ok(_) => {
                        config.remove_trailing_period = enabled;
                        settings.set_error("");
                    }
                    Err(error) => {
                        settings.set_remove_trailing_period(config.remove_trailing_period);
                        settings.set_error("Could not save settings.");
                        eprintln!("Cannot save settings: {error:#}");
                    }
                }
                None
            }
            TaoEvent::UserEvent(Event::SettingsChanged(settings::Change::LaunchOnStartup(
                enabled,
            ))) => {
                match startup::set_enabled(enabled) {
                    Ok(()) => settings.set_error(""),
                    Err(error) => {
                        settings.set_error("Could not change launch on startup.");
                        eprintln!("Cannot change launch on startup: {error:#}");
                    }
                }
                settings.refresh_startup();
                None
            }
            TaoEvent::UserEvent(Event::SettingsChanged(settings::Change::SoundPack(pack))) => {
                match Config::update(|saved| {
                    saved.sound_pack = pack;
                    Ok(())
                }) {
                    Ok(_) => {
                        config.sound_pack = pack;
                        settings.set_error("");
                    }
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
                if state.finish_cancellation(id, Completion::Microphone) {
                    set_status("Canceled");
                }
                if let State::Starting {
                    id: current,
                    canceled,
                } = &state
                    && *current == id
                {
                    let canceled = canceled.clone();
                    state = State::Idle;
                    match result {
                            Ok(audio::Start::Recording(audio)) => {
                                let canceled = canceled.clone();
                                state = State::Recording {
                                    audio,
                                    started: Instant::now(),
                                    canceled: canceled.clone(),
                                };
                                set_status("Recording");
                                sounds.play(config.sound_pack, sounds::Cue::Start);
                            }
                            Ok(audio::Start::ConfirmFallback(fallback)) => {
                                let selected = config
                                    .microphone
                                    .as_ref()
                                    .map(|d| d.name.as_str())
                                    .unwrap_or("Microphone");
                                if settings.confirm_fallback(selected, &fallback) {
                                    state = State::Starting { id, canceled };
                                    let seconds = config.max_recording_seconds;
                                    let proxy = recording_proxy.clone();
                                    start_recording(proxy, id, seconds, Some(fallback));
                                } else {
                                    canceled.store(true, Ordering::Release);
                                    set_status("Canceled");
                                }
                            }
                            Err(error) => {
                                canceled.store(true, Ordering::Release);
                                state = State::Idle;
                                set_status(&format!("{error:#}"));
                                output::sound("Basso");
                            }
                    }
                }
                None
            }
            TaoEvent::NewEvents(tao::event::StartCause::ResumeTimeReached { .. }) => match &state {
                State::Recording { started, .. }
                    if started.elapsed() >= Duration::from_secs(config.max_recording_seconds) =>
                {
                    Some("toggle".into())
                }
                _ => None,
            },
            TaoEvent::UserEvent(Event::RecordingFinished(id, result, stopped)) => {
                if state.finish_cancellation(id, Completion::Audio) {
                    set_status("Canceled");
                }
                if let State::Working { id: current, canceled, kind: WorkKind::FinalizeAudio } = &state
                    && *current == id
                {
                    match result {
                            Ok(wav) => {
                                let canceled = state.audio_finalized(id).unwrap();
                                let _ = jobs.send(Job::Transcribe(id, wav, canceled, stopped));
                                sounds.play(config.sound_pack, sounds::Cue::Stop);
                            }
                            Err(error) => {
                                sounds.play(config.sound_pack, sounds::Cue::Stop);
                                canceled.store(true, Ordering::Release);
                                state = State::Idle;
                                set_status(&format!("{error:#}"));
                                output::sound("Basso");
                            }
                    }
                }
                None
            }
            TaoEvent::UserEvent(Event::Finished(id, result)) => {
                if state.finish_cancellation(id, Completion::Worker) {
                    set_status("Canceled");
                }
                if let State::Working { id: current, canceled, kind: WorkKind::Warm | WorkKind::Download | WorkKind::Transcribe | WorkKind::Deliver } = &state
                    && *current == id
                    && state.accepts_result(id, &result)
                {
                    let canceled = canceled.clone();
                    match result {
                            Ok(Outcome::Transcript(text, stopped)) => {
                                state = State::Working { id, canceled: canceled.clone(), kind: WorkKind::Deliver };
                                let proxy = recording_proxy.clone();
                                let paste = config.paste;
                                let remove_period = config.remove_trailing_period;
                                thread::spawn(move || {
                                    let started = Instant::now();
                                    let result = output::deliver(
                                        output::format_transcript(&text, remove_period), paste, &canceled,
                                    );
                                    eprintln!("{}", serde_json::json!({
                                        "event": "delivery",
                                        "delivery_ms": started.elapsed().as_secs_f64() * 1000.0,
                                        "stop_to_dispatch_ms": stopped.elapsed().as_secs_f64() * 1000.0,
                                        "success": result.is_ok(),
                                    }));
                                    let _ = proxy.send_event(Event::Finished(id, result.map(|_| Outcome::Delivered)));
                                });
                            }
                            other => {
                                state = State::Idle;
                                match other {
                                    Ok(Outcome::Delivered) => {
                                        set_status("Transcript delivered");
                                        sounds.play(config.sound_pack, sounds::Cue::Complete);
                                    }
                                    Ok(Outcome::ModelLoaded) => set_status("Model loaded"),
                                    Ok(Outcome::ModelDownloaded) => set_status("Model downloaded"),
                                    Err(error) => {
                                        set_status(&format!("{error:#}"));
                                        output::sound("Basso");
                                    }
                                    Ok(Outcome::Transcript(..)) => unreachable!(),
                                }
                            }
                    }
                }
                None
            }
            TaoEvent::UserEvent(Event::WorkerUnloaded) => {
                if matches!(state, State::Idle) {
                    set_status("Model unloaded");
                }
                None
            }
            _ => None,
        };
        render_controls(&state, &settings, &inputs, &config);
        render_readiness(
            &state,
            observed_permissions,
            hotkeys.is_some(),
            &input_readiness,
            &config,
        );
        let deadline = match &state {
            State::Recording { started, .. } => next_permissions_check
                .min(*started + Duration::from_secs(config.max_recording_seconds)),
            _ => next_permissions_check,
        };
        let frame = animate_indicator(&state);
        *flow = ControlFlow::WaitUntil(frame.map_or(deadline, |frame| frame.min(deadline)));
        let Some(action) = action else {
            return;
        };
        match action.as_str() {
            "quit" => {
                if let State::Working { canceled, .. }
                | State::Starting { canceled, .. }
                | State::Canceling { canceled, .. }
                | State::Recording { canceled, .. } = &state
                {
                    canceled.store(true, Ordering::Release);
                }
                process.shutdown();
                *flow = ControlFlow::Exit;
            }
            "settings" => {
                settings.show();
            }
            "config" => {
                if !config::path().exists() {
                    let _ = config.save();
                }
                let _ = std::process::Command::new("/usr/bin/open")
                    .arg("-e")
                    .arg(config::path())
                    .spawn();
            }
            "permissions" => {
                let _ = std::process::Command::new("/usr/bin/open")
                    .arg(readiness::ACCESSIBILITY_URL)
                    .spawn();
            }
            "microphone" => {
                let _ = std::process::Command::new("/usr/bin/open")
                    .arg(readiness::MICROPHONE_URL)
                    .spawn();
            }
            "cancel" => match state.request_cancel() {
                CancelAction::WaitWorker => {
                    process.stop();
                    set_status("Canceling");
                }
                CancelAction::DiscardRecording => {
                    output::sound("Pop");
                    set_status("Canceled");
                }
                CancelAction::WaitMicrophone => set_status("Canceling microphone request"),
                CancelAction::None => {}
            },
            "warm" if matches!(state, State::Idle) => {
                next_id += 1;
                let canceled = Arc::new(AtomicBool::new(false));
                let _ = jobs.send(Job::Warm(next_id, canceled.clone()));
                state = State::Working {
                    id: next_id,
                    canceled,
                    kind: WorkKind::Warm,
                };
                set_status("Loading model");
            }
            "download" if matches!(state, State::Idle) => {
                if config.model != model::Store::managed().decoder() {
                    set_status("Set model to the managed path before downloading");
                    return;
                }
                next_id += 1;
                let canceled = Arc::new(AtomicBool::new(false));
                let _ = jobs.send(Job::Download(next_id, canceled.clone()));
                state = State::Working {
                    id: next_id,
                    canceled,
                    kind: WorkKind::Download,
                };
                set_status("Downloading model");
            }
            "toggle" | "stop" => {
                if action == "stop" && !matches!(state, State::Recording { .. }) {
                    return;
                }
                if action == "toggle" && last_toggle.elapsed() < Duration::from_millis(200) {
                    return;
                }
                last_toggle = Instant::now();
                if matches!(state, State::Working { .. } | State::Starting { .. } | State::Canceling { .. }) {
                    return;
                }
                match std::mem::replace(&mut state, State::Idle) {
                    State::Idle => {
                        let permissions = permissions::Snapshot::current();
                        if !permissions.recording_allowed() {
                            set_status("Enable Microphone");
                            return;
                        }
                        next_id += 1;
                        let id = next_id;
                        let canceled = Arc::new(AtomicBool::new(false));
                        let _ = jobs.send(Job::Prepare(canceled.clone()));
                        state = State::Starting { id, canceled };
                        set_status("Opening microphone");
                        let proxy = recording_proxy.clone();
                        let seconds = config.max_recording_seconds;
                        let microphone = config.microphone.clone();
                        start_recording(proxy, id, seconds, microphone);
                    }
                    State::Recording {
                        audio, canceled, ..
                    } => {
                        let stopped = Instant::now();
                        next_id += 1;
                        let id = next_id;
                        let proxy = recording_proxy.clone();
                        thread::spawn(move || {
                            let recording = audio.finish();
                            eprintln!("{}", serde_json::json!({
                                "event": "audio_finalized",
                                "finalization_ms": stopped.elapsed().as_secs_f64() * 1000.0,
                                "success": recording.is_ok(),
                            }));
                            let _ = proxy.send_event(Event::RecordingFinished(id, recording, stopped));
                        });
                        state = State::Working {
                            id,
                            canceled,
                            kind: WorkKind::FinalizeAudio,
                        };
                        set_status("Transcribing");
                    }
                    State::Working { .. } | State::Starting { .. } | State::Canceling { .. } => unreachable!(),
                }
            }
            _ => {}
        }
        render_readiness(
            &state,
            observed_permissions,
            hotkeys.is_some(),
            &input_readiness,
            &config,
        );
        render_controls(
            if matches!(*flow, ControlFlow::Exit) {
                &State::Idle
            } else {
                &state
            },
            &settings,
            &inputs,
            &config,
        );
        let frame = animate_indicator(if matches!(*flow, ControlFlow::Exit) {
            &State::Idle
        } else {
            &state
        });
        if !matches!(*flow, ControlFlow::Exit) {
            let deadline = match &state {
                State::Recording { started, .. } => next_permissions_check
                    .min(*started + Duration::from_secs(config.max_recording_seconds)),
                _ => next_permissions_check,
            };
            *flow = ControlFlow::WaitUntil(frame.map_or(deadline, |frame| frame.min(deadline)));
        }
    });
}

#[cfg(test)]
mod indicator_tests {
    use super::*;
    use indicator::Phase;

    #[test]
    fn finalization_accepts_only_the_current_audio_once() {
        let signal = Arc::new(AtomicBool::new(false));
        let mut state = State::Working {
            id: 7,
            canceled: signal.clone(),
            kind: WorkKind::FinalizeAudio,
        };
        assert!(state.audio_finalized(6).is_none());
        assert!(matches!(
            state,
            State::Working {
                kind: WorkKind::FinalizeAudio,
                ..
            }
        ));
        assert!(Arc::ptr_eq(&state.audio_finalized(7).unwrap(), &signal));
        assert!(matches!(
            state,
            State::Working {
                kind: WorkKind::Transcribe,
                ..
            }
        ));
        assert!(state.audio_finalized(7).is_none());
        state.request_cancel();
        assert_eq!(state.activity(), readiness::Activity::Canceling);
        assert!(!state.finish_cancellation(7, Completion::Audio));
        assert!(state.finish_cancellation(7, Completion::Worker));
        assert_eq!(state.activity(), readiness::Activity::Idle);
    }

    #[test]
    fn worker_results_must_match_the_current_phase_and_attempt() {
        let mut state = State::Working {
            id: 4,
            canceled: Arc::new(AtomicBool::new(false)),
            kind: WorkKind::FinalizeAudio,
        };
        let transcript = Ok(Outcome::Transcript("hello".into(), Instant::now()));
        assert!(!state.accepts_result(4, &transcript));
        state.audio_finalized(4).unwrap();
        assert!(!state.accepts_result(3, &transcript));
        assert!(state.accepts_result(4, &transcript));
        assert!(!state.accepts_result(4, &Ok(Outcome::Delivered)));
        if let State::Working { kind, .. } = &mut state {
            *kind = WorkKind::Deliver;
        }
        assert!(!state.accepts_result(4, &transcript));
        assert!(state.accepts_result(4, &Ok(Outcome::Delivered)));
        state.request_cancel();
        assert!(!state.accepts_result(4, &Ok(Outcome::Delivered)));
    }

    #[test]
    fn cancellation_waits_for_its_own_operation_and_ignores_duplicates() {
        for (initial, completion) in [
            (None, Completion::Microphone),
            (Some(WorkKind::FinalizeAudio), Completion::Audio),
            (Some(WorkKind::Transcribe), Completion::Worker),
            (Some(WorkKind::Deliver), Completion::Worker),
            (Some(WorkKind::Warm), Completion::Worker),
            (Some(WorkKind::Download), Completion::Worker),
        ] {
            let canceled = Arc::new(AtomicBool::new(false));
            let mut state = match initial {
                None => State::Starting {
                    id: 9,
                    canceled: canceled.clone(),
                },
                Some(kind) => State::Working {
                    id: 9,
                    canceled: canceled.clone(),
                    kind,
                },
            };
            state.request_cancel();
            assert!(canceled.load(Ordering::Acquire));
            assert!(matches!(state.request_cancel(), CancelAction::None));
            assert!(!state.finish_cancellation(8, completion));
            assert_eq!(state.activity(), readiness::Activity::Canceling);
            assert!(state.audio_finalized(9).is_none());
            assert!(state.finish_cancellation(9, completion));
            assert!(!state.finish_cancellation(9, completion));
            assert!(matches!(state, State::Idle));
        }
    }

    #[test]
    fn only_transcription_shows_a_processing_indicator() {
        let state = |kind| State::Working {
            id: 1,
            canceled: Arc::new(AtomicBool::new(false)),
            kind,
        };
        assert_eq!(state(WorkKind::Warm).indicator_phase(), Phase::Hidden);
        assert_eq!(state(WorkKind::Download).indicator_phase(), Phase::Hidden);
        assert_eq!(
            state(WorkKind::Transcribe).indicator_phase(),
            Phase::Transcribing
        );
    }

    #[test]
    fn canceling_transcription_hides_the_indicator_before_the_worker_finishes() {
        let canceled = Arc::new(AtomicBool::new(false));
        let mut state = State::Working {
            id: 1,
            canceled: canceled.clone(),
            kind: WorkKind::Transcribe,
        };
        assert_eq!(state.indicator_phase(), Phase::Transcribing);
        state.request_cancel();
        assert!(canceled.load(Ordering::Acquire));
        assert_eq!(state.indicator_phase(), Phase::Hidden);
        assert_eq!(State::Idle.indicator_phase(), Phase::Hidden);
    }

    #[test]
    fn microphone_setup_does_not_claim_to_be_recording() {
        let canceled = Arc::new(AtomicBool::new(false));
        let mut state = State::Starting {
            id: 1,
            canceled: canceled.clone(),
        };
        assert_eq!(state.indicator_phase(), Phase::Hidden);
        state.request_cancel();
        assert!(canceled.load(Ordering::Acquire));
        assert_eq!(state.indicator_phase(), Phase::Hidden);
    }
}

#[cfg(test)]
mod startup_tests {
    use super::*;

    #[test]
    fn a_bundled_first_run_downloads_missing_managed_weights_before_preloading() {
        let root = tempfile::tempdir().unwrap();
        let config = Config {
            model: root.path().join("decoder.gguf"),
            runtime: root.path().join("nemo-speech"),
            preload_model: true,
            ..Config::default()
        };
        fs::write(&config.model, b"decoder").unwrap();
        fs::write(&config.runtime, b"runtime").unwrap();
        assert_eq!(
            startup_action(&config, true, &config.model, false),
            Some("download")
        );
        assert_eq!(
            startup_action(&config, true, &config.model, true),
            Some("warm")
        );
    }

    #[test]
    fn ready_models_and_explicit_unbundled_runs_do_not_download_at_startup() {
        let config = Config::default();
        assert_eq!(startup_action(&config, true, &config.model, true), None);
        assert_eq!(startup_action(&config, false, &config.model, false), None);
    }

    #[test]
    fn a_custom_model_is_never_replaced_by_an_automatic_download() {
        let root = tempfile::tempdir().unwrap();
        let config = Config {
            model: root.path().join("custom.gguf"),
            ..Config::default()
        };
        assert_eq!(
            startup_action(&config, true, &root.path().join("managed.gguf"), false),
            None
        );
    }
}
