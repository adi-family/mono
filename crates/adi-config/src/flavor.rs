//! Which ADI installation this process belongs to.
//!
//! Everything that makes an install *addressable* used to be a constant somewhere: the TLD,
//! the launchd label prefix, the store directory, the resolver port, the front-door address.
//! One constant each means one install per machine, and testing a change means overwriting
//! the copy that is currently serving.
//!
//! A [`Flavor`] is that identity, resolved once from the environment. Two are named —
//! `release` and `dev` — and they are guaranteed to share nothing (see the tests). Any other
//! id derives a complete, self-consistent identity from its own name, so a third install
//! needs no code here, only `ADI_FLAVOR=<id>`.
//!
//! Every field is also individually overridable by its own variable, and an explicit variable
//! always beats the preset. `ADI_FLAVOR=dev ADI_DOMAIN=adi-test` is a legal combination.
//!
//! **The resolved identity is exported, not re-derived.** [`Flavor::env`] returns the whole
//! thing as environment variables, and launchers pass it to every child. A supervised service
//! therefore resolves the identity its installer had, rather than resolving the presets again
//! and hoping they still agree.

use std::net::Ipv4Addr;
use std::sync::OnceLock;

use serde::Serialize;

/// The flavour a process belongs to when `ADI_FLAVOR` is unset — the real install.
pub const DEFAULT_FLAVOR: &str = "release";

/// Selects the preset. Every other variable below overrides one field of it.
pub const FLAVOR_ENV: &str = "ADI_FLAVOR";

const APP_NAME_ENV: &str = "ADI_APP_NAME";
const BUNDLE_ID_ENV: &str = "ADI_BUNDLE_ID";
const DOMAIN_ENV: &str = "ADI_DOMAIN";
const LABEL_PREFIX_ENV: &str = "ADI_LABEL_PREFIX";
const DIR_ENV: &str = "ADI_DIR";
const RESOLVER_PORT_ENV: &str = "ADI_RESOLVER_PORT";
const FRONTDOOR_ADDR_ENV: &str = "ADI_FRONTDOOR_ADDR";
const SUPERVISOR_PORT_ENV: &str = "ADI_SUPERVISOR_PORT";
const AUTO_UPDATE_ENV: &str = "ADI_AUTO_UPDATE";

/// Ports for derived flavours. Above the release resolver (10053) and the `dev` one (10063),
/// below nothing in particular — the band is simply out of the way of both, of ADI DNS on
/// 15353, and of the ports manager's `8000..=9999`.
const DERIVED_PORT_BASE: u16 = 10_100;
const DERIVED_PORT_SPAN: u16 = 900;

/// Supervisor ports for derived flavours, past `release` (45099) and `dev` (45199). Well
/// clear of the ports manager's `8000..=9999`, so no lease can ever land on one.
const DERIVED_SUPERVISOR_BASE: u16 = 45_200;
const DERIVED_SUPERVISOR_SPAN: u16 = 700;

/// Front-door loopback aliases for derived flavours, past `release` (.53) and `dev` (.54).
/// All of `127.0.0.0/8` is on `lo0` by default on macOS, so any of these binds with no setup.
const DERIVED_OCTET_BASE: u8 = 55;
const DERIVED_OCTET_SPAN: u8 = 100;

/// One ADI installation's identity: everything two installs on the same machine must not
/// share. Resolve it with [`Flavor::current`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Flavor {
    /// The flavour id — `release`, `dev`, or any other name.
    pub id: String,
    /// The bundle's user-visible name (`ADI`, `ADI Dev`), and the `/Applications` entry.
    pub app_name: String,
    /// The macOS bundle identifier.
    pub bundle_id: String,
    /// The TLD this install serves: `adi` gives `app.adi`, `adi-dev` gives `app.adi-dev`.
    pub domain: String,
    /// The launchd/systemd namespace every service label is built from.
    pub label_prefix: String,
    /// The `$HOME`-relative store directory (`.adi`), holding `mono/` and with it the ports
    /// registry — which is why two flavours get disjoint ports without doing anything.
    pub dir_name: String,
    /// The port this install's resolver listens on, and therefore the port its OS route
    /// points at.
    pub resolver_port: u16,
    /// The loopback alias this install's front door binds `:80`/`:443` on.
    pub frontdoor_addr: Ipv4Addr,
    /// The throwaway port this install's dashboards supervisor binds. It serves nothing —
    /// adi-hive simply refuses to start without a bindable address — but two supervisors on
    /// one port means the second never starts.
    pub supervisor_port: u16,
    /// Whether the auto-updater runs. Only `release` updates itself: a dev build pulling the
    /// release channel over its own bundle is never what anyone wanted.
    pub auto_update: bool,
}

