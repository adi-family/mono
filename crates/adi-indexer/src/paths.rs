//! Where the indexer keeps things that outlive one project.
//!
//! Upstream these came from `lib-cli-common::AdiUserDirs`, which put them in the platform's
//! application-data directory (`~/Library/Application Support/com.adi.adi` on macOS). In this
//! workspace every subsystem lives in a module of the mono store, so the indexer does too —
//! one directory, `$ADI_DIR`-aware like the rest.

use std::path::PathBuf;

use adi_config::Config as Store;

/// The indexer's own directory in the mono store (`~/.adi/mono/indexer` by default).
#[must_use]
pub fn module_dir() -> PathBuf {
    Store::open().module("indexer").dir().to_path_buf()
}

/// User-level settings, merged under a project's own `.adi/config.toml`.
#[must_use]
pub fn user_config_path() -> PathBuf {
    module_dir().join("config.toml")
}

/// The content-addressed parse/embedding cache, shared by every project and worktree on the
/// machine: the same file bytes are parsed and embedded once, however many checkouts hold them.
#[must_use]
pub fn cache_dir() -> PathBuf {
    module_dir().join("cache")
}

/// A project's index directory (`<project>/.adi/tree`), holding the SQLite file and the vector
/// index beside the code they describe.
#[must_use]
pub fn index_dir(project_path: &std::path::Path) -> PathBuf {
    project_path.join(".adi").join("tree")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_hangs_off_the_indexer_module() {
        let module = module_dir();
        assert!(user_config_path().starts_with(&module));
        assert!(cache_dir().starts_with(&module));
        assert!(module.ends_with("indexer"));
    }

    #[test]
    fn the_index_lives_with_the_code() {
        let dir = index_dir(std::path::Path::new("/tmp/project"));
        assert_eq!(dir, PathBuf::from("/tmp/project/.adi/tree"));
    }
}
