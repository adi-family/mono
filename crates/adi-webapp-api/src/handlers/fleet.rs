//! `GET /api/fleet` and the local edits an operator makes to a paired node.
//!
//! The fleet registry ([`adi_mesh::fleet`]) is the machine's answer to "who am I paired with, and
//! what may they reach here?". This module is the only way the control panel sees it, which makes
//! two properties of `docs/fleet.md` this module's job rather than the page's.
//!
//! **Three names, never conflated (§2).** Every node goes out with all three: the `petname` this
//! machine calls it (and routes `<service>.<petname>.n.adi` by), the `nickname` it calls itself,
//! and the `key` underneath — the identity of record. `pending_nickname` carries the fourth thing
//! an operator needs: a node re-declaring a *different* nickname. That is filed, never applied
//! (§2 rule 4), so the endpoints that resolve it — [`fleet_accept_nickname`] and
//! [`fleet_dismiss_nickname`] — exist precisely so a rename takes a human's decision. Silently
//! dropping it on the floor is what would let any paired node rename itself to `main` and inherit
//! every link the real `main` had.
//!
//! **The verifier never leaves the machine (§5).** [`NodeRecord::auth`] holds a salted SHA-256
//! digest and its salt. Neither is ever serialized: [`FleetNode`] carries a single
//! `has_password` bool, because "is the gate configured?" is all the panel has to answer, and a
//! digest that reaches a browser is a digest an offline cracker can work on at leisure.
//!
//! Every mutation answers with a fresh [`FleetState`], the one-round-trip contract the mesh
//! endpoints already keep — the page never has to ask twice to know what it just did.
//!
//! **Both halves of pairing are here, and only one of them is whole.** [`fleet_invite`] mints, for
//! the side that can be dialled; [`fleet_join_token`] and [`fleet_joined`] are the two synchronous
//! ends of spending one, for the side that dials. The dial itself is not here because it needs the
//! endpoint the mesh daemon has already bound — `adi-app`'s `fleet_join` owns that, and calls
//! these on either side of it (`docs/fleet.md` §8).
//!
//! The store is passed in rather than opened here ([`FleetRegistry::load`] would reach for
//! `~/.adi/mono` directly), which is what lets these run against a temp root under test and never
//! touch the operator's real registry.
//!
//! Not here: the front door's `proxy.mesh_nodes` list, which decides whose `*.<petname>.n.adi`
//! the next leaf certificate covers. That bookkeeping belongs to the pairing path (`docs/fleet.md`
//! F2) and lives in `adi-core`, whose entry points read the real store unconditionally. So a
//! rename here re-points routing immediately and leaves HTTPS for the new name to the next
//! front-door update — the same split F2 already describes for a fresh pairing.
//!
//! [`NodeRecord::auth`]: adi_mesh::fleet::NodeRecord::auth

use std::time::Duration;

use adi_config::Config;
use adi_db::Db;
use adi_mesh::fleet::{FleetRegistry, Grant, Role};
use adi_mesh::join::InviteSpec;
use adi_mesh::{activity, join, ticket};
use serde::de::DeserializeOwned;

use crate::types::{
    FleetGrantRef, FleetInstructions, FleetInvite, FleetInviteCancel, FleetInviteRequest,
    FleetInvites, FleetJoinRef, FleetJoined, FleetNode, FleetPendingInvite, FleetRef, FleetRename,
    FleetState,
};

use super::response::{Response, error, ok_json};

/// How long an invite minted from the panel stays spendable.
///
/// Ten minutes, not the CLI's fifteen: this is the flow `adi-mesh-ffi`'s `viewer::INVITE_TTL`
/// already sized for — show a QR, scan it, done — which takes seconds, and an invite is a bearer
/// token until it is spent. The CLI's longer default is for the other shape of pairing, where the
/// token is carried to a cloud-init file and a machine is booted with it.
const INVITE_TTL: Duration = Duration::from_secs(10 * 60);

/// `GET /api/fleet` — every node paired with this machine, in petname order, each with whether it
/// is currently active (`adi_mesh::activity`'s presence bookkeeping) and when it was last seen.
///
/// A first read materializes an empty `fleet.toml`, so a machine that has never paired answers
/// an empty list rather than an error: nothing paired is a state, not a failure.
#[must_use]
pub fn fleet(store: &Config) -> Response {
    match snapshot(store) {
        Ok(state) => ok_json(&state),
        Err(e) => error(500, &e),
    }
}

/// `POST /api/fleet/invite` — mint a single-use pairing invite and draw it as a QR code.
///
/// The other half of pairing, and the only endpoint here that *adds* a node rather than editing
/// one already filed. It mints through [`join::mint_invite_for_with`] — the same call `adi-mono
/// mesh invite` makes — so there is one way an invite comes into existence and one book recording
/// it.
///
/// **Anything that can reach this panel can mint an invite, and that is deliberate.** The panel
/// binds loopback locally; over the mesh it sits behind the node's password *and* an `http:app`
/// grant, and `docs/fleet.md` §5 already argues the equivalent for grants — a peer that can ask
/// for a grant can already create a dashboard here and run a task on this machine, so refusing it
/// an invite protects nothing it could not reach the long way round. A panel reached over the mesh
/// may therefore mint; the gate is the grant that let it in, not a second one here.
///
/// The token is a bearer credential until it is spent, so it is answered and never kept: nothing
/// logs it, nothing routes to it, and the only copy that outlives the response is the one on the
/// operator's screen.
///
/// **The body is optional** (ADI-MONO-171 G1): absent or blank mints exactly what this always
/// has — the node's own offered nickname, the default username, and `http:app`. Given, it may
/// pin a petname, a username, a role (`admin`/`member`, applied only on a fresh pairing), a set
/// of grants, and free-form labels — see [`FleetInviteRequest`].
///
/// # Errors
/// 400 when the body is present but not the shape `FleetInviteRequest` wants, or names a grant or
/// role that does not parse. 409 when the mesh is not running here (a token nobody is listening
/// behind cannot work). 500 if the invite book cannot be written or the token cannot be drawn.
#[must_use]
pub fn fleet_invite(store: &Config, body: &[u8]) -> Response {
    let spec = match invite_spec(body) {
        Ok(spec) => spec,
        Err(response) => return response,
    };
    match mint(store, ticket::published(), adi_config::now_unix(), spec) {
        Ok(invite) => {
            super::usage_events::fleet_invite_created(store);
            ok_json(&invite)
        }
        Err(response) => response,
    }
}

/// An empty (or whitespace-only) body is the no-body call; so, on the wire, is a bare `null` —
/// the webapp's own "mint a code" button sends `()` as its JSON body (`adi-webapp`'s
/// `fetch::fleet_invite`, predating this spec), which serialises to the four bytes `null`, not to
/// nothing. Otherwise decode `FleetInviteRequest` and convert each field into its typed form,
/// failing on the first that does not parse.
fn invite_spec(body: &[u8]) -> Result<InviteSpec, Response> {
    if body.iter().all(u8::is_ascii_whitespace) || body == b"null" {
        return Ok(InviteSpec::default());
    }
    let req: FleetInviteRequest = serde_json::from_slice(body)
        .map_err(|e| error(400, &format!("invalid request body: {e}")))?;
    let grants = req
        .grants
        .iter()
        .map(|raw| raw.parse::<Grant>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| error(400, &format!("invalid grant: {e}")))?;
    let role = req
        .role
        .as_deref()
        .map(str::parse::<Role>)
        .transpose()
        .map_err(|e| error(400, &format!("invalid role: {e}")))?;
    Ok(InviteSpec {
        petname: req.petname.map(|p| p.trim().to_string()).filter(|p| !p.is_empty()),
        username: req.username.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()),
        role,
        grants,
        labels: req.labels,
    })
}