/// Resolved at most once per process. A `OnceLock` rather than a plain lookup so that
/// everything downstream sees one identity even if the environment changes underneath it —
/// and so [`Flavor::pin`] can install one before the environment gets a say.
static CURRENT: OnceLock<Flavor> = OnceLock::new();

impl Flavor {
    /// This process's flavour, resolved once from the environment (or from [`Flavor::pin`]).
    #[must_use]
    pub fn current() -> &'static Self {
        CURRENT.get_or_init(|| Self::resolve(&|key| std::env::var(key).ok()))
    }

    /// The identity for `id`, with the environment still overriding individual fields — so
    /// `--flavor dev` and `ADI_DOMAIN=adi-test` compose the same way `ADI_FLAVOR=dev` would.
    #[must_use]
    pub fn for_id(id: &str) -> Self {
        let id = id.to_string();
        Self::resolve(&move |key| {
            if key == FLAVOR_ENV {
                Some(id.clone())
            } else {
                std::env::var(key).ok()
            }
        })
    }

    /// Pin this process's flavour, for a caller that learns it from somewhere other than the
    /// environment — a `--flavor` flag, say.
    ///
    /// # Errors
    /// Returns the already-resolved flavour if one was resolved first. Pinning after that
    /// point changes nothing, and silently doing nothing is how a CLI ends up operating on
    /// the wrong install; the caller is expected to treat it as the bug it is.
    pub fn pin(flavor: Self) -> Result<(), &'static Self> {
        CURRENT.set(flavor).map_err(|_| Self::current())
    }

    /// Resolve against an arbitrary lookup. The tests use this rather than mutating the
    /// process environment, which is shared by every test in the binary.
    fn resolve(lookup: &dyn Fn(&str) -> Option<String>) -> Self {
        let get = |key: &str| lookup(key).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());

        let id = get(FLAVOR_ENV).unwrap_or_else(|| DEFAULT_FLAVOR.to_string());
        let preset = Preset::for_id(&id);

        Self {
            app_name: get(APP_NAME_ENV).unwrap_or(preset.app_name),
            bundle_id: get(BUNDLE_ID_ENV).unwrap_or(preset.bundle_id),
            domain: get(DOMAIN_ENV).unwrap_or(preset.domain),
            label_prefix: get(LABEL_PREFIX_ENV).unwrap_or(preset.label_prefix),
            dir_name: get(DIR_ENV).unwrap_or(preset.dir_name),
            resolver_port: get(RESOLVER_PORT_ENV)
                .and_then(|v| v.parse().ok())
                .unwrap_or(preset.resolver_port),
            frontdoor_addr: get(FRONTDOOR_ADDR_ENV)
                .and_then(|v| v.parse().ok())
                .unwrap_or(preset.frontdoor_addr),
            supervisor_port: get(SUPERVISOR_PORT_ENV)
                .and_then(|v| v.parse().ok())
                .unwrap_or(preset.supervisor_port),
            auto_update: get(AUTO_UPDATE_ENV)
                .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
                .unwrap_or(preset.auto_update),
            id,
        }
    }

    /// Whether this is the real install. Guards the few places that may only ever touch it.
    #[must_use]
    pub fn is_release(&self) -> bool {
        self.id == DEFAULT_FLAVOR
    }

    /// A service's supervisor label — `family.adi.app.dns` for `dns` under `release`.
    #[must_use]
    pub fn label(&self, service: &str) -> String {
        format!("{}.{service}", self.label_prefix)
    }

    /// A host in this install's zone — `app` gives `app.adi`, or `app.adi-dev`.
    #[must_use]
    pub fn host(&self, name: &str) -> String {
        format!("{name}.{}", self.domain)
    }

    /// The whole identity as environment variables, for handing to a child process or writing
    /// into a service definition.
    ///
    /// Every field is exported, not just the id: a child that re-derived the presets would
    /// agree only for as long as this file does not change, and a service installed by an
    /// older build outlives that.
    #[must_use]
    pub fn env(&self) -> Vec<(&'static str, String)> {
        vec![
            (FLAVOR_ENV, self.id.clone()),
            (APP_NAME_ENV, self.app_name.clone()),
            (BUNDLE_ID_ENV, self.bundle_id.clone()),
            (DOMAIN_ENV, self.domain.clone()),
            (LABEL_PREFIX_ENV, self.label_prefix.clone()),
            (DIR_ENV, self.dir_name.clone()),
            (RESOLVER_PORT_ENV, self.resolver_port.to_string()),
            (FRONTDOOR_ADDR_ENV, self.frontdoor_addr.to_string()),
            (SUPERVISOR_PORT_ENV, self.supervisor_port.to_string()),
            (AUTO_UPDATE_ENV, self.auto_update.to_string()),
        ]
    }
}

/// The per-id defaults, before any explicit override is applied.
struct Preset {
    app_name: String,
    bundle_id: String,
    domain: String,
    label_prefix: String,
    dir_name: String,
    resolver_port: u16,
    frontdoor_addr: Ipv4Addr,
    supervisor_port: u16,
    auto_update: bool,
}

