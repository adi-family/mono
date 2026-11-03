//! A primary React build alongside the original control panel, without replacing its assets.
//!
//! Only the primary UI's explicit document routes receive its SPA shell. Other requests may
//! read an existing build asset, but never fall back to that shell: the caller retains the
//! original panel's routing and its separate `ADI_WEBAPP_DIST` override.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) const DIST_ENV: &str = "ADI_PRIMARY_UI_DIST";

#[derive(Clone, Debug)]
pub(crate) struct PrimaryUi {
    root: PathBuf,
}

#[derive(Debug)]
pub(crate) struct Asset {
    pub(crate) relative: String,
    pub(crate) bytes: Vec<u8>,
}

impl PrimaryUi {
    /// Fail closed at startup if the build has no readable shell or escapes its directory.
    pub(crate) fn open(dir: &Path) -> io::Result<Self> {
        let primary = Self {
            root: dir.canonicalize()?,
        };
        primary.read_relative("index.html").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "index.html must be a readable file inside the UI distribution",
            )
        })?;
        Ok(primary)
    }

    /// All filesystem work is synchronous; the server calls this on its blocking pool.
    pub(crate) fn read(&self, path: &str) -> Option<Asset> {
        let relative = relative_path(path)?;
        Some(Asset {
            relative: relative.to_string(),
            bytes: self.read_relative(relative)?,
        })
    }

    fn read_relative(&self, relative: &str) -> Option<Vec<u8>> {
        // Canonicalize every read so a build cannot expose a symlink outside its root.
        let file = self.root.join(relative).canonicalize().ok()?;
        if !file.starts_with(&self.root) || !file.is_file() {
            return None;
        }
        fs::read(file).ok()
    }
}

pub(crate) fn relative_path(path: &str) -> Option<&str> {
    // React Router accepts a trailing slash on document routes. Strip only one so repeated
    // trailing separators cannot turn an otherwise invalid path into a fixed document match.
    let document = path.strip_suffix('/').unwrap_or(path);
    if path == "/"
        || matches!(
            document,
            "/index.html" | "/interface" | "/library" | "/library/agents/layout-reviewer" | "/chat"
        )
        || path.starts_with("/chat/")
    {
        return Some("index.html");
    }

    let relative = path.strip_prefix('/')?;
    // These documents always belong to the original panel, even if a build contains a
    // colliding file. /api is also excluded here as a defence independent of HTTP routing.
    if matches!(
        relative.split('/').next(),
        Some("api" | "extended" | "marketplace" | "embed")
    ) {
        return None;
    }
    // The URL is not percent-decoded. Reject dot components, hidden files, empty segments,
    // and Windows separators before joining, so the same rule holds on every platform.
    if relative.contains(['\\', '\0'])
        || relative
            .split('/')
            .any(|segment| segment.is_empty() || segment.starts_with('.'))
    {
        return None;
    }
    Some(relative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Build(PathBuf);

    impl Build {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "adi-primary-ui-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            fs::create_dir_all(&root).unwrap();
            let build = Self(root);
            build.write("index.html", b"<html>primary UI</html>");
            build
        }

        fn write(&self, relative: &str, bytes: &[u8]) {
            let file = self.0.join(relative);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, bytes).unwrap();
        }

        fn ui(&self) -> PrimaryUi {
            PrimaryUi::open(&self.0).unwrap()
        }
    }

    impl Drop for Build {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn primary_documents_and_compatible_chat_bookmarks_receive_the_shell() {
        let build = Build::new();
        let ui = build.ui();
        for path in [
            "/",
            "/index.html",
            "/index.html/",
            "/interface",
            "/interface/",
            "/library",
            "/library/",
            "/library/agents/layout-reviewer",
            "/library/agents/layout-reviewer/",
            "/chat",
            "/chat/",
            "/chat/designer/run-one",
            "/chat/@studio/designer/run-two",
        ] {
            let asset = ui.read(path).unwrap_or_else(|| panic!("missing {path}"));
            assert_eq!(asset.relative, "index.html", "{path}");
            assert_eq!(asset.bytes, b"<html>primary UI</html>", "{path}");
        }
    }

    #[test]
    fn primary_assets_are_served_only_when_the_file_exists() {
        let build = Build::new();
        build.write("assets/primary-abc123.js", b"primary script");
        let ui = build.ui();
        let asset = ui.read("/assets/primary-abc123.js").unwrap();
        assert_eq!(asset.relative, "assets/primary-abc123.js");
        assert_eq!(asset.bytes, b"primary script");
        for path in [
            "/assets/legacy-icon.png",
            "/legacy-123456789abc.js",
            "/sw.js",
            "/manifest.webmanifest",
            "/unknown-page",
            "/interface/unknown",
            "/library/unknown",
            "/chatty",
        ] {
            assert!(ui.read(path).is_none(), "must preserve fallback for {path}");
        }
    }

    #[test]
    fn legacy_documents_and_api_paths_never_get_the_primary_shell_or_colliding_files() {
        let build = Build::new();
        for path in [
            "extended/settings",
            "extended/agents",
            "marketplace/vendor/app",
            "embed/dashboard-agent",
            "api/health",
        ] {
            build.write(path, b"must not be served");
        }
        let ui = build.ui();
        for path in [
            "/extended",
            "/extended/settings",
            "/extended/agents",
            "/marketplace",
            "/marketplace/vendor/app",
            "/embed/dashboard-agent",
            "/api/health",
        ] {
            assert!(ui.read(path).is_none(), "must preserve fallback for {path}");
        }
    }

    #[test]
    fn unsafe_or_hidden_asset_paths_cannot_leave_the_distribution() {
        let build = Build::new();
        build.write("assets/primary.js", b"safe");
        build.write(".secret", b"hidden");
        let ui = build.ui();
        for path in [
            "/../index.html",
            "/assets/../index.html",
            "/assets/./primary.js",
            "//index.html",
            "/assets//primary.js",
            "/interface//",
            "/library//",
            "/assets\\..\\index.html",
            "/.secret",
            "/assets/\0primary.js",
            "index.html",
        ] {
            assert!(ui.read(path).is_none(), "must reject {path:?}");
        }
    }

    #[test]
    fn invalid_or_removed_builds_fall_back_to_the_original_ui() {
        let build = Build::new();
        let ui = build.ui();
        assert!(PrimaryUi::open(&build.0.join("missing")).is_err());
        assert!(PrimaryUi::open(&build.0.join("index.html")).is_err());
        fs::remove_file(build.0.join("index.html")).unwrap();
        assert!(PrimaryUi::open(&build.0).is_err());
        assert!(ui.read("/").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_may_reference_build_files_but_never_escape_the_root() {
        use std::os::unix::fs::symlink;

        let build = Build::new();
        let outside = Build::new();
        build.write("assets/primary.js", b"safe");
        outside.write("secret.js", b"must stay private");
        symlink("primary.js", build.0.join("assets/alias.js")).unwrap();
        symlink(
            outside.0.join("secret.js"),
            build.0.join("assets/escape.js"),
        )
        .unwrap();
        symlink(&outside.0, build.0.join("assets/outside")).unwrap();
        let ui = build.ui();
        assert_eq!(ui.read("/assets/alias.js").unwrap().bytes, b"safe");
        assert!(ui.read("/assets/escape.js").is_none());
        assert!(ui.read("/assets/outside/secret.js").is_none());

        fs::remove_file(build.0.join("index.html")).unwrap();
        symlink(outside.0.join("index.html"), build.0.join("index.html")).unwrap();
        assert!(PrimaryUi::open(&build.0).is_err());
        assert!(ui.read("/").is_none());
    }
}
