//! Keep the embedded webapp directory in sync with the build.
//!
//! `src/main.rs` embeds `../adi-webapp/dist` with `include_dir!`, which needs the
//! directory to exist at compile time. On a fresh checkout the webapp hasn't been built
//! yet, so create it empty here — adi-app then serves a placeholder until `trunk build`
//! populates it. We also emit `rerun-if-changed` per asset so the binary re-embeds when
//! the built output changes.
//!
//! In a release build we also precompress the dist in place: a `.br` and a `.gz` sibling
//! next to every compressible asset, so `include_dir!` picks them up as ordinary embedded
//! files and `main.rs`'s asset server can serve whichever the request's `Accept-Encoding`
//! prefers without ever compressing per request (see `pick_embedded`/`accepts_coding`).

use std::io::Write as _;
use std::path::Path;

fn main() {
    // The reported version follows the release tag when one is building us — same rule as
    // `adi-update::BUILT_VERSION`, so `/api/health` and `adi-mono --version` agree.
    println!("cargo:rerun-if-env-changed=ADI_VERSION");

    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let dist = Path::new(&manifest).join("../adi-webapp/dist");
    let _ = std::fs::create_dir_all(&dist);
    emit_rerun(&dist);

    // Only for the release profile: this is a multi-second pass over a ~15 MB wasm file at
    // brotli's top quality, and nobody wants to pay that on every debug-build iteration of a
    // handler that has nothing to do with the webapp. A debug build still serves the dist —
    // just as identity bytes, which `pick_embedded`'s fallback already handles.
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        precompress(&dist);
    }
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

/// File extensions worth a build-time `.br`/`.gz` sibling: text and wasm, where brotli/gzip
/// earn their keep. Already-compressed binary formats (png, woff2, ico) are skipped — nothing
/// left to gain, and it would only slow the build down.
const COMPRESSIBLE_EXTENSIONS: &[&str] = &[
    "wasm",
    "js",
    "mjs",
    "css",
    "html",
    "json",
    "svg",
    "webmanifest",
    "map",
];

/// Walk `dir` and write a `.br` (brotli, quality 11) and a `.gz` (gzip, level 9) sibling next
/// to every compressible file. `include_dir!` then embeds them as ordinary files, named by
/// appending the suffix — `foo.wasm` gets `foo.wasm.br` and `foo.wasm.gz` beside it — so
/// nothing downstream needs to know these are derived rather than part of the trunk build.
fn precompress(dir: &Path) {
    for path in list_files(dir) {
        let is_compressible = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| COMPRESSIBLE_EXTENSIONS.contains(&ext));
        if !is_compressible {
            continue;
        }
        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        write_sibling(&path, "br", &brotli_compress(&data));
        write_sibling(&path, "gz", &gzip_compress(&data));
    }
}

/// Every regular file under `dir`, recursively. Unlike [`emit_rerun`] this doesn't need to
/// run before the directory has any content, so it can be a plain `Vec` rather than a walk
/// interleaved with side effects.
fn list_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(list_files(&path));
        } else {
            out.push(path);
        }
    }
    out
}

fn write_sibling(path: &Path, suffix: &str, body: &[u8]) {
    let sibling = Path::new(&format!("{}.{suffix}", path.display())).to_path_buf();
    if std::fs::write(&sibling, body).is_err() {
        println!(
            "cargo:warning=failed writing precompressed asset {}",
            sibling.display()
        );
    }
}

fn brotli_compress(data: &[u8]) -> Vec<u8> {
    let params = brotli::enc::BrotliEncoderParams {
        quality: 11,
        lgwin: 24,
        ..Default::default()
    };
    let mut out = Vec::new();
    {
        let mut writer = brotli::CompressorWriter::with_params(&mut out, 4096, &params);
        writer.write_all(data).expect("compressing into a Vec cannot fail");
    }
    out
}

fn gzip_compress(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    encoder
        .write_all(data)
        .expect("compressing into a Vec cannot fail");
    encoder.finish().expect("compressing into a Vec cannot fail")
}