/// [`fleet_invite`] with the endpoint and the clock passed in — the seam the tests mint through,
/// since [`ticket::published`] reads the real store whatever store the handler was given.
fn mint(
    store: &Config,
    endpoint: Option<String>,
    now: u64,
    spec: InviteSpec,
) -> Result<FleetInvite, Response> {
    let Some(endpoint) = endpoint else {
        return Err(error(
            409,
            "the mesh is not running here, so a node would have nothing to dial \u{2014} start it \
             on the Mesh page and try again",
        ));
    };
    let token = join::mint_invite_for_with(&endpoint, INVITE_TTL, store, now, spec)
        .map_err(|e| error(500, &format!("minting an invite: {e}")))?;
    // Read the expiry back out of the token rather than recomputing it, so the page counts down to
    // what the minter will actually check the nonce against.
    let expires = join::decode_invite(&token, now)
        .map_err(|e| error(500, &format!("the invite just minted does not decode: {e}")))?
        .expires;
    let svg = super::qr::svg(&token).map_err(|e| error(500, &e))?;
    Ok(FleetInvite {
        token,
        expires,
        // Saturating rather than wrapping: a clock that has jumped is a reason to answer a short
        // TTL, never a 136-year one.
        ttl_secs: u32::try_from(expires.saturating_sub(now)).unwrap_or(u32::MAX),
        svg,
    })
}

/// `GET /api/fleet/invites` — every invite minted here that is still open.
///
/// # Errors
/// 500 if the invite book cannot be read.
#[must_use]
pub fn fleet_invites(store: &Config) -> Response {
    match join::InviteBook::load_from(store) {
        Ok(book) => ok_json(&FleetInvites {
            invites: book
                .open(adi_config::now_unix())
                .map(pending_invite)
                .collect(),
        }),
        Err(e) => error(500, &format!("reading the invite book: {e}")),
    }
}

fn pending_invite(invite: &join::PendingInvite) -> FleetPendingInvite {
    FleetPendingInvite {
        nonce: invite.nonce.clone(),
        expires: invite.expires,
        petname: invite.spec.petname.clone(),
        username: invite.spec.username.clone(),
        role: invite.spec.role.map(|r| r.to_string()),
        grants: invite.spec.grants.iter().map(ToString::to_string).collect(),
        labels: invite.spec.labels.clone(),
    }
}

/// `POST /api/fleet/invites/cancel` — revoke an open invite before anybody spends it.
///
/// # Errors
/// 400 for an unusable body, 404 when no open invite here carries that nonce (unknown, already
/// spent, or already expired), 500 if the book cannot be read or written.
#[must_use]
pub fn fleet_invite_cancel(store: &Config, body: &[u8]) -> Response {
    let req: FleetInviteCancel = match parse(body) {
        Ok(req) => req,
        Err(response) => return response,
    };
    let nonce = trimmed(&req.nonce);
    if nonce.is_empty() {
        return error(400, "expected JSON body { \"nonce\": \"<hex>\" }");
    }
    let mut book = match join::InviteBook::load_from(store) {
        Ok(book) => book,
        Err(e) => return error(500, &format!("reading the invite book: {e}")),
    };
    if let Err(problem) = book.cancel(&nonce, adi_config::now_unix()) {
        return error(404, &problem.to_string());
    }
    if let Err(e) = book.save_to(store) {
        return error(500, &format!("saving the invite book: {e}"));
    }
    fleet_invites(store)
}

/// The token from a `POST /api/fleet/join` body, checked as far as it can be checked *here*, and
/// whether to grant the viewer back what it granted us (ADI-MONO-171 G4).
///
/// The two halves of that endpoint are split because only one of them is synchronous: this reads
/// the body and the clock, and the caller does the dialling (`adi-app`'s `fleet_join`, which owns
/// the endpoint the handshake must go out on — see [`adi_mesh::Daemon::join`]). Decoding here is
/// what keeps a typo or a token that expired while it was being carried a **400 before anything
/// leaves the machine**, instead of a timeout the operator has to wait out and then interpret.
///
/// # Errors
/// 400 for an unusable body, an empty token, or one that does not decode as a live invite.
pub fn fleet_join_token(body: &[u8], now: u64) -> Result<(String, bool), Response> {
    let req: FleetJoinRef = parse(body)?;
    let token = trimmed(&req.token);
    if token.is_empty() {
        return Err(error(
            400,
            "expected JSON body { \"token\": \"adi-invite:…\" }",
        ));
    }
    // The decoded invite is dropped: it is read to be sure it *can* be read, and the dialling side
    // decodes it again for itself. Nothing here should hold a bearer token a moment longer than
    // the request that carried it.
    join::decode_invite(&token, now)
        .map_err(|e| error(400, &format!("that invite cannot be spent: {e}")))?;
    Ok((token, req.grant_back))
}

/// `POST /api/fleet/join` — the answer once the handshake has been accepted.
///
/// Answers with the credential *and* a fresh [`FleetState`], because this is the one mutation that
/// changes both books at once: the far side is now in the registry, and the password gating it
/// exists in plaintext exactly here, exactly once. The registry has already been saved by the time
/// this runs — [`adi_mesh::join::join_on`] writes it before returning — so the snapshot is taken
/// after the fact rather than predicted.
///
/// # Errors
/// 500 only if the registry it just wrote cannot be read back.
#[must_use]
pub fn fleet_joined(store: &Config, joined: &join::Joined) -> Response {
    let fleet = match snapshot(store) {
        Ok(fleet) => fleet,
        Err(e) => return error(500, &e),
    };
    ok_json(&FleetJoined {
        petname: joined.petname.clone(),
        viewer: joined.viewer.clone(),
        viewer_key: joined.viewer_key.to_string(),
        username: joined.username.clone(),
        password: joined.password.clone(),
        grants: joined.grants.iter().map(ToString::to_string).collect(),
        fleet,
    })
}

/// `POST /api/fleet/rename` — rename a node **locally**, with no involvement from the far side
/// (§2 rule 5): the escape hatch for belonging to two fleets that both call a machine `main`.
///
/// # Errors
/// 400 for an unusable body or a name that is not one lowercase DNS label, 404 when no node goes
/// by `petname` here, 409 when the new name already belongs to a different node.
#[must_use]
pub fn fleet_rename(store: &Config, body: &[u8]) -> Response {
    let req: FleetRename = match parse(body) {
        Ok(req) => req,
        Err(response) => return response,
    };
    let (from, to) = (trimmed(&req.petname), trimmed(&req.to));
    if from.is_empty() || to.is_empty() {
        return error(
            400,
            "expected JSON body { \"petname\": \"<node>\", \"to\": \"<new-name>\" }",
        );
    }
    fleet_edit(store, move |registry| {
        require_paired(registry, &from)?;
        if from != to && registry.get(&to).is_some() {
            return Err(error(
                409,
                &format!("the name {to:?} already belongs to another node here"),
            ));
        }
        registry
            .rename(&from, &to)
            .map_err(|e| error(400, &e.to_string()))?;
        // `connections.toml` (`docs/byo-credentials.md` §4.3) is keyed by the same petname but
        // lives in its own file, so the registry's own rename never moves it — run here, the one
        // coupling between the two files. Best-effort, like `unpair_and_forget_user`'s secrets
        // cleanup below: the rename itself already succeeded.
        move_connection(store, &from, &to);
        Ok(())
    })
}

/// Move this box's own `connections.toml` entry (if it has one) when a petname is renamed.
fn move_connection(store: &Config, from: &str, to: &str) {
    if let Ok(mut connections) = adi_mesh::connections::Connections::load_from(store) {
        connections.rename(from, to);
        let _ = connections.save_to(store);
    }
}

/// `POST /api/fleet/unpair` — forget a node: its key, its grants, and its credential.
///
/// The node is not told, and cannot be: unpairing is this machine withdrawing its own
/// authorization, and the next connection that key opens is rejected at the handshake.
///
/// # Errors
/// 400 for an unusable body, 404 when no node goes by that petname here.
#[must_use]
pub fn fleet_unpair(store: &Config, body: &[u8]) -> Response {
    let petname = match node_ref(body) {
        Ok(petname) => petname,
        Err(response) => return response,
    };
    fleet_edit(store, move |registry| {
        unpair_and_forget_user(store, registry, &petname)
            .map(|_| ())
            .ok_or_else(|| not_paired(&petname))
    })
}

