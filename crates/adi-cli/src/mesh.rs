//! The `mesh` command group: enrol this machine into a fleet, or enrol machines into ours
//! (`docs/fleet.md` §2, §5, E3).
//!
//! Two commands do the work and they run on opposite machines. On the **viewer** — the one you
//! sit at — `mesh invite` mints a one-time token. On the **node** — a box reached over ssh, a
//! console, or a cloud-init script — `mesh join <token>` dials the viewer and completes the
//! pairing. The node listens for nothing at any point, which is the property §6 is built around
//! and the reason pairing cannot be the one moment a port has to be open.
//!
//! Everything else here edits the local [fleet registry](adi_mesh::fleet): what this machine
//! calls each node, what each may reach, and what this machine calls itself.
//!
//! ## Why the CLI carries iroh
//!
//! This group is the reason `adi-mono` now depends on `adi-mesh`, and through it on iroh — a
//! meaningful addition to a binary that was previously a thin argv adapter over `adi-core`. It is
//! deliberate: a node is brought up over ssh or cloud-init, where there is no control panel, no
//! browser, and frequently no second binary. Pairing has to be reachable from a shell on the box
//! itself, or the pull-only bootstrap is only pull-only in the diagram.

use std::time::Duration;

use adi_core::dns::{Dns, MeshNodeChange};
use adi_mesh::fleet::{FleetRegistry, Grant, NodeRecord};
use adi_mesh::join::{self, Joined};
use adi_mesh::node::{self, NodeConfig};
use clap::Subcommand;

use crate::format::print_json;

/// Make the front door's TLS node list agree with the registry.
///
/// The viewer's half of a pairing happens inside the mesh daemon, which cannot reach `adi-core`
/// without inverting the crate layering — so a node paired *to* this machine never records itself
/// here, and its `https://` name stays uncovered. Rather than thread a callback out of the
/// daemon, the list converges the next time anyone asks to see the fleet: `add_mesh_node` is
/// idempotent, so this is a no-op on every run but the first after a pairing.
///
/// Deliberately silent. It is a repair, not an action the operator asked for, and a line of
/// output on every `mesh fleet` would train them to ignore the one that matters.
fn reconcile_front_door(registry: &FleetRegistry) {
    for petname in registry.petnames() {
        let _ = Dns.add_mesh_node(petname);
    }
}

