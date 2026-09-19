//! The typed `mesh.toml` config: what this machine exposes to peers (the `[host]`
//! allow-list) and what peer ports it forwards to local ports (`[[forwards]]`).
//!
//! It lives in the `mesh` module dir of the shared store (`~/.adi/mono/mesh/mesh.toml`),
//! beside the [identity](crate::identity) key. The daemon reads it once at startup; the
//! CLI mutators load / edit / save it. Kept free of iroh types so it parses and tests
//! standalone — peer-id parsing happens where the connection is (the host loop).

use adi_config::Config;
use serde::{Deserialize, Serialize};

use crate::fleet::FleetRegistry;

/// The shared-store module this crate owns: `~/.adi/mono/mesh/`.
pub const MODULE: &str = "mesh";

/// The typed config file within the module.
const CONFIG_FILE: &str = "mesh.toml";

/// The whole `mesh.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MeshConfig {
    /// Whether the mesh daemon should run on this machine. `None` means nobody has decided —
    /// distinct from `Some(false)`, which is an operator's explicit "off" — because the two
    /// resolve differently: an absent flag is inferred from whether this install already looks
    /// used ([`MeshConfig::resolved_enabled`]), while an explicit choice always wins and is never
    /// re-inferred. [`MeshConfig::load`] resolves and persists this the first time it is read, so
    /// it settles to `Some(_)` on disk after that; a config built by hand (`::default()` in a
    /// test) keeps it `None` until something resolves it.
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    /// What this machine serves to peers.
    pub host: HostConfig,
    /// Local ports this machine forwards to a peer's port.
    pub forwards: Vec<Forward>,
    /// The relay servers this machine may call home (`docs/fleet.md` §9). **Empty means the adi
    /// relays** ([`crate::relay::DEFAULT_RELAYS`]) — changed 2026-08-24, because n0's public relay
    /// refuses a browser websocket outright and a machine on it is unreachable from the browser
    /// client entirely. It used to mean n0's, which is what every machine used before this field
    /// existed. Naming your own here still overrides both.
    ///
    /// A **list**, not one URL, and that is the whole scaling story. iroh measures every relay in
    /// the map and each machine settles on its own nearest as its home relay, so adding a second
    /// region is a line here rather than a re-issue of anything: nothing routes through a relay
    /// name a peer was once told, because a peer is dialled by key and resolved through discovery.
    ///
    /// Strings rather than `RelayUrl` for the same reason [`HostConfig::authorized_peers`] holds
    /// strings: this file parses and tests without pulling iroh in. They are turned into a relay
    /// map where the endpoint is built — see [`crate::relay::relay_mode`].
    pub relays: Vec<String>,
    /// Nameservers to look the relay hosts and `dns.iroh.link` up through, **in addition to the
    /// ones the system is configured with**. Empty means the public fallbacks
    /// ([`crate::dns::DEFAULT_FALLBACK_DNS`]); naming any here replaces those, which is how a
    /// machine that must keep every query inside its own network says so.
    ///
    /// It exists because iroh reads only the system's *global* DNS configuration, while macOS
    /// resolves through every resolver in `scutil --dns` — so a VPN whose nameserver has stopped
    /// answering takes the mesh down on a machine where everything else still works. See
    /// [`crate::dns`], which has the measurement.
    ///
    /// Addresses, not hostnames (a resolver cannot resolve its own server), with an optional port
    /// and an optional `udp://` / `tcp://` / `https://` scheme.
    pub dns: Vec<String>,
}

/// The serving side: the ports peers may reach, and which peers may reach them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HostConfig {
    /// Local TCP ports exposed to peers (`127.0.0.1:<port>`). Empty exposes nothing.
    pub allow: Vec<u16>,
    /// `EndpointId`s permitted to reach the allowed ports. **Default-deny: empty admits
    /// nobody.** Naming a key is what opens a raw forward to it, and pairing does not do that
    /// for you — pairing writes a fleet record, which is a different (and also default-deny)
    /// gate. Changed 2026-08-24; empty used to mean *any* peer.
    pub authorized_peers: Vec<String>,
}

/// One forward: bind `127.0.0.1:<listen>` locally and tunnel it to `<peer>`'s `<port>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Forward {
    /// A label for logs and `list`; defaults to `<peer-prefix>:<port>` when unset.
    pub name: String,
    /// The local TCP port to bind on this machine.
    pub listen: u16,
    /// How to reach the peer: a ticket from `adi-mesh ticket` (id + addresses, reliable)
    /// or a bare `EndpointId` from `adi-mesh id` (relies on discovery).
    pub peer: String,
    /// The port to reach on the peer — must be on the peer's allow-list.
    pub port: u16,
}

