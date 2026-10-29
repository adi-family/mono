//! A dashboard's `config.toml` — the metadata its directory carries, independent of anything
//! the hive file says about running it.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The metadata file each dashboard directory carries.
///
/// Deliberately loose: every field is optional and a missing or malformed file degrades to the
/// default rather than failing the caller, because a dashboard is a directory anybody can copy
/// in and the listing that visits all of them must survive half-written ones.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// The project this dashboard is filed under (its id), or `None` when unfiled.
    #[serde(default)]
    pub project: Option<String>,
    /// When the dashboard was archived (Unix seconds), or `None` while it is live.
    ///
    /// "Archived" is also how an app arrives from a marketplace: landed with its hive file
    /// parked out of the supervisor's glob, so installing starts nothing. See the marketplace
    /// crate, which stamps this on arrival.
    #[serde(default)]
    pub archived_at: Option<u64>,
    /// The node this dashboard was moved to, when it was. Written beside
    /// [`archived_at`](Self::archived_at) rather than instead of it: the local remains are
    /// archived like any other archived dashboard, and this only says *why*.
    #[serde(default)]
    pub moved_to: Option<String>,
    /// The app's picture, as a Lucide icon name (`receipt`, `chart-column`). A name rather than
    /// an image file so the panel can draw it in its own ink, on its own tile, whether or not the
    /// app is running — a favicon can only be fetched from an app that is up.
    #[serde(default)]
    pub icon: Option<String>,
    /// The widgets the app offers the home screen, by id — each a `[widget.<id>]` table:
    ///
    /// ```toml
    /// [widget.chat]
    /// name = "Chat"
    /// url = "/widget/chat"
    /// size = "half"
    /// ```
    ///
    /// Declared here rather than discovered, so a screen learns what an app offers from one file
    /// it can read whether or not the app is running — framing a widget is what starts it.
    #[serde(default)]
    pub widget: BTreeMap<String, Widget>,
    /// Which template created this dashboard, and the exact command that did — written once, at
    /// creation, by the caller that ran the template's command (never by this crate). Purely
    /// informational: read by no code. Which scaffold a dashboard actually is gets decided from
    /// what is on disk (the shape of its `.adi/hive.yaml`, or whether `frontend/package.json`
    /// exists) — never from this field, which a dashboard's own owner is free to edit or clear
    /// without changing how it behaves.
    #[serde(default)]
    pub created_from: Option<CreatedFrom>,
}

/// `[created_from]` in a dashboard's `config.toml` — see [`Manifest::created_from`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CreatedFrom {
    /// The template id, e.g. `"vite-react"` or `"plain"`.
    pub template: String,
    /// The exact command that was run — absent for a dashboard that arrived by import, where
    /// only the template id travels (see [`crate::bundle::DashboardBundle::template`]).
    #[serde(default)]
    pub command: Option<String>,
}

/// One widget an app offers: a page on the app's own origin that the home screen frames.
///
/// Every field is optional for the same reason the manifest's are; a reader decides what a widget
/// with no `url` means (nothing to frame), rather than the parse refusing the whole manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Widget {
    /// What the screen calls it; the app's own name when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Where it is served, as a path on the app's origin — `/widget/chat`, the app's `widget`
    /// service. A path and not an address, because the app answers under a different hostname
    /// for every viewer (`<label>.adi` here, `<label>.<node>.n.adi` over the mesh).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// `small`, `medium`, `large` or `half`; the screen draws what it does not know as `small`.
    /// A word rather than an enum so a manifest written for a newer screen still parses here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
}

/// Read a dashboard directory's `config.toml` manifest, degrading a missing or malformed file to
/// the default (all fields absent) rather than failing.
#[must_use]
pub fn read_manifest(dir: &Path) -> Manifest {
    std::fs::read_to_string(dir.join("config.toml"))
        .ok()
        .and_then(|raw| toml::from_str::<Manifest>(&raw).ok())
        .unwrap_or_default()
}

/// Write a dashboard's `config.toml`, emitting only the fields that are present so a rewrite never
/// invents a blank `name`/`description` the manifest didn't already carry.
///
/// # Errors
/// [`std::io::Error`] on any write failure.
pub fn write_manifest(dir: &Path, manifest: &Manifest) -> std::io::Result<()> {
    let mut out = String::new();
    for (key, value) in [
        ("name", manifest.name.as_deref().map(toml_string)),
        (
            "description",
            manifest.description.as_deref().map(toml_string),
        ),
        ("project", manifest.project.as_deref().map(toml_string)),
        ("archived_at", manifest.archived_at.map(|ts| ts.to_string())),
        ("moved_to", manifest.moved_to.as_deref().map(toml_string)),
        ("icon", manifest.icon.as_deref().map(toml_string)),
    ] {
        if let Some(value) = value {
            out.push_str(key);
            out.push_str(" = ");
            out.push_str(&value);
            out.push('\n');
        }
    }
    // Tables last: in TOML a bare key after a `[table]` header belongs to that table.
    if let Some(created_from) = &manifest.created_from {
        use std::fmt::Write as _;
        out.push_str("\n[created_from]\n");
        let _ = writeln!(out, "template = {}", toml_string(&created_from.template));
        if let Some(command) = &created_from.command {
            let _ = writeln!(out, "command = {}", toml_literal_string(command));
        }
    }
    for (id, widget) in &manifest.widget {
        out.push_str(&format!("\n[widget.{}]\n", toml_key(id)));
        for (key, value) in [
            ("name", &widget.name),
            ("url", &widget.url),
            ("size", &widget.size),
        ] {
            if let Some(value) = value {
                out.push_str(&format!("{key} = {}\n", toml_string(value)));
            }
        }
    }
    std::fs::write(dir.join("config.toml"), out)
}

