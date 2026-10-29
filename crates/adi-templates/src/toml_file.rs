//! `template.toml` itself: its shape, reading one template or every template in the templates
//! directory, and seeding the two the platform ships.

use std::path::Path;

use serde::Deserialize;

/// The file every template directory carries.
pub const TEMPLATE_FILE: &str = "template.toml";

/// The template id a create request names when it omits one, for compatibility with every
/// caller that predates templates existing at all: the dependency-free scaffold.
pub const DEFAULT_TEMPLATE: &str = "plain";

/// The only engine a template may name today — `bash -c '<command>'`, nothing else.
pub const BASH_ENGINE: &str = "bash";

/// One dashboard template: a label for a human, the engine its `command` runs under, the CLIs
/// that must be on `PATH` before it does, and the command itself.
///
/// `id` is never read from the file — it is the directory's own name, the same way a
/// dashboard's id is its directory and not a field in its `config.toml`. A `template.toml` may
/// carry an `id` key of its own (the fixed shape names one, for a human reading the file to
/// confirm it matches the folder it's in), but this crate never parses it: two sources of truth
/// for one fact is how they end up disagreeing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub id: String,
    pub label: String,
    pub engine: String,
    pub requires: Vec<String>,
    pub command: String,
    /// `marketplace:<listing>` when the marketplace installed this template — informational,
    /// read by no code here.
    pub installed_from: Option<String>,
}

/// The fields `template.toml` actually carries, deserialized before [`Template::id`] is filled in
/// from the directory name.
#[derive(Debug, Deserialize)]
struct Raw {
    label: String,
    engine: String,
    #[serde(default)]
    requires: Vec<String>,
    command: String,
    #[serde(default)]
    installed_from: Option<String>,
}

/// Read one template directory's `template.toml`. `None` for a directory with no such file, or
/// one that does not parse — a template a human or the marketplace left half-written is simply
/// not offered, not a hard error for every other template's listing to carry.
#[must_use]
pub fn read_template(dir: &Path) -> Option<Template> {
    let id = dir.file_name()?.to_str()?.to_string();
    let raw = std::fs::read_to_string(dir.join(TEMPLATE_FILE)).ok()?;
    let parsed: Raw = toml::from_str(&raw).ok()?;
    Some(Template {
        id,
        label: parsed.label,
        engine: parsed.engine,
        requires: parsed.requires,
        command: parsed.command,
        installed_from: parsed.installed_from,
    })
}

/// Every template under `templates_dir`, sorted by id. A directory that is not a template (no
/// `template.toml`, or one that fails to parse) is left out rather than failing the whole list —
/// the same tolerance [`crate::toml_file`]'s sibling crate `adi-dashboards` gives a dashboard's
/// own `config.toml`.
#[must_use]
pub fn list_templates(templates_dir: &Path) -> Vec<Template> {
    let Ok(entries) = std::fs::read_dir(templates_dir) else {
        return Vec::new();
    };
    let mut templates: Vec<Template> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter_map(|p| read_template(&p))
        .collect();
    templates.sort_by(|a, b| a.id.cmp(&b.id));
    templates
}

