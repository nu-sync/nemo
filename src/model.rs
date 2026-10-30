use crate::{backend::Process, config, storage};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Deserialize)]
pub struct Asset {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Deserialize)]
pub struct Manifest {
    pub repository: String,
    pub revision: String,
    pub files: Vec<Asset>,
}

pub fn manifest() -> &'static Manifest {
    static MANIFEST: OnceLock<Manifest> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        serde_json::from_str(include_str!("../models/manifest.json"))
            .expect("valid embedded model manifest")
    })
}

pub struct Store {
    pub directory: PathBuf,
    manifest: Manifest,
}

impl Store {
    pub fn managed() -> Self {
        Self::at(&config::data_dir())
    }

    pub fn at(data: &Path) -> Self {
        Self {
            directory: data.join("models").join(&manifest().revision),
            manifest: manifest().clone(),
        }
    }

    pub fn decoder(&self) -> PathBuf {
        self.directory.join(&self.manifest.files[0].name)
    }

    pub fn ready(&self) -> bool {
        self.manifest.files.iter().all(|asset| {
            fs::metadata(self.directory.join(&asset.name))
                .is_ok_and(|m| m.is_file() && m.len() == asset.bytes)
        })
    }

    pub fn verify(&self, canceled: &AtomicBool) -> Result<()> {
        for asset in &self.manifest.files {
            verify_file(&self.directory.join(&asset.name), asset, canceled)?;
        }
        Ok(())
    }

    pub fn download(&self, process: &Process, canceled: &AtomicBool) -> Result<()> {
        fs::create_dir_all(&self.directory)?;
        let _lock = storage::Lock::acquire(&self.directory.join("install.lock"))?;
        for asset in &self.manifest.files {
            let url = format!(
                "https://huggingface.co/{}/resolve/{}/{}",
                self.manifest.repository, self.manifest.revision, asset.name
            );
            download_asset(&self.directory, asset, &url, process, canceled, 8)?;
        }
        Ok(())
    }

    pub fn migrate(&self, legacy: &Path, canceled: &AtomicBool) -> Result<()> {
        fs::create_dir_all(&self.directory)?;
        let _lock = storage::Lock::acquire(&self.directory.join("install.lock"))?;
        let legacy = legacy
            .canonicalize()
            .context("Legacy models directory does not exist")?;
        ensure!(
            legacy != self.directory.canonicalize()?,
            "Models are already in the managed directory"
        );
        for asset in &self.manifest.files {
            let target = self.directory.join(&asset.name);
            let source = legacy.join(&asset.name);
            if target.exists() {
                verify_file(&target, asset, canceled)?;
            }
            if source.exists() {
                verify_file(&source, asset, canceled)?;
            }
            ensure!(
                target.exists() || source.exists(),
                "Missing model {}",
                source.display()
            );
        }
        for asset in &self.manifest.files {
            ensure!(!canceled.load(Ordering::Acquire), "Canceled");
            let source = legacy.join(&asset.name);
            let target = self.directory.join(&asset.name);
            if !target.exists() {
                let temporary = storage::temporary(&target);
                let copied = (|| {
                    if fs::hard_link(&source, &temporary).is_err() {
                        let mut input = File::open(&source)?;
                        let mut output = File::create(&temporary)?;
                        let mut buffer = vec![0; 1024 * 1024];
                        loop {
                            ensure!(!canceled.load(Ordering::Acquire), "Canceled");
                            let read = input.read(&mut buffer)?;
                            if read == 0 {
                                break;
                            }
                            std::io::Write::write_all(&mut output, &buffer[..read])?;
                        }
                        output.sync_all()?;
                        verify_file(&temporary, asset, canceled)?;
                    }
                    fs::rename(&temporary, &target)?;
                    Ok(())
                })();
                if copied.is_err() {
                    let _ = fs::remove_file(&temporary);
                }
                copied?;
            }
            if source.canonicalize().ok().as_ref() != Some(&target.canonicalize()?) {
                let alias = storage::temporary(&source);
                symlink(&target, &alias)?;
                if let Err(error) = fs::rename(&alias, &source) {
                    let _ = fs::remove_file(alias);
                    return Err(error.into());
                }
            }
        }
        Ok(())
    }
}