/// [`FleetRegistry::unpair`], and — only when that was the **last** peer still carrying a user id
/// here — delete `secrets/users/<id>/` (`docs/byo-credentials.md` §3, ADI-MONO-179). The id is
/// never reissued, so this is the one moment it is safe to delete what it names; a second peer
/// still sharing the id (the partner sandbox's several devices pinned to one `p<uid>`) keeps the
/// storage alive.
///
/// A deletion failure is swallowed, not surfaced: the unpair itself already succeeded, and an
/// operator who asked to forget a node should not be told to go check `secrets/` by hand.
fn unpair_and_forget_user(
    store: &Config,
    registry: &mut FleetRegistry,
    petname: &str,
) -> Option<adi_mesh::fleet::NodeRecord> {
    let record = registry.unpair(petname)?;
    if let Some(user) = &record.user
        && !registry.has_user(user)
    {
        let _ = adi_secrets::Secrets::with_config(store.clone()).forget_user(user);
    }
    // This box's own `connections.toml` entry for `petname`, if any (`docs/byo-credentials.md`
    // §4.3) — the other half of the coupling `move_connection` handles for a rename.
    if let Ok(mut connections) = adi_mesh::connections::Connections::load_from(store) {
        if connections.remove(petname) {
            let _ = connections.save_to(store);
        }
    }
    Some(record)
}

/// `POST /api/fleet/sources/drop` — stop this machine reading a node, and unpair it if that was
/// all the pairing was still for.
///
/// A pairing carries two directions: this machine reading the node (a *source*), and the node
/// reading this machine (a *viewer*, which is whatever it has been granted here). Dropping the
/// first leaves the second alone — the node keeps its grants and stays paired — unless it has
/// none, in which case nothing is left for the pairing to do and the node is unpaired.
///
/// # Errors
/// 400 for an unusable body, 404 when no node goes by that petname here, 500 when the registry or
/// the dropped list cannot be written.
#[must_use]
pub fn fleet_drop_source(store: &Config, body: &[u8]) -> Response {
    let petname = match node_ref(body) {
        Ok(petname) => petname,
        Err(response) => return response,
    };
    fleet_edit(store, move |registry| {
        let record = registry.get(&petname).ok_or_else(|| not_paired(&petname))?;
        if record.grants.is_empty() {
            unpair_and_forget_user(store, registry, &petname);
            return Ok(());
        }
        let mut dropped = DroppedSources::load(store);
        dropped.add(&record.key, record.paired_at);
        dropped
            .save(store)
            .map_err(|e| error(500, &format!("writing the dropped sources: {e}")))
    })
}

/// `POST /api/fleet/viewers/drop` — stop a node reading this machine, and unpair it if this
/// machine no longer reads it either.
///
/// The mirror of [`fleet_drop_source`]: every grant the node holds here goes, and the pairing
/// stays for as long as the node is still a source.
///
/// # Errors
/// 400 for an unusable body, 404 when no node goes by that petname here.
#[must_use]
pub fn fleet_drop_viewer(store: &Config, body: &[u8]) -> Response {
    let petname = match node_ref(body) {
        Ok(petname) => petname,
        Err(response) => return response,
    };
    let mut dropped = DroppedSources::load(store);
    fleet_edit(store, move |registry| {
        let record = registry
            .get_mut(&petname)
            .ok_or_else(|| not_paired(&petname))?;
        if !dropped.holds(&record.key, record.paired_at) {
            record.grants.clear();
            return Ok(());
        }
        // The entry is for a pairing about to be gone; nothing could ever match it again.
        dropped.remove(&record.key, record.paired_at);
        unpair_and_forget_user(store, registry, &petname);
        dropped
            .save(store)
            .map_err(|e| error(500, &format!("writing the dropped sources: {e}")))
    })
}

/// Where [`DroppedSources`] is kept: beside `fleet.toml`, in the mesh module's directory.
const DROPPED_FILE: &str = "dropped-sources.toml";

/// The nodes this machine has stopped reading, each by key *and* pairing time — so pairing the
/// same machine again starts it as a source afresh, and a stale entry can only ever miss.
///
/// Its own file rather than a field on the registry's record: the registry is rewritten by every
/// adi that pairs, including one older than this list, and a field it does not know it would
/// drop on save — quietly turning a dropped source back on.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct DroppedSources {
    #[serde(default)]
    nodes: Vec<DroppedSource>,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct DroppedSource {
    key: String,
    paired_at: u64,
}

impl DroppedSources {
    fn file(store: &Config) -> adi_config::ConfigFile<Self> {
        store.module(adi_mesh::config::MODULE).file(DROPPED_FILE)
    }

    /// The list, or an empty one when it is missing or unreadable — every node a source, which
    /// is what a machine that never dropped one has.
    fn load(store: &Config) -> Self {
        let file = Self::file(store);
        if !file.exists() {
            return Self::default();
        }
        file.load().unwrap_or_default()
    }

    fn save(&self, store: &Config) -> Result<(), String> {
        Self::file(store).save(self).map_err(|e| e.to_string())
    }

    fn holds(&self, key: &str, paired_at: u64) -> bool {
        self.nodes
            .iter()
            .any(|n| n.key == key && n.paired_at == paired_at)
    }

    fn remove(&mut self, key: &str, paired_at: u64) {
        self.nodes.retain(|n| !(n.key == key && n.paired_at == paired_at));
    }

    fn add(&mut self, key: &str, paired_at: u64) {
        if !self.holds(key, paired_at) {
            self.nodes.push(DroppedSource {
                key: key.to_string(),
                paired_at,
            });
        }
    }
}

/// `POST /api/fleet/grants/add` — let a node reach one thing here.
///
/// Grants are the whole of the mesh-layer access control (§5) and they are **default-deny**: a
/// node holding none reaches nothing, so this endpoint is the only way anything opens up.
///
/// # Errors
/// 400 for an unusable body or a grant that does not parse (`http:*`, `http:<service>`,
/// `tcp:<ip>:<port>`, `ctl:<scope>`), 404 when no node goes by that petname here.
#[must_use]
pub fn fleet_grant(store: &Config, body: &[u8]) -> Response {
    let (petname, grant) = match grant_ref(body) {
        Ok(pair) => pair,
        Err(response) => return response,
    };
    fleet_edit(store, move |registry| {
        let record = registry
            .get_mut(&petname)
            .ok_or_else(|| not_paired(&petname))?;
        // Re-granting what a node already holds is not an error — the caller asked for a state,
        // and that state is what it gets.
        record.grant(grant);
        Ok(())
    })
}

/// `POST /api/fleet/grants/remove` — take one grant back. Idempotent: a grant the node did not
/// hold leaves it holding exactly what it did before.
///
/// # Errors
/// 400 for an unusable body or an unparseable grant, 404 when no node goes by that petname here.
#[must_use]
pub fn fleet_revoke(store: &Config, body: &[u8]) -> Response {
    let (petname, grant) = match grant_ref(body) {
        Ok(pair) => pair,
        Err(response) => return response,
    };
    fleet_edit(store, move |registry| {
        let record = registry
            .get_mut(&petname)
            .ok_or_else(|| not_paired(&petname))?;
        record.revoke(&grant);
        Ok(())
    })
}

/// `POST /api/fleet/instructions` — set or clear a node's standing agent instructions
/// (`adi_mesh::fleet::NodeRecord::agent_instructions`), spliced into any conversation *it* opens
/// here from now on (`docs/fleet.md` §13, ADI-MONO-13). An empty `instructions` clears it.
///
/// Frozen once per conversation, at the moment it is created, and never re-read afterwards (see
/// `adi_agents::LaunchOptions::owner_instructions`) — so an edit here only ever reaches a
/// conversation that node starts *after* it; nothing already running picks it up.
///
/// # Errors
/// 400 for an unusable body, 404 when no node goes by that petname here.
#[must_use]
pub fn fleet_instructions(store: &Config, body: &[u8]) -> Response {
    let req: FleetInstructions = match parse(body) {
        Ok(req) => req,
        Err(response) => return response,
    };
    let petname = trimmed(&req.petname);
    if petname.is_empty() {
        return error(
            400,
            "expected JSON body { \"petname\": \"<node>\", \"instructions\": \"<text>\" }",
        );
    }
    let instructions = req.instructions.trim();
    let instructions = if instructions.is_empty() {
        None
    } else {
        Some(instructions.to_string())
    };
    fleet_edit(store, move |registry| {
        let record = registry
            .get_mut(&petname)
            .ok_or_else(|| not_paired(&petname))?;
        record.agent_instructions = instructions.clone();
        Ok(())
    })
}

