//! The dashboard's hive file — the one the per-user supervisor runs and the front door routes —
//! and the hostname both of its services claim.
//!
//! A dashboard is **one origin**: the frontend owns the host's root and the backend claims
//! `/api` on the same host, so the page only ever uses relative URLs and never learns its own
//! address. That is what lets the same dashboard work at `<host>.adi`, at
//! `<host>.<node>.n.adi` over the mesh (where `127.0.0.1` would be the *viewer's* machine), and
//! behind a real domain later — for every viewer, with no substitution.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;

/// The dashboard's hive services: **one host, two services**. `{{HOST}}` is the hostname both
/// share and `{{DIR}}` the dashboard directory; nothing else is generated, and in particular no
/// port ever is — adi-hive leases those.
///
/// Kept as a template rather than a `format!` chain so the emitted YAML reads here exactly as it
/// lands on disk, comments and all.
const HIVE_TEMPLATE: &str = r#"# Dashboard hive services — run by the per-user supervisor (~/.adi/mono/dashboards/hive.yaml).
#
# One dashboard is one origin. Both services declare the same `proxy.host`: the frontend owns
# `/`, the backend claims `/api`. The page therefore only ever uses relative URLs and never
# learns its own address, which is what lets this dashboard work unchanged at `{{HOST}}`, at
# `<label>.<node>.n.adi` over the mesh, and behind a real domain later — for every viewer, with
# no substitution. Do not give the backend a host of its own: an absolute backend URL in the page
# would point at whatever machine the *browser* is on.
#
# The front door imports dashboards (stripping their runners, since it only routes) and picks
# both entries up; the per-user supervisor is what actually runs them.
#
# No port is declared: adi-hive leases a stable one per service from the ports manager (keyed
# `<dashboard-id>/frontend` and `<dashboard-id>/backend`) and injects it as $PORT. The leases are
# idempotent, so the front door resolves the same port the supervisor runs on.

version: "1"

services:
  frontend:
    restart: always
    proxy:
      host: {{HOST}}
    runner:
      type: script
      script:
        run: bun run frontend/index.ts
        working_dir: {{DIR}}

  backend:
    restart: always
    proxy:
      host: {{HOST}}
      path: /api
    runner:
      type: script
      script:
        run: bun run backend/index.ts
        working_dir: {{DIR}}
"#;

/// Render [`HIVE_TEMPLATE`] for one dashboard directory and hostname.
#[must_use]
pub fn hive_yaml(dir: &Path, host: &str) -> String {
    HIVE_TEMPLATE
        .replace("{{HOST}}", host)
        .replace("{{DIR}}", &dir.display().to_string())
}

/// The live name of the dashboard's hive file, as the supervisor's import glob names it. Writing
/// this file into a dashboard directory is the whole of "start it": the supervisor re-reads its
/// imports every few seconds, leases the ports, and runs both bun servers.
///
/// Archiving — or arriving from a marketplace — parks it under [`HIVE_ARCHIVED`] (which the glob
/// no longer matches), which is the whole of "stop it" / "not started".
pub const HIVE_LIVE: &str = "hive.yaml";
/// The parked name an unstarted dashboard's hive file takes — deliberately not `hive.yaml`, so
/// the supervisor's glob skips it.
pub const HIVE_ARCHIVED: &str = "hive.yaml.archived";

/// The zone every local service answers under, so a label becomes `<label>.adi`.
pub const HOST_ZONE: &str = "adi";

/// The path prefix the backend claims on the dashboard's host. The page's whole API base.
pub const API_PATH: &str = "/api";

/// The longest a single DNS label may be.
const MAX_LABEL: usize = 63;

/// Labels a dashboard may never take, because something else already answers there:
/// `n` is the reserved mesh zone (`docs/fleet.md` §1, and adi-hive refuses to route `n.adi`),
/// `app` is the control panel, and the rest would shadow infrastructure or read as one.
const RESERVED_LABELS: &[&str] = &["adi", "api", "app", "dns", "hive", "localhost", "n", "www"];

