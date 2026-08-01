//! Turning a verified download into an installed payload — the one part of an update that
//! genuinely differs per platform.
//!
//! * **macOS** ships the whole notarized `ADI.app` inside a DMG. The bundle is mounted,
//!   signature-checked, copied out, and swapped in with a directory rename.
//! * **Linux / Windows** ship a tarball or zip of loose binaries. There is no bundle and no
//!   codesign, so the trust anchor is the manifest's sha256 fetched over TLS; the binaries
//!   replace the ones beside the running executable, one rename-aside at a time.
//!
//! Both variants compile on every host — only the *choice* is per-OS ([`Payload::for_host`]) —
//! so the archive path stays testable from a macOS checkout, the same trick `adi-core`'s
//! `launchd` module uses for systemd units.

use std::fs;
use std::path::{Path, PathBuf};

use crate::engine::{DEFAULT_APP_PATH, Error};
use crate::shell;
use crate::state;

/// The file name of the CLI inside a payload — `adi-mono`, or `adi-mono.exe` on Windows.
fn mono_file() -> String {
    format!("adi-mono{}", std::env::consts::EXE_SUFFIX)
}

/// How deep [`find_bin_dir`] looks for the binaries inside an unpacked archive. The Linux
/// package puts them in `<pkg>/bin`, the Windows one directly in `<pkg>`; two levels covers
/// both with room to spare, and bounds the walk on a hostile archive.
const UNPACK_SEARCH_DEPTH: usize = 3;

/// What this host installs, and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Payload {
    /// A whole `ADI.app` bundle, delivered as a DMG.
    Bundle,
    /// Loose binaries, delivered as a tar.gz (Linux) or zip (Windows).
    Archive,
}

impl Payload {
    /// The payload kind this OS is updated with.
    pub(crate) fn for_host() -> Self {
        if cfg!(target_os = "macos") {
            Self::Bundle
        } else {
            Self::Archive
        }
    }