/// `POST /api/fleet/nickname/accept` — adopt the nickname a node now declares: acknowledge it
/// **and** move the petname to it. The one path by which a node's own declaration can change what
/// this machine calls it, and it still cannot land on a name another node holds.
///
/// # Errors
/// 400 for an unusable body, 404 when no node goes by that petname here, 409 when the node has
/// declared nothing or the declared name is unusable (taken, or not one lowercase label).
#[must_use]
pub fn fleet_accept_nickname(store: &Config, body: &[u8]) -> Response {
    let petname = match node_ref(body) {
        Ok(petname) => petname,
        Err(response) => return response,
    };
    fleet_edit(store, move |registry| {
        require_paired(registry, &petname)?;
        // Past the existence check every remaining refusal is a conflict about a name: nothing
        // pending, or a name another node already holds.
        registry
            .accept_nickname(&petname)
            .map(|_| ())
            .map_err(|e| error(409, &e.to_string()))
    })
}

/// `POST /api/fleet/nickname/dismiss` — acknowledge the declared nickname **without** moving the
/// petname: "yes, it calls itself that now; here it stays what I call it". Stops the change being
/// reported again.
///
/// # Errors
/// 400 for an unusable body, 404 when no node goes by that petname here, 409 when the node has
/// not declared a new nickname.
#[must_use]
pub fn fleet_dismiss_nickname(store: &Config, body: &[u8]) -> Response {
    let petname = match node_ref(body) {
        Ok(petname) => petname,
        Err(response) => return response,
    };
    fleet_edit(store, move |registry| {
        require_paired(registry, &petname)?;
        registry
            .dismiss_nickname(&petname)
            .map(|_| ())
            .ok_or_else(|| {
                error(
                    409,
                    &format!("node {petname:?} has not declared a new nickname"),
                )
            })
    })
}

/// The registry as the wire sees it: every node, in petname order (the registry is a `BTreeMap`,
/// so the order is the file's own and is stable between reads), joined against the shared
/// database's presence bookkeeping.
///
/// The join key is `nickname`, not `petname`: [`activity::record_seen`] is called with the
/// identity riding [`adi_mesh::auth::FLEET_NODE_HEADER`], which is the peer's own nickname — so a
/// node this machine has since renamed is still matched by what it calls itself, not by the label
/// filed for it here.
fn snapshot(store: &Config) -> Result<FleetState, String> {
    let registry =
        FleetRegistry::load_from(store).map_err(|e| format!("reading the fleet registry: {e}"))?;
    let db = Db::with_config(store.clone());
    let seen = activity::last_seen_all(&db);
    let reached = activity::last_reached_all(&db);
    let dropped = DroppedSources::load(store);
    let now = adi_config::now_unix();
    Ok(FleetState {
        nodes: registry
            .nodes
            .into_iter()
            .map(|(petname, record)| {
                let last_seen = seen.get(&record.nickname).copied();
                let active = last_seen.is_some_and(|seen| activity::is_active(seen, now));
                let source = !dropped.holds(&record.key, record.paired_at);
                let last_reached = reached.get(&record.key).copied();
                FleetNode {
                    petname,
                    key: record.key,
                    nickname: record.nickname,
                    paired_at: record.paired_at,
                    grants: record.grants.iter().map(ToString::to_string).collect(),
                    // The one thing said about the credential. Never `auth.digest`, never
                    // `auth.salt`.
                    has_password: record.auth.is_set(),
                    pending_nickname: record.pending_nickname,
                    last_seen,
                    active,
                    last_reached,
                    source,
                    agent_instructions: record.agent_instructions,
                    role: record.role.to_string(),
                    labels: record.labels,
                }
            })
            .collect(),
    })
}

/// Load the registry, apply `mutate`, save it, and answer with the fresh [`FleetState`] so the
/// client updates from one round-trip — the same contract `mesh_edit` keeps for `/api/mesh`.
///
/// `mutate` returns the [`Response`] it wants sent instead when it refuses, which is what lets
/// each endpoint pick its own status (a 404 for an unknown node, a 409 for a name clash) while
/// the load/save/answer half is written once. A refusal returns before the save, so a rejected
/// edit leaves the file untouched.
fn fleet_edit(
    store: &Config,
    mutate: impl FnOnce(&mut FleetRegistry) -> Result<(), Response>,
) -> Response {
    let mut registry = match FleetRegistry::load_from(store) {
        Ok(registry) => registry,
        Err(e) => return error(500, &format!("reading the fleet registry: {e}")),
    };
    if let Err(response) = mutate(&mut registry) {
        return response;
    }
    if let Err(e) = registry.save_to(store) {
        return error(500, &format!("saving the fleet registry: {e}"));
    }
    fleet(store)
}

/// Deserialize a request body, or the 400 to answer with.
fn parse<T: DeserializeOwned>(body: &[u8]) -> Result<T, Response> {
    serde_json::from_slice(body).map_err(|e| error(400, &format!("invalid request body: {e}")))
}

/// The petname from a body naming one node.
fn node_ref(body: &[u8]) -> Result<String, Response> {
    let req: FleetRef = parse(body)?;
    let petname = trimmed(&req.petname);
    if petname.is_empty() {
        return Err(error(400, "expected JSON body { \"petname\": \"<node>\" }"));
    }
    Ok(petname)
}

/// The `(petname, grant)` from a grant body, with the grant parsed here rather than stored as
/// text: a rule the registry cannot read is a rule that silently denies, so it is a 400 now
/// instead of a mystery later.
fn grant_ref(body: &[u8]) -> Result<(String, Grant), Response> {
    let req: FleetGrantRef = parse(body)?;
    let petname = trimmed(&req.petname);
    if petname.is_empty() {
        return Err(error(
            400,
            "expected JSON body { \"petname\": \"<node>\", \"grant\": \"<grant>\" }",
        ));
    }
    let grant = req
        .grant
        .parse::<Grant>()
        .map_err(|e| error(400, &format!("invalid grant: {e}")))?;
    Ok((petname, grant))
}

/// Refuse unless `petname` names a paired node, so every endpoint answers an unknown node with
/// the same 404 rather than each inventing one.
fn require_paired(registry: &FleetRegistry, petname: &str) -> Result<(), Response> {
    if registry.get(petname).is_some() {
        Ok(())
    } else {
        Err(not_paired(petname))
    }
}

fn not_paired(petname: &str) -> Response {
    error(
        404,
        &format!("no node named {petname:?} is paired with this machine"),
    )
}

fn trimmed(value: &str) -> String {
    value.trim().to_string()
}

#[cfg(test)]
mod tests {
    use adi_mesh::fleet::NodeRecord;
    use serde_json::Value;

    use super::*;

    /// A store rooted in a temp dir, so no test ever reads or writes the real `~/.adi/mono`
    /// registry. Same isolation the dashboards/tasks handler tests use.
    fn temp_store() -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-webapp-api-fleet-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    /// A key that looks like the real thing (an `EndpointId` renders as 52 lowercase base32
    /// characters), so the short form under test is shortening something worth shortening.
    fn key_of(seed: char) -> String {
        std::iter::repeat_n(seed, 48).collect::<String>() + "tail"
    }

    /// File a node under `petname` with the given grants, and a password when asked for one.
    fn pair(store: &Config, petname: &str, nickname: &str, grants: &[&str], password: bool) {
        let mut registry = FleetRegistry::load_from(store).expect("load");
        let mut record = NodeRecord {
            key: key_of(petname.chars().next().unwrap_or('k')),
            nickname: nickname.to_string(),
            paired_at: 1_700_000_000,
            ..NodeRecord::default()
        };
        for raw in grants {
            record.grant(raw.parse().expect("a test grant parses"));
        }
        if password {
            record.set_password("igor", "hunter2");
        }
        registry.nodes.insert(petname.to_string(), record);
        registry.save_to(store).expect("save");
    }

    /// File a declared-but-unacknowledged nickname against `petname` — what the registry records
    /// when a paired node re-introduces itself under a different name.
    fn declare(store: &Config, petname: &str, declared: &str) {
        let mut registry = FleetRegistry::load_from(store).expect("load");
        registry
            .get_mut(petname)
            .expect("a paired node")
            .pending_nickname = Some(declared.to_string());
        registry.save_to(store).expect("save");
    }

    /// The parsed body of a successful response.
    fn ok_body(response: &Response) -> Value {
        assert_eq!(response.status, 200, "{}", response.body);
        serde_json::from_str(&response.body).expect("a JSON body")
    }