impl HostConfig {
    /// Is `port` on the allow-list?
    #[must_use]
    pub fn port_allowed(&self, port: u16) -> bool {
        self.allow.contains(&port)
    }
}

impl MeshConfig {
    /// Load the config, materializing a default `mesh.toml` on first use so the user has
    /// a file to edit.
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
    /// An absent `enabled` is resolved and written back here, exactly once: the derivation
    /// ([`resolved_enabled`](Self::resolved_enabled), against this store's own fleet registry)
    /// runs only while the flag is `None`, so every later read is a plain file read that returns
    /// whatever was decided the first time — an explicit choice made in between is never
    /// overwritten, because by then `enabled` is already `Some(_)`.
    ///
    /// # Errors
    /// Any I/O or TOML error from the underlying store, including reading the fleet registry.
    pub fn load_from(store: &Config) -> anyhow::Result<Self> {
        let file = Self::file_in(store);
        let mut config: Self = file.load_or_create()?;
        if config.enabled.is_none() {
            let fleet = FleetRegistry::load_from(store)?;
            config.enabled = Some(config.resolved_enabled(&fleet));
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
        store.module(MODULE).file(CONFIG_FILE)
    }

    /// The explicit choice, if one has been made — `None` means nobody has decided yet, which is
    /// distinct from `Some(false)` (decided off). Used by `mesh status` to say whether the answer
    /// below came from an operator or from evidence.
    #[must_use]
    pub fn enabled_explicit(&self) -> Option<bool> {
        self.enabled
    }

    /// Turn the mesh on or off, persisting an explicit choice that always wins over inference
    /// from here on — what the panel's Start/Stop buttons, `mesh enable`/`disable`, and a
    /// completed join (asking to join a fleet is consent to run the mesh) all call.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = Some(enabled);
    }

    /// Whether the mesh should run: the explicit choice if one has been made, else inferred from
    /// evidence this install already looks used. A fresh install — no explicit choice, an empty
    /// fleet, nothing exposed or forwarded — stays off; anything already paired or configured
    /// keeps running. The explicit choice always wins, even over a fleet that looks used, which
    /// is what lets an operator's own "off" survive being paired with.
    #[must_use]
    pub fn resolved_enabled(&self, fleet: &FleetRegistry) -> bool {
        self.enabled.unwrap_or_else(|| self.looks_used(fleet))
    }

    /// Evidence this install is already in use, absent an explicit choice: a paired node, or a
    /// port/peer/forward of this machine's own that only means something with the mesh running.
    fn looks_used(&self, fleet: &FleetRegistry) -> bool {
        !fleet.is_empty()
            || !self.host.allow.is_empty()
            || !self.host.authorized_peers.is_empty()
            || !self.forwards.is_empty()
    }

