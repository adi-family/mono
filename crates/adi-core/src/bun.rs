//! Making sure `bun` is on the machine — the runtime every dashboard's pair of servers runs under.
//!
//! The platform has always *assumed* bun rather than installed it. [`adi_config::augmented_path`]
//! and the unit `PATH` in [`crate::launchd`] both name `~/.bun/bin` so that a supervised runner can
//! resolve a bare `bun run …`, and neither of them ever put anything there. On a Linux node
//! `apps/linux/install.sh` closes that gap at install time; macOS had no equivalent, because the
//! app ships as a signed bundle and there is no install script to hang it on.
//!
//! The failure that leaves behind is the silent one [`crate::dashboards`] was written for: a Mac
//! that never installed bun by hand still scaffolds a dashboard, still lists it, still derives its
//! hostname — and serves a dead host, because the supervisor cannot start what it cannot resolve.
//!
//! So this is the node installer's bun step, for macOS, written in Rust so it can run from
//! [`crate::Adi::enable`] alongside everything else the platform installs.
//!
//! Four decisions worth keeping:
//!
//! * **Into `~/.bun/bin`, not beside our own binaries.** On Linux the installer puts bun in
//!   `$PREFIX/bin` because that is the directory the systemd units carry. The Mac equivalent would
//!   be the app bundle, which is signed, notarized and read-only. `~/.bun/bin` is where bun's own
//!   installer puts it *and* what both PATH builders above already name, so nothing has to learn a
//!   new directory.
//! * **Fetched from oven-sh, pinned and checksummed — never vendored.** bun is MIT but statically
//!   links `JavaScriptCore` (LGPL-2) and tinycc (LGPL-2.1), so shipping the binary inside a signed
//!   bundle would carry their relink obligation into it. Downloading means the operator gets it
//!   from upstream unmodified, exactly as bun's own installer would, and we still pin what runs.
//! * **A bun that is already there is never touched**, not even to bring it up to the pin. An
//!   operator's own bun is theirs, and a dashboard is a `bun run` — it does not care which minor it
//!   gets. Upgrading one out from under a machine's other projects is not this function's business.
//! * **Never fatal.** Only dashboards need bun. A Mac with no route to GitHub is still a working
//!   Mac, so every failure here is a returned [`Outcome`], not a raised error and not a refusal to
//!   bring the rest of the stack up.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::proc;

/// The bun release an ADI install pins.
///
/// Kept in step with `BUN_VERSION` in `apps/linux/install.sh` by hand: a node installs bun from
/// that script and a Mac from here, and one fleet running two different bun versions is a
/// difference nobody would think to look for. Bumping one means bumping the other, and replacing
/// the checksums below with that release's — they are per version, not per file.
pub const VERSION: &str = "1.3.14";

/// `$HOME`-relative directory an ADI-installed bun lands in. See the module header for why this
/// one and not the bundle.
const INSTALL_DIR: [&str; 2] = [".bun", "bin"];

/// The program name, and so the file we look for inside the downloaded archive.
const BUN: &str = "bun";

/// How long to wait for the connection before deciding there is no route to GitHub. Deliberately
/// short: [`crate::Adi::ensure_enabled`] runs on every app launch, and on a machine with no
/// outbound access this is the only part of that launch which would otherwise sit and wait.
const CONNECT_TIMEOUT_SECS: u32 = 5;

/// How long the download itself may take. Long, because this is ~23 MB over whatever connection
/// the machine has, and giving up on a slow link would just fail the same way on the next launch.
const MAX_TIME_SECS: u32 = 600;

/// One of bun's published macOS builds: the release-asset stem, and the SHA-256 of the `.zip` it
/// is published in (from that release's `SHASUMS256.txt`).
struct Artifact {
    /// The asset stem, e.g. `bun-darwin-aarch64` — both the file name and the directory inside it.
    variant: &'static str,
    /// Hex SHA-256 of `<variant>.zip`, checked before the file is ever made executable.
    sha256: &'static str,
}