    /// The live install this payload replaces: the app bundle, or the directory the running
    /// executable sits in (which is what `apps/linux/install.sh` populated as `$PREFIX/bin`).
    pub(crate) fn target(self) -> PathBuf {
        match self {
            Self::Bundle => std::env::var_os("ADI_UPDATE_APP")
                .filter(|v| !v.is_empty())
                .map_or_else(|| PathBuf::from(DEFAULT_APP_PATH), PathBuf::from),
            Self::Archive => std::env::var_os("ADI_UPDATE_BIN_DIR")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::current_exe()
                        .ok()
                        .and_then(|exe| exe.parent().map(Path::to_path_buf))
                })
                .unwrap_or_default(),
        }
    }

    /// The version currently installed at `target` — the number update decisions compare
    /// against. Read from the install itself, never from this process: the running CLI may be
    /// a repo dev build while the installed payload is something else entirely.
    pub(crate) fn installed_version(self, target: &Path) -> String {
        let found = match self {
            Self::Bundle => plist_version(&target.join("Contents/Info.plist")),
            Self::Archive => fs::read_to_string(target.join("VERSION"))
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
        };
        found.unwrap_or_else(|| crate::BUILT_VERSION.to_string())
    }

    /// Unpack and authenticate `downloaded` inside `staging`, returning the staged payload —
    /// the `.app` to swap in, or the directory holding the new binaries.
    pub(crate) fn stage(self, downloaded: &Path, staging: &Path) -> Result<PathBuf, Error> {
        match self {
            Self::Bundle => stage_dmg(downloaded, staging),
            Self::Archive => stage_archive(downloaded, staging),
        }
    }

    /// Run the staged CLI before anything is swapped: it must start, and it must report the
    /// version the manifest promised.
    ///
    /// Exit status catches the payload that cannot run at all (wrong arch, missing library, a
    /// truncated build). The version comparison catches a *mis-stamped* release — one whose
    /// binaries were compiled without the release tag. That one matters more than it looks:
    /// the installed version would stay behind the published one, so every scheduled check
    /// would reinstall the same release forever. Both fail closed, before the live install is
    /// touched.
    pub(crate) fn preflight(self, staged: &Path, expected: &str) -> Result<(), Error> {
        let mono = match self {
            Self::Bundle => staged.join("Contents/Resources").join(mono_file()),
            Self::Archive => staged.join(mono_file()),
        };
        if !mono.exists() {
            return Err(Error::Preflight(format!(
                "the downloaded payload has no {} at {}",
                mono_file(),
                mono.display()
            )));
        }
        let out = shell::capture(&[mono.as_os_str(), std::ffi::OsStr::new("--version")]);
        if !out.ok() {
            return Err(Error::Preflight(format!(
                "`{} --version` failed ({}): {}",
                mono.display(),
                out.status,
                out.stderr.trim()
            )));
        }
        let reported = String::from_utf8_lossy(&out.stdout);
        let reported = reported.trim();
        if !reported.split_whitespace().any(|w| w == expected) {
            return Err(Error::Preflight(format!(
                "the release is mis-stamped: manifest says {expected}, but the downloaded \
                 binary reports {reported:?} — installing it would leave this machine \
                 reinstalling the same release on every check"
            )));
        }
        Ok(())
    }

    /// Move `staged` into `target`, parking whatever was there under `backups`. Returns the
    /// backup path, or `None` when there was no previous install to keep.
    ///
    /// Never leaves the machine without an install: a failure part-way through puts back
    /// everything already moved.
    pub(crate) fn swap(
        self,
        staged: &Path,
        target: &Path,
        backups: &Path,
        from: &str,
        version: &str,
    ) -> Result<Option<PathBuf>, Error> {
        fs::create_dir_all(backups).map_err(|e| Error::Install(e.to_string()))?;
        match self {
            Self::Bundle => swap_bundle(staged, target, backups, from),
            Self::Archive => swap_binaries(staged, target, backups, from, version),
        }
    }

    /// Put a backup taken by [`Self::swap`] back in place — the rollback path, used when the
    /// freshly-installed version fails its health check.
    pub(crate) fn restore(self, backup: &Path, target: &Path) -> Result<(), Error> {
        match self {
            Self::Bundle => {
                let _ = fs::remove_dir_all(target);
                fs::rename(backup, target).map_err(|e| Error::Install(e.to_string()))
            }
            Self::Archive => {
                for entry in read_files(backup)? {
                    let name = entry.file_name().ok_or_else(|| {
                        Error::Install(format!("unnamed backup entry {}", entry.display()))
                    })?;
                    place(&entry, &target.join(name))?;
                }
                Ok(())
            }
        }
    }

    /// Whether `name` in the backups directory was written by this payload kind — what
    /// pruning uses to keep the two naming schemes apart.
    pub(crate) fn owns_backup(self, name: &str) -> bool {
        match self {
            Self::Bundle => name.starts_with("ADI.app."),
            Self::Archive => name.starts_with("bin."),
        }
    }
}

// ── macOS: the DMG bundle ───────────────────────────────────────────────────────────────────

/// `CFBundleShortVersionString` from an `Info.plist`, if it is readable.
fn plist_version(plist: &Path) -> Option<String> {
    if !plist.exists() {
        return None;
    }
    let out = shell::capture(&[
        "/usr/bin/plutil",
        "-extract",
        "CFBundleShortVersionString",
        "raw",
        "-o",
        "-",
        &plist.to_string_lossy(),
    ]);
    out.ok()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|v| !v.is_empty())
}

fn stage_dmg(dmg: &Path, staging: &Path) -> Result<PathBuf, Error> {
    let mnt = staging.join("mnt");
    fs::create_dir_all(&mnt).map_err(|e| Error::Dmg(e.to_string()))?;
    let staged = staging.join("ADI.app");
    {
        let _mount = Mount::attach(dmg, &mnt)?;
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
    } // <- detach before the caller touches the live install
    Ok(staged)
}

