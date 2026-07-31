//! What this machine knows about **itself** as a node (`docs/fleet.md` §2).
//!
//! The [fleet registry](crate::fleet) is the other half of §2: it holds what this machine knows
//! about *everyone else*. The one thing missing from it is the one thing a registry cannot hold —
//! the name this machine offers when it is the one being paired. That is the **nickname**, and
//! §2 is precise about its status: it is a *suggestion*. The far side pins whatever petname it
//! likes, and a later change here is a notification there, never a re-point.
//!
//! Two callers need it, and they must never disagree:
//!
//! * the [join handshake](crate::join) sends it as the name being offered, and
//! * the gateway challenges with it as the Basic-auth realm (`docs/fleet.md` §5) — the string a
//!   human reads in the browser's password prompt.
//!
//! Before this file existed the second one had no name to use and fell back to `adi node <short
//! key>`. A key is honest but unreadable, and — worse — it is not the name the operator agreed
//! at pairing, so the prompt named something the operator could not match against their fleet
//! list. One accessor now answers both, which is the point: a realm that disagreed with the
//! offered nickname would be a difference with no possible meaning.
//!
//! # Where the name comes from
//!
//! In order: [`NAME_ENV`], then `node.toml`, then — only when the file is first materialised —
//! the machine's own hostname, coerced through [`fleet::sanitize_name`](crate::fleet::sanitize_name).
//! The derivation runs **once**, at creation, and the result is written down. A nickname that
//! silently followed the system hostname would file a §2 rule-4 change against this machine on
//! every viewer in the fleet the first time somebody renamed the box in System Settings.
//!
//! Nothing here can fail into namelessness: an unusable hostname sanitises to `node`, and a fleet
//! full of machines called `node` is a solved problem — the second one becomes `node-2` on the
//! viewer, because a clash is a suggestion, never a refusal.

use adi_config::Config;
use anyhow::ensure;
use serde::{Deserialize, Serialize};
use tracing::warn;

/// The typed config file within the `mesh` module dir, beside `mesh.toml` and `fleet.toml`.
const NODE_FILE: &str = "node.toml";

/// Overrides the stored nickname for one process.
///
/// It is kept — rather than deleted along with the short-key fallback it used to guard — because
/// it is the only way to run two nodes on one machine, which is exactly what testing the fleet
/// needs: a second `$ADI_DIR` gives the second node its own store, and this gives it its own
/// name without an interactive edit. It overrides *the accessor*, not the file, so it moves the
/// offered nickname and the realm together and can never make the two disagree.
pub const NAME_ENV: &str = "ADI_NODE_NAME";

/// The whole `node.toml`: what this machine calls itself.
///
/// One field today. It is a typed file rather than a bare string on disk for the same reason
/// `mesh.toml` is: the next thing a node needs to know about itself (a contact line for the
/// not-paired page, a declared location) is then a field, not a format change.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeConfig {
    /// The name this machine offers at pairing. Empty only in memory — [`load`](Self::load)
    /// fills and persists a derived one, so the file always names the node.
    pub nickname: String,
}

impl NodeConfig {
    /// Load the node's own config, materialising `node.toml` with a hostname-derived nickname on
    /// first use.
    ///
    /// # Errors
    /// Any I/O or TOML error from the underlying store.
    pub fn load() -> anyhow::Result<Self> {
        Self::load_from(&Config::open())
    }

