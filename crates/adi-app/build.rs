//! Keep the embedded webapp directory in sync with the build.
//!
//! `src/main.rs` embeds `../adi-webapp/dist` with `include_dir!`, which needs the
//! directory to exist at compile time. On a fresh checkout the webapp hasn't been built
//! yet, so create it empty here — adi-app then serves a placeholder until `trunk build`
//! populates it. We also emit `rerun-if-changed` per asset so the binary re-embeds when
//! the built output changes.

use std::path::Path;

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let dist = Path::new(&manifest).join("../adi-webapp/dist");
    let _ = std::fs::create_dir_all(&dist);
    emit_rerun(&dist);
}

/// Recursively emit `cargo:rerun-if-changed` for `dir` and everything under it.
fn emit_rerun(dir: &Path) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            emit_rerun(&path);
        } else {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}