/// The label used when neither the name nor the id yields a usable one — a host must always exist.
const FALLBACK_LABEL: &str = "dashboard";

/// The hostname both of a dashboard's services share: `<label>.adi`.
///
/// Deterministic, and derived from what a human already typed: a slug of the dashboard's name,
/// falling back to its id when the name has nothing DNS-usable in it (all-unicode, punctuation
/// only), is reserved, or is already claimed by another dashboard — and then to a numbered
/// [`FALLBACK_LABEL`]. A collision costs you a pretty hostname, never a working one.
///
/// The id fallback is checked against the claimed labels like the name is. It did not have to be
/// while ids were UUIDs — no dashboard could declare `ec5bd98c-….adi` as its host — but an id is a
/// slug of the name now, so it can collide with exactly what the name collided with.
#[must_use]
pub fn dashboard_host(dir: &Path, name: &str) -> String {
    let id = dir
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    format!("{}.{HOST_ZONE}", host_label(dir, &id, name))
}

/// The label part of [`dashboard_host`]. Split out so the fallback chain is testable on its own.
fn host_label(dir: &Path, id: &str, name: &str) -> String {
    let taken = claimed_labels(dir);
    let free = |label: &String| !is_reserved(label) && !taken.contains(label);

    slugify(name)
        .filter(free)
        .or_else(|| slugify(id).filter(free))
        .unwrap_or_else(|| {
            // The one rule every registry settles a collision by (`adi_config::unique_id`), so a
            // fallback label is numbered the same way here as anywhere else in the store.
            adi_config::unique_id(FALLBACK_LABEL, |label| !free(&label.to_string()))
        })
}

/// Whether `label` is one of the names a dashboard must not take. Compared case-insensitively
/// even though [`slugify`] already lowercases, so a hand-edited hive file is judged the same way.
fn is_reserved(label: &str) -> bool {
    RESERVED_LABELS.contains(&label.to_ascii_lowercase().as_str())
}

/// Reduce free text to a single DNS label: ASCII-lowercased, every other character a separator,
/// runs of separators collapsed, trimmed, capped at [`MAX_LABEL`]. `None` when nothing usable is
/// left — a name written entirely in a non-Latin script is the common case, and inventing a
/// transliteration for it would be a worse hostname than the id.
fn slugify(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.truncate(MAX_LABEL);
    let label = out.trim_matches('-').to_string();
    (!label.is_empty()).then_some(label)
}

/// Every host label already claimed by a dashboard *other* than the one in `dir`.
///
/// Read from the siblings' hive files rather than re-derived from their names, so a host that was
/// hand-picked (or derived under an older rule) still counts as taken — two dashboards answering
/// on one hostname is a routing coin-flip, and the point of the check is that it never happens.
fn claimed_labels(dir: &Path) -> BTreeSet<String> {
    let Some(root) = dir.parent() else {
        return BTreeSet::new();
    };
    let Ok(entries) = std::fs::read_dir(root) else {
        return BTreeSet::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p != dir)
        .filter_map(|p| declared_host(&p))
        .map(|host| label_of(&host))
        .collect()
}

