//! Dotted numeric versions (`major.minor.patch`), compared numerically — enough for
//! the workspace's own `x.y.z` scheme without pulling in a semver dependency.

use std::fmt;

/// A parsed `major.minor.patch` version; missing components default to `0`, and a
/// leading `v` (as in a `v0.2.0` release tag) is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    /// Parse `"0.2.0"` / `"v0.2"` / `"1"`; `None` on anything non-numeric or with
    /// more than three components.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('v');
        let mut parts = s.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next().map_or(Some(0), |p| p.parse().ok())?;
        let patch = parts.next().map_or(Some(0), |p| p.parse().ok())?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
        })
    }

    /// True when `published` is a strictly newer version than `installed`. Unparseable
    /// strings are never "newer", so a corrupt manifest can't trigger an install.
    #[must_use]
    pub fn is_newer(published: &str, installed: &str) -> bool {
        match (Self::parse(published), Self::parse(installed)) {
            (Some(p), Some(i)) => p > i,
            _ => false,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_short_and_tagged_forms() {
        assert_eq!(
            Version::parse("0.2.1"),
            Some(Version {
                major: 0,
                minor: 2,
                patch: 1
            })
        );
        assert_eq!(
            Version::parse("v1.3"),
            Some(Version {
                major: 1,
                minor: 3,
                patch: 0
            })
        );
        assert_eq!(
            Version::parse("2"),
            Some(Version {
                major: 2,
                minor: 0,
                patch: 0
            })
        );
        assert_eq!(
            Version::parse(" 0.1.0 "),
            Some(Version {
                major: 0,
                minor: 1,
                patch: 0
            })
        );
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(Version::parse(""), None);
        assert_eq!(Version::parse("abc"), None);
        assert_eq!(Version::parse("1.2.3.4"), None);
        assert_eq!(Version::parse("1.2-beta"), None);
    }

    #[test]
    fn orders_numerically_not_lexically() {
        assert!(Version::parse("0.10.0") > Version::parse("0.9.9"));
        assert!(Version::parse("1.0.0") > Version::parse("0.99.99"));
    }

    #[test]
    fn is_newer_is_strict_and_fails_closed() {
        assert!(Version::is_newer("0.2.0", "0.1.0"));
        assert!(!Version::is_newer("0.1.0", "0.1.0"));
        assert!(!Version::is_newer("0.1.0", "0.2.0"));
        assert!(!Version::is_newer("not-a-version", "0.1.0"));
        assert!(!Version::is_newer("0.2.0", "not-a-version"));
    }
}
