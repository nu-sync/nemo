use anyhow::{Result, bail};
use std::{
    ffi::{CStr, CString},
    process::Command,
};

pub const LOCAL_ONLY: &str = include_str!("../scripts/local-only.sb");

#[link(name = "sandbox")]
unsafe extern "C" {
    fn sandbox_init(
        profile: *const libc::c_char,
        flags: u64,
        error: *mut *mut libc::c_char,
    ) -> libc::c_int;
    fn sandbox_free_error(error: *mut libc::c_char);
}

pub fn local_only() -> Result<()> {
    apply(LOCAL_ONLY)
}

pub fn download_only() -> Result<()> {
    let data = crate::config::data_dir();
    anyhow::ensure!(data.is_absolute(), "NEMO_DATA_DIR must be absolute");
    std::fs::create_dir_all(data.join("models"))?;
    let path = |path: &std::path::Path| serde_json::to_string(&path.to_string_lossy()).unwrap();
    apply(&format!(
        "(version 1)(allow default)(deny network*)(deny device-microphone)(deny process-exec)\n\
         (allow network-outbound (remote tcp \"*:443\"))\n\
         (allow network* (local unix-socket)(remote unix-socket))\n\
         (deny file-write*)(allow file-write* (subpath {})(literal \"/dev/null\"))\n\
         (deny file-read-data)\n\
         (allow file-read-data (literal \"/\")(subpath \"/System\")(subpath \"/usr/bin\")\n\
         (subpath \"/usr/lib\")(subpath \"/usr/share\")(subpath \"/private/etc/ssl\")\n\
         (literal \"/private/etc/hosts\")(literal \"/private/etc/resolv.conf\")\n\
         (literal \"/private/etc/services\")(literal \"/private/etc/protocols\")(literal \"/private/etc/localtime\")\n\
         (subpath \"/dev\")(subpath {})(literal {}))",
        path(&data.join("models").canonicalize()?),
        path(&data.join("models").canonicalize()?),
        path(&std::env::current_exe()?.canonicalize()?),
    ))
}

fn apply(profile: &str) -> Result<()> {
    let profile = CString::new(profile)?;
    let mut error = std::ptr::null_mut();
    if unsafe { sandbox_init(profile.as_ptr(), 0, &mut error) } != 0 {
        let message = if error.is_null() {
            "unknown sandbox error".into()
        } else {
            let message = unsafe { CStr::from_ptr(error) }
                .to_string_lossy()
                .into_owned();
            unsafe { sandbox_free_error(error) };
            message
        };
        bail!("Cannot enforce local-only networking: {message}");
    }
    Ok(())
}

/// The bundled runtime needs no environment at all.
pub fn inference_command(runtime: &std::path::Path) -> Command {
    let mut command = Command::new("/usr/bin/sandbox-exec");
    command.args(["-p", LOCAL_ONLY]).arg(runtime).env_clear();
    command
}

/// An out-of-tree engine helper is a script, so its launcher has to find an
/// interpreter. It gets the smallest environment that can do that and nothing else:
/// inheriting this process's environment would hand the engine every token and API key
/// the user has exported.
pub fn helper_inference_command(runtime: &std::path::Path) -> Command {
    let mut command = inference_command(runtime);
    command.env("PATH", HELPER_PATH);
    if let Some(home) = std::env::var_os("HOME") {
        command.env("HOME", home);
    }
    if let Some(cache) = std::env::var_os("UV_CACHE_DIR") {
        command.env("UV_CACHE_DIR", cache);
    }
    command
}

/// Fixed rather than inherited, so the engine cannot be steered by the caller's PATH.
const HELPER_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

#[cfg(test)]
mod helper_env_tests {
    use super::*;

    #[test]
    fn the_bundled_runtime_gets_no_environment_at_all() {
        let command = inference_command(std::path::Path::new("/runtime/nemo-speech"));
        assert_eq!(command.get_envs().count(), 0);
    }

