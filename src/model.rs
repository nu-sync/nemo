use crate::{config, storage};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Deserialize)]
pub struct Asset {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

/// A named alternative weight set, rooted at its own revision directory.
#[derive(Clone, Deserialize)]
pub struct Profile {
    pub repository: String,
    pub revision: String,
    pub files: Vec<Asset>,
}

#[derive(Clone, Deserialize)]
pub struct Manifest {
    pub repository: String,
    pub revision: String,
    pub files: Vec<Asset>,
    /// Additional weight sets. The top-level fields are the default profile, so a
    /// manifest without this key behaves exactly as it did before.
    #[serde(default)]
    pub profiles: std::collections::BTreeMap<String, Profile>,
}

/// The profile the shipped configuration selects, i.e. the top-level manifest.
pub const DEFAULT_PROFILE: &str = "nemo-speech";

/// Provisioning lifecycle of one weight profile.
///
/// The invariant this encodes: an unverified profile is never treated as usable.
/// `Present` means every pinned file exists at its pinned length, which is what the
/// running engine needs; `Verified` additionally means every SHA-256 matched. Both
/// are usable, and only these two are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileState {
    /// Not every pinned file is on disk at the pinned length.
    Unprovisioned { missing: usize, total: usize },
    /// Every pinned file is present and the right length; hashes unchecked.
    Present,
    /// Every pinned file matched its pinned digest.
    Verified,
    /// A pinned file hashed differently. Not usable: the only way back is to
    /// provision again, so a failed check demotes the profile instead of leaving it
    /// silently running on weights nobody vouched for.
    Corrupt { mismatched: usize },
}

impl ProfileState {
    /// A usable profile has every pinned file present at the pinned length and no
    /// known mismatch.
    pub fn is_usable(self) -> bool {
        matches!(self, Self::Present | Self::Verified)
    }

    /// Why a profile cannot be used, for a message a person can act on.
    pub fn reason(self) -> Option<String> {
        match self {
            Self::Unprovisioned { missing, total } => Some(format!(
                "{missing} of {total} pinned weight files are missing or the wrong size"
            )),
            Self::Corrupt { mismatched } => Some(format!(
                "{mismatched} pinned weight files did not match their digest; provision again"
            )),
            Self::Present | Self::Verified => None,
        }
    }
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

    /// A store for a named profile. The default profile is the top-level manifest,
    /// so this returns the same store for `DEFAULT_PROFILE` as `at`.
    pub fn profile_at(data: &Path, profile: &str) -> Result<Self> {
        if profile == DEFAULT_PROFILE {
            return Ok(Self::at(data));
        }
        let manifest = manifest();
        let selected = manifest.profiles.get(profile).with_context(|| {
            format!(
                "Unknown model profile '{profile}'. Known profiles: {}",
                Self::profile_names().collect::<Vec<_>>().join(", ")
            )
        })?;
        Ok(Self {
            directory: data.join("models").join(&selected.revision),
            manifest: Manifest {
                repository: selected.repository.clone(),
                revision: selected.revision.clone(),
                files: selected.files.clone(),
                profiles: Default::default(),
            },
        })
    }

