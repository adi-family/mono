//! `adi-structs` — write every crate's `structs.gen.md`.
//!
//! One page per crate, at the crate root, listing every struct, enum and type alias it declares:
//! the shape only — attributes, fields, variants — with the prose stripped out. It exists so the
//! data a subsystem moves around can be read on one screen instead of chased across forty files,
//! and so a change to that data shows up as a diff in review rather than as a surprise later.
//!
//! It is a *reader*, never a compiler: the source is parsed with `syn` and never built, so it
//! costs no link step and works on a crate that does not currently compile.
//!
//! ```text
//! cargo run -p adi-structs                    # every crate under crates/
//! cargo run -p adi-structs -- adi-agents      # just these
//! cargo run -p adi-structs -- --check         # write nothing; fail if any page is stale
//! cargo run -p adi-structs -- --stdout adi-db # print instead of writing
//! ```
//!
//! `--check` is what CI and the pre-commit hook lean on: it exits non-zero, and names the stale
//! pages, when the committed markdown no longer matches the source.

mod render;
mod scan;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The file each crate's types are written to, at the crate root.
const OUTPUT: &str = "structs.gen.md";

const USAGE: &str = "\
adi-structs — regenerate each crate's structs.gen.md

USAGE:
    adi-structs [OPTIONS] [CRATE...]

ARGS:
    CRATE       crate name or path; defaults to every crate under crates/

OPTIONS:
    --check     write nothing; exit 1 if any page is out of date
    --stdout    print the page(s) instead of writing them
    -h, --help  show this message";

fn main() -> ExitCode {
    let mut check = false;
    let mut to_stdout = false;
    let mut wanted: Vec<String> = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--check" => check = true,
            "--stdout" => to_stdout = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other if other.starts_with('-') => {
                eprintln!("adi-structs: unknown flag {other}\n\n{USAGE}");
                return ExitCode::FAILURE;
            }
            other => wanted.push(other.to_string()),
        }
    }

    let Some(root) = workspace_root() else {
        eprintln!("adi-structs: no workspace Cargo.toml found above the current directory");
        return ExitCode::FAILURE;
    };

    let crates = match resolve(&root, &wanted) {
        Ok(crates) => crates,
        Err(e) => {
            eprintln!("adi-structs: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut stale = Vec::new();
    let mut written = 0usize;
    for dir in crates {
        let krate = match scan::scan_crate(&dir) {
            Ok(krate) => krate,
            Err(e) => {
                eprintln!("adi-structs: {e}");
                return ExitCode::FAILURE;
            }
        };
        let page = render::page(&krate);
        if to_stdout {
            print!("{page}");
            continue;
        }
        let path = dir.join(OUTPUT);
        let current = fs::read_to_string(&path).ok();
        if current.as_deref() == Some(page.as_str()) {
            continue;
        }
        let rel = path.strip_prefix(&root).unwrap_or(&path).display();
        if check {
            stale.push(rel.to_string());
            continue;
        }
        if let Err(e) = fs::write(&path, &page) {
            eprintln!("adi-structs: {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
        println!("wrote {rel}");
        written += 1;
    }

    if check && !stale.is_empty() {
        eprintln!(
            "adi-structs: {} out of date:\n  {}\nrun `cargo run -p adi-structs` and commit the result.",
            if stale.len() == 1 { "1 page is" } else { "pages are" },
            stale.join("\n  ")
        );
        return ExitCode::FAILURE;
    }
    if !to_stdout && !check && written == 0 {
        println!("adi-structs: every page already up to date");
    }
    ExitCode::SUCCESS
}

/// Walk up from the current directory to the manifest that declares `[workspace]`, so the tool
/// behaves the same from the repo root, a crate directory, or a git hook's working directory.
fn workspace_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let manifest = dir.join("Cargo.toml");
        if fs::read_to_string(&manifest).is_ok_and(|t| t.contains("[workspace]")) {
            return Some(dir);
        }
        if !dir.pop() {
            // Fall back to the location this binary was compiled from — right even if the tool is
            // invoked from somewhere else entirely.
            return Path::new(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .nth(2)
                .map(Path::to_path_buf);
        }
    }
}

/// Map the requested crate names (or nothing, meaning all of them) to crate directories.
fn resolve(root: &Path, wanted: &[String]) -> Result<Vec<PathBuf>, String> {
    let all = member_crates(root);
    if wanted.is_empty() {
        return Ok(all);
    }
    wanted
        .iter()
        .map(|name| {
            let bare = name.trim_end_matches('/').rsplit('/').next().unwrap_or(name);
            all.iter()
                .find(|dir| dir.file_name().is_some_and(|f| f == bare))
                .cloned()
                .ok_or_else(|| format!("no crate named '{name}' under {}/crates", root.display()))
        })
        .collect()
}

/// Every directory under `crates/` that holds a `Cargo.toml` and a `src/`, sorted by name.
fn member_crates(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root.join("crates")) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("Cargo.toml").is_file() && p.join("src").is_dir())
        .collect();
    dirs.sort();
    dirs
}