/// Record (or drop) a petname in the front door's TLS node list.
///
/// Certificate bookkeeping, and deliberately **advisory**: the list feeds only the leaf's SAN
/// set, while routing `*.n.adi` is one gateway rule that never consults it. So a node that could
/// not be recorded is still reachable over `http://` immediately — only `https://` waits for the
/// next front-door start. That is why this returns nothing and only speaks up on a real failure:
/// turning a certificate detail into a failed pairing would be a far worse outcome than a
/// browser warning the operator can fix by re-running the command.
///
/// It must never run *ahead* of the registry save — this is bookkeeping about a pairing that has
/// already happened.
fn record_front_door(petname: &str, present: bool) {
    let change = if present {
        Dns.add_mesh_node(petname)
    } else {
        Dns.remove_mesh_node(petname)
    };
    if change == MeshNodeChange::Failed {
        let verb = if present { "record" } else { "drop" };
        eprintln!(
            "note: could not {verb} {petname} in the front door's certificate list. \
             http://<service>.{petname}.n.adi works now; https:// will warn until the front door \
             is refreshed."
        );
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum MeshCommand {
    /// Mint a one-time invite for a node to join this fleet (run this on the viewer).
    Invite {
        /// Minutes the invite stays valid. One machine may spend it, once, within this window.
        #[arg(long, default_value_t = join::DEFAULT_TTL.as_secs() / 60)]
        ttl: u64,
        #[arg(long)]
        json: bool,
    },
    /// Join a fleet with an invite token (run this on the node). Dials out; opens nothing.
    Join {
        /// The `adi-invite:…` token printed by `mesh invite` on the viewer.
        token: String,
        #[arg(long)]
        json: bool,
    },
    /// List the paired nodes: petname, key, grants, whether a password is set.
    Fleet {
        #[arg(long)]
        json: bool,
    },
    /// List the paired nodes (alias for `fleet`).
    List {
        #[arg(long)]
        json: bool,
    },
    /// Rename a node locally — the far side is not involved (`docs/fleet.md` §2 rule 5).
    Rename {
        /// Its current petname.
        from: String,
        /// The petname to give it here.
        to: String,
    },
    /// Forget a node: it can no longer be reached, and it can no longer reach this machine.
    Unpair {
        /// The node's petname.
        petname: String,
    },
    /// Let a node reach one more thing here (`http:nosh`, `http:*`, `tcp:127.0.0.1:22`, `ctl:read`).
    Grant {
        /// The node's petname.
        petname: String,
        /// The grant to add.
        grant: String,
    },
    /// Take a grant back.
    Revoke {
        /// The node's petname.
        petname: String,
        /// The grant to remove.
        grant: String,
    },
    /// Adopt the nickname a node has started calling itself, moving its petname to match.
    AcceptName {
        /// The node's current petname.
        petname: String,
    },
    /// Set the password a paired peer must present to reach this machine's services, and print it
    /// once.
    ///
    /// Pairing prints the password once and stores only a verifier, which is the right default and
    /// a dead end when the password is lost: the only way back used to be re-pairing. This sets a
    /// new one without disturbing the pairing, the grants, or the petname.
    Passwd {
        /// The peer whose password to replace.
        petname: String,
        /// Use this password instead of a generated one. Omit for a strong random one.
        #[arg(long)]
        password: Option<String>,
        /// The username to require. Defaults to the one already stored, else `adi`.
        #[arg(long)]
        username: Option<String>,
    },
    /// Show — or set — the name this machine offers when it is the one being paired.
    Name {
        /// The new nickname (one lowercase DNS label). Omit to just show the current one.
        nickname: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

/// Dispatch a `mesh` subcommand over the mesh store.
///
/// The only command group that does not go through the `adi-core` facade: its state is the mesh
/// module's own (`fleet.toml`, `node.toml`, `invites.toml`), and routing it through `Adi` would
/// mean re-exporting the registry through a crate that has no other reason to know about peers.
///
/// # Errors
/// Any store, token, or handshake failure, as a message the caller prints.
pub(crate) fn run_mesh(command: MeshCommand) -> Result<(), String> {
    match command {
        MeshCommand::Invite { ttl, json } => invite(ttl, json),
        MeshCommand::Join { token, json } => join_fleet(&token, json),
        MeshCommand::Fleet { json } | MeshCommand::List { json } => list(json),
        MeshCommand::Rename { from, to } => rename(&from, &to),
        MeshCommand::Unpair { petname } => unpair(&petname),
        MeshCommand::Grant { petname, grant } => add_grant(&petname, &grant),
        MeshCommand::Revoke { petname, grant } => remove_grant(&petname, &grant),
        MeshCommand::AcceptName { petname } => accept_name(&petname),
        MeshCommand::Passwd {
            petname,
            password,
            username,
        } => set_passwd(&petname, password.as_deref(), username.as_deref()),
        MeshCommand::Name { nickname, json } => name(nickname.as_deref(), json),
    }
}

// -- pairing ---------------------------------------------------------------------------------

/// Mint an invite and print it with the one instruction the operator needs next.
fn invite(ttl_minutes: u64, json: bool) -> Result<(), String> {
    let token = join::mint_invite(Duration::from_secs(ttl_minutes.saturating_mul(60)))
        .map_err(|e| e.to_string())?;
    // Read the expiry back out of the token we just minted rather than recomputing it, so what is
    // reported is what the node will actually be checked against.
    let expires = join::decode_invite(&token, now_unix())
        .map(|invite| invite.expires)
        .map_err(|e| e.to_string())?;

    if json {
        print_json(&serde_json::json!({
            "token": token,
            "expires": expires,
            "ttl_minutes": ttl_minutes,
        }));
        return Ok(());
    }
    println!("{token}");
    println!();
    println!("Run this on the node — it dials out, so nothing has to be open there:");
    println!("  adi-mono mesh join {token}");
    println!(
        "Good for one machine, once, for the next {ttl_minutes} minute(s) (expires at {expires} unix)."
    );
    Ok(())
}

/// Dial the viewer, complete the handshake, and print the credentials **once**.
fn join_fleet(token: &str, json: bool) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("starting a runtime for the mesh handshake: {e}"))?;
    let joined = runtime
        .block_on(join::join(token))
        .map_err(|e| format!("{e:#}"))?;
    // The registry has saved by now, so this machine's next front-door certificate can cover
    // `*.<viewer>.n.adi` and the node can browse back.
    record_front_door(&joined.viewer, true);

    if json {
        print_json(&serde_json::json!({
            "petname": joined.petname,
            "viewer": joined.viewer,
            "viewer_key": joined.viewer_key.to_string(),
            "username": joined.username,
            "password": joined.password,
            "grants": joined.grants,
        }));
        return Ok(());
    }
    print_joined(&joined);
    Ok(())
}

/// The one place the plaintext password is ever shown. It is not stored on either machine — both
/// keep only a salted verifier — so the line saying so is part of the output, not a nicety.
fn print_joined(joined: &Joined) {
    println!("Joined the fleet as `{}`.", joined.petname);
    println!("  the viewer is filed here as: {}", joined.viewer);
    println!("  viewer key: {}", joined.viewer_key);
    println!("  it may reach: {}", grants_line(&joined.grants));
    println!();
    println!("  username: {}", joined.username);
    println!("  password: {}", joined.password);
    println!();
    println!(
        "Copy the password now: it is not stored anywhere, on either machine — only a salted \
         verifier is. The browser will ask for it the first time you open"
    );
    println!("  http://app.{}.n.adi/", joined.petname);
}

// -- the registry ----------------------------------------------------------------------------

/// Print every paired node, or the JSON the panel would read.
fn list(json: bool) -> Result<(), String> {
    let registry = FleetRegistry::load().map_err(|e| e.to_string())?;
    reconcile_front_door(&registry);
    if json {
        let nodes: Vec<serde_json::Value> = registry
            .nodes
            .iter()
            .map(|(petname, record)| node_json(petname, record))
            .collect();
        print_json(&nodes);
        return Ok(());
    }
    if registry.is_empty() {
        println!("No nodes are paired with this machine.");
        println!("Mint an invite with `adi-mono mesh invite`, then run `mesh join` on the node.");
        return Ok(());
    }
    for (petname, record) in &registry.nodes {
        print_node(petname, record);
    }
    Ok(())
}

fn node_json(petname: &str, record: &NodeRecord) -> serde_json::Value {
    serde_json::json!({
        "petname": petname,
        "key": record.key,
        "short_key": short_key(record),
        "nickname": record.nickname,
        "pending_nickname": record.pending_nickname,
        "paired_at": record.paired_at,
        "grants": record.grants,
        "password_set": record.auth.is_set(),
    })
}

fn print_node(petname: &str, record: &NodeRecord) {
    let password = if record.auth.is_set() {
        "password set"
    } else {
        "no password"
    };
    println!("{petname} — {} [{password}]", short_key(record));
    println!("  key: {}", record.key);
    println!("  grants: {}", grants_line(&record.grants));
    if record.nickname != petname {
        println!("  calls itself: {}", record.nickname);
    }
    // A declared rename is a notification and nothing else (§2 rule 4) — so it is printed with
    // the command that acts on it, never applied here.
    if let Some(declared) = &record.pending_nickname {
        println!(
            "  now calls itself: {declared}  (adi-mono mesh accept-name {petname}, or \
             mesh rename {petname} {declared})"
        );
    }
}

fn rename(from: &str, to: &str) -> Result<(), String> {
    let renamed = edit(|registry| {
        registry.rename(from, to).map_err(|e| e.to_string())?;
        Ok(format!("Renamed node {from} to {to} on this machine only."))
    });
    // The certificate list follows the petname — but only once the rename actually took. On the
    // error path the old name is still the live one, so touching the list would describe a state
    // that does not exist.
    if renamed.is_ok() {
        record_front_door(from, false);
        record_front_door(to, true);
    }
    renamed
}

fn unpair(petname: &str) -> Result<(), String> {
    let unpaired = edit(|registry| match registry.unpair(petname) {
        Some(record) => Ok(format!(
            "Unpaired {petname} ({}). It can no longer reach this machine.",
            short_key(&record)
        )),
        None => Err(format!("no node is named {petname:?} here")),
    });
    // Only after the registry saved: this is bookkeeping about a pairing that already changed.
    if unpaired.is_ok() {
        record_front_door(petname, false);
    }
    unpaired
}

fn add_grant(petname: &str, raw: &str) -> Result<(), String> {
    let grant: Grant = raw.parse().map_err(|e| format!("{e}"))?;
    edit(|registry| {
        let record = record_mut(registry, petname)?;
        let added = record.grant(grant.clone());
        Ok(if added {
            format!("{petname} may now reach {grant}.")
        } else {
            format!("{petname} could already reach {grant}.")
        })
    })
}

fn remove_grant(petname: &str, raw: &str) -> Result<(), String> {
    let grant: Grant = raw.parse().map_err(|e| format!("{e}"))?;
    edit(|registry| {
        let record = record_mut(registry, petname)?;
        let removed = record.revoke(&grant);
        Ok(if removed {
            format!("{petname} may no longer reach {grant}.")
        } else {
            format!("{petname} did not hold {grant}.")
        })
    })
}

fn accept_name(petname: &str) -> Result<(), String> {
    edit(|registry| {
        let adopted = registry
            .accept_nickname(petname)
            .map_err(|e| e.to_string())?;
        Ok(format!("{petname} is now called {adopted} here."))
    })
}

/// Replace a peer's password and print it once, the same way pairing does.
///
/// Printed, never stored: this machine keeps a salted verifier, so a password that is not written
/// down now is gone. That is the same bargain pairing makes, and this command exists so losing it
/// costs one line instead of a re-pairing that would rotate the petname's whole relationship.
fn set_passwd(petname: &str, password: Option<&str>, username: Option<&str>) -> Result<(), String> {
    let password = password.map_or_else(join::random_password, str::to_string);
    if password.is_empty() {
        return Err("an empty password would lock nothing".to_string());
    }
    let shown = password.clone();

    let mut user = String::new();
    let user_out = &mut user;
    edit(|registry| {
        let record = record_mut(registry, petname)?;
        // Keep whatever username the pairing agreed unless the caller says otherwise; changing it
        // silently would break the peer's stored credentials for no reason it could see.
        let chosen = username
            .map(str::to_string)
            .or_else(|| (!record.auth.user.is_empty()).then(|| record.auth.user.clone()))
            .unwrap_or_else(|| "adi".to_string());
        record.set_password(&chosen, &password);
        user_out.clone_from(&chosen);
        Ok(format!("Set a new password for {petname}."))
    })?;

    println!();
    println!("  username: {user}");
    println!("  password: {shown}");
    println!();
    println!("Copy it now — only a salted verifier is stored here, so this is the only time it is");
    println!("shown. The browser will ask for it the next time you open a service on {petname}.");
    Ok(())
}

// -- this machine's own name -----------------------------------------------------------------

/// Show, or set, the nickname this machine offers at pairing — and challenges with.
fn name(nickname: Option<&str>, json: bool) -> Result<(), String> {
    let mut config = NodeConfig::load().map_err(|e| e.to_string())?;
    let renamed = if let Some(nickname) = nickname {
        config.set_nickname(nickname).map_err(|e| e.to_string())?;
        config.save().map_err(|e| e.to_string())?;
        true
    } else {
        false
    };

    let effective = node::nickname();
    if json {
        print_json(&serde_json::json!({
            "nickname": effective,
            "stored": config.nickname,
            "overridden": effective != config.nickname,
        }));
        return Ok(());
    }
    println!("{effective}");
    if effective != config.nickname {
        println!(
            "  (${} overrides node.toml, which says {:?})",
            node::NAME_ENV,
            config.nickname
        );
    }
    if renamed {
        // §2 rule 4: a node's own declaration never re-points anybody's links.
        println!(
            "Nodes that already paired keep the petname they pinned; they will see this as a \
             suggested rename to accept or ignore."
        );
    }
    Ok(())
}

// -- helpers ---------------------------------------------------------------------------------

/// Load the registry, apply `edit`, save it, and print what changed. The save happens only when
/// `edit` succeeded, so a rejected grant or an unknown petname leaves the file untouched.
fn edit(edit: impl FnOnce(&mut FleetRegistry) -> Result<String, String>) -> Result<(), String> {
    let mut registry = FleetRegistry::load().map_err(|e| e.to_string())?;
    let message = edit(&mut registry)?;
    registry.save().map_err(|e| e.to_string())?;
    println!("{message}");
    Ok(())
}

fn record_mut<'a>(
    registry: &'a mut FleetRegistry,
    petname: &str,
) -> Result<&'a mut NodeRecord, String> {
    registry
        .get_mut(petname)
        .ok_or_else(|| format!("no node is named {petname:?} here"))
}