impl Artifact {
    /// Where oven-sh publishes this build.
    fn url(&self) -> String {
        format!(
            "https://github.com/oven-sh/bun/releases/download/bun-v{VERSION}/{}.zip",
            self.variant
        )
    }
}

/// What [`ensure`] did. Serializable so the CLI can print it as JSON without a second shape.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    /// bun was already resolvable, and was left exactly as it was.
    Present {
        /// Where it was found.
        path: PathBuf,
        /// What `bun --version` says, or `unknown` if it would not answer.
        version: String,
    },
    /// bun was absent and has been fetched, verified and installed.
    Installed {
        /// Where it now is.
        path: PathBuf,
        /// What the freshly installed copy reports, or the pinned [`VERSION`] if it would not run.
        version: String,
    },
    /// This platform does not install bun from here — Linux nodes get it from
    /// `apps/linux/install.sh`, and no Windows build is pinned yet.
    Unmanaged,
    /// bun is absent and could not be installed. Said in a sentence, for a human.
    Failed {
        /// What went wrong, in the imperative-free past tense the CLI prints verbatim.
        why: String,
    },
}

impl Outcome {
    /// Whether bun is on the machine now, however it got there. False only for
    /// [`Failed`](Outcome::Failed) and for a platform we do not manage.
    #[must_use]
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Present { .. } | Self::Installed { .. })
    }

    /// One line, for a terminal.
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::Present { path, version } => {
                format!("bun {version} already installed at {}", path.display())
            }
            Self::Installed { path, version } => {
                format!("bun {version} installed into {}", path.display())
            }
            Self::Unmanaged => {
                "bun is not installed from here on this platform; dashboards need it".to_string()
            }
            Self::Failed { why } => {
                format!("continuing without bun — dashboards will not run: {why}")
            }
        }
    }
}

/// Make sure bun is on the machine, installing the pinned build if it is not.
///
/// Cheap and side-effect-free in the common case: a machine that has bun pays one `stat` per
/// candidate directory and nothing else. Only a machine without it reaches the network.
#[must_use]
pub fn ensure() -> Outcome {
    ensure_under(home().as_deref())
}

/// [`ensure`], against an explicit home directory.
///
/// The seam exists so the tests can point the whole module at a scratch directory — the same one
/// `Config::with_root` gives the store. Mutating `HOME` instead would be process-wide, and this
/// crate's tests share a process.
fn ensure_under(home: Option<&Path>) -> Outcome {
    if let Some(path) = located_under(home) {
        let version = version_of(&path);
        return Outcome::Present { path, version };
    }
    let Some(artifact) = artifact() else {
        return Outcome::Unmanaged;
    };
    match install(&artifact, home) {
        Ok(path) => {
            let version = version_of(&path);
            Outcome::Installed { path, version }
        }
        Err(why) => Outcome::Failed { why },
    }
}

/// Where bun already is: our own copy first, then anything the operator put on the augmented
/// `PATH`.
///
/// Ours is preferred over `PATH` deliberately. The two are usually the same file, and when they
/// are not, the one this function would have installed is the one whose version we pin and whose
/// checksum we checked.
#[must_use]
pub fn located() -> Option<PathBuf> {
    located_under(home().as_deref())
}

/// [`located`], against an explicit home directory. See [`ensure_under`].
fn located_under(home: Option<&Path>) -> Option<PathBuf> {
    let ours = home.map(install_path_under);
    if ours.as_ref().is_some_and(|p| p.is_file()) {
        return ours;
    }
    // Not `command -v`: that is a shell builtin, so asking it costs a `/bin/sh` and answers about
    // the shell's PATH rather than the one a supervised runner is actually given.
    adi_config::augmented_path()
        .split(':')
        .filter(|dir| !dir.is_empty())
        .map(|dir| Path::new(dir).join(BUN))
        .find(|candidate| candidate.is_file())
}

