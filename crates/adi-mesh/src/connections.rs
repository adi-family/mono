//! `connections.toml`, on **this device**: per-server-connection BYO-credentials settings
//! (`docs/byo-credentials.md` §4.3, §4.4, ADI-MONO-180 P2.4) — which of this device's own values
//! it releases to which paired server, for how long, and whether it unlocks on connect without
//! asking.
//!
//! Lives beside `fleet.toml` and `identity.key` in the `mesh` module dir
//! (`crate::config::MODULE`), keyed by **petname** — this device's own name for the server, the
//! same key [`crate::fleet::FleetRegistry::nodes`] uses — but in its **own file**, never a field
//! on [`crate::fleet::NodeRecord`]: the registry is the mesh's to rewrite on every pair, rename
//! and grant, and a user's own choice about their own values has no business inside it. That is
//! the one coupling between the two files: a [`crate::fleet::FleetRegistry::rename`] has to move
//! this file's entry too, and an unpair has to delete it — both are the caller's job (wherever
//! the registry mutation happens), via [`Connections::rename`]/[`Connections::remove`], exactly
//! as `adi_secrets::Secrets::forget_user` is wired at the call site rather than inside the
//! registry itself.

use adi_config::Config;
use anyhow::{bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

/// The typed config file within the `mesh` module dir.
const CONNECTIONS_FILE: &str = "connections.toml";

/// How long a device's lease outlives the connection (`docs/byo-credentials.md` §4.3) —
/// `0` | `"<n>h"` | `"forever"` on the wire, via [`FromStr`]/[`fmt::Display`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum LeaseLength {
    /// Only while connected, ending a short grace after the last renew.
    #[default]
    Connected,
    /// This many hours after the device was last seen.
    Hours(u64),
    /// Until revoked, unpaired, or the server restarts.
    Forever,
}

impl LeaseLength {
    /// A total order over duration, with [`Self::Forever`] as the top: lets [`Self::clamp`]
    /// (and anything else comparing two lengths) work by comparing one number instead of
    /// matching every combination of variants by hand.
    fn rank(self) -> u64 {
        match self {
            Self::Connected => 0,
            Self::Hours(n) => n.max(1), // `Hours(0)` would rank below `Connected`; never offered.
            Self::Forever => u64::MAX,
        }
    }

    fn from_rank(rank: u64) -> Self {
        match rank {
            0 => Self::Connected,
            u64::MAX => Self::Forever,
            n => Self::Hours(n),
        }
    }

    /// Clamp to `max` (the server's `max_lease`, already parsed) — `Forever` becomes `max`, and
    /// anything longer than it is shortened to it. `None` is "no cap": `self` unchanged
    /// (`docs/byo-credentials.md` §4.1).
    #[must_use]
    pub fn clamp(self, max: Option<Self>) -> Self {
        match max {
            Some(max) => Self::from_rank(self.rank().min(max.rank())),
            None => self,
        }
    }
}

impl fmt::Display for LeaseLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connected => f.write_str("0"),
            Self::Hours(n) => write!(f, "{n}h"),
            Self::Forever => f.write_str("forever"),
        }
    }
}

impl FromStr for LeaseLength {
    type Err = anyhow::Error;

    fn from_str(raw: &str) -> anyhow::Result<Self> {
        let raw = raw.trim();
        if raw == "0" {
            return Ok(Self::Connected);
        }
        if raw == "forever" {
            return Ok(Self::Forever);
        }
        let Some(hours) = raw.strip_suffix('h') else {
            bail!("{raw:?} is not a lease length (want `0`, `\"<n>h\"`, or `\"forever\"`)");
        };
        let hours: u64 = hours
            .parse()
            .map_err(|_| anyhow::anyhow!("{raw:?} is not a lease length (bad hour count)"))?;
        ensure!(hours > 0, "{raw:?}: 0h is written as `0`");
        Ok(Self::Hours(hours))
    }
}

impl TryFrom<String> for LeaseLength {
    type Error = anyhow::Error;

    fn try_from(raw: String) -> anyhow::Result<Self> {
        raw.parse()
    }
}

impl From<LeaseLength> for String {
    fn from(length: LeaseLength) -> Self {
        length.to_string()
    }
}

/// One server connection's BYO-credentials settings (`docs/byo-credentials.md` §4.3).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Connection {
    pub secrets: SecretsSettings,
}

/// The `[connections.<server>.secrets]` table.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SecretsSettings {
    /// Which of this device's own values this server may lease. Empty — the default — means
    /// this device never unlocks anything to this server.
    pub share: Vec<String>,
    /// How long a lease outlives this connection.
    pub lease: LeaseLength,
    /// Unlock on connect without asking.
    pub auto_unlock: bool,
}

/// The whole `connections.toml`, keyed by petname — this device's own name for the server.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Connections {
    pub connections: BTreeMap<String, Connection>,
}

impl Connections {
    /// Load from the standard store, materializing an empty file on first use.
    ///
    /// # Errors
    /// Any I/O or TOML error from the underlying store.
    pub fn load() -> anyhow::Result<Self> {
        Self::load_from(&Config::open())
    }