impl Preset {
    fn for_id(id: &str) -> Self {
        match id {
            DEFAULT_FLAVOR => Self {
                app_name: "ADI".to_string(),
                bundle_id: "family.adi.ADI".to_string(),
                domain: "adi".to_string(),
                label_prefix: "family.adi.app".to_string(),
                dir_name: ".adi".to_string(),
                resolver_port: 10_053,
                frontdoor_addr: Ipv4Addr::new(127, 0, 0, 53),
                supervisor_port: 45_099,
                auto_update: true,
            },
            "dev" => Self {
                app_name: "ADI Dev".to_string(),
                bundle_id: "family.adi.ADIDev".to_string(),
                domain: "adi-dev".to_string(),
                label_prefix: "family.adi-dev.app".to_string(),
                dir_name: ".adi-dev".to_string(),
                resolver_port: 10_063,
                frontdoor_addr: Ipv4Addr::new(127, 0, 0, 54),
                supervisor_port: 45_199,
                auto_update: false,
            },
            other => Self::derived(other),
        }
    }

    /// An identity for an id with no preset, derived entirely from the name so that two
    /// machines given the same id agree.
    ///
    /// The port and loopback alias come from a hash, which means two *custom* flavours can in
    /// principle collide — `ADI_RESOLVER_PORT` / `ADI_FRONTDOOR_ADDR` are the answer if they
    /// ever do. `release` and `dev` are pinned above and cannot be hit by a derived one: the
    /// bands start past both.
    fn derived(id: &str) -> Self {
        let slug = slugify(id);
        let hash = fnv1a(slug.as_bytes());
        let port = DERIVED_PORT_BASE + u16::try_from(hash % u64::from(DERIVED_PORT_SPAN)).unwrap_or(0);
        let octet = DERIVED_OCTET_BASE + u8::try_from(hash % u64::from(DERIVED_OCTET_SPAN)).unwrap_or(0);

        Self {
            app_name: format!("ADI {}", titlecase(&slug)),
            bundle_id: format!("family.adi.ADI{}", titlecase(&slug).replace(['-', ' '], "")),
            domain: format!("adi-{slug}"),
            label_prefix: format!("family.adi-{slug}.app"),
            dir_name: format!(".adi-{slug}"),
            resolver_port: port,
            frontdoor_addr: Ipv4Addr::new(127, 0, 0, octet),
            supervisor_port: DERIVED_SUPERVISOR_BASE
                + u16::try_from(hash % u64::from(DERIVED_SUPERVISOR_SPAN)).unwrap_or(0),
            auto_update: false,
        }
    }
}

/// Reduce an id to something usable as a DNS label and a directory name in one step, so the
/// domain and the store directory can never disagree about what the id was.
fn slugify(id: &str) -> String {
    let cleaned: String = id
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = cleaned.trim_matches('-').to_string();
    if slug.is_empty() {
        "custom".to_string()
    } else {
        slug
    }
}