/// Where an ADI-installed bun goes: `$HOME/.bun/bin/bun`. `None` when this process has no `HOME`,
/// which is a real launchd possibility and the one case where there is nowhere to put it.
#[must_use]
pub fn install_path() -> Option<PathBuf> {
    Some(install_path_under(&home()?))
}

/// [`install_path`], against an explicit home directory. See [`ensure_under`].
fn install_path_under(home: &Path) -> PathBuf {
    let mut path = home.to_path_buf();
    path.extend(INSTALL_DIR);
    path.push(BUN);
    path
}

/// This process's home directory — the one thing here that reads the environment.
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// The build to fetch for this machine, or `None` on a platform that gets bun some other way.
///
/// Gated with `cfg!` rather than `#[cfg]` so every platform still *compiles* this module: a macOS
/// arm that only a Mac ever type-checks is exactly how two releases in this tree shipped broken.
fn artifact() -> Option<Artifact> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    // Ask the machine, not the binary. `std::env::consts::ARCH` is what *this build* was compiled
    // for, so an x86_64 build running under Rosetta on Apple Silicon would fetch the x64 bun and
    // then run it translated. `hw.optional.arm64` is 1 on an Apple Silicon Mac either way.
    if sysctl_flag("hw.optional.arm64") {
        return Some(Artifact {
            variant: "bun-darwin-aarch64",
            sha256: "d8b96221828ad6f97ac7ac0ab7e95872341af763001e8803e8267652c2652620",
        });
    }
    // bun publishes two Intel builds: the default needs AVX2, `-baseline` does not. Picking wrong
    // gives an "Illegal instruction" crash the first time a dashboard starts, rather than a clear
    // failure here — so ask the CPU, exactly as the Linux installer reads `/proc/cpuinfo`.
    if sysctl_flag("hw.optional.avx2_0") {
        Some(Artifact {
            variant: "bun-darwin-x64",
            sha256: "4183df3374623e5bab315c547cfa0974533cd457d86b73b639f7a87974cd6633",
        })
    } else {
        Some(Artifact {
            variant: "bun-darwin-x64-baseline",
            sha256: "3e35ad6f53971a9834bf9e6786e2adf72b5f1921cc9a9c5fde073d2972944076",
        })
    }
}

/// Whether a boolean `sysctl` reads as set. A key the kernel does not have prints nothing and
/// exits non-zero, which reads the same as `0` — correct for every `hw.optional.*` flag, since an
/// absent capability key means the machine does not have the capability.
fn sysctl_flag(key: &str) -> bool {
    let out = proc::run(&["/usr/sbin/sysctl", "-n", key]);
    out.ok() && out.text.trim() == "1"
}

/// Download, verify, unpack and place the pinned build. Returns where it landed.
fn install(artifact: &Artifact, home: Option<&Path>) -> Result<PathBuf, String> {
    let target = home.map(install_path_under).ok_or_else(|| {
        "this process has no HOME, so there is nowhere to install bun".to_string()
    })?;
    let staging = Staging::new()?;
    let zip = staging.dir.join("bun.zip");

    let out = proc::run(&[
        "curl",
        "-fsSL",
        "--retry",
        "3",
        "--connect-timeout",
        &CONNECT_TIMEOUT_SECS.to_string(),
        "--max-time",
        &MAX_TIME_SECS.to_string(),
        "-o",
        &zip.to_string_lossy(),
        &artifact.url(),
    ]);
    if !out.ok() {
        return Err(format!(
            "could not download {}: {}",
            artifact.url(),
            out.text.trim()
        ));
    }

    // Verified before it is ever made executable: this binary goes on to run every dashboard on
    // the machine, and a checksum checked afterwards has already lost the argument.
    let got = adi_update::sha256(&zip).map_err(|e| e.to_string())?;
    if got != artifact.sha256 {
        return Err(format!(
            "checksum mismatch for {}: expected {}, got {got}",
            artifact.variant, artifact.sha256
        ));
    }

    // One `tar -xf` reads the zip: macOS's `tar` is bsdtar, which handles zip as well as tar.gz
    // and detects the format itself. It is also why this needs no `unzip` — the tool bun's own
    // installer stops without.
    let out = proc::run(&[
        std::ffi::OsStr::new("tar"),
        std::ffi::OsStr::new("-xf"),
        zip.as_os_str(),
        std::ffi::OsStr::new("-C"),
        staging.dir.as_os_str(),
    ]);
    if !out.ok() {
        return Err(format!("could not unpack bun: {}", out.text.trim()));
    }
    let unpacked = find_bun(&staging.dir, 2)
        .ok_or_else(|| "no bun executable inside the archive".to_string())?;

    let dir = target
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", target.display()))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;

    // Staged beside the target and renamed into place, rather than copied onto it: `~/.bun/bin` is
    // on the PATH of everything the platform launches, so a half-written `bun` there would be
    // found and run. `rename` within one directory cannot be partial, and cannot cross a device.
    let pending = dir.join(format!("{BUN}.adi-incoming"));
    let _ = std::fs::remove_file(&pending);
    std::fs::copy(&unpacked, &pending)
        .map_err(|e| format!("could not place bun in {}: {e}", dir.display()))?;
    make_executable(&pending)?;
    std::fs::rename(&pending, &target).map_err(|e| {
        let _ = std::fs::remove_file(&pending);
        format!("could not install bun to {}: {e}", target.display())
    })?;
    Ok(target)
}

