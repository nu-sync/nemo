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

pub fn inference_command(runtime: &std::path::Path) -> Command {
    let mut command = Command::new("/usr/bin/sandbox-exec");
    command.args(["-p", LOCAL_ONLY]).arg(runtime).env_clear();
    command
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