/// The key's short form, falling back to the stored string when it does not parse — a
/// hand-mangled key should still be visible, not blank.
fn short_key(record: &NodeRecord) -> String {
    record
        .endpoint_id()
        .map_or_else(|| record.key.clone(), |id| id.fmt_short().to_string())
}

/// Grants on one line, or an explicit `(none)` — because an empty list is default-**deny**
/// (`docs/fleet.md` §5) and printing nothing would read as "unrestricted".
fn grants_line(grants: &[Grant]) -> String {
    if grants.is_empty() {
        return "(none — this peer may reach nothing)".to_string();
    }
    grants
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Wall-clock seconds since the epoch, saturating at 0 for a clock set before it — the same rule
/// the store uses for its timestamps.
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory as _, Parser};

    /// A parser wrapper, so the group's argv surface can be exercised without the whole `adi-mono`
    /// command tree.
    #[derive(Debug, Parser)]
    #[command(name = "mesh")]
    struct Harness {
        #[command(subcommand)]
        command: MeshCommand,
    }

    /// `["mesh", ..words]` — the binary name clap expects in front of the words under test.
    fn command_line(words: &[&str]) -> Vec<String> {
        std::iter::once("mesh")
            .chain(words.iter().copied())
            .map(ToString::to_string)
            .collect()
    }

    fn parse(words: &[&str]) -> MeshCommand {
        Harness::try_parse_from(command_line(words))
            .unwrap_or_else(|e| panic!("{words:?} should parse: {e}"))
            .command
    }

    fn rejects(words: &[&str]) {
        assert!(
            Harness::try_parse_from(command_line(words)).is_err(),
            "{words:?} should be rejected"
        );
    }

    #[test]
    fn the_group_is_a_well_formed_clap_tree() {
        Harness::command().debug_assert();
    }

    #[test]
    fn invite_defaults_its_ttl_to_the_libraries_default() {
        let expected = join::DEFAULT_TTL.as_secs() / 60;
        match parse(&["invite"]) {
            MeshCommand::Invite { ttl, json } => {
                assert_eq!(ttl, expected);
                assert!(!json);
            }
            other => panic!("expected invite, got {other:?}"),
        }
        match parse(&["invite", "--ttl", "60", "--json"]) {
            MeshCommand::Invite { ttl, json } => {
                assert_eq!(ttl, 60);
                assert!(json);
            }
            other => panic!("expected invite, got {other:?}"),
        }
        rejects(&["invite", "--ttl"]);
        rejects(&["invite", "--ttl", "soon"]);
    }

    #[test]
    fn join_takes_the_token_positionally() {
        match parse(&["join", "adi-invite:beef"]) {
            MeshCommand::Join { token, json } => {
                assert_eq!(token, "adi-invite:beef");
                assert!(!json);
            }
            other => panic!("expected join, got {other:?}"),
        }
        rejects(&["join"]);
        rejects(&["join", "a", "b"]);
    }

    #[test]
    fn fleet_and_list_are_the_same_command_spelled_twice() {
        for spelling in ["fleet", "list"] {
            assert!(
                matches!(
                    parse(&[spelling, "--json"]),
                    MeshCommand::Fleet { json: true } | MeshCommand::List { json: true }
                ),
                "{spelling} --json"
            );
        }
    }

    #[test]
    fn the_registry_mutators_take_their_arguments_positionally() {
        match parse(&["rename", "laptop-b", "desk"]) {
            MeshCommand::Rename { from, to } => {
                assert_eq!((from.as_str(), to.as_str()), ("laptop-b", "desk"));
            }
            other => panic!("expected rename, got {other:?}"),
        }
        match parse(&["grant", "laptop-b", "http:nosh"]) {
            MeshCommand::Grant { petname, grant } => {
                assert_eq!((petname.as_str(), grant.as_str()), ("laptop-b", "http:nosh"));
            }
            other => panic!("expected grant, got {other:?}"),
        }
        match parse(&["revoke", "laptop-b", "tcp:127.0.0.1:22"]) {
            MeshCommand::Revoke { petname, grant } => {
                assert_eq!(grant, "tcp:127.0.0.1:22");
                assert_eq!(petname, "laptop-b");
            }
            other => panic!("expected revoke, got {other:?}"),
        }
        assert!(matches!(parse(&["unpair", "desk"]), MeshCommand::Unpair { .. }));
        rejects(&["rename", "only-one"]);
        rejects(&["grant", "laptop-b"]);
        rejects(&["unpair"]);
    }

    #[test]
    fn accept_name_is_spelled_in_kebab_case() {
        match parse(&["accept-name", "laptop-b"]) {
            MeshCommand::AcceptName { petname } => assert_eq!(petname, "laptop-b"),
            other => panic!("expected accept-name, got {other:?}"),
        }
        rejects(&["acceptname", "laptop-b"]);
        rejects(&["accept_name", "laptop-b"]);
    }

    /// The password must be a flag, never positional: `mesh passwd laptop-b hunter2` has to be a
    /// parse error rather than quietly setting the password of a node called `hunter2`, and a
    /// bare `mesh passwd laptop-b` has to mean "generate one".
    #[test]
    fn passwd_takes_the_peer_positionally_and_the_secret_as_a_flag() {
        match parse(&["passwd", "laptop-b"]) {
            MeshCommand::Passwd {
                petname,
                password,
                username,
            } => {
                assert_eq!(petname, "laptop-b");
                assert!(password.is_none(), "no password means generate one");
                assert!(username.is_none(), "no username means keep the stored one");
            }
            other => panic!("expected passwd, got {other:?}"),
        }

        match parse(&["passwd", "laptop-b", "--password", "s3cret", "--username", "igor"]) {
            MeshCommand::Passwd {
                password, username, ..
            } => {
                assert_eq!(password.as_deref(), Some("s3cret"));
                assert_eq!(username.as_deref(), Some("igor"));
            }
            other => panic!("expected passwd, got {other:?}"),
        }

        rejects(&["passwd"]);
        rejects(&["passwd", "laptop-b", "hunter2"]);
    }

    #[test]
    fn name_takes_an_optional_nickname() {
        match parse(&["name"]) {
            MeshCommand::Name { nickname, json } => {
                assert!(nickname.is_none(), "no argument is a read");
                assert!(!json);
            }
            other => panic!("expected name, got {other:?}"),
        }
        match parse(&["name", "laptop-b", "--json"]) {
            MeshCommand::Name { nickname, json } => {
                assert_eq!(nickname.as_deref(), Some("laptop-b"));
                assert!(json);
            }
            other => panic!("expected name, got {other:?}"),
        }
        rejects(&["name", "a", "b"]);
    }

    #[test]
    fn an_unknown_subcommand_is_rejected_rather_than_guessed() {
        rejects(&["pair"]);
        rejects(&[]);
    }

    #[test]
    fn grants_render_with_an_explicit_empty_case() {
        assert!(grants_line(&[]).contains("none"));
        let grants: Vec<Grant> = ["http:app", "tcp:127.0.0.1:22"]
            .iter()
            .map(|g| g.parse().expect("a valid grant"))
            .collect();
        assert_eq!(grants_line(&grants), "http:app · tcp:127.0.0.1:22");
    }

    #[test]
    fn a_node_line_shows_the_short_key_and_whether_a_password_is_set() {
        let key = "unparseable-by-design";
        let mut record = NodeRecord {
            key: key.to_string(),
            nickname: "laptop-b".to_string(),
            ..NodeRecord::default()
        };
        // An unreadable key still shows something actionable rather than a blank column.
        assert_eq!(short_key(&record), key);
        assert!(!record.auth.is_set());
        record.set_password("adi", "hunter2");
        assert!(record.auth.is_set());

        let json = node_json("laptop-b", &record);
        assert_eq!(json["petname"], "laptop-b");
        assert_eq!(json["password_set"], true);
        assert_eq!(json["grants"].as_array().expect("array").len(), 0);
    }
}
