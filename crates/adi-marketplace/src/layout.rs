//! Reading a bundle repository's tree: what is really there, at the pinned commit, regardless of
//! what the manifest's advisory `elements[]` preview claims (`docs/marketplace-bundles.md`
//! decision #2). This is what a bundle installer dispatches on — nothing here lands a file, it
//! only looks.
//!
//! A repository is laid out by kind, one top-level directory per kind that applies, all optional:
//!
//! ```text
//! agents/<name>.toml        # an agent definition
//! tools/<name>.{sh,ts}      # an owned tool script
//! dashboards/<name>/        # a dashboard — frontend/index.ts, backend/index.ts, same as v1's app
//! llm/<name>.toml           # an LLM backend
//! embeddings/<name>.toml    # an embedding backend
//! services/<name>.yaml      # one hive service
//! triggers/<name>.toml      # a trigger definition
//! project/config.toml       # the bundle's own project scaffold — at most one per bundle
//! ```
//!
//! **A legacy v1 app repository — `frontend/index.ts` and `backend/index.ts` at the root, no
//! kind directories at all — is still a valid bundle:** [`read_layout`] reads it as one element,
//! a dashboard, filed under a synthetic `dashboards/<slug>` position (the bundle's own slug,
//! since the repository itself carries no name for it). [`Layout::legacy`] is how a bundle
//! installer tells the two apart, because decision #6 keeps this one case on v1's own mechanism
//! verbatim rather than folding it into the general one.
//!
//! [`scan_for_rust`] is the other half of what a bundle installer needs before it stages
//! anything: the static content rule from "No Rust in an item" — a `Cargo.toml`, a `.rs` file, or
//! a compiled binary's magic number anywhere under a kind directory refuses the whole bundle. It
//! belongs beside `install::stage`'s existing `NotAnApp` check, in whatever pipeline a bundle
//! installer builds alongside it — the same place in spirit, new code because `stage` itself does
//! not generalize (`docs/marketplace-bundles.md`, "What v1's code assumes this design changes").

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::kind::Kind;

/// The two files that make a directory a dashboard — v1's own contract
/// (`guides/dashboards.md`), reused verbatim for a `dashboards/<name>/` element and for the
/// legacy root-of-repository case.
const DASHBOARD_FILES: [&str; 2] = ["frontend/index.ts", "backend/index.ts"];

/// What a repository's tree really carries, read off the pinned commit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Layout {
    /// Every element found, in the order [`Kind::ALL`] lists the kinds.
    pub elements: Vec<LayoutElement>,
    /// Whether this is the legacy v1 shape: no kind directories, so the one element found (if
    /// any) is [`read_layout`]'s synthetic reading of the repository root as a dashboard.
    pub legacy: bool,
}

/// One element as the tree actually carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutElement {
    /// Which of the eight kinds this is.
    pub kind: Kind,
    /// The published name — the file or directory stem. Absent only for the `project` singleton.
    pub name: Option<String>,
    /// Where it lives, relative to the repository root. Empty for the legacy dashboard, whose
    /// content is the repository root itself rather than a position under it.
    pub path: PathBuf,
}

/// Walk `root` — a checked-out tree standing at the pinned commit — and report every element it
/// really carries.
///
/// `slug` is the bundle's own published slug, used only for the legacy case: a v1 repository
/// carries no name of its own for the single dashboard it is, so the synthetic element is filed
/// under the bundle's.
///
/// # Errors
/// [`Error::Io`] when a kind directory cannot be read.
pub fn read_layout(root: &Path, slug: &str) -> Result<Layout> {
    let has_kind_dir = Kind::ALL.iter().any(|kind| root.join(kind.dir()).is_dir());
    if !has_kind_dir {
        return Ok(Layout {
            elements: legacy_elements(root, slug),
            legacy: true,
        });
    }

    let mut elements = Vec::new();
    for kind in Kind::ALL {
        if kind == Kind::Project {
            if root.join(kind.dir()).join("config.toml").is_file() {
                elements.push(LayoutElement {
                    kind,
                    name: None,
                    path: Path::new(kind.dir()).join("config.toml"),
                });
            }
            continue;
        }
        let dir = root.join(kind.dir());
        if !dir.is_dir() {
            continue;
        }
        for entry in sorted_entries(&dir)? {
            if let Some(name) = element_name(kind, &entry) {
                elements.push(LayoutElement {
                    kind,
                    name: Some(name),
                    path: Path::new(kind.dir()).join(entry.file_name()),
                });
            }
        }
    }
    Ok(Layout {
        elements,
        legacy: false,
    })
}