    /// Persist the config atomically.
    ///
    /// # Errors
    /// Any encode or I/O error from the underlying store.
    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&Config::open())
    }

    /// [`load`](Self::load) against an explicit store — for tests and alternate installs.
    ///
    /// A blank nickname (a fresh file, or one an operator emptied by hand) is filled from the
    /// hostname **and written back**, so the derivation happens exactly once in this machine's
    /// life and every later read is a plain file read.
    ///
    /// # Errors
    /// Any I/O or TOML error from the underlying store.
    pub fn load_from(store: &Config) -> anyhow::Result<Self> {
        let file = Self::file_in(store);
        let mut config: Self = file.load_or_create()?;
        if config.nickname.trim().is_empty() {
            config.nickname = nickname_from_hostname(machine_hostname().as_deref());
            file.save(&config)?;
        }
        Ok(config)
    }

    /// [`save`](Self::save) against an explicit store.
    ///
    /// # Errors
    /// Any encode or I/O error from the underlying store.
    pub fn save_to(&self, store: &Config) -> anyhow::Result<()> {
        Self::file_in(store).save(self)?;
        Ok(())
    }

    fn file_in(store: &Config) -> adi_config::ConfigFile<Self> {
        store.module(crate::config::MODULE).file(NODE_FILE)
    }

    /// Rename this machine.
    ///
    /// Strict, unlike everything on the pairing path: a name an *operator* typed is a decision, so
    /// a typo is worth an error naming the coercion they probably meant. A nickname arriving over
    /// the wire gets the opposite treatment — §2 rule 3 forbids refusing one — and the two are
    /// different situations, not an inconsistency.
    ///
    /// # Errors
    /// If `nickname` is not one lowercase DNS label.
    pub fn set_nickname(&mut self, nickname: &str) -> anyhow::Result<()> {
        let nickname = nickname.trim();
        ensure!(
            crate::fleet::valid_name(nickname),
            "{nickname:?} is not a valid node name (one lowercase DNS label, 1..=63 bytes) — try {:?}",
            crate::fleet::sanitize_name(nickname)
        );
        self.nickname = nickname.to_string();
        Ok(())
    }
}

/// This machine's nickname: [`NAME_ENV`] if it is set, else what `node.toml` says.
///
/// Infallible on purpose. Both callers are on paths where having no name is not an option — a
/// join with no nickname offers nothing, and a `401` with no realm is malformed — so a store that
/// cannot be read degrades to the same `node` a blank hostname would, with a warning, rather than
/// taking down the handshake it was only supposed to label.
#[must_use]
pub fn nickname() -> String {
    if let Some(name) = override_from(std::env::var(NAME_ENV).ok().as_deref()) {
        return name;
    }
    NodeConfig::load().map_or_else(
        |e| {
            warn!(error = %e, "node: could not read node.toml; falling back to a derived name");
            nickname_from_hostname(machine_hostname().as_deref())
        },
        |config| config.nickname,
    )
}

/// The pure half of the [`NAME_ENV`] lookup, so the override's behaviour is testable without
/// touching the process environment (which is `unsafe` to mutate under edition 2024, and shared
/// by every test in the binary besides).
///
/// Sanitised rather than validated: an operator exporting `ADI_NODE_NAME="Build Box"` meant a
/// name, and refusing it would leave the node nameless for the sake of punctuation.
fn override_from(raw: Option<&str>) -> Option<String> {
    let raw = raw.map(str::trim).filter(|raw| !raw.is_empty())?;
    Some(crate::fleet::sanitize_name(raw))
}

/// Turn a raw system hostname into a nickname: its **first label**, coerced to a valid name.
///
/// The first label only, because a hostname is frequently qualified — macOS hands out
/// `Igors-MacBook-Pro.local`, a cloud box `web-1.eu-west-1.compute.internal` — and a petname is
/// one DNS label by §2 rule 1. Keeping the whole thing would turn every dot into a hyphen and
/// give the fleet `igors-macbook-pro-local`, which is the same machine described worse.
#[must_use]
pub fn nickname_from_hostname(raw: Option<&str>) -> String {
    let first_label = raw
        .unwrap_or_default()
        .trim()
        .split('.')
        .next()
        .unwrap_or_default();
    crate::fleet::sanitize_name(first_label)
}