/// Rename the old bundle aside, rename the new one in. Same-volume renames, so the window
/// with no app at the target path is two atomic metadata operations.
fn swap_bundle(
    staged: &Path,
    target: &Path,
    backups: &Path,
    from: &str,
) -> Result<Option<PathBuf>, Error> {
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let backup = if target.exists() {
        let dest = backups.join(format!("ADI.app.{from}.{}", state::now_unix()));
        fs::rename(target, &dest).map_err(|e| Error::Install(swap_hint(&e, target)))?;
        Some(dest)
    } else {
        None
    };
    if let Err(e) = fs::rename(staged, target) {
        if let Some(b) = &backup {
            let _ = fs::rename(b, target);
        }
        return Err(Error::Install(swap_hint(&e, target)));
    }
    Ok(backup)
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
        .unwrap_or_else(|| crate::engine::DEFAULT_TEAM_ID.to_string());
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

/// Map a swap failure to a message that names the usual culprit (macOS App Management blocks
/// unentitled processes from replacing a signed bundle in /Applications).
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

// ── Linux / Windows: the binary archive ─────────────────────────────────────────────────────

/// Unpack the archive and locate the binaries inside it.
///
/// One `tar -xf` handles both packages: bsdtar — which is what `tar.exe` is on Windows 10 and
/// later — reads zip as well as tar.gz, and detects the compression itself.
fn stage_archive(archive: &Path, staging: &Path) -> Result<PathBuf, Error> {
    let unpack = staging.join("unpack");
    fs::create_dir_all(&unpack).map_err(|e| Error::Archive(e.to_string()))?;
    let out = shell::run(&[
        std::ffi::OsStr::new("tar"),
        std::ffi::OsStr::new("-xf"),
        archive.as_os_str(),
        std::ffi::OsStr::new("-C"),
        unpack.as_os_str(),
    ]);
    if !out.ok() {
        return Err(Error::Archive(format!(
            "tar could not unpack {}: {}",
            archive.display(),
            out.text.trim()
        )));
    }
    find_bin_dir(&unpack, UNPACK_SEARCH_DEPTH).ok_or_else(|| {
        Error::Archive(format!(
            "no {} anywhere in the unpacked archive",
            mono_file()
        ))
    })
}

/// The directory holding `adi-mono` within `root`, searched breadth-first to `depth` levels.
/// The Linux package keeps it in `<pkg>/bin`, the Windows one directly in `<pkg>`.
fn find_bin_dir(root: &Path, depth: usize) -> Option<PathBuf> {
    let mut level = vec![root.to_path_buf()];
    let mono = mono_file();
    for _ in 0..depth {
        let mut next = Vec::new();
        for dir in level {
            if dir.join(&mono).is_file() {
                return Some(dir);
            }
            if let Ok(entries) = fs::read_dir(&dir) {
                next.extend(
                    entries
                        .filter_map(|e| e.ok().map(|e| e.path()))
                        .filter(|p| p.is_dir()),
                );
            }
        }
        if next.is_empty() {
            return None;
        }
        level = next;
    }
    None
}

/// Replace each binary in `target` with the staged one, parking the old copy in a per-update
/// backup directory first.
///
/// Rename-aside rather than overwrite-in-place, because the file being replaced is very
/// likely *running* — including this very process. Both platforms allow renaming a running
/// executable (only deleting it is refused on Windows), and the running image keeps the file
/// it already opened, so the swap is safe while the old services are still up.
fn swap_binaries(
    staged: &Path,
    target: &Path,
    backups: &Path,
    from: &str,
    version: &str,
) -> Result<Option<PathBuf>, Error> {
    fs::create_dir_all(target).map_err(|e| Error::Install(e.to_string()))?;
    let backup = backups.join(format!("bin.{from}.{}", state::now_unix()));
    fs::create_dir_all(&backup).map_err(|e| Error::Install(e.to_string()))?;

    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new(); // (parked copy, where it came from)
    let mut had_previous = false;
    for src in read_files(staged)? {
        let Some(name) = src.file_name() else { continue };
        let dest = target.join(name);
        if dest.exists() {
            let parked = backup.join(name);
            if let Err(e) = place(&dest, &parked) {
                unwind(&moved, target);
                return Err(e);
            }
            moved.push((parked, dest.clone()));
            had_previous = true;
        }
        if let Err(e) = place(&src, &dest).and_then(|()| make_executable(&dest)) {
            unwind(&moved, target);
            return Err(e);
        }
    }

    // The archive carries no Info.plist, so the installed version is recorded beside the
    // binaries — this file is what `installed_version` reads on the next check.
    fs::write(target.join("VERSION"), format!("{version}\n"))
        .map_err(|e| Error::Install(format!("could not record the installed version: {e}")))?;

    Ok(had_previous.then_some(backup))
}

/// Put every parked file back where it came from, best effort — the machine keeps a working
/// install even if the thing that interrupted the swap also breaks the unwind.
fn unwind(moved: &[(PathBuf, PathBuf)], _target: &Path) {
    for (parked, original) in moved {
        let _ = place(parked, original);
    }
}

/// Move `from` to `to`, falling back to copy-then-remove when the two are on different
/// filesystems (`~/.adi` and `$PREFIX/bin` need not share a volume).
fn place(from: &Path, to: &Path) -> Result<(), Error> {
    if let Some(parent) = to.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::rename(from, to).is_err() {
        fs::copy(from, to)
            .map_err(|e| Error::Install(format!("could not place {}: {e}", to.display())))?;
        let _ = fs::remove_file(from);
    }
    Ok(())
}

/// Restore the executable bit a copy-based `place` would have dropped. A no-op off unix,
/// where executability is carried by the file name.
fn make_executable(path: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|e| {
            Error::Install(format!("could not chmod {}: {e}", path.display()))
        })?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// The plain files directly inside `dir`, sorted so a swap is reproducible.
fn read_files(dir: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| Error::Install(format!("could not read {}: {e}", dir.display())))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    Ok(files)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "adi-update-payload-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, body: &str) {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn find_app_locates_the_bundle() {
        let dir = scratch("findapp");
        fs::create_dir_all(dir.join("ADI.app")).unwrap();
        fs::create_dir_all(dir.join("noise")).unwrap();
        assert_eq!(find_app(&dir).expect("found"), dir.join("ADI.app"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_app_errors_on_an_empty_dir() {
        let dir = scratch("findapp-empty");
        assert!(matches!(find_app(&dir), Err(Error::Dmg(_))));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_bin_dir_handles_both_package_layouts() {
        // Linux: <pkg>/bin/adi-mono
        let linux = scratch("layout-linux");
        write(&linux.join("adi-linux-x64/bin").join(mono_file()), "x");
        write(&linux.join("adi-linux-x64/README.md"), "docs");
        assert_eq!(
            find_bin_dir(&linux, UNPACK_SEARCH_DEPTH).expect("linux"),
            linux.join("adi-linux-x64/bin")
        );

        // Windows: <pkg>/adi-mono.exe, no bin/ level at all.
        let win = scratch("layout-win");
        write(&win.join("ADI-windows-x64").join(mono_file()), "x");
        assert_eq!(
            find_bin_dir(&win, UNPACK_SEARCH_DEPTH).expect("win"),
            win.join("ADI-windows-x64")
        );

        let empty = scratch("layout-empty");
        write(&empty.join("pkg/README.md"), "no binaries here");
        assert!(find_bin_dir(&empty, UNPACK_SEARCH_DEPTH).is_none());

        for d in [linux, win, empty] {
            let _ = fs::remove_dir_all(d);
        }
    }

    #[test]
    fn swapping_binaries_parks_the_old_ones_and_records_the_version() {
        let root = scratch("swap-archive");
        let staged = root.join("staged");
        let target = root.join("bin");
        let backups = root.join("backups");
        write(&staged.join(mono_file()), "new-mono");
        write(&staged.join("adi-dns"), "new-dns");
        write(&target.join(mono_file()), "old-mono");
        write(&target.join("adi-dns"), "old-dns");

        let backup = Payload::Archive
            .swap(&staged, &target, &backups, "0.1.0", "0.2.0")
            .expect("swap")
            .expect("a previous install was there, so there is a backup");

        assert_eq!(fs::read_to_string(target.join(mono_file())).unwrap(), "new-mono");
        assert_eq!(fs::read_to_string(backup.join(mono_file())).unwrap(), "old-mono");
        assert_eq!(fs::read_to_string(target.join("VERSION")).unwrap(), "0.2.0\n");
        assert_eq!(Payload::Archive.installed_version(&target), "0.2.0");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_first_install_reports_no_backup() {
        let root = scratch("swap-fresh");
        let staged = root.join("staged");
        write(&staged.join(mono_file()), "mono");
        let backup = Payload::Archive
            .swap(&staged, &root.join("bin"), &root.join("backups"), "0.1.0", "0.2.0")
            .expect("swap");
        assert!(backup.is_none(), "nothing was replaced, so nothing to roll back to");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn restoring_a_backup_puts_the_old_binaries_back() {
        let root = scratch("restore-archive");
        let staged = root.join("staged");
        let target = root.join("bin");
        let backups = root.join("backups");
        write(&staged.join(mono_file()), "new-mono");
        write(&target.join(mono_file()), "old-mono");

        let backup = Payload::Archive
            .swap(&staged, &target, &backups, "0.1.0", "0.2.0")
            .expect("swap")
            .expect("backup");
        Payload::Archive.restore(&backup, &target).expect("restore");

        assert_eq!(fs::read_to_string(target.join(mono_file())).unwrap(), "old-mono");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_archive_install_falls_back_to_the_built_version_without_a_version_file() {
        let dir = scratch("noversion");
        assert_eq!(Payload::Archive.installed_version(&dir), crate::BUILT_VERSION);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_names_stay_in_their_own_lane() {
        assert!(Payload::Bundle.owns_backup("ADI.app.0.1.0.123"));
        assert!(!Payload::Bundle.owns_backup("bin.0.1.0.123"));
        assert!(Payload::Archive.owns_backup("bin.0.1.0.123"));
        assert!(!Payload::Archive.owns_backup("ADI.app.0.1.0.123"));
    }

    #[cfg(unix)]
    #[test]
    fn a_real_tarball_stages_and_preflights_end_to_end() {
        // The one seam the other tests step around: `tar -xf` on a genuine archive laid out
        // the way apps/linux/build.sh lays one out, straight through to running the CLI it
        // contains. Everything here is real — a real gzip, a real unpack, a real exec.
        let root = scratch("stage-tarball");
        let pkg = root.join("src/adi-linux-x64");
        write(&pkg.join("bin").join(mono_file()), "#!/bin/sh\necho 'adi-mono 0.2.0'\n");
        make_executable(&pkg.join("bin").join(mono_file())).unwrap();
        write(&pkg.join("README.md"), "node package");

        let tarball = root.join("adi-linux-x64.tar.gz");
        let tar = shell::run(&[
            std::ffi::OsStr::new("tar"),
            std::ffi::OsStr::new("-czf"),
            tarball.as_os_str(),
            std::ffi::OsStr::new("-C"),
            root.join("src").as_os_str(),
            std::ffi::OsStr::new("adi-linux-x64"),
        ]);
        assert!(tar.ok(), "could not build the fixture tarball: {}", tar.text);

        let staging = root.join("staging");
        fs::create_dir_all(&staging).unwrap();
        let staged = Payload::Archive
            .stage(&tarball, &staging)
            .expect("a node tarball unpacks and its bin/ is found");
        assert!(staged.join(mono_file()).is_file());

        Payload::Archive
            .preflight(&staged, "0.2.0")
            .expect("the staged CLI runs and reports the manifest's version");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn staging_a_file_that_is_not_an_archive_fails_rather_than_installing_nothing() {
        let root = scratch("stage-garbage");
        let junk = root.join("not-an-archive.tar.gz");
        write(&junk, "this is not a tarball");
        let staging = root.join("staging");
        fs::create_dir_all(&staging).unwrap();
        assert!(matches!(
            Payload::Archive.stage(&junk, &staging),
            Err(Error::Archive(_))
        ));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn preflight_rejects_a_payload_with_no_cli() {
        let dir = scratch("preflight-missing");
        let err = Payload::Archive.preflight(&dir, "0.2.0").expect_err("no binary");
        assert!(matches!(err, Error::Preflight(_)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn preflight_accepts_the_promised_version_and_rejects_any_other() {
        let dir = scratch("preflight-version");
        // A stand-in for the real CLI: prints what a mis-stamped build would print.
        let fake = dir.join(mono_file());
        write(&fake, "#!/bin/sh\necho \"adi-mono $FAKE_VERSION\"\n");
        make_executable(&fake).unwrap();

        // Can't set env for the child through `shell::capture`, so bake the number in instead.
        write(&fake, "#!/bin/sh\necho 'adi-mono 0.2.0'\n");
        make_executable(&fake).unwrap();
        Payload::Archive.preflight(&dir, "0.2.0").expect("version matches the manifest");

        let err = Payload::Archive
            .preflight(&dir, "0.3.0")
            .expect_err("a binary that reports an older version than the manifest is mis-stamped");
        assert!(matches!(err, Error::Preflight(_)));

        let _ = fs::remove_dir_all(&dir);
    }
}
