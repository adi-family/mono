//! The update pipeline: check → download → verify → swap. Every step is guarded —
//! checksum before mounting, code signature + Team ID before installing, and the
//! previous install is renamed aside (not deleted) so a failed swap rolls back.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::manifest::Manifest;
use crate::settings::Settings;
use crate::shell;
use crate::state::{self, State};
use crate::version::Version;

/// Where the app bundle lives on a provisioned machine; override with `ADI_UPDATE_APP`
/// (used by tests and non-standard installs).
pub const DEFAULT_APP_PATH: &str = "/Applications/ADI.app";

/// The Apple Developer Team ID every genuine ADI release is signed with; a downloaded
/// bundle signed by anyone else is rejected. Override with `ADI_UPDATE_TEAM_ID`.
pub const DEFAULT_TEAM_ID: &str = "752556J5V6";

/// How long a stale lock (from a crashed updater) blocks the next run.
const LOCK_STALE_SECS: u64 = 2 * 3600;

/// Previous installs kept in `update/backups` for manual rollback.
const BACKUPS_KEPT: usize = 2;

/// What went wrong, specific enough for the CLI/log line to be actionable.
#[derive(Debug)]
pub enum Error {
    /// Fetching or parsing the release manifest failed (offline, bad URL, bad JSON).
    Manifest(String),
    /// Downloading the DMG failed.
    Download(String),
    /// The downloaded bytes don't match the manifest's sha256.
    Checksum { expected: String, actual: String },
    /// Mounting or reading the DMG failed.
    Dmg(String),
    /// The bundle's code signature or Team ID didn't verify.
    Signature(String),
    /// Swapping the installed app failed (the previous install was rolled back).
    Install(String),
    /// Another updater run holds the lock.
    Busy(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest(e) => write!(f, "could not fetch the release manifest: {e}"),
            Self::Download(e) => write!(f, "could not download the update: {e}"),
            Self::Checksum { expected, actual } => write!(
                f,
                "downloaded DMG failed its checksum (expected sha256 {expected}, got {actual})"
            ),
            Self::Dmg(e) => write!(f, "could not open the downloaded DMG: {e}"),
            Self::Signature(e) => write!(f, "downloaded app failed signature verification: {e}"),
            Self::Install(e) => write!(f, "could not install the update: {e}"),
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
    #[serde(skip)]
    pub manifest: Manifest,
}

/// Result of a completed install.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Installed {
    pub from: String,
    pub to: String,
    /// The live bundle path that now holds the new version.
    pub app: PathBuf,
    /// Where the previous install was moved (kept for manual rollback), if there was one.
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

    /// The persisted last check/install record.
    #[must_use]
    pub fn state(&self) -> State {
        State::load(&self.module)
    }

    /// The bundle the updater manages: `ADI_UPDATE_APP` or [`DEFAULT_APP_PATH`].
    #[must_use]
    pub fn target_app() -> PathBuf {
        std::env::var_os("ADI_UPDATE_APP")
            .filter(|v| !v.is_empty())
            .map_or_else(|| PathBuf::from(DEFAULT_APP_PATH), PathBuf::from)
    }

    /// The version of the *installed* app (its `Info.plist`), which is what update
    /// decisions compare against — the running CLI may be older or newer than the
    /// bundle on disk. Falls back to the built-in version when no app is installed.
    #[must_use]
    pub fn installed_version() -> String {
        let plist = Self::target_app().join("Contents/Info.plist");
        if plist.exists() {
            let out = shell::capture(&[
                "/usr/bin/plutil",
                "-extract",
                "CFBundleShortVersionString",
                "raw",
                "-o",
                "-",
                &plist.to_string_lossy(),
            ]);
            if out.ok() {
                let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !v.is_empty() {
                    return v;
                }
            }
        }
        crate::BUILT_VERSION.to_string()
    }

