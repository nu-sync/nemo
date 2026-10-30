use std::{
    fs,
    process::{Command, Output},
};

fn cli(root: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nemo"))
        .args(args)
        .env("NEMO_DATA_DIR", root.join("data"))
        .env("HOME", root.join("home"))
        .env_remove("NEMO_APP_PATH")
        .env_remove("NEMO_RUNTIME")
        .current_dir(root)
        .output()
        .unwrap()
}

#[test]
fn fresh_configuration_works_outside_the_checkout() {
    let root = tempfile::tempdir().unwrap();
    let output = cli(root.path(), &["config"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config: toml::Value = toml::from_str(&String::from_utf8(output.stdout).unwrap()).unwrap();
    let model = std::path::PathBuf::from(config["model"].as_str().unwrap());
    assert!(model.starts_with(root.path().join("data/models")));
    assert!(!config["runtime"].as_str().unwrap().contains("vendor"));
    assert_eq!(
        config["runtime"].as_str(),
        root.path()
            .join("home/Applications/Nemo.app/Contents/Resources/runtime/nemo-speech")
            .to_str()
    );
    assert!(config.get("remove_trailing_period").is_none());
    let path = cli(root.path(), &["model", "path"]);
    assert_eq!(
        String::from_utf8(path.stdout).unwrap().trim(),
        model.to_str().unwrap()
    );
}

#[test]
fn updating_a_shortcut_preserves_existing_model_and_preferences() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("data")).unwrap();
    let config_path = root.path().join("data/config.toml");
    fs::write(&config_path, "hotkey = 'Control+Shift+Space'\nmodel = '/custom/model.gguf'\nruntime = '/custom/nemo-speech'\nidle_unload_seconds = 17\nmax_recording_seconds = 42\npaste = false\nremove_trailing_period = true\nsound_pack = 'macos_recording'\n").unwrap();
    let output = cli(root.path(), &["config", "--hotkey", "RightOption"]);
    assert!(output.status.success());
    let config: toml::Value = toml::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(config["hotkey"].as_str(), Some("RightOption"));
    assert_eq!(config["model"].as_str(), Some("/custom/model.gguf"));
    assert_eq!(config["idle_unload_seconds"].as_integer(), Some(17));
    assert_eq!(config["paste"].as_bool(), Some(false));
    assert_eq!(config["remove_trailing_period"].as_bool(), Some(true));
    assert_eq!(config["sound_pack"].as_str(), Some("macos_recording"));
    for optional in [
        "preload_model",
        "model_warmup",
        "inference_threads",
        "encoder_batch_size",
    ] {
        assert!(
            config.get(optional).is_none(),
            "Default-only settings must not make an older app's config unreadable: {optional}"
        );
    }
    let before = fs::read(&config_path).unwrap();
    assert!(
        !cli(root.path(), &["config", "--hotkey", "invalid-key"])
            .status
            .success()
    );
    assert_eq!(fs::read(&config_path).unwrap(), before);
}

#[test]
fn permissions_can_be_inspected_without_a_microphone_stream_or_daemon() {
    let root = tempfile::tempdir().unwrap();
    let output = cli(root.path(), &["permissions"]);
    assert!(output.status.success());
    let permissions: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(permissions["accessibility"].is_boolean());
    assert!(
        ["authorized", "not_determined", "denied", "restricted"]
            .contains(&permissions["microphone"].as_str().unwrap())
    );
    assert!(!root.path().join("data/run/control.sock").exists());
}

#[test]
fn missing_model_verification_fails_without_downloading_or_creating_files() {
    let root = tempfile::tempdir().unwrap();
    assert!(!cli(root.path(), &["model", "verify"]).status.success());
    assert!(!root.path().join("data/models").exists());
}