/// Quote a value as a TOML basic string, escaping what that grammar requires.
fn toml_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}

/// Quote a value as a TOML literal string (`'...'`) when it has nothing a literal string
/// forbids (a `'` or a newline), the same way `template.toml`'s own `command` is written — so a
/// command carrying `"$VAR"` round-trips with no escaping to read. Falls back to a basic string
/// otherwise, since a literal string cannot represent one at all.
fn toml_literal_string(value: &str) -> String {
    if value.contains('\'') || value.contains('\n') {
        toml_string(value)
    } else {
        format!("'{value}'")
    }
}

/// A table key as TOML accepts it: bare when it is only the characters a bare key allows, quoted
/// otherwise.
fn toml_key(key: &str) -> String {
    let bare = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if bare { key.to_string() } else { toml_string(key) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "adi-dashboards-manifest-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch root");
        root
    }

    #[test]
    fn a_written_manifest_round_trips_and_omits_absent_fields() {
        let dir = scratch("roundtrip");
        write_manifest(
            &dir,
            &Manifest {
                name: Some("Nosh".to_string()),
                description: Some("a \"quoted\" thing\non two lines".to_string()),
                ..Manifest::default()
            },
        )
        .expect("write");

        let raw = std::fs::read_to_string(dir.join("config.toml")).expect("read");
        assert!(!raw.contains("project"), "absent fields stay absent: {raw}");
        assert!(!raw.contains("archived_at"), "{raw}");

        let back = read_manifest(&dir);
        assert_eq!(back.name.as_deref(), Some("Nosh"));
        assert_eq!(
            back.description.as_deref(),
            Some("a \"quoted\" thing\non two lines")
        );
        assert_eq!(back.project, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_present_field_is_written() {
        let dir = scratch("all-fields");
        write_manifest(
            &dir,
            &Manifest {
                name: Some("Nosh".to_string()),
                description: Some("what it is for".to_string()),
                project: Some("demo".to_string()),
                archived_at: Some(1_786_839_320),
                moved_to: Some("laptop-b".to_string()),
                icon: Some("receipt".to_string()),
                widget: BTreeMap::new(),
                created_from: Some(CreatedFrom {
                    template: "vite-react".to_string(),
                    command: Some(
                        r#"bun create adi-family/dashboard-vite-react "$ADI_DASHBOARD_DIR""#
                            .to_string(),
                    ),
                }),
            },
        )
        .expect("write");

        let raw = std::fs::read_to_string(dir.join("config.toml")).expect("read");
        assert!(raw.contains("name = \"Nosh\"\n"), "{raw}");
        assert!(raw.contains("archived_at = 1786839320\n"), "{raw}");
        assert!(raw.contains("moved_to = \"laptop-b\"\n"), "{raw}");
        assert!(raw.contains("icon = \"receipt\"\n"), "{raw}");
        assert!(raw.contains("[created_from]\n"), "{raw}");
        assert!(raw.contains("template = \"vite-react\"\n"), "{raw}");
        assert!(
            raw.contains(r#"command = 'bun create adi-family/dashboard-vite-react "$ADI_DASHBOARD_DIR"'"#),
            "the command is a TOML literal string, unescaped: {raw}"
        );

        let back = read_manifest(&dir);
        assert_eq!(back.created_from.as_ref().map(|c| c.template.as_str()), Some("vite-react"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_created_from_with_a_quote_in_its_command_falls_back_to_a_basic_string() {
        let dir = scratch("created-from-quote");
        write_manifest(
            &dir,
            &Manifest {
                created_from: Some(CreatedFrom {
                    template: "local".to_string(),
                    command: Some("echo 'it's quoted'".to_string()),
                }),
                ..Manifest::default()
            },
        )
        .expect("write");
        let back = read_manifest(&dir);
        assert_eq!(
            back.created_from.and_then(|c| c.command),
            Some("echo 'it's quoted'".to_string())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn widgets_are_read_and_survive_a_rewrite() {
        let dir = scratch("widgets");
        std::fs::write(
            dir.join("config.toml"),
            "name = \"Agent board\"\n\n[widget.chat]\nname = \"Chat\"\nurl = \"/widget/chat\"\nsize = \"half\"\n\n[widget.\"a b\"]\nurl = \"/widget/ab\"\n",
        )
        .expect("config");

        let mut manifest = read_manifest(&dir);
        let chat = &manifest.widget["chat"];
        assert_eq!(chat.name.as_deref(), Some("Chat"));
        assert_eq!(chat.url.as_deref(), Some("/widget/chat"));
        assert_eq!(chat.size.as_deref(), Some("half"));
        assert_eq!(manifest.widget["a b"].size, None);

        // Archiving rewrites the file; the widgets must come back out of it unchanged.
        manifest.archived_at = Some(1);
        write_manifest(&dir, &manifest).expect("write");
        assert_eq!(read_manifest(&dir), manifest);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_or_broken_manifest_degrades_to_empty() {
        let dir = scratch("degrade");
        assert_eq!(read_manifest(&dir), Manifest::default());

        std::fs::write(dir.join("config.toml"), "name = [oh no\n").expect("broken");
        assert_eq!(read_manifest(&dir), Manifest::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
