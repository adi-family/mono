//! Where this build is running from, and whether that is somewhere services may point at.
//!
//! Every service adi installs records an absolute path to a binary — `adi-dns`, `adi-hive`,
//! `adi-app`, resolved as siblings of the running executable — and a supervisor keeps starting
//! that path at login for as long as the service exists. So the question that matters at install
//! time is not "is this the App Store copy", it is **will this path still be there tomorrow**.
//!
//! Two places fail that and are easy to end up in by accident:
//!
//! * **A mounted disk image.** Launching straight out of the downloaded `.dmg` without dragging
//!   anywhere is the single most common way to first run a Mac app. The services install happily
//!   and then die the moment the image is ejected, pointing at a `/Volumes` path that no longer
//!   exists — and they cannot be repaired by relaunching, because the next launch is a different
//!   mount.
//! * **App translocation.** Gatekeeper runs a quarantined bundle from a randomised read-only
//!   mount under `/private/var/folders/…/AppTranslocation/…` instead of where it actually is.
//!   That path is per-launch, so a service written from there is stale before it is ever started.
//!
//! Anything else — `/Applications`, `~/Applications`, a checkout's `target/release` — is a real
//! path that persists, and is allowed. This deliberately does *not* insist on `/Applications`:
//! a development build run from the tree is a normal thing to do and there is nothing wrong
//! with it.

use std::path::{Path, PathBuf};

/// Where the running executable lives, judged only by whether services may reference it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// A path that will still be there after a reboot.
    Durable(PathBuf),
    /// A mounted volume — a disk image, a network share, an external disk.
    Volume(PathBuf),
    /// Gatekeeper's randomised read-only copy. Different on every launch.
    Translocated(PathBuf),
}

impl Location {
    /// Whether a service may record a path into this location.
    #[must_use]
    pub fn is_durable(&self) -> bool {
        matches!(self, Self::Durable(_))
    }

    /// The path itself, whatever the verdict.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Durable(p) | Self::Volume(p) | Self::Translocated(p) => p,
        }
    }

    /// What to tell someone who cannot install from here, in the imperative.
    #[must_use]
    pub fn explain(&self) -> Option<String> {
        match self {
            Self::Durable(_) => None,
            Self::Volume(path) => Some(format!(
                "adi is running from a mounted volume ({}).\n\
                 Services installed from here would stop working the moment it is ejected, so \
                 nothing was installed.\n\
                 Drag ADI to your Applications folder and open it from there.",
                path.display()
            )),
            Self::Translocated(_) => Some(
                "macOS is running adi from a temporary randomised copy, which it does to \
                 quarantined apps that have not been moved.\n\
                 That path changes on every launch, so services installed from here would be \
                 stale immediately and nothing was installed.\n\
                 Drag ADI to your Applications folder and open it from there."
                    .to_string(),
            ),
        }
    }
}

/// Classify `path`.
///
/// Split from [`current`] so the interesting part is testable: the real
/// [`std::env::current_exe`] is wherever the test binary happens to live.
#[must_use]
pub fn classify(path: &Path) -> Location {
    let text = path.to_string_lossy();
    // Translocation is checked first: the randomised mount is itself under a path that looks
    // like nothing else, and a translocated bundle is not on /Volumes.
    if text.contains("/AppTranslocation/") {
        Location::Translocated(path.to_path_buf())
    } else if path.starts_with("/Volumes/") {
        Location::Volume(path.to_path_buf())
    } else {
        Location::Durable(path.to_path_buf())
    }
}

/// Where this process is running from.
#[must_use]
pub fn current() -> Location {
    std::env::current_exe().map_or_else(
        // Unknowable rather than bad: refusing to install because the path could not be read
        // would be worse than the risk it is guarding against.
        |_| Location::Durable(PathBuf::new()),
        |exe| classify(&exe),
    )
}

/// Refuse, loudly, if services must not be installed from here.
///
/// Returns `false` when the caller should stop. The message goes to stderr rather than being
/// returned, because every caller does the same thing with it and the CLI's job here is to say
/// the one useful sentence: move the app.
#[must_use]
pub fn allow_install() -> bool {
    match current().explain() {
        None => true,
        Some(reason) => {
            eprintln!("adi: {reason}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_normal_install_is_durable() {
        assert!(
            classify(Path::new(
                "/Applications/ADI.app/Contents/Resources/adi-mono"
            ))
            .is_durable()
        );
        assert!(
            classify(Path::new(
                "/Users/x/Applications/ADI.app/Contents/Resources/adi-mono"
            ))
            .is_durable()
        );
    }

    /// A checkout is a perfectly good place to run from — this guard is about paths that
    /// *disappear*, not about blessing `/Applications`.
    #[test]
    fn a_development_build_is_durable() {
        assert!(classify(Path::new("/Users/x/adi-family/target/release/adi-mono")).is_durable());
    }

    #[test]
    fn a_mounted_disk_image_is_not() {
        let loc = classify(Path::new(
            "/Volumes/ADI/ADI.app/Contents/Resources/adi-mono",
        ));
        assert_eq!(
            loc,
            Location::Volume(PathBuf::from(
                "/Volumes/ADI/ADI.app/Contents/Resources/adi-mono"
            ))
        );
        assert!(!loc.is_durable());
        assert!(loc.explain().unwrap().contains("ejected"));
    }

    #[test]
    fn a_translocated_bundle_is_not() {
        let path = Path::new(
            "/private/var/folders/_9/abc/T/AppTranslocation/1E4F/d/ADI.app/Contents/Resources/adi-mono",
        );
        assert!(!classify(path).is_durable());
        assert!(classify(path).explain().unwrap().contains("every launch"));
    }

    /// Both bad cases have to name the fix, since the whole point of the message is that the
    /// reader has to do something.
    #[test]
    fn every_refusal_says_what_to_do() {
        for path in [
            "/Volumes/ADI/ADI.app/Contents/Resources/adi-mono",
            "/private/var/folders/x/AppTranslocation/y/ADI.app/Contents/Resources/adi-mono",
        ] {
            let reason = classify(Path::new(path)).explain().expect("a refusal");
            assert!(reason.contains("Applications folder"), "{reason}");
        }
    }
}