fn publish(partial: &Path, target: &Path, asset: &Asset, canceled: &AtomicBool) -> Result<()> {
    if let Err(error) = verify_file(partial, asset, canceled) {
        if !canceled.load(Ordering::Acquire) {
            let _ = fs::remove_file(partial);
        }
        return Err(error.context("Download was not published; retry to download a fresh copy"));
    }
    File::open(partial)?.sync_all()?;
    fs::set_permissions(partial, fs::Permissions::from_mode(0o444))?;
    fs::rename(partial, target)?;
    Ok(())
}

fn download_asset(
    directory: &Path,
    asset: &Asset,
    url: &str,
    process: &Process,
    canceled: &AtomicBool,
    retries: u8,
) -> Result<()> {
    let target = directory.join(&asset.name);
    if verify_file(&target, asset, canceled).is_ok() {
        return Ok(());
    }
    ensure!(!canceled.load(Ordering::Acquire), "Canceled");
    let partial = target.with_extension("gguf.part");
    ensure!(
        !fs::symlink_metadata(&partial).is_ok_and(|m| m.file_type().is_symlink()),
        "Refusing a symlink at {}",
        partial.display()
    );
    if verify_file(&partial, asset, canceled).is_ok() {
        return publish(&partial, &target, asset, canceled);
    }
    if fs::metadata(&partial).is_ok_and(|m| m.len() >= asset.bytes) {
        fs::remove_file(&partial)?;
    }
    ensure!(!canceled.load(Ordering::Acquire), "Canceled");
    let url = resolve_download_url(directory, url, process, canceled)?;
    let mut command = download_command();
    command
        .args([
            "--fail",
            "--retry",
            &retries.to_string(),
            "--retry-delay",
            "2",
            "--connect-timeout",
            "30",
            "--speed-limit",
            "1024",
            "--speed-time",
            "60",
            "--continue-at",
            "-",
            "--output",
        ])
        .arg(&partial)
        .args(["--url", &url])
        .stdin(Stdio::null());
    process.run(&mut command, canceled)?;
    publish(&partial, &target, asset, canceled)?;
    eprintln!("Verified {}", asset.name);
    Ok(())
}

fn download_command() -> Command {
    let mut command = Command::new("/usr/bin/curl");
    command
        .env_clear()
        .args(["--disable", "--noproxy", "*", "--proto", "=https"]);
    #[cfg(test)]
    command.args(["--proto", "=http,https"]);
    command
}

fn allowed_download_url(url: &str) -> Result<()> {
    let url = reqwest::Url::parse(url)?;
    ensure!(
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port_or_known_default() == Some(443)
            && matches!(
                url.host_str(),
                Some(
                    "huggingface.co"
                        | "us.aws.cdn.hf.co"
                        | "cas-bridge.xethub.hf.co"
                        | "cdn-lfs.huggingface.co"
                        | "cdn-lfs-us-1.hf.co"
                )
            ),
        "Model download destination is not approved"
    );
    Ok(())
}

fn resolve_download_url(
    directory: &Path,
    initial: &str,
    process: &Process,
    canceled: &AtomicBool,
) -> Result<String> {
    #[cfg(test)]
    if initial.starts_with("http://127.0.0.1:") {
        return Ok(initial.to_owned());
    }
    let metadata = storage::temporary(&directory.join("redirect"));
    let result = (|| {
        let mut url = initial.to_owned();
        for _ in 0..8 {
            allowed_download_url(&url)?;
            let mut command = download_command();
            command
                .args([
                    "--head",
                    "--silent",
                    "--show-error",
                    "--fail",
                    "--max-time",
                    "30",
                    "--output",
                    "/dev/null",
                    "--write-out",
                    "%{http_code}\n%{redirect_url}",
                    "--url",
                    &url,
                ])
                .stdout(Stdio::from(File::create(&metadata)?));
            process.run(&mut command, canceled)?;
            let response = fs::read_to_string(&metadata)?;
            let (status, redirect) = response
                .split_once('\n')
                .context("Invalid download response")?;
            if status.starts_with('2') {
                return Ok(url);
            }
            ensure!(
                status.starts_with('3') && !redirect.is_empty(),
                "Unexpected model download response"
            );
            url = redirect.to_owned();
        }
        anyhow::bail!("Too many model download redirects")
    })();
    let _ = fs::remove_file(metadata);
    result
}