/// The first label of a hostname, lowercased — `nosh.adi` → `nosh`.
fn label_of(host: &str) -> String {
    host.trim()
        .trim_end_matches('.')
        .split('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// The dashboard's declared hostname, preferring the frontend's (it owns the host's root) and
/// falling back to the backend's. `None` when there is no hive file, it does not parse, or no
/// service declares a `proxy.host` — all three meaning "nothing has been claimed yet".
#[must_use]
pub fn declared_host(dir: &Path) -> Option<String> {
    let parsed = parse_hive(dir)?.1;
    let host = |svc: &str| {
        parsed
            .services
            .get(svc)
            .and_then(|s| s.proxy.as_ref())
            .map(|p| p.host.trim().to_string())
            .filter(|h| !h.is_empty())
    };
    host("frontend").or_else(|| host("backend"))
}

/// The proxy-relevant subset of a dashboard's hive file — enough to tell whether it already
/// declares one origin, and which host it declares. Unknown fields (runners, restart policy) are
/// ignored: this parse decides *whether* to rewrite, never what to keep.
#[derive(Debug, Deserialize)]
pub struct HiveFile {
    #[serde(default)]
    pub services: BTreeMap<String, HiveService>,
}

#[derive(Debug, Deserialize)]
pub struct HiveService {
    #[serde(default)]
    pub proxy: Option<HiveProxy>,
}

#[derive(Debug, Deserialize)]
pub struct HiveProxy {
    pub host: String,
    #[serde(default)]
    pub path: Option<String>,
}

/// Read and parse whichever of the two hive file names is on disk, returning that path with it.
/// The live name wins; an archived dashboard keeps its parked file, and reading that one too is
/// what stops a restore from bringing back a shape nobody has looked at since it was parked.
#[must_use]
pub fn parse_hive(dir: &Path) -> Option<(std::path::PathBuf, HiveFile)> {
    let path = [HIVE_LIVE, HIVE_ARCHIVED]
        .iter()
        .map(|f| dir.join(".adi").join(f))
        .find(|p| p.is_file())?;
    let raw = std::fs::read_to_string(&path).ok()?;
    let parsed = serde_yaml_ng::from_str::<HiveFile>(&raw).ok()?;
    Some((path, parsed))
}

/// Whether a parsed hive file already declares one origin: both services on the same host, the
/// frontend as that host's fallback route, the backend claiming [`API_PATH`]. Paths are compared
/// after the same normalisation adi-hive applies, so `/api/` and `api` count as current.
#[must_use]
pub fn is_one_origin(parsed: &HiveFile) -> bool {
    let proxy = |svc: &str| parsed.services.get(svc).and_then(|s| s.proxy.as_ref());
    let (Some(frontend), Some(backend)) = (proxy("frontend"), proxy("backend")) else {
        return false;
    };
    let same_host = !frontend.host.trim().is_empty()
        && frontend
            .host
            .trim()
            .eq_ignore_ascii_case(backend.host.trim());
    same_host
        && path_claim(frontend.path.as_deref()).is_none()
        && path_claim(backend.path.as_deref()).as_deref() == Some(API_PATH)
}

/// Normalise a `proxy.path` the way adi-hive's router does: `None` (the host's fallback) or a
/// `/`-rooted prefix with no trailing slash. `/` is the fallback, so it normalises to `None`.
fn path_claim(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    Some(if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    })
}