    /// Fetch and parse the release manifest from the configured URL.
    ///
    /// # Errors
    /// [`Error::Manifest`] when the fetch or parse fails.
    pub fn fetch_manifest(&self) -> Result<Manifest, Error> {
        let mut argv = vec![
            "/usr/bin/curl".to_string(),
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
                let available = Version::is_newer(&m.version, &installed);
                state.latest_version = Some(m.version.clone());
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
        let update_available = Version::is_newer(&manifest.version, &installed);
        Ok(Check {
            installed,
            latest: manifest.version.clone(),
            update_available,
            manifest,
        })
    }

    /// Download, verify, and install the manifest's DMG, atomically swapping the
    /// target bundle. The caller decides whether to restart services afterwards.
    ///
    /// # Errors
    /// Any [`Error`]; on a failed swap the previous install is rolled back in place.
    pub fn install(&self, manifest: &Manifest) -> Result<Installed, Error> {
        let _lock = Lock::acquire(&self.module)?;

        let staging = self.module.raw_path("staging");
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging).map_err(|e| Error::Install(e.to_string()))?;

        // Download + checksum: nothing is mounted or executed until the bytes match.
        let dmg = staging.join("ADI.dmg");
        self.download(&manifest.dmg.url, &dmg)?;
        let actual = sha256(&dmg)?;
        let expected = manifest.dmg.sha256.trim().to_ascii_lowercase();
        if actual != expected {
            return Err(Error::Checksum { expected, actual });
        }

        // Mount, verify the bundle's signature/team, and copy it out of the DMG.
        let mnt = staging.join("mnt");
        fs::create_dir_all(&mnt).map_err(|e| Error::Dmg(e.to_string()))?;
        let staged = staging.join("ADI.app");
        {
            let _mount = Mount::attach(&dmg, &mnt)?;
            let inner = find_app(&mnt)?;
            verify_signature(&inner)?;
            let copy = shell::run(&[
                Path::new("/usr/bin/ditto").as_os_str(),
                inner.as_os_str(),
                staged.as_os_str(),
            ]);
            if !copy.ok() {
                return Err(Error::Dmg(format!("ditto failed: {}", copy.text.trim())));
            }
        } // <- detach before touching the live install

        // Swap: rename the old bundle aside, rename the new one in. Same-volume renames,
        // so the window with no app at the target path is two atomic metadata ops.
        let from = Self::installed_version();
        let target = Self::target_app();
        if let Some(parent) = target.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let backup = if target.exists() {
            let dir = self.module.raw_path("backups");
            fs::create_dir_all(&dir).map_err(|e| Error::Install(e.to_string()))?;
            let dest = dir.join(format!("ADI.app.{from}.{}", state::now_unix()));
            fs::rename(&target, &dest).map_err(|e| Error::Install(swap_hint(&e, &target)))?;
            Some(dest)
        } else {
            None
        };
        if let Err(e) = fs::rename(&staged, &target) {
            // Never leave the machine without an install: put the old bundle back.
            if let Some(b) = &backup {
                let _ = fs::rename(b, &target);
            }
            return Err(Error::Install(swap_hint(&e, &target)));
        }
        self.prune_backups();
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
            app: target,
            backup,
        })
    }

    fn download(&self, url: &str, dest: &Path) -> Result<(), Error> {
        let mut argv = vec![
            "/usr/bin/curl".to_string(),
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

    /// Keep only the newest [`BACKUPS_KEPT`] entries in `update/backups`.
    fn prune_backups(&self) {
        let dir = self.module.raw_path("backups");
        let Ok(entries) = fs::read_dir(&dir) else {
            return;
        };
        let mut backups: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("ADI.app."))
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

/// Map a swap failure to a message that names the usual culprit (macOS App Management
/// blocks unentitled processes from replacing a signed bundle in /Applications).
fn swap_hint(e: &std::io::Error, target: &Path) -> String {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        format!(
            "{e} — macOS App Management may be blocking this process from replacing {}; \
             run the update from the ADI background updater agent, or grant the invoking \
             terminal App Management in System Settings → Privacy & Security",
            target.display()
        )
    } else {
        format!("{e} (replacing {})", target.display())
    }
}

/// Hex sha256 of a file, via the system `shasum`.
fn sha256(path: &Path) -> Result<String, Error> {
    let out = shell::capture(&[
        Path::new("/usr/bin/shasum").as_os_str(),
        std::ffi::OsStr::new("-a"),
        std::ffi::OsStr::new("256"),
        path.as_os_str(),
    ]);
    if !out.ok() {
        return Err(Error::Download(format!(
            "shasum failed: {}",
            out.stderr.trim()
        )));
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| Error::Download("shasum produced no output".to_string()))
}

/// The single `*.app` inside the mounted DMG.
fn find_app(mnt: &Path) -> Result<PathBuf, Error> {
    let entries = fs::read_dir(mnt).map_err(|e| Error::Dmg(e.to_string()))?;
    entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().is_some_and(|ext| ext == "app"))
        .ok_or_else(|| Error::Dmg("no .app bundle inside the DMG".to_string()))
}

