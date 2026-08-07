//! The store opts itself out of Spotlight the first time anything creates a directory in
//! it — no install step, no manual `touch`.
//!
//! This lives in `tests/` rather than beside the unit tests on purpose: the marker is
//! written behind a `Once`, so it can only fire for one root per process. An integration
//! test gets a process of its own, which is the only way to exercise the real public path
//! (`Config::open` → `Module::ensure_dir`) instead of the helper underneath it.

#![cfg(target_os = "macos")]

use std::path::PathBuf;

fn scratch_home() -> PathBuf {
    std::env::temp_dir().join(format!(
        "adi-config-never-index-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ))
}

#[test]
fn a_first_module_write_leaves_the_store_unindexed() {
    let home = scratch_home();
    let _ = std::fs::remove_dir_all(&home);

    // `set_var` is unsafe in edition 2024 because another thread reading the environment
    // concurrently is UB. Safe here by construction: this binary holds exactly one test,
    // and the writes happen before anything that could spawn a thread.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("ADI_DIR", ".adi");
    }

    let store_root = home.join(".adi");
    let marker = store_root.join(".metadata_never_index");
    assert!(!marker.exists(), "precondition: nothing on disk yet");

    // The ordinary way a subsystem reaches its settings directory.
    let dir = adi_config::Config::open()
        .module("hive")
        .ensure_dir()
        .expect("ensure_dir")
        .to_path_buf();

    assert!(dir.is_dir(), "module dir missing at {}", dir.display());
    assert!(
        marker.is_file(),
        "store root was left indexable — no marker at {}",
        marker.display()
    );
    // The marker belongs to the whole store, not to the one module that happened to be
    // created first; anything under the root inherits it.
    assert!(
        dir.starts_with(&store_root),
        "{} is not under {}",
        dir.display(),
        store_root.display()
    );

    let _ = std::fs::remove_dir_all(&home);
}