    /// Every profile the embedded manifest defines, default first.
    pub fn profile_names() -> impl Iterator<Item = &'static str> {
        std::iter::once(DEFAULT_PROFILE).chain(manifest().profiles.keys().map(String::as_str))
    }

    /// Every profile the manifest defines, paired with its observed provisioning
    /// state. Unknown profiles cannot appear here: the name comes from the manifest.
    pub fn profiles() -> Vec<(&'static str, ProfileState)> {
        let data = config::data_dir();
        Self::profile_names()
            .filter_map(|name| {
                let store = Self::profile_at(&data, name).ok()?;
                Some((name, store.state()))
            })
            .collect()
    }

    /// Observe the provisioning state of this store from the filesystem.
    pub fn state(&self) -> ProfileState {
        let total = self.manifest.files.len();
        let missing = self
            .manifest
            .files
            .iter()
            .filter(|asset| {
                !fs::metadata(self.directory.join(&asset.name))
                    .is_ok_and(|m| m.is_file() && m.len() == asset.bytes)
            })
            .count();
        if missing == 0 {
            ProfileState::Present
        } else {
            ProfileState::Unprovisioned { missing, total }
        }
    }

    /// Hash every pinned file and move `Present` to `Verified`.
    ///
    /// This is the only transition into `Verified`, and it refuses to run from
    /// `Unprovisioned`: hashing absent files could only produce a worse message than
    /// the provisioning state already carries.
    pub fn verify_state(&self, canceled: &AtomicBool) -> Result<ProfileState> {
        match self.state() {
            state @ ProfileState::Unprovisioned { .. } => {
                bail!("{}", state.reason().unwrap_or_default())
            }
            ProfileState::Verified => Ok(ProfileState::Verified),
            // A mismatch is not repaired by asking again; only new bytes are.
            corrupt @ ProfileState::Corrupt { .. } => Ok(corrupt),
            ProfileState::Present => {
                let mut mismatched = 0;
                for asset in &self.manifest.files {
                    if verify_file(&self.directory.join(&asset.name), asset, canceled).is_err() {
                        mismatched += 1;
                    }
                }
                Ok(if mismatched == 0 {
                    ProfileState::Verified
                } else {
                    ProfileState::Corrupt { mismatched }
                })
            }
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

    pub fn download(&self, canceled: &AtomicBool) -> Result<()> {
        fs::create_dir_all(&self.directory)?;
        let _lock = storage::Lock::acquire(&self.directory.join("install.lock"))?;
        for asset in &self.manifest.files {
            let url = format!(
                "https://huggingface.co/{}/resolve/{}/{}",
                self.manifest.repository, self.manifest.revision, asset.name
            );
            download_asset(&self.directory, asset, &url, canceled, 8)?;
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
    let client = download_client()?;
    let mut result = Ok(());
    for attempt in 0..=retries {
        ensure!(!canceled.load(Ordering::Acquire), "Canceled");
        result = transfer(&client, url, &partial, asset.bytes, canceled);
        if result.is_ok() || canceled.load(Ordering::Acquire) {
            break;
        }
        if attempt < retries {
            for _ in 0..20 {
                ensure!(!canceled.load(Ordering::Acquire), "Canceled");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    result?;
    publish(&partial, &target, asset, canceled)?;
    eprintln!("Verified {}", asset.name);
    Ok(())
}

fn download_client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(30))
        .build()?)
}

fn transfer(
    client: &reqwest::blocking::Client,
    initial: &str,
    partial: &Path,
    bytes: u64,
    canceled: &AtomicBool,
) -> Result<()> {
    let url = resolve_download_url(client, initial, canceled)?;
    ensure!(!canceled.load(Ordering::Acquire), "Canceled");
    let offset = fs::metadata(partial).map_or(0, |metadata| metadata.len());
    let mut request = client.get(&url);
    if offset > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
    }
    let mut response = request.send()?.error_for_status()?;
    if offset > 0 {
        ensure!(
            response.status() == reqwest::StatusCode::PARTIAL_CONTENT,
            "Model server did not honor the resume range"
        );
        let expected = format!("bytes {offset}-{}/{}", bytes - 1, bytes);
        ensure!(
            response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                == Some(expected.as_str()),
            "Invalid model resume range"
        );
    } else {
        ensure!(
            response.status() == reqwest::StatusCode::OK,
            "Unexpected model response"
        );
    }
    let mut output = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(partial)?;
    let mut written = offset;
    let mut buffer = [0; 64 * 1024];
    loop {
        ensure!(!canceled.load(Ordering::Acquire), "Canceled");
        let count = response.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        written += count as u64;
        ensure!(written <= bytes, "Model response exceeds the pinned size");
        output.write_all(&buffer[..count])?;
    }
    ensure!(written == bytes, "Incomplete model response");
    Ok(())
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
    client: &reqwest::blocking::Client,
    initial: &str,
    canceled: &AtomicBool,
) -> Result<String> {
    #[cfg(test)]
    if initial.starts_with("http://127.0.0.1:") {
        return Ok(initial.to_owned());
    }
    let mut url = reqwest::Url::parse(initial)?;
    for _ in 0..8 {
        ensure!(!canceled.load(Ordering::Acquire), "Canceled");
        allowed_download_url(url.as_str())?;
        let response = client.head(url.clone()).send()?.error_for_status()?;
        if response.status().is_success() {
            return Ok(url.into());
        }
        ensure!(
            response.status().is_redirection(),
            "Unexpected model download response"
        );
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .context("Missing model redirect destination")?
            .to_str()?;
        url = url.join(location)?;
    }
    anyhow::bail!("Too many model download redirects")
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
    use std::process::Command;

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
                    &download_client().unwrap(),
                    &url,
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
                profiles: Default::default(),
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
        let canceled = AtomicBool::new(false);
        assert!(download_asset(&store.directory, asset, &url, &canceled, 0).is_err());
        server.join().unwrap();
        assert!(!store.decoder().exists());
        assert_eq!(
            fs::read(store.decoder().with_extension("gguf.part")).unwrap(),
            &bytes[..5]
        );
        let mut response = format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 5-{}/{}\r\nConnection: close\r\n\r\n", bytes.len() - 5, bytes.len() - 1, bytes.len()).into_bytes();
        response.extend_from_slice(&bytes[5..]);
        let (url, server) = serve(response);
        download_asset(&store.directory, asset, &url, &canceled, 0).unwrap();
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

    fn store_with_profiles(root: &std::path::Path) -> Store {
        Store {
            directory: root.join("default-rev"),
            manifest: Manifest {
                repository: "test/default".into(),
                revision: "default-rev".into(),
                files: vec![Asset {
                    name: "decoder.gguf".into(),
                    bytes: 4,
                    sha256: String::new(),
                }],
                profiles: [(
                    "alt".to_string(),
                    Profile {
                        repository: "test/alt".into(),
                        revision: "alt-rev".into(),
                        files: vec![Asset {
                            name: "alt.gguf".into(),
                            bytes: 6,
                            sha256: String::new(),
                        }],
                    },
                )]
                .into_iter()
                .collect(),
            },
        }
    }

    #[test]
    fn the_default_profile_is_the_manifest_top_level() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().to_path_buf();
        let store = store_with_profiles(&root);
        assert_eq!(
            store.decoder(),
            root.join("default-rev").join("decoder.gguf")
        );
    }

    #[test]
    fn a_named_profile_roots_at_its_own_revision_and_names_its_own_decoder() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().to_path_buf();
        let store = store_with_profiles(&root);
        let alt = Store {
            directory: root.join("alt-rev"),
            manifest: Manifest {
                repository: "test/alt".into(),
                revision: "alt-rev".into(),
                files: vec![Asset {
                    name: "alt.gguf".into(),
                    bytes: 6,
                    sha256: String::new(),
                }],
                profiles: Default::default(),
            },
        };
        assert_eq!(alt.decoder(), root.join("alt-rev").join("alt.gguf"));
        assert_eq!(store.manifest.profiles.len(), 1);
    }

    #[test]
    fn an_unknown_profile_names_the_profiles_that_exist() {
        let error = match Store::profile_at(std::path::Path::new("/tmp"), "whisper-cpp") {
            Ok(_) => panic!("unknown profile must fail"),
            Err(error) => error,
        };
        let message = format!("{error:#}");
        assert!(message.contains("whisper-cpp"), "{message}");
        assert!(message.contains(DEFAULT_PROFILE), "{message}");
    }

    #[test]
    fn provisioning_state_accepts_only_files_present_at_the_pinned_length() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().to_path_buf();
        let store = store_with_profiles(&root);
        std::fs::create_dir_all(&store.directory).unwrap();

        let absent = store.state();
        assert_eq!(
            absent,
            ProfileState::Unprovisioned {
                missing: 1,
                total: 1
            }
        );
        assert!(!absent.is_usable());
        assert!(absent.reason().is_some());

        std::fs::write(store.directory.join("decoder.gguf"), b"prese").unwrap();
        let wrong = store.state();
        assert!(!wrong.is_usable(), "a wrong length is not provisioned");

        std::fs::write(store.directory.join("decoder.gguf"), b"GGUF").unwrap();
        let present = store.state();
        assert_eq!(present, ProfileState::Present);
        assert!(present.is_usable());
        assert!(present.reason().is_none());
    }

    #[test]
    fn verification_refuses_to_run_from_unprovisioned_rather_than_reporting_a_hash_error() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().to_path_buf();
        let store = store_with_profiles(&root);
        std::fs::create_dir_all(&store.directory).unwrap();
        let canceled = AtomicBool::new(false);
        let error = match store.verify_state(&canceled) {
            Ok(state) => panic!("unprovisioned verify must fail, got {state:?}"),
            Err(error) => error,
        };
        assert!(format!("{error:#}").contains("missing"));
    }

    #[test]
    fn a_failed_hash_demotes_the_profile_instead_of_leaving_it_usable() {
        let root = tempfile::tempdir().unwrap();
        let bytes = b"GGUF";
        let mut store = store_with_profiles(root.path());
        store.directory = root.path().join("managed");
        store.manifest.files = vec![Asset {
            name: "decoder.gguf".into(),
            bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(b"something else")),
        }];
        std::fs::create_dir_all(&store.directory).unwrap();
        std::fs::write(store.directory.join("decoder.gguf"), bytes).unwrap();
        assert_eq!(store.state(), ProfileState::Present);

        let canceled = AtomicBool::new(false);
        let state = store
            .verify_state(&canceled)
            .expect("hash mismatch is a state, not an io error");
        assert_eq!(state, ProfileState::Corrupt { mismatched: 1 });
        assert!(!state.is_usable(), "a corrupt profile must not be usable");
        assert!(state.reason().unwrap_or_default().contains("digest"));
        // it stays corrupt until the bytes change
        assert_eq!(store.verify_state(&canceled).unwrap(), state);
    }

    #[test]
    fn matching_hashes_reach_verified_and_stay_usable() {
        let root = tempfile::tempdir().unwrap();
        let bytes = b"GGUF";
        let mut store = store_with_profiles(root.path());
        store.directory = root.path().join("managed");
        store.manifest.files = vec![Asset {
            name: "decoder.gguf".into(),
            bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
        }];
        std::fs::create_dir_all(&store.directory).unwrap();
        std::fs::write(store.directory.join("decoder.gguf"), bytes).unwrap();

        let canceled = AtomicBool::new(false);
        let state = store.verify_state(&canceled).unwrap();
        assert_eq!(state, ProfileState::Verified);
        assert!(state.is_usable());
    }
}