    #[test]
    fn fleet_reports_every_node_with_all_three_of_its_names() {
        let store = temp_store();
        pair(
            &store,
            "laptop-b",
            "laptop-b",
            &["http:*", "ctl:read"],
            true,
        );
        pair(&store, "desk", "workstation", &[], false);

        let v = ok_body(&fleet(&store));
        let nodes = v["nodes"].as_array().expect("nodes");
        assert_eq!(nodes.len(), 2);

        // Petname order, which is the registry's own file order.
        assert_eq!(nodes[0]["petname"], "desk");
        assert_eq!(nodes[1]["petname"], "laptop-b");

        let laptop = &nodes[1];
        // The three names of §2, each in its own field: what we call it, what it calls itself,
        // and the key underneath.
        assert_eq!(laptop["nickname"], "laptop-b");
        assert_eq!(laptop["key"], key_of('l'));
        assert_eq!(laptop["paired_at"], 1_700_000_000_u64);
        assert_eq!(
            laptop["grants"]
                .as_array()
                .unwrap()
                .iter()
                .map(|g| g.as_str().unwrap())
                .collect::<Vec<_>>(),
            ["http:*", "ctl:read"],
            "grants travel in the string form an operator types"
        );
        assert_eq!(laptop["has_password"], true);
        assert!(laptop["pending_nickname"].is_null());
        assert!(
            laptop["last_seen"].is_null(),
            "paired, but never recorded as seen"
        );
        assert_eq!(laptop["active"], false);

        // The node that calls itself something else than we file it under still reads clearly.
        assert_eq!(nodes[0]["nickname"], "workstation");
        assert!(nodes[0]["grants"].as_array().unwrap().is_empty());
    }

    /// `GET /api/fleet` reports a node's role and labels (ADI-MONO-171 G1/G3) — defaulting to
    /// admin and no labels for a record paired before either existed.
    #[test]
    fn fleet_reports_role_and_labels() {
        let store = temp_store();
        pair(&store, "laptop-b", "laptop-b", &[], false);
        let v = ok_body(&fleet(&store));
        assert_eq!(v["nodes"][0]["role"], "admin", "no role on file is admin");
        assert_eq!(v["nodes"][0]["labels"], serde_json::json!({}));

        let mut registry = FleetRegistry::load_from(&store).expect("load");
        let record = registry.get_mut("laptop-b").expect("record");
        record.role = Role::Member;
        record
            .labels
            .insert("principal".to_string(), "p42".to_string());
        registry.save_to(&store).expect("save");

        let v = ok_body(&fleet(&store));
        assert_eq!(v["nodes"][0]["role"], "member");
        assert_eq!(v["nodes"][0]["labels"]["principal"], "p42");
    }

    #[test]
    fn a_node_with_no_password_says_so() {
        let store = temp_store();
        pair(&store, "desk", "desk", &[], false);

        let v = ok_body(&fleet(&store));
        assert_eq!(
            v["nodes"][0]["has_password"], false,
            "an unset credential is reported, not hidden — §5 is fail-closed and the operator \
             has to see which nodes are ungated"
        );
    }

    /// The verifier stays on the machine. Asserted against the serialized body, not the struct:
    /// a field added to the DTO later, or a `#[serde(flatten)]` of the record, would pass a
    /// struct-level check and still put the digest on the wire.
    #[test]
    fn the_response_never_carries_the_salt_or_the_digest() {
        let store = temp_store();
        pair(&store, "laptop-b", "laptop-b", &["http:*"], true);

        // What the file actually holds, so the assertion is against this credential's real
        // material rather than a guess at its shape.
        let registry = FleetRegistry::load_from(&store).expect("load");
        let auth = &registry.get("laptop-b").expect("paired").auth;
        assert!(auth.is_set() && !auth.salt.is_empty() && !auth.digest.is_empty());

        let body = fleet(&store).body;
        assert!(body.contains("has_password"), "{body}");
        for leaked in [&auth.salt, &auth.digest, &auth.user] {
            assert!(
                !body.contains(leaked.as_str()),
                "the response leaks {leaked:?}: {body}"
            );
        }
        for field in ["salt", "digest", "auth"] {
            assert!(
                !body.contains(field),
                "the response names {field:?}: {body}"
            );
        }
    }