    /// The resolved on/off state for a caller with no [`FleetRegistry`] at hand — every real
    /// caller reaches this through [`load`](Self::load), which has already resolved `enabled` to
    /// `Some(_)`; a config built directly (`::default()` in a test) reads as off, the same
    /// "absent means off" rule stated for a caller that has nothing to infer from either.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled.unwrap_or(false)
    }

    /// Add `port` to the allow-list; returns `false` if it was already present.
    pub fn allow_port(&mut self, port: u16) -> bool {
        if self.host.allow.contains(&port) {
            return false;
        }
        self.host.allow.push(port);
        self.host.allow.sort_unstable();
        true
    }

    /// Remove `port` from the allow-list; returns `true` if it was present.
    pub fn deny_port(&mut self, port: u16) -> bool {
        let before = self.host.allow.len();
        self.host.allow.retain(|p| *p != port);
        self.host.allow.len() != before
    }

    /// Authorize `peer` (an `EndpointId` string) to use the allowed ports; returns `false`
    /// if it was already authorized.
    pub fn allow_peer(&mut self, peer: String) -> bool {
        if self.host.authorized_peers.contains(&peer) {
            return false;
        }
        self.host.authorized_peers.push(peer);
        true
    }

    /// Remove `peer` from the authorized set; returns `true` if it was present. Emptying the
    /// set closes the allowed ports to everyone rather than opening them to anyone.
    pub fn deny_peer(&mut self, peer: &str) -> bool {
        let before = self.host.authorized_peers.len();
        self.host.authorized_peers.retain(|p| p != peer);
        self.host.authorized_peers.len() != before
    }

    /// Add or replace the forward bound to `forward.listen`; returns `true` if it
    /// replaced an existing one on the same local port.
    pub fn add_forward(&mut self, forward: Forward) -> bool {
        if let Some(slot) = self
            .forwards
            .iter_mut()
            .find(|f| f.listen == forward.listen)
        {
            *slot = forward;
            return true;
        }
        self.forwards.push(forward);
        false
    }

    /// Remove the forward bound to `listen`; returns `true` if one was removed.
    pub fn remove_forward(&mut self, listen: u16) -> bool {
        let before = self.forwards.len();
        self.forwards.retain(|f| f.listen != listen);
        self.forwards.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_list_is_deduped_sorted_and_removable() {
        let mut cfg = MeshConfig::default();
        assert!(cfg.allow_port(3000));
        assert!(cfg.allow_port(80));
        assert!(!cfg.allow_port(3000), "duplicate is a no-op");
        assert_eq!(cfg.host.allow, vec![80, 3000]);
        assert!(cfg.host.port_allowed(80));
        assert!(!cfg.host.port_allowed(8080));

        assert!(cfg.deny_port(80));
        assert!(!cfg.deny_port(80), "already gone");
        assert_eq!(cfg.host.allow, vec![3000]);
    }

    #[test]
    fn forward_add_replaces_on_same_local_port() {
        let mut cfg = MeshConfig::default();
        assert!(!cfg.add_forward(Forward {
            name: "a".into(),
            listen: 5000,
            peer: "peer-a".into(),
            port: 3000,
        }));
        assert!(cfg.add_forward(Forward {
            name: "b".into(),
            listen: 5000,
            peer: "peer-b".into(),
            port: 3001,
        }));
        assert_eq!(cfg.forwards.len(), 1);
        assert_eq!(cfg.forwards[0].peer, "peer-b");

        assert!(cfg.remove_forward(5000));
        assert!(cfg.forwards.is_empty());
        assert!(!cfg.remove_forward(5000));
    }

    #[test]
    fn parses_a_full_mesh_toml() {
        let cfg: MeshConfig = toml::from_str(
            r#"
[host]
allow = [3000, 5432]
authorized_peers = ["abc123"]

[[forwards]]
name = "db"
listen = 6000
peer = "def456"
port = 5432
"#,
        )
        .expect("parses");
        assert_eq!(cfg.host.allow, vec![3000, 5432]);
        assert_eq!(cfg.host.authorized_peers, vec!["abc123".to_string()]);
        assert_eq!(cfg.forwards.len(), 1);
        assert_eq!(cfg.forwards[0].listen, 6000);
        assert_eq!(cfg.forwards[0].port, 5432);
    }

    #[test]
    fn empty_toml_is_an_empty_config() {
        let cfg: MeshConfig = toml::from_str("").expect("empty parses");
        assert!(cfg.host.allow.is_empty());
        assert!(cfg.forwards.is_empty());
        assert!(
            cfg.relays.is_empty(),
            "no relays configured is the public-relay default, and must stay expressible"
        );
        assert!(
            cfg.dns.is_empty(),
            "no nameservers configured is the public-fallback default, and must stay expressible"
        );
    }

    #[test]
    fn relays_parse_as_a_list_and_round_trip() {
        let cfg: MeshConfig = toml::from_str(
            r#"
relays = [
  "https://mad.mono-relay.withadi.dev",
  "https://fra.mono-relay.withadi.dev",
]

[host]
allow = [3000]
"#,
        )
        .expect("parses");
        assert_eq!(
            cfg.relays,
            vec![
                "https://mad.mono-relay.withadi.dev".to_string(),
                "https://fra.mono-relay.withadi.dev".to_string(),
            ],
        );
        assert_eq!(
            cfg.host.allow,
            vec![3000],
            "the rest of the file is untouched"
        );

        // A machine that saves its config must not lose its relays — this file is edited by hand
        // *and* rewritten by every `mesh allow`/`grant` mutation.
        let round_tripped: MeshConfig =
            toml::from_str(&toml::to_string(&cfg).expect("serializes")).expect("re-parses");
        assert_eq!(round_tripped.relays, cfg.relays);
    }

    #[test]
    fn dns_servers_parse_as_a_list_and_round_trip() {
        let cfg: MeshConfig = toml::from_str(
            r#"
dns = ["1.1.1.1", "https://9.9.9.9"]

[host]
allow = [3000]
"#,
        )
        .expect("parses");
        assert_eq!(
            cfg.dns,
            vec!["1.1.1.1".to_string(), "https://9.9.9.9".to_string()],
        );

        // Same reason as the relays: every `mesh allow`/`grant` mutation rewrites this file, and a
        // machine that loses its nameservers loses its mesh the next time its VPN comes up.
        let round_tripped: MeshConfig =
            toml::from_str(&toml::to_string(&cfg).expect("serializes")).expect("re-parses");
        assert_eq!(round_tripped.dns, cfg.dns);
    }

    #[test]
    fn a_config_written_before_relays_existed_still_loads() {
        // adi-app, the CLI and a node's binaries are versioned apart, so an older `mesh.toml` has
        // to keep working — as "the public relays", which is exactly what it meant.
        let cfg: MeshConfig = toml::from_str(
            r#"
[host]
allow = [3000]
authorized_peers = ["abc123"]
"#,
        )
        .expect("parses");
        assert!(cfg.relays.is_empty());
    }

    // -- opt-in: whether the mesh should run at all -------------------------------------

    fn paired_fleet() -> crate::fleet::FleetRegistry {
        let mut fleet = crate::fleet::FleetRegistry::default();
        fleet
            .nodes
            .insert("laptop-b".to_string(), crate::fleet::NodeRecord::default());
        fleet
    }

    #[test]
    fn absent_and_fresh_resolves_off() {
        let cfg = MeshConfig::default();
        assert!(
            !cfg.resolved_enabled(&crate::fleet::FleetRegistry::default()),
            "a fresh install with no explicit choice must come up off"
        );
    }

    #[test]
    fn absent_and_paired_resolves_on() {
        let cfg = MeshConfig::default();
        assert!(
            cfg.resolved_enabled(&paired_fleet()),
            "an existing pairing is evidence this install is already in use"
        );
    }

    #[test]
    fn explicit_false_beats_a_paired_fleet() {
        let mut cfg = MeshConfig::default();
        cfg.set_enabled(false);
        assert!(
            !cfg.resolved_enabled(&paired_fleet()),
            "an operator's explicit off must survive being paired with — this is the rule that \
             keeps the operator's own machine off"
        );
    }

    #[test]
    fn explicit_true_beats_a_fresh_fleet() {
        let mut cfg = MeshConfig::default();
        cfg.set_enabled(true);
        assert!(
            cfg.resolved_enabled(&crate::fleet::FleetRegistry::default()),
            "an operator's explicit on must not be second-guessed by a fresh fleet"
        );
    }

    #[test]
    fn allowed_ports_authorized_peers_and_forwards_are_also_evidence_of_use() {
        let mut allow = MeshConfig::default();
        allow.allow_port(3000);
        assert!(allow.resolved_enabled(&crate::fleet::FleetRegistry::default()));

        let mut peer = MeshConfig::default();
        peer.allow_peer("abc123".to_string());
        assert!(peer.resolved_enabled(&crate::fleet::FleetRegistry::default()));

        let mut fwd = MeshConfig::default();
        fwd.add_forward(Forward {
            name: "db".into(),
            listen: 6000,
            peer: "def456".into(),
            port: 5432,
        });
        assert!(fwd.resolved_enabled(&crate::fleet::FleetRegistry::default()));
    }

    #[test]
    fn load_resolves_and_persists_the_inference_exactly_once() {
        let dir = std::env::temp_dir().join(format!(
            "adi-mesh-config-resolve-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Config::with_root(&dir);

        // Fresh store, nothing paired: the first load infers off and writes it down.
        let loaded = MeshConfig::load_from(&store).expect("first load");
        assert_eq!(loaded.enabled_explicit(), Some(false));
        assert!(!loaded.enabled());

        // Pairing a node afterwards must not flip an already-resolved "off" — the inference ran
        // once, and an explicit-looking `Some(false)` on disk is now indistinguishable from an
        // operator's own choice, which is the point: it stays put either way.
        let mut fleet = FleetRegistry::load_from(&store).expect("fleet");
        fleet
            .nodes
            .insert("laptop-b".to_string(), crate::fleet::NodeRecord::default());
        fleet.save_to(&store).expect("save fleet");

        let reloaded = MeshConfig::load_from(&store).expect("second load");
        assert_eq!(
            reloaded.enabled_explicit(),
            Some(false),
            "the resolution already ran and wrote itself down; it does not re-run"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