/// The one element a v1-shaped repository reads as, or none for a tree with no kind directories
/// that also is not a runnable dashboard — reported, not refused, because [`read_layout`] only
/// looks; refusing a bundle with nothing installable is a bundle installer's call, not a reader's.
fn legacy_elements(root: &Path, slug: &str) -> Vec<LayoutElement> {
    if DASHBOARD_FILES.iter().all(|f| root.join(f).is_file()) {
        vec![LayoutElement {
            kind: Kind::Dashboard,
            name: Some(slug.to_string()),
            path: PathBuf::new(),
        }]
    } else {
        Vec::new()
    }
}

/// This kind directory's entries, sorted by file name — the filesystem's own order is not stable
/// between reads, and a layout has to be.
fn sorted_entries(dir: &Path) -> Result<Vec<fs::DirEntry>> {
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(dir)?.filter_map(std::result::Result::ok).collect();
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

/// The published name one directory entry carries for `kind`, or `None` when it does not match
/// that kind's shape — an unrelated file left beside the real entries is not an element.
fn element_name(kind: Kind, entry: &fs::DirEntry) -> Option<String> {
    let file_name = entry.file_name();
    let file_name = file_name.to_str()?;
    match kind {
        Kind::Agent | Kind::Llm | Kind::Embedding | Kind::Trigger => {
            file_name.strip_suffix(".toml").map(str::to_string)
        }
        Kind::Tool => file_name
            .strip_suffix(".sh")
            .or_else(|| file_name.strip_suffix(".ts"))
            .map(str::to_string),
        Kind::Service => file_name.strip_suffix(".yaml").map(str::to_string),
        Kind::Dashboard => {
            let dir = entry.path();
            (dir.is_dir() && DASHBOARD_FILES.iter().all(|f| dir.join(f).is_file()))
                .then(|| file_name.to_string())
        }
        // The singleton is read directly by `read_layout`, off `config.toml` rather than off a
        // directory listing — it has no per-entry name to derive.
        Kind::Project => None,
    }
}

/// The byte sequences that open a compiled binary. Just enough of each to be unambiguous — the
/// scan only reads this many bytes per file.
const ELF_MAGIC: &[u8] = b"\x7fELF";
const PE_MAGIC: &[u8] = b"MZ";
/// Mach-O, thin, both endiannesses, 32- and 64-bit.
const MACHO_MAGICS: [[u8; 4]; 4] = [
    [0xfe, 0xed, 0xfa, 0xce],
    [0xce, 0xfa, 0xed, 0xfe],
    [0xfe, 0xed, 0xfa, 0xcf],
    [0xcf, 0xfa, 0xed, 0xfe],
];
/// Mach-O, fat/universal — the same four bytes a Java `.class` file opens with, and a bundle has
/// no more business carrying one of those than a native binary either.
const MACHO_FAT_MAGIC: [u8; 4] = [0xca, 0xfe, 0xba, 0xbe];

/// What a file's leading bytes say it is, if it is a compiled binary.
fn magic_name(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(ELF_MAGIC) {
        Some("a compiled binary (ELF)")
    } else if MACHO_MAGICS.iter().any(|m| head.starts_with(m)) || head.starts_with(&MACHO_FAT_MAGIC)
    {
        Some("a compiled binary (Mach-O)")
    } else if head.starts_with(PE_MAGIC) {
        Some("a compiled binary (PE)")
    } else {
        None
    }
}

/// Refuse `root` if it carries Rust — source or a compiled binary — anywhere under one of the
/// eight kind directories: a `Cargo.toml`, a `.rs` file, or a file opening with an ELF, Mach-O or
/// PE magic number ("No Rust in an item", `docs/marketplace-bundles.md`).
///
/// A publishing-time content rule, not a sandbox: it catches what is checked in, not what a
/// tool's script or a hive service's `runner.docker` image might do at runtime — see the design
/// document for exactly what it does and does not buy.
///
/// The legacy v1 shape has no kind directories at all, so this is a no-op for it — v1 never had
/// this rule, and decision #6 keeps that repository shape on v1's own mechanism unchanged.
///
/// `label` names the bundle in the refusal, the way [`crate::Error::NotAnApp`] names the
/// entry's slug.
///
/// # Errors
/// [`Error::CarriesRust`] naming what was found and where; [`Error::Io`] when a kind directory
/// cannot be read.
pub fn scan_for_rust(root: &Path, label: &str) -> Result<()> {
    for kind in Kind::ALL {
        let dir = root.join(kind.dir());
        if dir.is_dir() {
            scan_dir(&dir, root, label)?;
        }
    }
    Ok(())
}

/// [`scan_for_rust`]'s recursion, one kind directory (or a directory under it) at a time.
fn scan_dir(dir: &Path, root: &Path, label: &str) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            scan_dir(&path, root, label)?;
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(&path).display().to_string();
        if path.file_name().and_then(|n| n.to_str()) == Some("Cargo.toml") {
            return Err(carries_rust(label, "a Cargo.toml", rel));
        }
        if path.extension().is_some_and(|ext| ext == "rs") {
            return Err(carries_rust(label, "a .rs file", rel));
        }
        if let Some(what) = read_magic(&path)? {
            return Err(carries_rust(label, what, rel));
        }
    }
    Ok(())
}