/// Restore the executable bit `std::fs::copy` carries but a future non-unix arm would not.
fn make_executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("could not chmod {}: {e}", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// The `bun` file within `root`, searched breadth-first to `depth` levels — the archive holds it
/// one level down, in a directory named after the variant, but that is bun's layout to change.
fn find_bun(root: &Path, depth: usize) -> Option<PathBuf> {
    let mut level = vec![root.to_path_buf()];
    for _ in 0..depth {
        let mut next = Vec::new();
        for dir in level {
            let candidate = dir.join(BUN);
            if candidate.is_file() {
                return Some(candidate);
            }
            if let Ok(entries) = std::fs::read_dir(&dir) {
                next.extend(
                    entries
                        .filter_map(|e| e.ok().map(|e| e.path()))
                        .filter(|p| p.is_dir()),
                );
            }
        }
        level = next;
    }
    None
}

/// What a bun at this path calls itself, or `unknown` when it will not answer — which is worth
/// reporting rather than failing on, since the file being there is what the dashboards need.
fn version_of(path: &Path) -> String {
    let out = proc::run(&[path.as_os_str(), std::ffi::OsStr::new("--version")]);
    let version = out.text.trim();
    if out.ok() && !version.is_empty() {
        version.to_string()
    } else {
        "unknown".to_string()
    }
}

/// A scratch directory that removes itself, so a failed or panicking install leaves no ~23 MB
/// archive behind in the temp dir.
struct Staging {
    dir: PathBuf,
}

impl Staging {
    fn new() -> Result<Self, String> {
        let dir = std::env::temp_dir().join(format!("adi-bun-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
        Ok(Self { dir })
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "adi-core-bun-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[test]
    fn the_install_path_sits_under_the_home_it_is_given() {
        let home = scratch("install-path");
        assert_eq!(
            install_path_under(&home),
            home.join(".bun").join("bin").join("bun")
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The contract the whole module rests on: the directory it installs into is one
    /// [`adi_config::augmented_path`] hands to every supervised runner. If that list is ever
    /// rewritten without this one, bun lands somewhere a dashboard cannot resolve it from — the
    /// exact failure on a node that made `apps/linux/install.sh` fetch bun into `$PREFIX/bin`.
    #[test]
    fn what_we_install_into_is_on_the_path_a_runner_is_launched_with() {
        let Some(path) = install_path() else {
            return; // No HOME in this environment; nothing to assert about.
        };
        let dir = path
            .parent()
            .expect("parent")
            .to_string_lossy()
            .into_owned();
        assert!(
            adi_config::augmented_path()
                .split(':')
                .any(|entry| entry == dir),
            "augmented_path() no longer contains {dir}"
        );
    }

    /// An existing bun is reported and left alone — the promise in the module header, and the one
    /// behaviour a bad edit here would break silently on somebody else's machine.
    #[test]
    fn an_existing_bun_is_reported_rather_than_replaced() {
        let home = scratch("existing");
        let bin = home.join(".bun").join("bin");
        std::fs::create_dir_all(&bin).expect("bin");
        let fake = bin.join("bun");
        std::fs::write(&fake, "#!/bin/sh\necho 9.9.9\n").expect("write");
        make_executable(&fake).expect("chmod");

        let outcome = ensure_under(Some(&home));
        assert!(outcome.is_available(), "{outcome:?}");
        match &outcome {
            Outcome::Present { path, version } => {
                assert_eq!(path, &fake);
                assert_eq!(version, "9.9.9");
            }
            other => panic!("expected an already-present bun, got {other:?}"),
        }
        // Untouched: the bytes that were written, not a downloaded binary.
        assert_eq!(
            std::fs::read_to_string(&fake).expect("read"),
            "#!/bin/sh\necho 9.9.9\n"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Nowhere to put it is a reported failure, not a panic and not a silent success. A launchd
    /// job with no `HOME` is a real way to reach this.
    #[test]
    fn no_home_is_a_failure_with_a_reason() {
        // A home that exists but holds no bun would reach the network, so this asserts only the
        // one branch that cannot: no home at all.
        match install(
            &Artifact {
                variant: "bun-darwin-aarch64",
                sha256: "d8b96221828ad6f97ac7ac0ab7e95872341af763001e8803e8267652c2652620",
            },
            None,
        ) {
            Err(why) => assert!(why.contains("HOME"), "{why}"),
            Ok(path) => panic!("installed to {} with no HOME", path.display()),
        }
    }

    #[test]
    fn find_bun_reaches_the_directory_the_archive_actually_uses() {
        let root = scratch("find");
        let inner = root.join("bun-darwin-aarch64");
        std::fs::create_dir_all(&inner).expect("inner");
        std::fs::write(inner.join("bun"), "binary").expect("write");
        assert_eq!(find_bun(&root, 2), Some(inner.join("bun")));
        // A directory deeper than the search is not found, which is the point of the bound.
        assert_eq!(find_bun(&root, 1), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every pinned artifact must name a real asset of the pinned release, and carry a hex
    /// SHA-256 of the right length — the two ways a hand-edited pin goes wrong without failing
    /// until a machine with no bun tries to install one.
    #[test]
    fn the_pins_are_well_formed() {
        for artifact in [
            Artifact {
                variant: "bun-darwin-aarch64",
                sha256: "d8b96221828ad6f97ac7ac0ab7e95872341af763001e8803e8267652c2652620",
            },
            Artifact {
                variant: "bun-darwin-x64",
                sha256: "4183df3374623e5bab315c547cfa0974533cd457d86b73b639f7a87974cd6633",
            },
            Artifact {
                variant: "bun-darwin-x64-baseline",
                sha256: "3e35ad6f53971a9834bf9e6786e2adf72b5f1921cc9a9c5fde073d2972944076",
            },
        ] {
            assert_eq!(artifact.sha256.len(), 64, "{}", artifact.variant);
            assert!(
                artifact.sha256.chars().all(|c| c.is_ascii_hexdigit()),
                "{}",
                artifact.variant
            );
            assert!(
                artifact
                    .url()
                    .ends_with(&format!("{}.zip", artifact.variant))
            );
            assert!(artifact.url().contains(&format!("bun-v{VERSION}/")));
        }
    }

    /// The one thing a non-macOS build must do here, and the reason the platform test is `cfg!`
    /// rather than `#[cfg]`: every platform compiles this, and every platform answers.
    #[test]
    fn only_macos_installs_bun_from_here() {
        assert_eq!(artifact().is_some(), cfg!(target_os = "macos"));
    }
}