    /// Persist to the standard store, atomically.
    ///
    /// # Errors
    /// Any encode or I/O error from the underlying store.
    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&Config::open())
    }

    /// [`load`](Self::load) against an explicit store — for tests and alternate installs.
    ///
    /// # Errors
    /// Any I/O or TOML error from the underlying store.
    pub fn load_from(store: &Config) -> anyhow::Result<Self> {
        Ok(Self::file_in(store).load_or_create()?)
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
        store.module(crate::config::MODULE).file(CONNECTIONS_FILE)
    }

    /// This connection's settings, if any have ever been set.
    #[must_use]
    pub fn get(&self, petname: &str) -> Option<&Connection> {
        self.connections.get(petname)
    }

    /// Edit (creating if absent) one connection's secrets settings.
    pub fn edit_secrets(&mut self, petname: &str, edit: impl FnOnce(&mut SecretsSettings)) {
        edit(&mut self.connections.entry(petname.to_string()).or_default().secrets);
    }

    /// Follow a petname rename: move `from`'s entry to `to`, if it has one. The mesh's own
    /// [`crate::fleet::FleetRegistry::rename`] has no knowledge of this file; the caller runs
    /// this right after that call succeeds.
    pub fn rename(&mut self, from: &str, to: &str) {
        if let Some(entry) = self.connections.remove(from) {
            self.connections.insert(to.to_string(), entry);
        }
    }

    /// Follow an unpair: delete `petname`'s entry, if any. Returns whether one was removed —
    /// the caller's own job to run alongside `FleetRegistry::unpair`.
    pub fn remove(&mut self, petname: &str) -> bool {
        self.connections.remove(petname).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-mesh-connections-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    #[test]
    fn lease_length_round_trips_through_its_wire_spelling() {
        for (text, length) in [
            ("0", LeaseLength::Connected),
            ("24h", LeaseLength::Hours(24)),
            ("72h", LeaseLength::Hours(72)),
            ("forever", LeaseLength::Forever),
        ] {
            assert_eq!(text.parse::<LeaseLength>().expect(text), length);
            assert_eq!(length.to_string(), text);
        }
        assert!("nonsense".parse::<LeaseLength>().is_err());
        assert!("0h".parse::<LeaseLength>().is_err(), "write 0h as 0");
        assert!("-3h".parse::<LeaseLength>().is_err());
    }

    #[test]
    fn clamp_shortens_forever_and_anything_longer_than_the_cap() {
        let cap = Some(LeaseLength::Hours(24));
        assert_eq!(LeaseLength::Forever.clamp(cap), LeaseLength::Hours(24));
        assert_eq!(LeaseLength::Hours(72).clamp(cap), LeaseLength::Hours(24));
        assert_eq!(LeaseLength::Hours(12).clamp(cap), LeaseLength::Hours(12));
        assert_eq!(LeaseLength::Connected.clamp(cap), LeaseLength::Connected);
    }

    #[test]
    fn clamp_is_a_no_op_with_no_cap() {
        assert_eq!(LeaseLength::Forever.clamp(None), LeaseLength::Forever);
        assert_eq!(LeaseLength::Hours(5).clamp(None), LeaseLength::Hours(5));
    }

    #[test]
    fn a_fresh_store_has_no_connections() {
        let store = scratch("fresh");
        let connections = Connections::load_from(&store).expect("load");
        assert!(connections.connections.is_empty());
        assert!(connections.get("server-a").is_none());
    }

    #[test]
    fn edit_creates_and_round_trips_through_the_store() {
        let store = scratch("edit-roundtrip");
        let mut connections = Connections::load_from(&store).expect("load");
        connections.edit_secrets("server-a", |s| {
            s.share = vec!["CLAUDE_CODE_OAUTH_TOKEN".to_string()];
            s.lease = LeaseLength::Hours(24);
            s.auto_unlock = true;
        });
        connections.save_to(&store).expect("save");

        let reloaded = Connections::load_from(&store).expect("reload");
        let entry = reloaded.get("server-a").expect("server-a");
        assert_eq!(entry.secrets.share, vec!["CLAUDE_CODE_OAUTH_TOKEN".to_string()]);
        assert_eq!(entry.secrets.lease, LeaseLength::Hours(24));
        assert!(entry.secrets.auto_unlock);

        let rendered = toml::to_string_pretty(&reloaded).expect("render");
        assert!(rendered.contains("[connections.server-a.secrets]"), "{rendered}");
        assert!(rendered.contains("lease = \"24h\""), "{rendered}");
    }

    #[test]
    fn rename_moves_the_entry_and_a_missing_one_is_a_no_op() {
        let mut connections = Connections::default();
        connections.edit_secrets("server-a", |s| s.auto_unlock = true);

        connections.rename("server-a", "server-b");
        assert!(connections.get("server-a").is_none());
        assert!(connections.get("server-b").expect("moved").secrets.auto_unlock);

        // No entry for "ghost" — nothing to move, and not an error.
        connections.rename("ghost", "also-ghost");
        assert_eq!(connections.connections.len(), 1);
    }

    #[test]
    fn remove_deletes_the_entry_and_reports_whether_one_was_there() {
        let mut connections = Connections::default();
        connections.edit_secrets("server-a", |s| s.auto_unlock = true);

        assert!(connections.remove("server-a"));
        assert!(connections.get("server-a").is_none());
        assert!(!connections.remove("server-a"), "already gone");
    }
}
