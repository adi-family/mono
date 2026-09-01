//! The update pipeline: check → download → verify → preflight → swap. Every step is guarded —
//! checksum before anything is unpacked, code signature + Team ID before a bundle is installed,
//! the new CLI is made to run and state its version before the live install is touched, and the
//! previous install is renamed aside (not deleted) so both a failed swap and a failed health
//! check can put it back.
//!
//! What a payload *is* differs per platform (a DMG'd app bundle on macOS, a tarball of binaries
//! on a node); that difference lives entirely in [`crate::payload`], so this module reads the
//! same on every OS.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::manifest::{Manifest, host_platform};
use crate::payload::Payload;
use crate::settings::Settings;
use crate::shell;
use crate::state::{self, State};
use crate::version::Version;

/// Where the app bundle lives on a provisioned Mac; override with `ADI_UPDATE_APP`
/// (used by tests and non-standard installs). On Linux and Windows there is no bundle —
/// the updater replaces the binaries beside the running executable, or `ADI_UPDATE_BIN_DIR`.
///
/// Named after the flavour rather than fixed at `ADI.app`: writing that path from a process
/// that is not the release install means overwriting a *different* install than the one that
/// asked. `adi-core` will not even register the updater outside the release flavour, so this
/// is the second of two locks on the same door.
#[must_use]
pub fn default_app_path() -> String {
    format!("/Applications/{}.app", adi_config::Flavor::current().app_name)
}

/// The Apple Developer Team ID every genuine ADI release is signed with; a downloaded
/// bundle signed by anyone else is rejected. Override with `ADI_UPDATE_TEAM_ID`.
pub const DEFAULT_TEAM_ID: &str = "752556J5V6";

/// How the manifest and the artifact are fetched. Resolved through `PATH` rather than named
/// absolutely: `/usr/bin/curl` is a macOS path, and while Linux distributions usually agree
/// with it, Windows keeps `curl.exe` in System32 — where an absolute unix path finds nothing
/// and every check fails before it has even read the manifest.
const CURL: &str = "curl";

/// How long a stale lock (from a crashed updater) blocks the next run.
const LOCK_STALE_SECS: u64 = 2 * 3600;

/// Previous installs kept in `update/backups` for rollback.
const BACKUPS_KEPT: usize = 2;

/// What went wrong, specific enough for the CLI/log line to be actionable.
#[derive(Debug)]
pub enum Error {
    /// Fetching or parsing the release manifest failed (offline, bad URL, bad JSON).
    Manifest(String),
    /// The release publishes nothing for this OS/architecture.
    Unsupported(String),
    /// Downloading the artifact failed.
    Download(String),
    /// The downloaded bytes don't match the manifest's sha256.
    Checksum { expected: String, actual: String },
    /// Mounting or reading the DMG failed.
    Dmg(String),
    /// Unpacking the tarball/zip failed.
    Archive(String),
    /// The bundle's code signature or Team ID didn't verify.
    Signature(String),
    /// The downloaded CLI wouldn't run, or reported a version the manifest didn't promise.
    Preflight(String),
    /// Swapping the installed payload failed (the previous install was rolled back).
    Install(String),
    /// The new version installed but failed its health check; the previous one is back.
    HealthCheck(String),
    /// Another updater run holds the lock.
    Busy(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest(e) => write!(f, "could not fetch the release manifest: {e}"),
            Self::Unsupported(e) => write!(f, "no update published for this platform: {e}"),
            Self::Download(e) => write!(f, "could not download the update: {e}"),
            Self::Checksum { expected, actual } => write!(
                f,
                "the download failed its checksum (expected sha256 {expected}, got {actual})"
            ),
            Self::Dmg(e) => write!(f, "could not open the downloaded DMG: {e}"),
            Self::Archive(e) => write!(f, "could not unpack the downloaded archive: {e}"),
            Self::Signature(e) => write!(f, "downloaded app failed signature verification: {e}"),
            Self::Preflight(e) => write!(f, "the downloaded build did not pass preflight: {e}"),
            Self::Install(e) => write!(f, "could not install the update: {e}"),
            Self::HealthCheck(e) => write!(f, "the update was rolled back: {e}"),
            Self::Busy(e) => write!(f, "another update is already in progress: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// Result of a manifest check: what's installed vs what's published.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Check {
    pub installed: String,
    pub latest: String,
    pub update_available: bool,
    /// The platform key this host looked for ([`host_platform`]).
    pub platform: String,
    /// Whether the published release carries an artifact for [`Self::platform`]. A newer
    /// version that doesn't is *not* an available update — reporting one would leave every
    /// scheduled run failing on a download that was never published.
    pub has_artifact: bool,
    #[serde(skip)]
    pub manifest: Manifest,
}

/// Result of a completed install.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Installed {
    pub from: String,
    pub to: String,
    /// The live path that now holds the new version — the app bundle, or the binary directory.
    pub path: PathBuf,
    /// Where the previous install was parked, if there was one. Rollback needs this.
    pub backup: Option<PathBuf>,
}

/// The update engine over one settings/state directory (`~/.adi/mono/update`).
#[derive(Debug)]
pub struct Engine {
    settings: Settings,
    module: adi_config::Module,
}

impl Engine {
    /// The engine over the standard store.
    #[must_use]
    pub fn open() -> Self {
        Self::with_module(crate::settings::module())
    }