/// The two templates the platform ships, written into `templates_dir` once — each only when its
/// own folder is missing, and never overwriting one that is already there (a template directory
/// is the operator's or the marketplace's to edit or replace once it exists).
///
/// # Errors
/// [`std::io::Error`] on any write failure. A failure to seed one template does not stop the
/// other from being tried.
pub fn seed_defaults(templates_dir: &Path) -> std::io::Result<()> {
    for (id, toml) in [
        ("plain", include_str!("../seed/plain/template.toml")),
        ("vite-react", include_str!("../seed/vite-react/template.toml")),
    ] {
        let dir = templates_dir.join(id);
        if dir.is_dir() {
            continue;
        }
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(TEMPLATE_FILE), toml)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "adi-templates-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch root");
        root
    }

    #[test]
    fn a_template_parses_with_its_id_from_the_directory_name() {
        let root = scratch("parse");
        let dir = root.join("vite-react");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(
            dir.join(TEMPLATE_FILE),
            "id       = \"vite-react\"\nlabel    = \"Vite + React 19 + Hono\"\nengine   = \"bash\"\nrequires = [\"bun\"]\ncommand  = 'bun create adi-family/dashboard-vite-react \"$ADI_DASHBOARD_DIR\"'\n",
        )
        .expect("toml");

        let template = read_template(&dir).expect("parses");
        assert_eq!(template.id, "vite-react");
        assert_eq!(template.label, "Vite + React 19 + Hono");
        assert_eq!(template.engine, "bash");
        assert_eq!(template.requires, vec!["bun".to_string()]);
        assert!(template.command.contains("dashboard-vite-react"));
        assert_eq!(template.installed_from, None);
    }

    #[test]
    fn the_directory_name_wins_over_a_mismatched_id_field() {
        // A copy-pasted template.toml whose own `id` key disagrees with the folder it landed in —
        // the folder is the address, so the folder wins, silently. The `id` key in the file is for
        // a human to read, never for this crate to trust.
        let root = scratch("mismatch");
        let dir = root.join("my-copy");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(
            dir.join(TEMPLATE_FILE),
            "id = \"vite-react\"\nlabel = \"Copy\"\nengine = \"bash\"\ncommand = \"true\"\n",
        )
        .expect("toml");

        assert_eq!(read_template(&dir).expect("parses").id, "my-copy");
    }

    #[test]
    fn a_directory_with_no_template_toml_is_not_a_template() {
        let root = scratch("missing");
        let dir = root.join("empty");
        std::fs::create_dir_all(&dir).expect("dir");
        assert_eq!(read_template(&dir), None);
    }

    #[test]
    fn a_template_toml_missing_a_required_field_does_not_parse() {
        let root = scratch("broken");
        let dir = root.join("broken");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join(TEMPLATE_FILE), "label = \"No command\"\nengine = \"bash\"\n")
            .expect("toml");
        assert_eq!(read_template(&dir), None);
    }

    #[test]
    fn listing_skips_what_is_not_a_template_and_sorts_the_rest() {
        let root = scratch("listing");
        for id in ["zeta", "alpha"] {
            let dir = root.join(id);
            std::fs::create_dir_all(&dir).expect("dir");
            std::fs::write(
                dir.join(TEMPLATE_FILE),
                format!("label = \"{id}\"\nengine = \"bash\"\ncommand = \"true\"\n"),
            )
            .expect("toml");
        }
        std::fs::write(root.join("not-a-dir"), "x").expect("stray file");
        std::fs::create_dir_all(root.join("no-toml")).expect("dir with nothing in it");

        let ids: Vec<String> = list_templates(&root).into_iter().map(|t| t.id).collect();
        assert_eq!(ids, vec!["alpha".to_string(), "zeta".to_string()]);
    }

    #[test]
    fn seeding_writes_both_defaults_once_and_never_overwrites_an_existing_folder() {
        let root = scratch("seed");
        seed_defaults(&root).expect("seed");

        let plain = read_template(&root.join("plain")).expect("plain seeded");
        assert_eq!(plain.engine, BASH_ENGINE);
        assert!(plain.command.contains("dashboard-plain"));
        let vite = read_template(&root.join("vite-react")).expect("vite-react seeded");
        assert!(vite.command.contains("dashboard-vite-react"));
        assert!(!vite.command.contains("ref"), "no ref/version/pin field: {}", vite.command);

        // An operator's own edit survives a second seeding pass — the folder already exists.
        std::fs::write(
            root.join("plain").join(TEMPLATE_FILE),
            "label = \"Mine now\"\nengine = \"bash\"\ncommand = \"true\"\n",
        )
        .expect("overwrite by hand");
        seed_defaults(&root).expect("seed again");
        assert_eq!(read_template(&root.join("plain")).expect("still there").label, "Mine now");
    }
}