fn carries_rust(label: &str, what: &str, path: String) -> Error {
    Error::CarriesRust(label.to_string(), what.to_string(), path)
}

/// The first four bytes of `path`, read for [`magic_name`] — a file too short to carry a magic
/// number is, by construction, not one.
fn read_magic(path: &Path) -> Result<Option<&'static str>> {
    let mut file = fs::File::open(path)?;
    let mut head = [0u8; 4];
    let n = file.read(&mut head).unwrap_or(0);
    Ok(magic_name(&head[..n]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "adi-marketplace-layout-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch root");
        root
    }

    fn dashboard_at(dir: &Path) {
        std::fs::create_dir_all(dir.join("frontend")).expect("frontend");
        std::fs::create_dir_all(dir.join("backend")).expect("backend");
        std::fs::write(dir.join("frontend").join("index.ts"), "// front\n").expect("front");
        std::fs::write(dir.join("backend").join("index.ts"), "// back\n").expect("back");
    }

    #[test]
    fn a_legacy_v1_repository_reads_as_one_dashboard_at_the_bundles_own_slug() {
        let root = scratch("legacy");
        dashboard_at(&root);

        let layout = read_layout(&root, "crm").expect("reads");
        assert!(layout.legacy);
        assert_eq!(
            layout.elements,
            vec![LayoutElement {
                kind: Kind::Dashboard,
                name: Some("crm".to_string()),
                path: PathBuf::new(),
            }]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_legacy_shaped_root_missing_half_the_dashboard_reports_nothing_rather_than_erroring() {
        let root = scratch("legacy-incomplete");
        std::fs::create_dir_all(root.join("frontend")).expect("frontend");
        std::fs::write(root.join("frontend").join("index.ts"), "// front\n").expect("front");

        let layout = read_layout(&root, "crm").expect("reads");
        assert!(layout.legacy);
        assert!(layout.elements.is_empty(), "reading is not refusing");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_kind_directory_present_even_empty_rules_out_the_legacy_reading() {
        let root = scratch("not-legacy");
        dashboard_at(&root);
        std::fs::create_dir_all(root.join("agents")).expect("agents");

        let layout = read_layout(&root, "crm").expect("reads");
        assert!(!layout.legacy, "a kind directory means this is the general shape, not v1's");
        assert!(layout.elements.is_empty(), "an empty agents/ carries nothing");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_full_bundle_reports_every_kind_it_carries() {
        let root = scratch("full");
        std::fs::create_dir_all(root.join("agents")).expect("d");
        std::fs::write(root.join("agents").join("sales-bot.toml"), "").expect("f");
        std::fs::create_dir_all(root.join("tools")).expect("d");
        std::fs::write(root.join("tools").join("csv-import.sh"), "#!/bin/sh\n").expect("f");
        dashboard_at(&root.join("dashboards").join("crm"));
        std::fs::create_dir_all(root.join("llm")).expect("d");
        std::fs::write(root.join("llm").join("gpt5.toml"), "").expect("f");
        std::fs::create_dir_all(root.join("embeddings")).expect("d");
        std::fs::write(root.join("embeddings").join("e5.toml"), "").expect("f");
        std::fs::create_dir_all(root.join("services")).expect("d");
        std::fs::write(root.join("services").join("redis.yaml"), "").expect("f");
        std::fs::create_dir_all(root.join("triggers")).expect("d");
        std::fs::write(root.join("triggers").join("nightly.toml"), "").expect("f");
        std::fs::create_dir_all(root.join("project")).expect("d");
        std::fs::write(root.join("project").join("config.toml"), "").expect("f");
        // Unrelated files beside the real entries are not elements.
        std::fs::write(root.join("agents").join("README.md"), "").expect("f");

        let layout = read_layout(&root, "crm-suite").expect("reads");
        assert!(!layout.legacy);
        let named = |kind: Kind| -> Vec<&str> {
            layout
                .elements
                .iter()
                .filter(|e| e.kind == kind)
                .filter_map(|e| e.name.as_deref())
                .collect()
        };
        assert_eq!(named(Kind::Agent), vec!["sales-bot"]);
        assert_eq!(named(Kind::Tool), vec!["csv-import"]);
        assert_eq!(named(Kind::Dashboard), vec!["crm"]);
        assert_eq!(named(Kind::Llm), vec!["gpt5"]);
        assert_eq!(named(Kind::Embedding), vec!["e5"]);
        assert_eq!(named(Kind::Service), vec!["redis"]);
        assert_eq!(named(Kind::Trigger), vec!["nightly"]);
        assert_eq!(
            layout.elements.iter().find(|e| e.kind == Kind::Project).map(|e| &e.name),
            Some(&None),
            "the singleton carries no name"
        );
        assert_eq!(layout.elements.len(), 8, "the readme did not become a ninth");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_dashboard_directory_missing_its_backend_is_not_reported() {
        let root = scratch("half-dashboard");
        std::fs::create_dir_all(root.join("dashboards").join("broken").join("frontend"))
            .expect("d");
        std::fs::write(
            root.join("dashboards").join("broken").join("frontend").join("index.ts"),
            "",
        )
        .expect("f");

        let layout = read_layout(&root, "crm-suite").expect("reads");
        assert!(layout.elements.is_empty(), "{:?}", layout.elements);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_no_rust_scan_is_a_no_op_outside_the_kind_directories() {
        let root = scratch("outside");
        std::fs::write(root.join("Cargo.toml"), "[package]\n").expect("f");
        std::fs::write(root.join("build.rs"), "fn main() {}\n").expect("f");

        scan_for_rust(&root, "crm-suite").expect("nothing under a kind directory to catch");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_no_rust_scan_refuses_a_cargo_toml_under_a_kind_directory() {
        let root = scratch("cargo-toml");
        std::fs::create_dir_all(root.join("tools").join("evil")).expect("d");
        std::fs::write(root.join("tools").join("evil").join("Cargo.toml"), "[package]\n")
            .expect("f");

        let err = scan_for_rust(&root, "crm-suite").expect_err("refused");
        assert!(matches!(err, Error::CarriesRust(_, _, _)));
        let msg = err.to_string();
        assert!(msg.contains("Cargo.toml"), "{msg}");
        assert!(msg.contains("tools/evil/Cargo.toml"), "{msg}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_no_rust_scan_refuses_a_rs_file_nested_under_a_dashboard() {
        let root = scratch("rs-file");
        std::fs::create_dir_all(root.join("dashboards").join("crm").join("backend"))
            .expect("d");
        std::fs::write(
            root.join("dashboards").join("crm").join("backend").join("main.rs"),
            "fn main() {}\n",
        )
        .expect("f");

        let err = scan_for_rust(&root, "crm-suite").expect_err("refused");
        let msg = err.to_string();
        assert!(msg.contains(".rs file"), "{msg}");
        assert!(msg.contains("dashboards/crm/backend/main.rs"), "{msg}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_no_rust_scan_refuses_each_compiled_binary_magic() {
        for (tag, magic, name) in [
            ("elf", &[0x7f, b'E', b'L', b'F'][..], "ELF"),
            ("macho", &[0xfe, 0xed, 0xfa, 0xce][..], "Mach-O"),
            ("macho-fat", &[0xca, 0xfe, 0xba, 0xbe][..], "Mach-O"),
            ("pe", &[b'M', b'Z', 0x90, 0x00][..], "PE"),
        ] {
            let root = scratch(&format!("magic-{tag}"));
            std::fs::create_dir_all(root.join("tools")).expect("d");
            std::fs::write(root.join("tools").join("built"), magic).expect("f");

            let err = scan_for_rust(&root, "crm-suite").expect_err(tag);
            assert!(err.to_string().contains(name), "{tag}: {err}");
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn an_ordinary_bundle_scans_clean() {
        let root = scratch("clean");
        std::fs::create_dir_all(root.join("agents")).expect("d");
        std::fs::write(root.join("agents").join("sales-bot.toml"), "name = \"x\"\n").expect("f");
        std::fs::create_dir_all(root.join("tools")).expect("d");
        std::fs::write(root.join("tools").join("csv-import.sh"), "#!/bin/sh\necho hi\n")
            .expect("f");

        scan_for_rust(&root, "crm-suite").expect("nothing to refuse");
        let _ = std::fs::remove_dir_all(&root);
    }
}