    /// The engine over an explicit module directory — tests and alternate stores.
    #[must_use]
    pub fn with_module(module: adi_config::Module) -> Self {
        Self {
            settings: Settings::load(&module),
            module,
        }
    }

    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The updater's own store directory — `~/.adi/mono/update`, holding `config.toml`,
    /// `state.json` and the backups.
    ///
    /// Exposed for callers that keep a file *beside* those: the control panel writes an
    /// in-flight marker there when it hands an install to the CLI. Re-deriving the path at
    /// each call site is how two of them end up disagreeing about which directory this is.
    #[must_use]
    pub fn module(&self) -> &adi_config::Module {
        &self.module
    }

    /// The persisted last check/install record.
    #[must_use]
    pub fn state(&self) -> State {
        State::load(&self.module)
    }

    /// The live install this host updates: `/Applications/ADI.app` on macOS, the directory
    /// holding the running binaries elsewhere.
    #[must_use]
    pub fn target() -> PathBuf {
        Payload::for_host().target()
    }

    /// The version of the *installed* payload, which is what update decisions compare
    /// against — the running CLI may be older or newer than what is installed. Falls back to
    /// the built-in version when nothing is installed.
    #[must_use]
    pub fn installed_version() -> String {
        let payload = Payload::for_host();
        payload.installed_version(&payload.target())
    }

    /// Fetch and parse the release manifest from the configured URL.
    ///
    /// # Errors
    /// [`Error::Manifest`] when the fetch or parse fails.
    pub fn fetch_manifest(&self) -> Result<Manifest, Error> {
        let mut argv = vec![
            CURL.to_string(),
            "-fsSL".to_string(),
            "--retry".to_string(),
            "2".to_string(),
            "--max-time".to_string(),
            "30".to_string(),
        ];
        if let Some(header) = &self.settings.auth_header {
            argv.push("-H".to_string());
            argv.push(header.clone());
        }
        argv.push(self.settings.manifest_url.clone());
        let out = shell::capture(&argv);
        if !out.ok() {
            return Err(Error::Manifest(format!(
                "{} ({})",
                out.stderr.trim(),
                self.settings.manifest_url
            )));
        }
        Manifest::from_json(&out.stdout).map_err(|e| Error::Manifest(e.to_string()))
    }

    /// Check the manifest against the installed version, persisting the result to
    /// `state.json` (including fetch errors, so `update status` explains silence).
    ///
    /// # Errors
    /// [`Error::Manifest`] when the fetch or parse fails.
    pub fn check(&self) -> Result<Check, Error> {
        let installed = Self::installed_version();
        let result = self.fetch_manifest();

        let mut state = self.state();
        state.last_check_unix = Some(state::now_unix());
        state.installed_version = Some(installed.clone());
        match &result {
            Ok(m) => {
                // A release with no artifact for this platform is not an available update —
                // reporting one would leave `update run` failing on every scheduled check.
                let available =
                    Version::is_newer(&m.version, &installed) && m.artifact_for_host().is_some();
                state.latest_version = Some(m.version.clone());
                state.latest_notes.clone_from(&m.notes);
                state.latest_has_artifact = Some(m.artifact_for_host().is_some());
                state.last_outcome = Some(
                    if available {
                        "update-available"
                    } else {
                        "up-to-date"
                    }
                    .to_string(),
                );
                state.last_error = None;
            }
            Err(e) => {
                state.last_outcome = Some("error".to_string());
                state.last_error = Some(e.to_string());
            }
        }
        state.save(&self.module);

        let manifest = result?;
        let has_artifact = manifest.artifact_for_host().is_some();
        let update_available = Version::is_newer(&manifest.version, &installed) && has_artifact;
        Ok(Check {
            installed,
            latest: manifest.version.clone(),
            update_available,
            platform: host_platform(),
            has_artifact,
            manifest,
        })
    }