    #[test]
    fn a_helper_gets_a_fixed_path_and_never_the_parent_environment() {
        // Planted to prove it is not inherited.
        unsafe { std::env::set_var("NEMO_TEST_SECRET", "sk-do-not-leak") };
        let home = std::env::var_os("HOME");
        let command = helper_inference_command(std::path::Path::new("/runtime/helper.py"));
        let pairs: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)> = command
            .get_envs()
            .map(|(key, value)| (key.to_os_string(), value.map(|v| v.to_os_string())))
            .collect();

        let keys: Vec<String> = pairs
            .iter()
            .map(|(key, _)| key.to_string_lossy().into_owned())
            .collect();
        assert!(keys.contains(&"PATH".to_string()), "{keys:?}");
        assert!(
            !keys.contains(&"NEMO_TEST_SECRET".to_string()),
            "the parent environment must not be inherited: {keys:?}"
        );
        assert!(!keys.iter().any(|key| key.contains("TOKEN")), "{keys:?}");

        let path = pairs
            .iter()
            .find(|(key, _)| key == "PATH")
            .and_then(|(_, value)| value.clone())
            .expect("PATH is set");
        assert_eq!(path.to_string_lossy(), HELPER_PATH);

        if let Some(home) = home {
            assert!(keys.contains(&"HOME".to_string()));
            assert!(command.get_envs().count() >= 2, "{home:?}");
        }
        unsafe { std::env::remove_var("NEMO_TEST_SECRET") };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, net::TcpListener, thread};

    #[test]
    fn download_helper_cannot_read_private_audio() {
        const MARKER: &str = "NEMO_DOWNLOAD_SANDBOX_TEST";
        if let Some(secret) = std::env::var_os(MARKER) {
            download_only().unwrap();
            assert_eq!(
                Command::new("/usr/bin/true")
                    .status()
                    .unwrap_err()
                    .raw_os_error(),
                Some(libc::EPERM)
            );
            assert_eq!(
                std::fs::read(crate::config::data_dir().join("models/fixture")).unwrap(),
                b"model"
            );
            assert_eq!(
                std::fs::read(secret).unwrap_err().raw_os_error(),
                Some(libc::EPERM)
            );
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        std::fs::create_dir_all(data.join("models")).unwrap();
        std::fs::write(data.join("models/fixture"), b"model").unwrap();
        let secret = root.path().join("private-recording.wav");
        std::fs::write(&secret, b"private synthetic fixture").unwrap();
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "network::tests::download_helper_cannot_read_private_audio",
                "--nocapture",
            ])
            .env(MARKER, secret)
            .env("NEMO_DATA_DIR", data)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    fn controller_rejects_external_connections() {
        const MARKER: &str = "NEMO_SANDBOX_TEST_CHILD";
        if std::env::var_os(MARKER).is_some() {
            local_only().unwrap();
            let address = "1.1.1.1:443".parse().unwrap();
            let error =
                std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_secs(2))
                    .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(libc::EPERM));
            return;
        }
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "network::tests::controller_rejects_external_connections",
                "--nocapture",
            ])
            .env(MARKER, "1")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    fn inference_can_reach_loopback_but_cannot_connect_externally() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .unwrap();
        });
        let local = inference_command(std::path::Path::new("/usr/bin/curl"))
            .args([
                "--disable",
                "--noproxy",
                "*",
                "--max-time",
                "5",
                &format!("http://{address}"),
            ])
            .output()
            .unwrap();
        assert!(
            local.status.success(),
            "{}",
            String::from_utf8_lossy(&local.stderr)
        );
        assert_eq!(local.stdout, b"ok");
        server.join().unwrap();
        let remote = inference_command(std::path::Path::new("/usr/bin/curl"))
            .args([
                "--disable",
                "--noproxy",
                "*",
                "--max-time",
                "5",
                "http://192.0.2.1",
            ])
            .output()
            .unwrap();
        assert!(!remote.status.success());
        assert!(
            String::from_utf8_lossy(&remote.stderr).contains("after 0 ms")
                || remote.status.code() == Some(7)
        );
    }
}