fn verify_file(path: &Path, asset: &Asset, canceled: &AtomicBool) -> Result<()> {
    let mut file = File::open(path).with_context(|| format!("Missing {}", path.display()))?;
    ensure!(
        file.metadata()?.len() == asset.bytes,
        "Incomplete model {}",
        path.display()
    );
    let mut checksum = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        ensure!(!canceled.load(Ordering::Acquire), "Canceled");
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        checksum.update(&buffer[..read]);
    }
    ensure!(
        format!("{:x}", checksum.finalize()) == asset.sha256,
        "Checksum mismatch: {}",
        path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "contacts pinned Hugging Face model hosts with HEAD only"]
    fn pinned_model_redirects_work_inside_download_sandbox() {
        const MARKER: &str = "NEMO_MODEL_HEAD_TEST";
        if std::env::var_os(MARKER).is_some() {
            crate::network::download_only().unwrap();
            let store = Store::managed();
            fs::create_dir_all(&store.directory).unwrap();
            for asset in &store.manifest.files {
                let url = format!(
                    "https://huggingface.co/{}/resolve/{}/{}",
                    store.manifest.repository, store.manifest.revision, asset.name
                );
                let resolved = resolve_download_url(
                    &store.directory,
                    &url,
                    &Process::default(),
                    &AtomicBool::new(false),
                )
                .unwrap();
                allowed_download_url(&resolved).unwrap();
                println!(
                    "Approved model download host: {}",
                    reqwest::Url::parse(&resolved).unwrap().host_str().unwrap()
                );
            }
            return;
        }
        let data = tempfile::tempdir().unwrap();
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "model::tests::pinned_model_redirects_work_inside_download_sandbox",
                "--ignored",
                "--nocapture",
            ])
            .env(MARKER, "1")
            .env("NEMO_DATA_DIR", data.path())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        print!("{}", String::from_utf8_lossy(&result.stdout));
    }

    #[test]
    fn model_download_rejects_unapproved_destinations() {
        for url in [
            "http://huggingface.co/model",
            "https://huggingface.co.evil.test/model",
            "https://example.com/model",
            "https://user:secret@huggingface.co/model",
            "https://huggingface.co:8443/model",
            "file:///tmp/model",
        ] {
            assert!(allowed_download_url(url).is_err(), "{url}");
        }
        for url in [
            "https://huggingface.co/model",
            "https://us.aws.cdn.hf.co/model?signature=fixture",
        ] {
            allowed_download_url(url).unwrap();
        }
    }
    fn fixture(root: &Path) -> Store {
        let bytes = b"GGUF test weights";
        Store {
            directory: root.join("managed"),
            manifest: Manifest {
                repository: "test/model".into(),
                revision: "test".into(),
                files: vec![Asset {
                    name: "decoder.gguf".into(),
                    bytes: bytes.len() as u64,
                    sha256: format!("{:x}", Sha256::digest(bytes)),
                }],
            },
        }
    }
    #[test]
    fn migration_is_repeatable_and_keeps_the_old_app_path_working() {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("checkout-models");
        fs::create_dir(&legacy).unwrap();
        fs::write(legacy.join("decoder.gguf"), b"GGUF test weights").unwrap();
        let store = fixture(root.path());
        store.migrate(&legacy, &AtomicBool::new(false)).unwrap();
        store.migrate(&legacy, &AtomicBool::new(false)).unwrap();
        store.verify(&AtomicBool::new(false)).unwrap();
        assert!(store.ready());
        assert!(legacy.join("decoder.gguf").is_symlink());
        assert_eq!(
            fs::read(legacy.join("decoder.gguf")).unwrap(),
            b"GGUF test weights"
        );
    }
    #[test]
    fn failed_validation_does_not_move_the_legacy_files() {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("old");
        fs::create_dir(&legacy).unwrap();
        fs::write(legacy.join("decoder.gguf"), b"GGUF bad weights!").unwrap();
        let store = fixture(root.path());
        assert!(store.migrate(&legacy, &AtomicBool::new(false)).is_err());
        assert!(!legacy.join("decoder.gguf").is_symlink());
        assert!(!store.decoder().exists());
    }
    #[test]
    fn canceled_migration_leaves_the_source_untouched() {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("old");
        fs::create_dir(&legacy).unwrap();
        fs::write(legacy.join("decoder.gguf"), b"GGUF test weights").unwrap();
        assert!(
            fixture(root.path())
                .migrate(&legacy, &AtomicBool::new(true))
                .is_err()
        );
        assert!(!legacy.join("decoder.gguf").is_symlink());
    }

    #[test]
    fn interrupted_http_download_resumes_and_only_then_publishes() {
        use std::{
            io::{BufRead, BufReader, Write},
            net::TcpListener,
            thread,
        };
        fn serve(response: Vec<u8>) -> (String, std::thread::JoinHandle<String>) {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/weights", listener.local_addr().unwrap());
            let worker = thread::spawn(move || {
                let (mut connection, _) = listener.accept().unwrap();
                let mut request = String::new();
                let mut reader = BufReader::new(connection.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    request.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                }
                connection.write_all(&response).unwrap();
                request
            });
            (url, worker)
        }
        let root = tempfile::tempdir().unwrap();
        let store = fixture(root.path());
        fs::create_dir(&store.directory).unwrap();
        let asset = &store.manifest.files[0];
        let bytes = b"GGUF test weights";
        let (url, server) = serve(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\nGGUF ",
                bytes.len()
            )
            .into_bytes(),
        );
        let process = Process::default();
        let canceled = AtomicBool::new(false);
        assert!(download_asset(&store.directory, asset, &url, &process, &canceled, 0).is_err());
        server.join().unwrap();
        assert!(!store.decoder().exists());
        assert_eq!(
            fs::read(store.decoder().with_extension("gguf.part")).unwrap(),
            &bytes[..5]
        );
        let mut response = format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 5-{}/{}\r\nConnection: close\r\n\r\n", bytes.len() - 5, bytes.len() - 1, bytes.len()).into_bytes();
        response.extend_from_slice(&bytes[5..]);
        let (url, server) = serve(response);
        download_asset(&store.directory, asset, &url, &process, &canceled, 0).unwrap();
        assert!(
            server
                .join()
                .unwrap()
                .to_lowercase()
                .contains("range: bytes=5-")
        );
        assert_eq!(fs::read(store.decoder()).unwrap(), bytes);
        assert!(!store.decoder().with_extension("gguf.part").exists());
    }

    #[test]
    fn completed_partial_is_recovered_without_another_network_request() {
        let root = tempfile::tempdir().unwrap();
        let store = fixture(root.path());
        fs::create_dir(&store.directory).unwrap();
        fs::write(
            store.decoder().with_extension("gguf.part"),
            b"GGUF test weights",
        )
        .unwrap();
        download_asset(
            &store.directory,
            &store.manifest.files[0],
            "http://127.0.0.1:1/must-not-be-used",
            &Process::default(),
            &AtomicBool::new(false),
            0,
        )
        .unwrap();
        assert!(store.ready());
    }

    #[test]
    fn bad_checksum_never_replaces_a_published_file_and_is_retryable() {
        let root = tempfile::tempdir().unwrap();
        let store = fixture(root.path());
        fs::create_dir(&store.directory).unwrap();
        fs::write(store.decoder(), b"previous version").unwrap();
        let partial = store.decoder().with_extension("gguf.part");
        fs::write(&partial, b"GGUF bad weights!").unwrap();
        assert!(
            publish(
                &partial,
                &store.decoder(),
                &store.manifest.files[0],
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert_eq!(fs::read(store.decoder()).unwrap(), b"previous version");
        assert!(!partial.exists());
    }
}