    /// Download, verify, preflight, and install this release's artifact for the running
    /// platform, replacing the live install. The caller decides whether to restart services
    /// afterwards — and, if they fail to come up, calls [`Self::rollback`].
    ///
    /// # Errors
    /// Any [`Error`]; nothing is swapped until the bytes are verified and the new CLI has run,
    /// and a failure part-way through the swap puts the previous install back.
    pub fn install(&self, manifest: &Manifest) -> Result<Installed, Error> {
        let _lock = Lock::acquire(&self.module)?;
        let payload = Payload::for_host();
        let artifact = manifest.artifact_for_host().ok_or_else(|| {
            Error::Unsupported(format!(
                "release {} has no artifact for {}",
                manifest.version,
                host_platform()
            ))
        })?;

        let staging = self.module.raw_path("staging");
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging).map_err(|e| Error::Install(e.to_string()))?;

        // Download + checksum: nothing is mounted, unpacked or executed until the bytes match.
        let downloaded = staging.join(download_name(&artifact.url));
        self.download(&artifact.url, &downloaded)?;
        let actual = sha256(&downloaded)?;
        let expected = artifact.sha256.trim().to_ascii_lowercase();
        if actual != expected {
            return Err(Error::Checksum { expected, actual });
        }

        // Authenticate and unpack, then make the new CLI prove it runs — all still off to the
        // side, with the live install untouched.
        let staged = payload.stage(&downloaded, &staging)?;
        payload.preflight(&staged, &manifest.version)?;

        let target = payload.target();
        let from = payload.installed_version(&target);
        let backup = payload.swap(
            &staged,
            &target,
            &self.module.raw_path("backups"),
            &from,
            &manifest.version,
        )?;
        self.prune_backups(payload);
        let _ = fs::remove_dir_all(&staging);

        let mut state = self.state();
        state.installed_version = Some(manifest.version.clone());
        state.latest_version = Some(manifest.version.clone());
        state.last_outcome = Some("installed".to_string());
        state.last_error = None;
        state.last_install_unix = Some(state::now_unix());
        state.save(&self.module);

        Ok(Installed {
            from,
            to: manifest.version.clone(),
            path: target,
            backup,
        })
    }

    /// Put the previous install back after a failed health check, recording why.
    ///
    /// # Errors
    /// [`Error::Install`] when there is no backup to return to, or restoring it fails — at
    /// which point the machine is running the new version and the caller must say so loudly.
    pub fn rollback(&self, installed: &Installed, why: &str) -> Result<(), Error> {
        let backup = installed.backup.as_ref().ok_or_else(|| {
            Error::Install(format!(
                "{} was a first install, so there is no previous version to roll back to",
                installed.to
            ))
        })?;
        Payload::for_host().restore(backup, &installed.path)?;

        let mut state = self.state();
        state.installed_version = Some(installed.from.clone());
        state.last_outcome = Some("rolled-back".to_string());
        state.last_error = Some(format!(
            "{} failed its health check and was rolled back to {}: {why}",
            installed.to, installed.from
        ));
        state.save(&self.module);
        Ok(())
    }

    fn download(&self, url: &str, dest: &Path) -> Result<(), Error> {
        let mut argv = vec![
            CURL.to_string(),
            "-fsSL".to_string(),
            "--retry".to_string(),
            "3".to_string(),
            "--max-time".to_string(),
            "3600".to_string(),
            "-o".to_string(),
            dest.to_string_lossy().into_owned(),
        ];
        if let Some(header) = &self.settings.auth_header {
            argv.push("-H".to_string());
            argv.push(header.clone());
        }
        argv.push(url.to_string());
        let out = shell::run(&argv);
        if !out.ok() {
            return Err(Error::Download(format!("{} ({url})", out.text.trim())));
        }
        Ok(())
    }

    /// Keep only the newest [`BACKUPS_KEPT`] entries this payload kind owns.
    fn prune_backups(&self, payload: Payload) {
        let dir = self.module.raw_path("backups");
        let Ok(entries) = fs::read_dir(&dir) else {
            return;
        };
        let mut backups: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| payload.owns_backup(&n.to_string_lossy()))
            })
            .collect();
        backups.sort_by_key(|p| {
            fs::metadata(p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });
        while backups.len() > BACKUPS_KEPT {
            let oldest = backups.remove(0);
            let _ = fs::remove_dir_all(&oldest);
        }
    }
}

/// The file name to save a download under: the URL's last segment when it is a plain file
/// name, otherwise a neutral default. Never trusts the URL enough to let it escape staging.
fn download_name(url: &str) -> String {
    let tail = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let safe = !tail.is_empty()
        && tail != "."
        && tail != ".."
        && !tail.contains(['/', '\\'])
        && Path::new(tail).file_name().is_some_and(|n| n == tail);
    if safe {
        tail.to_string()
    } else {
        "adi-update-download".to_string()
    }
}