    #[test]
    fn rename_moves_the_node_and_the_next_read_agrees() {
        let store = temp_store();
        pair(&store, "main", "main", &["http:nosh"], false);
        pair(&store, "desk", "desk", &[], false);

        let v = ok_body(&fleet_rename(&store, br#"{"petname":"main","to":"work"}"#));
        let node = v["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["petname"] == "work")
            .expect("renamed");
        assert_eq!(node["key"], key_of('m'), "the key rode along with the name");
        assert_eq!(node["grants"][0], "http:nosh", "and so did its grants");

        // The mutation answered with the fresh state, and a separate read says the same.
        let again = ok_body(&fleet(&store));
        let names: Vec<&str> = again["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["petname"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["desk", "work"]);

        // A name another node holds is a conflict, an unknown node a 404, and a name that is not
        // one lowercase label a 400 — none of which changes the file.
        assert_eq!(
            fleet_rename(&store, br#"{"petname":"work","to":"desk"}"#).status,
            409
        );
        assert_eq!(
            fleet_rename(&store, br#"{"petname":"ghost","to":"x"}"#).status,
            404
        );
        assert_eq!(
            fleet_rename(&store, br#"{"petname":"work","to":"Work"}"#).status,
            400
        );
        assert_eq!(fleet_rename(&store, br#"{"petname":"work"}"#).status, 400);
        assert_eq!(fleet_rename(&store, b"not json").status, 400);
        assert_eq!(ok_body(&fleet(&store))["nodes"][1]["petname"], "work");
    }

    #[test]
    fn dropping_one_direction_keeps_the_pairing_until_neither_is_left() {
        let store = temp_store();
        pair(&store, "laptop-b", "laptop-b", &["http:app"], false);
        pair(&store, "desk", "desk", &[], false);
        let node = |v: &serde_json::Value, name: &str| {
            v["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|n| n["petname"] == name)
                .cloned()
        };

        // A viewer that is also a source: dropping the source leaves it paired and viewing.
        let v = ok_body(&fleet_drop_source(&store, br#"{"petname":"laptop-b"}"#));
        let laptop = node(&v, "laptop-b").expect("still paired");
        assert_eq!(laptop["source"], false);
        assert_eq!(laptop["grants"], serde_json::json!(["http:app"]));

        // Dropping the viewer too leaves nothing, so it unpairs.
        let v = ok_body(&fleet_drop_viewer(&store, br#"{"petname":"laptop-b"}"#));
        assert!(node(&v, "laptop-b").is_none());
        assert!(DroppedSources::load(&store).nodes.is_empty(), "its entry went with it");

        // A source that views nothing here unpairs on its own drop.
        let v = ok_body(&fleet_drop_source(&store, br#"{"petname":"desk"}"#));
        assert!(node(&v, "desk").is_none());

        // Pairing the same machine again starts it as a source afresh: the drop was for the
        // pairing that is gone.
        pair(&store, "laptop-b", "laptop-b", &[], false);
        let mut registry = FleetRegistry::load_from(&store).expect("load");
        registry.get_mut("laptop-b").expect("paired").paired_at += 1;
        registry.save_to(&store).expect("save");
        assert_eq!(node(&ok_body(&fleet(&store)), "laptop-b").unwrap()["source"], true);
    }

    #[test]
    fn dropping_a_viewer_that_is_a_source_only_takes_its_grants() {
        let store = temp_store();
        pair(&store, "laptop-b", "laptop-b", &["http:app", "http:nosh"], false);
        let v = ok_body(&fleet_drop_viewer(&store, br#"{"petname":"laptop-b"}"#));
        assert_eq!(v["nodes"][0]["grants"], serde_json::json!([]));
        assert_eq!(v["nodes"][0]["source"], true);
        assert_eq!(fleet_drop_viewer(&store, br#"{"petname":"gone"}"#).status, 404);
    }

    #[test]
    fn unpair_forgets_the_node() {
        let store = temp_store();
        pair(&store, "laptop-b", "laptop-b", &["http:*"], true);
        pair(&store, "desk", "desk", &[], false);

        let v = ok_body(&fleet_unpair(&store, br#"{"petname":"laptop-b"}"#));
        let names: Vec<&str> = v["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["petname"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["desk"]);
        assert_eq!(
            ok_body(&fleet(&store))["nodes"].as_array().unwrap().len(),
            1
        );

        // Unpairing what is already gone is a 404, not a silent success.
        assert_eq!(
            fleet_unpair(&store, br#"{"petname":"laptop-b"}"#).status,
            404
        );
        assert_eq!(fleet_unpair(&store, br#"{"petname":"  "}"#).status, 400);
    }

    /// File `petname`'s record with the given user id — the ADI-MONO-179 counterpart of [`pair`].
    fn pair_with_user(store: &Config, petname: &str, user: &str) {
        let mut registry = FleetRegistry::load_from(store).expect("load");
        registry.get_mut(petname).expect("just paired").user = Some(user.to_string());
        registry.save_to(store).expect("save");
    }

    #[test]
    fn unpairing_the_last_peer_of_a_user_deletes_its_secrets() {
        let store = temp_store();
        pair(&store, "laptop-b", "laptop-b", &[], false);
        pair_with_user(&store, "laptop-b", "u-aaa");
        let secrets = adi_secrets::Secrets::with_config(store.clone());
        secrets.set(None, "K", "v", None).expect("global secret");
        // There is no API that writes a user-scoped secret yet (phase 1), so the scope directory
        // is created directly — the same thing a later phase's `set --user` would leave behind.
        let user_dir = secrets.dir().join("users/u-aaa");
        std::fs::create_dir_all(&user_dir).expect("mkdir");
        std::fs::write(user_dir.join("K.toml"), "stub").expect("write");

        let _ = fleet_unpair(&store, br#"{"petname":"laptop-b"}"#);

        assert!(!user_dir.exists(), "the last peer of u-aaa is gone");
        assert!(
            secrets.dir().join("global/K.toml").exists(),
            "an unrelated scope is untouched"
        );
    }

    #[test]
    fn unpairing_one_of_two_peers_sharing_a_user_keeps_its_secrets() {
        let store = temp_store();
        pair(&store, "laptop-b", "laptop-b", &[], false);
        pair(&store, "phone-b", "phone-b", &[], false);
        pair_with_user(&store, "laptop-b", "u-aaa");
        pair_with_user(&store, "phone-b", "u-aaa");
        let secrets = adi_secrets::Secrets::with_config(store.clone());
        let user_dir = secrets.dir().join("users/u-aaa");
        std::fs::create_dir_all(&user_dir).expect("mkdir");
        std::fs::write(user_dir.join("K.toml"), "stub").expect("write");

        let _ = fleet_unpair(&store, br#"{"petname":"laptop-b"}"#);

        assert!(user_dir.exists(), "phone-b still carries u-aaa");

        let _ = fleet_unpair(&store, br#"{"petname":"phone-b"}"#);

        assert!(!user_dir.exists(), "now the last peer is gone too");
    }

    #[test]
    fn granting_and_revoking_change_what_the_next_read_returns() {
        let store = temp_store();
        pair(&store, "desk", "desk", &[], false);

        // Default-deny: a fresh node holds nothing at all.
        assert!(
            ok_body(&fleet(&store))["nodes"][0]["grants"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        let v = ok_body(&fleet_grant(
            &store,
            br#"{"petname":"desk","grant":"http:nosh"}"#,
        ));
        assert_eq!(v["nodes"][0]["grants"][0], "http:nosh");

        // A second grant of the same rule is not a duplicate, and a different one is added.
        let _ = fleet_grant(&store, br#"{"petname":"desk","grant":"http:nosh"}"#);
        let v = ok_body(&fleet_grant(
            &store,
            br#"{"petname":"desk","grant":"tcp:127.0.0.1:22"}"#,
        ));
        assert_eq!(
            v["nodes"][0]["grants"]
                .as_array()
                .unwrap()
                .iter()
                .map(|g| g.as_str().unwrap())
                .collect::<Vec<_>>(),
            ["http:nosh", "tcp:127.0.0.1:22"]
        );

        let v = ok_body(&fleet_revoke(
            &store,
            br#"{"petname":"desk","grant":"http:nosh"}"#,
        ));
        assert_eq!(v["nodes"][0]["grants"].as_array().unwrap().len(), 1);
        // Revoking what is not held is idempotent, not an error.
        let v = ok_body(&fleet_revoke(
            &store,
            br#"{"petname":"desk","grant":"http:nosh"}"#,
        ));
        assert_eq!(v["nodes"][0]["grants"][0], "tcp:127.0.0.1:22");

        // And the file, read afresh, says exactly that.
        assert_eq!(
            ok_body(&fleet(&store))["nodes"][0]["grants"][0],
            "tcp:127.0.0.1:22"
        );
    }

    /// Setting instructions is what freezes into a fresh conversation that node opens
    /// (ADI-MONO-13); this only checks what the registry now holds and what the next read answers.
    #[test]
    fn setting_and_clearing_instructions_changes_what_the_next_read_returns() {
        let store = temp_store();
        pair(&store, "desk", "desk", &[], false);

        // A fresh node adds nothing by default.
        assert!(ok_body(&fleet(&store))["nodes"][0]["agent_instructions"].is_null());

        let v = ok_body(&fleet_instructions(
            &store,
            br#"{"petname":"desk","instructions":"Always run the tests first."}"#,
        ));
        assert_eq!(
            v["nodes"][0]["agent_instructions"],
            "Always run the tests first."
        );

        // Surrounding whitespace is trimmed, the same as every other free-text field here.
        let v = ok_body(&fleet_instructions(
            &store,
            br#"{"petname":"desk","instructions":"  Be terse.  "}"#,
        ));
        assert_eq!(v["nodes"][0]["agent_instructions"], "Be terse.");

        // An empty (or all-whitespace) body clears it, rather than leaving the last value stuck.
        let v = ok_body(&fleet_instructions(
            &store,
            br#"{"petname":"desk","instructions":"   "}"#,
        ));
        assert!(v["nodes"][0]["agent_instructions"].is_null());

        // And the file, read afresh, agrees.
        assert!(ok_body(&fleet(&store))["nodes"][0]["agent_instructions"].is_null());

        // An unusable body or an unknown node is refused, and neither writes anything.
        assert_eq!(
            fleet_instructions(&store, br#"{"petname":"  ","instructions":"x"}"#).status,
            400
        );
        assert_eq!(
            fleet_instructions(&store, br#"{"petname":"ghost","instructions":"x"}"#).status,
            404
        );
        assert_eq!(fleet_instructions(&store, b"not json").status, 400);
    }

    #[test]
    fn a_grant_that_does_not_parse_is_a_400() {
        let store = temp_store();
        pair(&store, "desk", "desk", &[], false);

        for raw in [
            r#"{"petname":"desk","grant":"ftp:*"}"#,
            r#"{"petname":"desk","grant":"http"}"#,
            r#"{"petname":"desk","grant":"http:NOSH"}"#,
            r#"{"petname":"desk","grant":"tcp:127.0.0.1"}"#,
            r#"{"petname":"desk","grant":""}"#,
        ] {
            let response = fleet_grant(&store, raw.as_bytes());
            assert_eq!(response.status, 400, "{raw} → {}", response.body);
            assert!(response.body.contains("invalid grant"), "{}", response.body);
        }
        // A missing node is still a 404 — the grant is parsed first, so the message says which
        // of the two the caller got wrong.
        assert_eq!(
            fleet_grant(&store, br#"{"petname":"ghost","grant":"http:*"}"#).status,
            404
        );
        assert_eq!(fleet_grant(&store, b"{}").status, 400);
        // Nothing landed on the node through any of that.
        assert!(
            ok_body(&fleet(&store))["nodes"][0]["grants"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_declared_nickname_surfaces_and_accepting_it_moves_the_petname() {
        let store = temp_store();
        pair(&store, "laptop-b", "laptop-b", &["http:*"], false);
        declare(&store, "laptop-b", "desk");

        // It is a notification, not a re-point: the petname is untouched until acted on.
        let v = ok_body(&fleet(&store));
        assert_eq!(v["nodes"][0]["petname"], "laptop-b");
        assert_eq!(v["nodes"][0]["nickname"], "laptop-b");
        assert_eq!(v["nodes"][0]["pending_nickname"], "desk");

        let v = ok_body(&fleet_accept_nickname(&store, br#"{"petname":"laptop-b"}"#));
        let node = &v["nodes"][0];
        assert_eq!(node["petname"], "desk", "the petname moved to the new name");
        assert_eq!(node["nickname"], "desk");
        assert!(node["pending_nickname"].is_null());
        assert_eq!(node["grants"][0], "http:*", "the node kept what it held");
        assert_eq!(ok_body(&fleet(&store))["nodes"][0]["petname"], "desk");

        // Nothing pending any more, and an unknown node is still a 404.
        assert_eq!(
            fleet_accept_nickname(&store, br#"{"petname":"desk"}"#).status,
            409
        );
        assert_eq!(
            fleet_accept_nickname(&store, br#"{"petname":"ghost"}"#).status,
            404
        );
    }

    #[test]
    fn a_declared_nickname_can_never_take_over_another_nodes_petname() {
        let store = temp_store();
        pair(&store, "main", "main", &[], false);
        pair(&store, "laptop-b", "laptop-b", &[], false);
        // The attacker re-introduces itself as `main`.
        declare(&store, "laptop-b", "main");

        let response = fleet_accept_nickname(&store, br#"{"petname":"laptop-b"}"#);
        assert_eq!(response.status, 409, "{}", response.body);

        // `main` still resolves to the node it always did.
        let v = ok_body(&fleet(&store));
        let main = v["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["petname"] == "main")
            .expect("the honest node");
        assert_eq!(main["key"], key_of('m'));
    }

    #[test]
    fn dismissing_a_declared_nickname_keeps_the_petname() {
        let store = temp_store();
        pair(&store, "laptop-b", "laptop-b", &[], false);
        declare(&store, "laptop-b", "desk");

        let v = ok_body(&fleet_dismiss_nickname(
            &store,
            br#"{"petname":"laptop-b"}"#,
        ));
        let node = &v["nodes"][0];
        assert_eq!(node["petname"], "laptop-b", "we keep calling it what we do");
        assert_eq!(
            node["nickname"], "desk",
            "while acknowledging what it now calls itself"
        );
        assert!(node["pending_nickname"].is_null(), "and stop being told");

        // Nothing left to dismiss.
        assert_eq!(
            fleet_dismiss_nickname(&store, br#"{"petname":"laptop-b"}"#).status,
            409
        );
        assert_eq!(
            fleet_dismiss_nickname(&store, br#"{"petname":"ghost"}"#).status,
            404
        );
    }

    #[test]
    fn an_empty_registry_is_an_empty_list_not_an_error() {
        let store = temp_store();
        let v = ok_body(&fleet(&store));
        assert!(v["nodes"].as_array().unwrap().is_empty());
    }

    /// `active` and `known` are different questions: every paired node is known, but only one
    /// seen inside the window is active. The join is by nickname (what a node calls itself), the
    /// same identity `record_seen` is called with — not petname (what this machine calls it).
    #[test]
    fn presence_reports_active_within_the_window_and_merely_known_past_it() {
        let store = temp_store();
        pair(&store, "laptop-b", "laptop-b", &[], false);
        pair(&store, "desk", "desk", &[], false);

        let db = Db::with_config(store.clone());
        let now = adi_config::now_unix();
        activity::try_record_seen(&db, "laptop-b", now - 10).expect("recent sighting");
        activity::try_record_seen(&db, "desk", now - 3600).expect("stale sighting");

        let v = ok_body(&fleet(&store));
        let nodes = v["nodes"].as_array().unwrap();
        let laptop = nodes.iter().find(|n| n["petname"] == "laptop-b").unwrap();
        let desk = nodes.iter().find(|n| n["petname"] == "desk").unwrap();

        assert_eq!(laptop["active"], true, "seen 10s ago, well inside the window");
        assert_eq!(laptop["last_seen"], now - 10);

        assert_eq!(desk["active"], false, "seen an hour ago: known, not active");
        assert_eq!(desk["last_seen"], now - 3600);
    }

    /// An `adimesh:` ticket of the shape [`ticket::published`] hands back — the endpoint an invite
    /// carries, and the reason a minted token is 953 characters rather than 200.
    fn sample_endpoint() -> String {
        format!("adimesh:{}", key_of('e'))
    }

    /// A real ed25519 public key, because an `EndpointId` is one and [`key_of`]'s filler is not:
    /// parsing checks the bytes are a point on the curve. Fixed rather than generated, so this
    /// crate's tests need no key material of their own.
    const VIEWER_KEY: &str = "0ccefa176ef3183396e0f13bff258ccefa0d045a0be2f9b412b3c0ee9ea80f78";

    #[test]
    fn minting_answers_a_spendable_token_with_its_expiry_and_its_qr() {
        let store = temp_store();
        let now = 1_700_000_000;

        let invite = mint(&store, Some(sample_endpoint()), now, InviteSpec::default())
            .expect("the mesh is running");

        // Ten minutes, and the expiry is the token's own — not a number computed twice.
        assert_eq!(u64::from(invite.ttl_secs), INVITE_TTL.as_secs());
        assert_eq!(invite.expires, now + INVITE_TTL.as_secs());
        let decoded = adi_mesh::join::decode_invite(&invite.token, now).expect("a live invite");
        assert_eq!(decoded.expires, invite.expires);
        assert_eq!(decoded.endpoint, sample_endpoint());

        // The nonce was recorded, so exactly one node can spend it. Without this the token is a
        // string the minter has never heard of and every join is refused.
        let book = adi_mesh::join::InviteBook::load_from(&store).expect("the book");
        assert!(
            book.invites.iter().any(|i| i.nonce == decoded.nonce),
            "the minted nonce is not in the book: {:?}",
            book.invites
        );

        // The QR is the token, not a description of it.
        assert!(invite.svg.starts_with("<svg "), "{}", &invite.svg[..40]);
        assert!(
            !invite.svg.contains(&invite.token),
            "an SVG is not a text field"
        );

        // A second call is a second invite: minting is never idempotent, or two nodes would race
        // for one nonce and the second would be told it was replayed.
        let again = mint(&store, Some(sample_endpoint()), now, InviteSpec::default())
            .expect("mint again");
        assert_ne!(again.token, invite.token);
    }

    #[test]
    fn minting_without_a_running_mesh_is_refused_rather_than_answered_with_a_dead_token() {
        let store = temp_store();
        let response = mint(&store, None, 1_700_000_000, InviteSpec::default())
            .expect_err("nothing to dial");
        assert_eq!(response.status, 409, "{}", response.body);
        assert!(
            response.body.contains("mesh is not running"),
            "{}",
            response.body
        );
        // And nothing was written: a refusal must not leave a nonce behind.
        let book = adi_mesh::join::InviteBook::load_from(&store).expect("the book");
        assert!(book.invites.is_empty());
    }

    /// A token good enough to spend is handed back untouched — including the whitespace a paste
    /// out of a terminal or a chat window brings with it, which is the ordinary case here.
    #[test]
    fn a_live_invite_is_accepted_for_spending_however_it_was_pasted() {
        let store = temp_store();
        let now = 1_700_000_000;
        let token = mint(&store, Some(sample_endpoint()), now, InviteSpec::default())
            .expect("mint")
            .token;

        let body = serde_json::to_vec(&FleetJoinRef {
            token: format!("  {token}\n"),
            grant_back: true,
        })
        .expect("a body");
        assert_eq!(
            fleet_join_token(&body, now).expect("live"),
            (token, true)
        );
    }

    /// Everything refusable about a token is refused *before* the dial, so the operator reads why
    /// instead of waiting out a handshake timeout and guessing.
    #[test]
    fn an_unspendable_invite_is_a_400_before_anything_is_dialled() {
        let store = temp_store();
        let now = 1_700_000_000;
        let token = mint(&store, Some(sample_endpoint()), now, InviteSpec::default())
            .expect("mint")
            .token;

        // Expired: live at `now`, dead ten minutes and a second later.
        let body = serde_json::to_vec(&FleetJoinRef {
            token: token.clone(),
            grant_back: true,
        })
        .expect("a body");
        let late = fleet_join_token(&body, now + INVITE_TTL.as_secs() + 1).expect_err("expired");
        assert_eq!(late.status, 400, "{}", late.body);
        assert!(late.body.contains("cannot be spent"), "{}", late.body);

        for raw in [
            br#"{"token":"not-an-invite"}"#.as_slice(),
            br#"{"token":"   "}"#.as_slice(),
            b"{}".as_slice(),
            b"not json".as_slice(),
        ] {
            let response = fleet_join_token(raw, now).expect_err("unusable");
            assert_eq!(
                response.status,
                400,
                "{} → {}",
                String::from_utf8_lossy(raw),
                response.body
            );
        }
    }

    // -- the invite spec, over the handler (ADI-MONO-171 G1) ------------------------------

    /// An empty body parses to the all-default spec — the no-body call's whole contract. `null`
    /// is in the same bucket: `adi-webapp`'s own `fetch::fleet_invite` sends `()` as its JSON
    /// body, which serialises to literal `null`, not to an empty string.
    #[test]
    fn an_empty_blank_or_null_body_is_the_default_invite_spec() {
        for body in [b"".as_slice(), b"   \n".as_slice(), b"null".as_slice()] {
            assert_eq!(invite_spec(body).expect("default"), InviteSpec::default());
        }
    }

    #[test]
    fn a_full_invite_body_becomes_the_matching_spec() {
        let body = br#"{
            "petname": "p42-device",
            "username": "p42",
            "role": "member",
            "grants": ["http:app", "http:hub"],
            "labels": {"principal": "p42"}
        }"#;
        let spec = invite_spec(body).expect("a valid spec");
        assert_eq!(spec.petname.as_deref(), Some("p42-device"));
        assert_eq!(spec.username.as_deref(), Some("p42"));
        assert_eq!(spec.role, Some(Role::Member));
        assert_eq!(
            spec.grants,
            vec!["http:app".parse().unwrap(), "http:hub".parse().unwrap()]
        );
        assert_eq!(spec.labels.get("principal").map(String::as_str), Some("p42"));
    }

    #[test]
    fn an_invalid_grant_or_role_in_the_body_is_a_400() {
        assert_eq!(
            invite_spec(br#"{"grants":["ftp:*"]}"#).expect_err("bad grant").status,
            400
        );
        assert_eq!(
            invite_spec(br#"{"role":"owner"}"#).expect_err("bad role").status,
            400
        );
        assert_eq!(invite_spec(b"not json").expect_err("bad json").status, 400);
    }

    /// `POST /api/fleet/invite` with a body mints exactly the spec it was given, recorded in the
    /// book so `decide()` can apply it once the node spends it (the applying half is pinned in
    /// `adi_mesh::join`'s own tests; this is the handler's half — the body becomes the book entry).
    #[test]
    fn fleet_invite_with_a_body_records_the_spec_in_the_book() {
        let store = temp_store();
        let now = 1_700_000_000;
        let response = fleet_invite(
            &store,
            br#"{"petname":"p42-device","username":"p42","role":"member","grants":["http:hub"]}"#,
        );
        let invite: FleetInvite = serde_json::from_str(&response.body).expect("a fleet invite");
        let decoded = adi_mesh::join::decode_invite(&invite.token, now).expect("live");

        let book = adi_mesh::join::InviteBook::load_from(&store).expect("the book");
        let pending = book
            .invites
            .iter()
            .find(|i| i.nonce == decoded.nonce)
            .expect("recorded");
        assert_eq!(pending.spec.petname.as_deref(), Some("p42-device"));
        assert_eq!(pending.spec.username.as_deref(), Some("p42"));
        assert_eq!(pending.spec.role, Some(Role::Member));
        assert_eq!(pending.spec.grants, vec!["http:hub".parse().unwrap()]);
    }

    /// A no-body call still mints the empty spec — nothing about the book changed by this feature
    /// existing.
    #[test]
    fn fleet_invite_with_no_body_mints_the_default_spec() {
        let store = temp_store();
        let response = fleet_invite(&store, b"");
        let invite: FleetInvite = serde_json::from_str(&response.body).expect("a fleet invite");
        let decoded =
            adi_mesh::join::decode_invite(&invite.token, 1_700_000_000).expect("live");
        let book = adi_mesh::join::InviteBook::load_from(&store).expect("the book");
        let pending = book
            .invites
            .iter()
            .find(|i| i.nonce == decoded.nonce)
            .expect("recorded");
        assert_eq!(pending.spec, InviteSpec::default());
    }

    #[test]
    fn invites_lists_only_whats_open_and_cancel_closes_one() {
        let store = temp_store();
        let first = fleet_invite(&store, br#"{"petname":"p1-device"}"#);
        let first: FleetInvite = serde_json::from_str(&first.body).expect("invite");
        let _second = fleet_invite(&store, br#"{"petname":"p2-device"}"#);

        let listed: FleetInvites =
            serde_json::from_str(&fleet_invites(&store).body).expect("invites");
        assert_eq!(listed.invites.len(), 2);
        assert!(
            listed
                .invites
                .iter()
                .any(|i| i.petname.as_deref() == Some("p1-device"))
        );

        let nonce = adi_mesh::join::decode_invite(&first.token, 1_700_000_000)
            .expect("live")
            .nonce;
        let after_cancel: FleetInvites = serde_json::from_str(
            &fleet_invite_cancel(&store, format!(r#"{{"nonce":"{nonce}"}}"#).as_bytes()).body,
        )
        .expect("invites");
        assert_eq!(after_cancel.invites.len(), 1, "the cancelled one is gone");

        // Cancelling again, or an unknown nonce, is a 404 — nothing to cancel.
        assert_eq!(
            fleet_invite_cancel(&store, format!(r#"{{"nonce":"{nonce}"}}"#).as_bytes()).status,
            404
        );
        assert_eq!(
            fleet_invite_cancel(&store, br#"{"nonce":"deadbeef"}"#).status,
            404
        );
        assert_eq!(fleet_invite_cancel(&store, br#"{"nonce":"  "}"#).status, 400);

        // And the cancelled nonce can no longer be spent.
        let mut registry = FleetRegistry::default();
        let mut book = adi_mesh::join::InviteBook::load_from(&store).expect("book");
        let reply = adi_mesh::join::decide(
            &mut registry,
            &mut book,
            &"0ccefa176ef3183396e0f13bff258ccefa0d045a0be2f9b412b3c0ee9ea80f78"
                .parse()
                .expect("a key"),
            &adi_mesh::join::JoinRequest {
                v: 1,
                nonce,
                nickname: "node".to_string(),
            },
            "pw",
            1_700_000_000,
        );
        assert!(
            matches!(reply, adi_mesh::join::JoinReply::Refused { .. }),
            "{reply:?}"
        );
    }

    /// The answer carries both names and both books: what the far side calls this machine, what
    /// this machine calls it back, the credential that exists nowhere else, and the registry as it
    /// stands — so the page needs no second request to show what it just did.
    #[test]
    fn a_join_answers_with_the_credential_and_the_fresh_registry() {
        let store = temp_store();
        pair(&store, "studio", "studio", &["http:app"], true);
        let joined = adi_mesh::join::Joined {
            petname: "igors-macbook-pro".to_string(),
            viewer: "studio".to_string(),
            viewer_key: VIEWER_KEY.parse().expect("a real ed25519 public key"),
            username: "adi".to_string(),
            password: "correct-horse".to_string(),
            grants: adi_mesh::join::default_grants(),
        };

        let v = ok_body(&fleet_joined(&store, &joined));

        // Two names, never conflated: `petname` resolves only over there, `viewer` only here.
        assert_eq!(v["petname"], "igors-macbook-pro");
        assert_eq!(v["viewer"], "studio");
        assert_eq!(v["viewer_key"], VIEWER_KEY);
        assert_eq!(v["username"], "adi");
        assert_eq!(v["password"], "correct-horse");
        assert_eq!(v["grants"][0], "http:app");
        assert_eq!(v["fleet"]["nodes"][0]["petname"], "studio");

        // And the link the credential was minted to open is the local name, not the remote one.
        let answer: FleetJoined = serde_json::from_str(&fleet_joined(&store, &joined).body)
            .expect("the answer deserializes");
        assert_eq!(answer.app_host(), "app.studio.n.adi");
    }

    /// The two renderings the page needs, kept beside the wire they are derived from.
    #[test]
    fn a_node_renders_a_short_key_and_its_control_panel_host() {
        let store = temp_store();
        pair(&store, "laptop-b", "laptop-b", &[], false);
        let state: FleetState =
            serde_json::from_str(&fleet(&store).body).expect("the state deserializes");
        let node = &state.nodes[0];

        assert_eq!(node.app_host(), "app.laptop-b.n.adi");
        let short = node.key_short();
        assert!(
            short.starts_with("llllllll") && short.ends_with("tail"),
            "{short}"
        );
        assert!(short.len() < node.key.len());

        // A key short enough to read whole is left alone.
        let stubby = FleetNode {
            key: "abc123".to_string(),
            ..node.clone()
        };
        assert_eq!(stubby.key_short(), "abc123");
    }
}