/// The hostname an arriving dashboard takes: the one it answered on where it came from, when
/// that label is free here, and a freshly derived one when it is not.
///
/// Keeping the label is what makes a transfer feel like a move rather than a copy — the same
/// dashboard is `nosh.adi` locally and `nosh.<node>.n.adi` through the mesh. But it is only ever a
/// preference: a label another dashboard on this machine already claims would make routing a
/// coin-flip, and a reserved one would shadow infrastructure.
#[must_use]
pub fn preferred_host(dir: &Path, name: &str, offered: Option<&str>) -> String {
    let taken = claimed_labels(dir);
    let offered = offered
        .map(label_of)
        .filter(|label| slugify(label).as_deref() == Some(label.as_str()))
        .filter(|label| !is_reserved(label) && !taken.contains(label));
    match offered {
        Some(label) => format!("{label}.{HOST_ZONE}"),
        None => dashboard_host(dir, name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "adi-dashboards-hive-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch root");
        root
    }

    /// A minimal sibling that claims `host`: the only thing [`claimed_labels`] reads.
    fn neighbour_claiming(root: &Path, id: &str, host: Option<&str>) -> std::path::PathBuf {
        let dir = root.join(id);
        std::fs::create_dir_all(dir.join(".adi")).expect("hive dir");
        let hive = match host {
            Some(host) => {
                format!("version: \"1\"\nservices:\n  frontend:\n    proxy:\n      host: {host}\n")
            }
            None => {
                "version: \"1\"\nservices:\n  frontend:\n    runner: {type: script}\n".to_string()
            }
        };
        std::fs::write(dir.join(".adi").join(HIVE_LIVE), hive).expect("hive file");
        dir
    }

    /// The rule from `docs/fleet.md` §2: `^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$`. Spelled out here
    /// rather than reused from the implementation, so the tests check the contract, not the code.
    fn is_dns_label(label: &str) -> bool {
        let bytes = label.as_bytes();
        let alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
        !bytes.is_empty()
            && bytes.len() <= 63
            && alnum(bytes[0])
            && alnum(bytes[bytes.len() - 1])
            && bytes.iter().all(|&b| alnum(b) || b == b'-')
    }

    /// Parse a dashboard's hive file the way adi-hive will.
    fn hive_of(dir: &Path) -> HiveFile {
        parse_hive(dir).expect("hive file parses").1
    }

    #[test]
    fn a_display_name_becomes_one_lowercase_dns_label() {
        assert_eq!(
            slugify("NakitYok Status").as_deref(),
            Some("nakityok-status")
        );
        assert_eq!(slugify("  My  Dash!!  ").as_deref(), Some("my-dash"));
        assert_eq!(slugify("CRM").as_deref(), Some("crm"));
        assert_eq!(slugify("v2.1 metrics").as_deref(), Some("v2-1-metrics"));
    }

    #[test]
    fn a_name_with_nothing_ascii_in_it_slugs_to_nothing() {
        // Transliterating would invent a hostname nobody chose; the id is the honest fallback.
        for name in ["Панель мониторинга", "ダッシュボード", "—", "  ", "!!!"]
        {
            assert_eq!(slugify(name), None, "{name}");
        }
    }

    #[test]
    fn a_long_name_is_cut_to_a_valid_label() {
        for name in [
            "a".repeat(200),
            format!("{} status page", "b".repeat(70)),
            format!("{}   ", "c".repeat(63)),
        ] {
            let label = slugify(&name).expect("a label");
            assert!(is_dns_label(&label), "{label:?} from {name:?}");
        }
    }

    #[test]
    fn the_host_label_falls_back_to_the_id_when_the_name_yields_none() {
        let root = scratch("fallback-id");
        let id = "84ddcba0-5aaf-4992-80d7-4fdda4bd6339";
        let label = host_label(&root.join(id), id, "Панель");
        assert_eq!(label, id);
        assert!(is_dns_label(&label));
    }

    #[test]
    fn a_label_another_dashboard_already_claims_falls_back_to_the_id() {
        let root = scratch("collision");
        // The neighbour is on `crm.adi` already — derived or hand-picked, it makes no difference.
        let neighbour = neighbour_claiming(&root, "1111", Some("crm.adi"));

        let id = "2222";
        assert_eq!(host_label(&root.join(id), id, "CRM"), id);
        // …while the neighbour itself keeps it: its own host never counts as taken.
        assert_eq!(host_label(&neighbour, "1111", "CRM"), "crm");
    }

    #[test]
    fn reserved_labels_are_never_handed_to_a_dashboard() {
        let root = scratch("reserved");
        // `n` is the mesh zone and `app` the control panel — either would shadow live routing.
        for (id, name) in [("3333", "app"), ("4444", "N"), ("5555", "www")] {
            assert_eq!(host_label(&root.join(id), id, name), id, "{name}");
        }
    }

    #[test]
    fn the_derived_host_is_a_label_under_the_adi_zone() {
        let root = scratch("host");
        let dir = root.join("6666");
        let host = dashboard_host(&dir, "NakitYok Status");
        assert_eq!(host, "nakityok-status.adi");
        assert!(is_dns_label(host.split('.').next().expect("label")));
    }

    #[test]
    fn an_unclaimable_id_falls_back_to_a_numbered_dashboard_label() {
        let root = scratch("numbered");
        // Both the name and the id slug to nothing usable, and a neighbour already took the
        // plain fallback — the chain has to keep counting rather than hand out a taken label.
        let neighbour = neighbour_claiming(&root, "x1", Some("dashboard.adi"));
        let claimed = claimed_labels(&root.join("x2"));
        let label = adi_config::unique_id(FALLBACK_LABEL, |l| claimed.contains(l) || l == "app");
        assert_eq!(label, "dashboard-2");
        assert!(is_dns_label(&label));
        drop(neighbour);
    }

    #[test]
    fn the_template_declares_one_origin_and_leaves_ports_to_the_manager() {
        let root = scratch("template");
        let dir = root.join("7777");
        let yaml = hive_yaml(&dir, "nosh.adi");
        std::fs::create_dir_all(dir.join(".adi")).expect("hive dir");
        std::fs::write(dir.join(".adi").join(HIVE_LIVE), &yaml).expect("write");

        let hive = hive_of(&dir);
        assert!(is_one_origin(&hive));
        let frontend = hive.services["frontend"].proxy.as_ref().expect("frontend");
        let backend = hive.services["backend"].proxy.as_ref().expect("backend");
        assert_eq!(frontend.host, "nosh.adi");
        assert_eq!(backend.host, "nosh.adi", "both services share one host");
        assert_eq!(frontend.path, None, "the frontend owns the host's root");
        assert_eq!(backend.path.as_deref(), Some("/api"));

        assert!(!yaml.contains("ports:"), "{yaml}");
        assert!(!yaml.contains("rollout:"), "{yaml}");
        assert!(yaml.contains("run: bun run frontend/index.ts"), "{yaml}");
        assert!(
            yaml.contains(&format!("working_dir: {}", dir.display())),
            "{yaml}"
        );
    }

    #[test]
    fn the_parked_hive_file_is_what_a_hiveless_directory_is_judged_by() {
        let root = scratch("parked");
        let dir = neighbour_claiming(&root, "8888", Some("nosh.adi"));
        assert_eq!(declared_host(&dir).as_deref(), Some("nosh.adi"));

        // Parked, it still names the host it will take when it comes back.
        std::fs::rename(
            dir.join(".adi").join(HIVE_LIVE),
            dir.join(".adi").join(HIVE_ARCHIVED),
        )
        .expect("park");
        assert_eq!(declared_host(&dir).as_deref(), Some("nosh.adi"));

        // A directory with no hive file at all has claimed nothing.
        let bare = root.join("9999");
        std::fs::create_dir_all(&bare).expect("bare");
        assert_eq!(declared_host(&bare), None);
    }

    #[test]
    fn an_offered_host_gives_way_to_one_this_machine_already_uses() {
        let root = scratch("preferred");
        neighbour_claiming(&root, "resident", Some("nosh.adi"));

        // Taken, and the name would derive the same taken label: fall back to the id rather than
        // hand two dashboards one hostname.
        assert_eq!(
            preferred_host(&root.join("d6"), "Nosh", Some("nosh.adi")),
            "d6.adi"
        );
        // Free, and a clean label of its own: kept, so a transfer feels like a move.
        assert_eq!(
            preferred_host(&root.join("d6"), "Nosh", Some("metrics.adi")),
            "metrics.adi"
        );
        // Absent, or not really a label: derived from the name.
        assert_eq!(
            preferred_host(&root.join("d7"), "Metrics", None),
            "metrics.adi"
        );
        assert_eq!(
            preferred_host(&root.join("d8"), "Nosh", Some("Not A Label!")),
            "d8.adi"
        );
        // Reserved is refused however it arrives.
        assert_eq!(
            preferred_host(&root.join("d7"), "Metrics", Some("app.adi")),
            "metrics.adi"
        );
    }
}