/// Hex sha256 of a file, hashed in-process.
///
/// This one step deliberately does *not* shell out like the rest of the engine does. There is
/// no checksum tool all three platforms ship: `shasum` is macOS's (it comes from perl, and a
/// stock Debian, Ubuntu or Alpine has no such file), `sha256sum` is coreutils and absent on
/// macOS and Windows, and Windows only has `certutil`, whose output format changed between
/// releases. Shelling out to any one of them is how Linux nodes came to download the artifact
/// on every scheduled run and then fail here, forever, with `--quiet` swallowing the reason.
///
/// Streamed in chunks: the artifact is tens of megabytes and there is no reason to hold it in
/// memory a second time when it is already on disk.
///
/// Public because it is not only the updater's problem: `adi-core`'s bun installer verifies its
/// download the same way, and the paragraphs above are the whole reason there is one of these
/// rather than a `shasum` call per caller.
///
/// # Errors
/// [`Error::Download`] when `path` cannot be read.
pub fn sha256(path: &Path) -> Result<String, Error> {
    use std::io::Read;

    use sha2::{Digest, Sha256};

    let mut file = fs::File::open(path)
        .map_err(|e| Error::Download(format!("could not read {}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buf)
            .map_err(|e| Error::Download(format!("could not read {}: {e}", path.display())))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut acc, b| {
            use std::fmt::Write as _;
            let _ = write!(acc, "{b:02x}");
            acc
        }))
}

/// One-update-at-a-time lock (`update/update.lock`); a lock older than
/// [`LOCK_STALE_SECS`] is treated as left over from a crash and broken.
#[derive(Debug)]
struct Lock {
    path: PathBuf,
}

impl Lock {
    fn acquire(module: &adi_config::Module) -> Result<Self, Error> {
        let _ = module.ensure_dir();
        let path = module.raw_path("update.lock");
        for attempt in 0..2 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Ok(Self { path }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age.as_secs() > LOCK_STALE_SECS);
                    if stale && attempt == 0 {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    return Err(Error::Busy(format!("lock held at {}", path.display())));
                }
                Err(e) => return Err(Error::Install(e.to_string())),
            }
        }
        Err(Error::Busy(format!("lock held at {}", path.display())))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "adi-update-engine-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ))
    }

    #[test]
    fn lock_is_exclusive_and_released_on_drop() {
        let dir = scratch("lock");
        let _ = fs::remove_dir_all(&dir);
        let module = adi_config::Config::with_root(&dir).module("update");

        let first = Lock::acquire(&module).expect("first lock");
        assert!(matches!(Lock::acquire(&module), Err(Error::Busy(_))));
        drop(first);
        let _second = Lock::acquire(&module).expect("relock after drop");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sha256_matches_a_known_vector() {
        let dir = scratch("sha");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("data");
        fs::write(&file, b"abc").unwrap();
        assert_eq!(
            sha256(&file).expect("sha"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        // Empty input, and input larger than one read buffer — the two ends of the streaming
        // loop, where an off-by-one would still produce a plausible-looking 64 hex chars.
        let empty = dir.join("empty");
        fs::write(&empty, b"").unwrap();
        assert_eq!(
            sha256(&empty).expect("sha"),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        let big = dir.join("big");
        fs::write(&big, vec![b'a'; 200_000]).unwrap();
        let hash = sha256(&big).expect("sha");
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sha256_of_a_missing_file_is_an_error_not_a_hash() {
        let dir = scratch("sha-missing");
        let _ = fs::remove_dir_all(&dir);
        assert!(matches!(
            sha256(&dir.join("never-downloaded")),
            Err(Error::Download(_))
        ));
    }

    #[test]
    fn download_name_keeps_the_asset_name_and_refuses_traversal() {
        assert_eq!(download_name("https://x/y/ADI.dmg"), "ADI.dmg");
        assert_eq!(
            download_name("https://x/adi-linux-x64.tar.gz?token=1"),
            "adi-linux-x64.tar.gz"
        );
        for hostile in [
            "https://x/..",
            "https://x/y/",
            "https://x/%2e%2e/../etc/passwd/",
        ] {
            let name = download_name(hostile);
            assert!(
                !name.contains('/') && name != ".." ,
                "{hostile} produced {name}"
            );
        }
    }

    #[test]
    fn rollback_without_a_backup_is_an_error_not_a_silent_noop() {
        let dir = scratch("rollback-none");
        let _ = fs::remove_dir_all(&dir);
        let engine = Engine::with_module(adi_config::Config::with_root(&dir).module("update"));
        let installed = Installed {
            from: "0.1.0".to_string(),
            to: "0.2.0".to_string(),
            path: dir.join("ADI.app"),
            backup: None,
        };
        assert!(matches!(
            engine.rollback(&installed, "app never came up"),
            Err(Error::Install(_))
        ));
        let _ = fs::remove_dir_all(&dir);
    }
}