fn titlecase(slug: &str) -> String {
    slug.split('-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// FNV-1a. Any stable hash would do; the requirement is only that it does not change between
/// builds, which `DefaultHasher` explicitly does not promise.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |key| {
            owned
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
        }
    }

    fn flavor(pairs: &[(&str, &str)]) -> Flavor {
        Flavor::resolve(&env(pairs))
    }

    #[test]
    fn nothing_set_is_the_real_install() {
        let f = flavor(&[]);
        assert!(f.is_release());
        assert_eq!(f.domain, "adi");
        assert_eq!(f.dir_name, ".adi");
        assert_eq!(f.label("dns"), "family.adi.app.dns");
        assert_eq!(f.host("app"), "app.adi");
        assert!(f.auto_update);
    }

    /// The whole point of the type: a dev install must collide with the real one nowhere. Any
    /// future edit that points a `dev` path at a `release` one fails here.
    #[test]
    fn release_and_dev_share_nothing() {
        let release = flavor(&[]);
        let dev = flavor(&[(FLAVOR_ENV, "dev")]);

        assert_ne!(release.app_name, dev.app_name);
        assert_ne!(release.bundle_id, dev.bundle_id);
        assert_ne!(release.domain, dev.domain);
        assert_ne!(release.label_prefix, dev.label_prefix);
        assert_ne!(release.dir_name, dev.dir_name);
        assert_ne!(release.resolver_port, dev.resolver_port);
        assert_ne!(release.frontdoor_addr, dev.frontdoor_addr);
        assert_ne!(release.supervisor_port, dev.supervisor_port);

        for service in ["dns", "dns-landing", "control-panel", "dashboards", "updater"] {
            assert_ne!(release.label(service), dev.label(service), "label {service}");
        }
        // Whole-label inequality is not enough: one flavour's namespace must not *contain*
        // the other's. Compare the way the OS will, at component boundaries rather than in
        // the raw strings — `.adi-dev` starts with `.adi` as text but is a sibling directory,
        // and `app.adi-dev` is not matched by a route for `.adi`.
        for (a, b) in [(&release, &dev), (&dev, &release)] {
            assert!(
                !Path::new(&a.dir_name).starts_with(&b.dir_name),
                "{} is inside {}",
                a.dir_name,
                b.dir_name
            );
            assert!(
                !a.host("app").ends_with(&format!(".{}", b.domain)),
                "{} falls in the .{} zone",
                a.host("app"),
                b.domain
            );
            assert!(
                !a.label_prefix.starts_with(&format!("{}.", b.label_prefix)),
                "{} is namespaced under {}",
                a.label_prefix,
                b.label_prefix
            );
        }
    }

    /// `dev` must never be the thing that overwrites `/Applications/ADI.app`.
    #[test]
    fn only_release_updates_itself() {
        assert!(flavor(&[]).auto_update);
        assert!(!flavor(&[(FLAVOR_ENV, "dev")]).auto_update);
        assert!(!flavor(&[(FLAVOR_ENV, "staging")]).auto_update);
    }

    #[test]
    fn an_explicit_variable_beats_the_preset() {
        let f = flavor(&[(FLAVOR_ENV, "dev"), (DOMAIN_ENV, "adi-test")]);
        assert_eq!(f.domain, "adi-test");
        assert_eq!(f.dir_name, ".adi-dev", "the rest of the preset still applies");

        // ADI_DIR predates this type and still wins on its own.
        assert_eq!(flavor(&[(DIR_ENV, ".adi-scratch")]).dir_name, ".adi-scratch");
    }

    #[test]
    fn an_unknown_id_derives_a_whole_identity() {
        let f = flavor(&[(FLAVOR_ENV, "staging")]);
        assert_eq!(f.domain, "adi-staging");
        assert_eq!(f.dir_name, ".adi-staging");
        assert_eq!(f.label_prefix, "family.adi-staging.app");
        assert_eq!(f.app_name, "ADI Staging");
        assert!(f.resolver_port >= DERIVED_PORT_BASE);
        // Clear of both presets, and of ADI DNS on 15353.
        assert!(![10_053, 10_063, 15_353].contains(&f.resolver_port));
        assert!(!(8_000..=9_999).contains(&f.resolver_port));
        assert!(!(8_000..=9_999).contains(&f.supervisor_port), "a lease could take it");
        assert!(![45_099, 45_199].contains(&f.supervisor_port), "clear of both presets");
        assert_eq!(flavor(&[(FLAVOR_ENV, "staging")]), f, "derivation is stable");
    }

    #[test]
    fn a_hostile_id_still_yields_usable_names() {
        let f = flavor(&[(FLAVOR_ENV, "  My Branch/v2!  ")]);
        assert_eq!(f.domain, "adi-my-branch-v2");
        assert_eq!(f.dir_name, ".adi-my-branch-v2");
        assert!(!f.domain.ends_with('-'));
        assert_eq!(flavor(&[(FLAVOR_ENV, "   ")]).id, DEFAULT_FLAVOR, "blank is not an id");
    }

    #[test]
    fn for_id_matches_the_environment_route() {
        // `for_id` reads the real environment for every field but the id, so a shell that
        // already pins one would make this compare two different things. Skip rather than
        // flake: the composition itself is covered by `an_explicit_variable_beats_the_preset`.
        let ambient = [DOMAIN_ENV, DIR_ENV, APP_NAME_ENV, LABEL_PREFIX_ENV,
                       RESOLVER_PORT_ENV, FRONTDOOR_ADDR_ENV, BUNDLE_ID_ENV, AUTO_UPDATE_ENV];
        if ambient.iter().any(|k| std::env::var_os(k).is_some()) {
            return;
        }
        // The two ways of naming a flavour must not disagree: `--flavor dev` is exactly
        // `ADI_FLAVOR=dev`.
        assert_eq!(Flavor::for_id("dev"), flavor(&[(FLAVOR_ENV, "dev")]));
        assert_eq!(Flavor::for_id("").id, DEFAULT_FLAVOR, "a blank flag is not an id");
    }

    /// A child re-resolving from what its parent exported must land on the same identity, or
    /// a service installed by one build drifts the moment the presets are edited.
    #[test]
    fn exported_env_round_trips() {
        for id in ["release", "dev", "staging"] {
            let parent = flavor(&[(FLAVOR_ENV, id)]);
            let exported = parent.env();
            let child = Flavor::resolve(&|key| {
                exported
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| v.clone())
            });
            assert_eq!(parent, child, "flavour {id} did not round-trip");
        }
    }
}