#[test]
fn explicit_inspection_commands_never_bootstrap_an_application() {
    let root = tempfile::tempdir().unwrap();
    let app = root.path().join("Nemo.app");
    for arguments in [
        vec!["--help"],
        vec!["config"],
        vec!["permissions"],
        vec!["model", "path"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_nemo"))
            .args(arguments)
            .env("NEMO_DATA_DIR", root.path().join("data"))
            .env("NEMO_APP_PATH", &app)
            .current_dir(root.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!app.exists());
        assert!(!root.path().join("data/runtime-builds").exists());
        assert!(!root.path().join("data/models").exists());
    }
}

#[test]
fn configuration_uses_an_explicit_app_destination_before_it_is_installed() {
    let root = tempfile::tempdir().unwrap();
    let app = root.path().join("Nemo.app");
    let output = Command::new(env!("CARGO_BIN_EXE_nemo"))
        .arg("config")
        .env("NEMO_DATA_DIR", root.path().join("data"))
        .env("NEMO_APP_PATH", &app)
        .env_remove("NEMO_RUNTIME")
        .output()
        .unwrap();
    assert!(output.status.success());
    let config: toml::Value = toml::from_str(&String::from_utf8(output.stdout).unwrap()).unwrap();
    assert_eq!(
        config["runtime"].as_str(),
        app.join("Contents/Resources/runtime/nemo-speech").to_str()
    );
    assert!(!app.exists());
}

#[test]
fn invalid_bootstrap_paths_fail_before_creating_setup_state() {
    let root = tempfile::tempdir().unwrap();
    for (data, app) in [
        (
            root.path().join("data"),
            std::path::PathBuf::from("relative/Nemo.app"),
        ),
        (
            std::path::PathBuf::from("relative-data"),
            root.path().join("Nemo.app"),
        ),
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_nemo"))
            .env("NEMO_DATA_DIR", data)
            .env("NEMO_APP_PATH", app)
            .current_dir(root.path())
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("absolute path"));
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn installer_quit_requires_an_explicit_idle_acknowledgement() {
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixListener,
        thread,
    };
    for (response, expected_success) in [
        ("accepted", true),
        ("busy", false),
        ("unknown command", false),
    ] {
        let root = tempfile::tempdir().unwrap();
        let run = root.path().join("data/run");
        fs::create_dir_all(&run).unwrap();
        let listener = UnixListener::bind(run.join("control.sock")).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut command = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut command)
                .unwrap();
            assert_eq!(command.trim(), "quit-idle");
            writeln!(stream, "{response}").unwrap();
        });
        let output = cli(root.path(), &["quit", "--if-idle"]);
        server.join().unwrap();
        assert_eq!(output.status.success(), expected_success);
    }
}

#[test]
#[ignore = "requires a graphical macOS session; silent daemon startup only"]
fn daemon_identity_keeps_the_startup_fingerprint_after_the_file_is_replaced() {
    use std::{
        process::Stdio,
        thread,
        time::{Duration, Instant},
    };
    struct Daemon(std::process::Child);
    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join("nemo");
    fs::copy(env!("CARGO_BIN_EXE_nemo"), &binary).unwrap();
    let expected = nemo_build::sha256(&binary).unwrap();
    let mut daemon = Daemon(
        Command::new(&binary)
            .arg("run")
            .env("NEMO_DATA_DIR", root.path().join("data"))
            .env("NEMO_APP_PATH", root.path().join("Nemo.app"))
            .env_remove("NEMO_RUNTIME")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let identity = loop {
        let response = cli(root.path(), &["identity"]);
        if response.status.success() {
            break serde_json::from_slice::<serde_json::Value>(&response.stdout).unwrap();
        }
        assert!(
            daemon.0.try_wait().unwrap().is_none(),
            "daemon exited during startup"
        );
        assert!(Instant::now() < deadline, "daemon did not start");
        thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(identity["pid"].as_u64(), Some(daemon.0.id() as u64));
    assert_eq!(identity["sha256"], expected);
    let replacement = root.path().join("replacement");
    fs::copy("/usr/bin/true", &replacement).unwrap();
    fs::rename(replacement, &binary).unwrap();
    assert_ne!(nemo_build::sha256(&binary).unwrap(), expected);
    let response = cli(root.path(), &["identity"]);
    assert!(response.status.success());
    let identity: serde_json::Value = serde_json::from_slice(&response.stdout).unwrap();
    assert_eq!(identity["sha256"], expected);
    assert_eq!(identity["pid"].as_u64(), Some(daemon.0.id() as u64));
}

#[test]
fn microphone_preference_survives_process_restarts_and_other_setting_updates() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("data")).unwrap();
    let path = root.path().join("data/config.toml");
    fs::write(
        &path,
        "[microphone]\nid = 'coreaudio:headset-uid'\nname = 'USB Headset'\n",
    )
    .unwrap();
    for args in [
        vec!["config"],
        vec!["config", "--hotkey", "RightOption"],
        vec!["config"],
    ] {
        let output = cli(root.path(), &args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let config: toml::Value =
            toml::from_str(&String::from_utf8(output.stdout).unwrap()).unwrap();
        assert_eq!(
            config["microphone"]["id"].as_str(),
            Some("coreaudio:headset-uid")
        );
        assert_eq!(config["microphone"]["name"].as_str(), Some("USB Headset"));
    }
    fs::write(&path, "").unwrap();
    let output = cli(root.path(), &["config"]);
    let config: toml::Value = toml::from_str(&String::from_utf8(output.stdout).unwrap()).unwrap();
    assert!(config.get("microphone").is_none());
}