/// Reject anything not signed as a genuine ADI release: the signature must verify
/// (`codesign --verify --deep --strict`) and the signing Team ID must match.
/// `ADI_UPDATE_INSECURE_SKIP_CODESIGN=1` skips this — for tests only.
fn verify_signature(app: &Path) -> Result<(), Error> {
    if std::env::var_os("ADI_UPDATE_INSECURE_SKIP_CODESIGN").is_some_and(|v| v == "1") {
        eprintln!(
            "adi-update: WARNING: skipping code-signature verification (ADI_UPDATE_INSECURE_SKIP_CODESIGN=1)"
        );
        return Ok(());
    }
    let verify = shell::run(&[
        Path::new("/usr/bin/codesign").as_os_str(),
        std::ffi::OsStr::new("--verify"),
        std::ffi::OsStr::new("--deep"),
        std::ffi::OsStr::new("--strict"),
        app.as_os_str(),
    ]);
    if !verify.ok() {
        return Err(Error::Signature(verify.text.trim().to_string()));
    }
    let expected_team = std::env::var("ADI_UPDATE_TEAM_ID")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_TEAM_ID.to_string());
    let info = shell::run(&[
        Path::new("/usr/bin/codesign").as_os_str(),
        std::ffi::OsStr::new("-dv"),
        std::ffi::OsStr::new("--verbose=4"),
        app.as_os_str(),
    ]);
    let wanted = format!("TeamIdentifier={expected_team}");
    if !info.text.lines().any(|l| l.trim() == wanted) {
        return Err(Error::Signature(format!(
            "bundle is not signed by team {expected_team}"
        )));
    }
    Ok(())
}

/// Mounted-DMG guard: always detaches, even on an error path.
#[derive(Debug)]
struct Mount {
    mnt: PathBuf,
}

impl Mount {
    fn attach(dmg: &Path, mnt: &Path) -> Result<Self, Error> {
        let out = shell::run(&[
            Path::new("/usr/bin/hdiutil").as_os_str(),
            std::ffi::OsStr::new("attach"),
            dmg.as_os_str(),
            std::ffi::OsStr::new("-nobrowse"),
            std::ffi::OsStr::new("-noautoopen"),
            std::ffi::OsStr::new("-readonly"),
            std::ffi::OsStr::new("-mountpoint"),
            mnt.as_os_str(),
        ]);
        if !out.ok() {
            return Err(Error::Dmg(format!(
                "hdiutil attach failed: {}",
                out.text.trim()
            )));
        }
        Ok(Self {
            mnt: mnt.to_path_buf(),
        })
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        let gentle = shell::run(&[
            Path::new("/usr/bin/hdiutil").as_os_str(),
            std::ffi::OsStr::new("detach"),
            self.mnt.as_os_str(),
            std::ffi::OsStr::new("-quiet"),
        ]);
        if !gentle.ok() {
            let _ = shell::run(&[
                Path::new("/usr/bin/hdiutil").as_os_str(),
                std::ffi::OsStr::new("detach"),
                self.mnt.as_os_str(),
                std::ffi::OsStr::new("-force"),
                std::ffi::OsStr::new("-quiet"),
            ]);
        }
    }
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
    fn find_app_locates_the_bundle() {
        let dir = scratch("findapp");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("ADI.app")).unwrap();
        fs::create_dir_all(dir.join("noise")).unwrap();
        assert_eq!(find_app(&dir).expect("found"), dir.join("ADI.app"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_app_errors_on_an_empty_dir() {
        let dir = scratch("findapp-empty");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(matches!(find_app(&dir), Err(Error::Dmg(_))));
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
        let _ = fs::remove_dir_all(&dir);
    }
}
