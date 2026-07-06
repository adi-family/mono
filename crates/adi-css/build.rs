//! Compile the SCSS design system to CSS at build time (pure-Rust, via `grass`) and drop
//! it in `$OUT_DIR/adi.css`, which `src/lib.rs` embeds as the `STYLESHEET` const. The wasm
//! webapp doesn't use this path — Trunk compiles the same SCSS itself; this is for the
//! Rust-side consumers (server-rendered pages) that want the stylesheet as a string.

use std::path::Path;

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let scss_dir = Path::new(&manifest).join("scss");
    let entry = scss_dir.join("adi.scss");
    let out = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("adi.css");

    let css = grass::from_path(&entry, &grass::Options::default())
        .unwrap_or_else(|e| panic!("compiling {}: {e}", entry.display()));
    std::fs::write(&out, css).expect("writing compiled adi.css");

    // Recompile whenever any SCSS partial changes.
    println!("cargo:rerun-if-changed={}", scss_dir.display());
    for entry in std::fs::read_dir(&scss_dir).into_iter().flatten().flatten() {
        println!("cargo:rerun-if-changed={}", entry.path().display());
    }
}