/// This machine's hostname, best-effort, with no new dependency and no `unsafe`.
///
/// Three sources, cheapest first: the environment variables the two families of shell export
/// (`COMPUTERNAME` on Windows, `HOSTNAME` on most Unix shells), the file Linux keeps it in, and
/// finally the `hostname` command — which is what macOS needs, having no `/etc/hostname`.
///
/// The subprocess is the reason this is called from [`NodeConfig::load_from`] only when the file
/// is being created: it runs once in the machine's life, not on every read of the node's name.
fn machine_hostname() -> Option<String> {
    for var in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(value) = std::env::var(var)
            && !value.trim().is_empty()
        {
            return Some(value);
        }
    }
    if let Ok(text) = std::fs::read_to_string("/etc/hostname")
        && !text.trim().is_empty()
    {
        return Some(text);
    }
    let output = std::process::Command::new("hostname").output().ok()?;
    let name = String::from_utf8(output.stdout).ok()?;
    if name.trim().is_empty() {
        return None;
    }
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::valid_name;

    fn scratch(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "adi-mesh-node-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ))
    }

    #[test]
    fn nickname_is_derived_from_the_hostnames_first_label() {
        assert_eq!(
            nickname_from_hostname(Some("Igors-MacBook-Pro.local")),
            "igors-macbook-pro"
        );
        assert_eq!(
            nickname_from_hostname(Some("web-1.eu-west-1.compute.internal")),
            "web-1"
        );
        assert_eq!(nickname_from_hostname(Some("  laptop-b\n")), "laptop-b");
    }

    #[test]
    fn an_unusable_hostname_still_yields_a_valid_name() {
        for raw in [None, Some(""), Some("   "), Some("***"), Some(".")] {
            let derived = nickname_from_hostname(raw);
            assert!(valid_name(&derived), "{raw:?} derived {derived:?}");
            // The shared fallback, so a nameless machine is still addressable — and a second one
            // becomes `node-2` on the viewer rather than failing to pair.
            assert_eq!(derived, "node", "{raw:?}");
        }
        assert!(valid_name(&nickname_from_hostname(Some(&"x".repeat(200)))));
    }

    #[test]
    fn the_env_override_is_coerced_not_refused() {
        assert_eq!(override_from(Some("Build Box")).as_deref(), Some("build-box"));
        assert_eq!(override_from(Some(" node-7 ")).as_deref(), Some("node-7"));
        // An unset or blank variable is not an override at all — the file wins.
        assert_eq!(override_from(None), None);
        assert_eq!(override_from(Some("   ")), None);
    }

    #[test]
    fn node_toml_round_trips_through_the_store() {
        let dir = scratch("roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        let store = Config::with_root(&dir);

        // First load materialises the file with a derived name rather than failing.
        let created = NodeConfig::load_from(&store).expect("first load");
        assert!(
            valid_name(&created.nickname),
            "derived {:?}",
            created.nickname
        );

        let mut config = NodeConfig::load_from(&store).expect("second load");
        assert_eq!(
            config.nickname, created.nickname,
            "the derivation runs once; a later read is a plain file read"
        );

        config.set_nickname("laptop-b").expect("a valid name");
        config.save_to(&store).expect("save");

        let reloaded = NodeConfig::load_from(&store).expect("reload");
        assert_eq!(reloaded.nickname, "laptop-b");

        let rendered = toml::to_string_pretty(&reloaded).expect("render");
        assert!(rendered.contains("nickname = \"laptop-b\""), "{rendered}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_blanked_nickname_is_refilled_and_written_back() {
        let dir = scratch("blank");
        let _ = std::fs::remove_dir_all(&dir);
        let store = Config::with_root(&dir);

        NodeConfig::default().save_to(&store).expect("save blank");
        let filled = NodeConfig::load_from(&store).expect("load");
        assert!(valid_name(&filled.nickname), "{:?}", filled.nickname);

        // Written back, not just filled in memory: the next process must see the same name.
        let raw = std::fs::read_to_string(store.module(crate::config::MODULE).dir().join(NODE_FILE))
            .expect("read node.toml");
        assert!(
            raw.contains(&format!("nickname = \"{}\"", filled.nickname)),
            "{raw}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_nickname_refuses_an_invalid_name_and_says_what_would_work() {
        let mut config = NodeConfig::default();
        let err = config.set_nickname("Laptop B").expect_err("not a label");
        assert!(err.to_string().contains("laptop-b"), "{err}");
        assert!(config.nickname.is_empty(), "a refused name changes nothing");

        for bad in ["", "-a", "a-", "a.b", &"x".repeat(64)] {
            assert!(config.set_nickname(bad).is_err(), "{bad:?} should be refused");
        }
        config.set_nickname("  desk  ").expect("trimmed and accepted");
        assert_eq!(config.nickname, "desk");
    }

    #[test]
    fn empty_toml_is_a_nameless_config_in_memory() {
        let config: NodeConfig = toml::from_str("").expect("empty parses");
        assert!(config.nickname.is_empty());
    }
}
