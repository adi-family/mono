//! `adi-app`'s in-memory lease table and the `/api/leases/*` routes
//! (`docs/byo-credentials.md` §7, §9, ADI-MONO-180 P2.3).
//!
//! Leases live here, not in `adi-secrets` or `adi-agents`: "`adi-app` starts the runs" (§9), and
//! nothing else on the box can read one except through [`adi_agents::LeaseValues`], which
//! [`LeaseTable`] implements directly so `main.rs` can hand `Agents::with_lease_values` an
//! `Arc<LeaseTable>` cast to the trait object — no new indirection for phase 1's layer
//! (`adi_agents::lease`) to call through.
//!
//! **Never on disk, never past this process's lifetime.** A value lives in
//! [`Lease::values`] as a [`Zeroizing<String>`] and is dropped — zeroed — the moment its lease is
//! replaced or revoked, or this process exits. A restart drops every lease outright: there is no
//! load path here, on purpose.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use adi_config::Config;
use adi_events::{Event, Events};
use adi_mesh::connections::LeaseLength;
use adi_mesh::lease_seal::SessionKey;
use adi_secrets::UserSecretsPolicy;
use adi_webapp_api::handlers::{self, Response};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// `adi.secrets.lease.granted` — a device's unlock installed a live lease (`docs/byo-credentials.md`
/// §10, ADI-MONO-181 P2.8). Nothing in this process subscribes to it: a session parked on this
/// user's lease resumes on the next `advance_queue` poll regardless, the same lazy clock every
/// queued message already waits on — this is purely for the rail and anyone else watching.
#[derive(Debug, Serialize)]
struct LeaseGranted<'a> {
    user: &'a str,
    names: &'a [String],
}

impl Event for LeaseGranted<'_> {
    const NAME: &'static str = "adi.secrets.lease.granted";
}

/// `adi.secrets.lease.revoked` — `mesh lock` or unpairing a user's last peer
/// (`docs/byo-credentials.md` §10.1, ADI-MONO-181 P2.6).
#[derive(Debug, Serialize)]
struct LeaseRevoked<'a> {
    user: &'a str,
    stopped: &'a [String],
}

impl Event for LeaseRevoked<'_> {
    const NAME: &'static str = "adi.secrets.lease.revoked";
}

/// `lease = "0"`'s grace after the last renew — generous enough that a renew loop's own interval
/// (seconds, not minutes) never races it, short enough that a device that genuinely went away is
/// not mistaken for one still connected. §15 Q10 ("the renew interval `H` ... for `lease = 0`"),
/// settled here: `H` is left to the renewing side (ADI-MONO-180 P2.4's worker renews well inside
/// this), and this is the one number the server itself commits to.
const CONNECTED_GRACE: Duration = Duration::from_secs(90);

/// One user's live lease: the values a device unlocked, and how long they outlive it.
struct Lease {
    /// The peer that unlocked it — for [`list`]'s display and a future unpair hook (§10.1,
    /// ADI-MONO-181).
    petname: String,
    /// `NAME -> value`, `NAME` ∈ this unlock's `shared ∩ overridable` (checked at unlock time;
    /// re-checked against the *current* policy on every read through [`LeaseTable::value`] via
    /// `adi_agents::lease::apply`, which is where the rest of §5's narrowing already happens).
    values: BTreeMap<String, Zeroizing<String>>,
    length: LeaseLength,
    last_seen: Instant,
}

impl Lease {
    fn is_live(&self) -> bool {
        match ttl(self.length) {
            None => true,
            Some(ttl) => self.last_seen.elapsed() < ttl,
        }
    }
}

/// How long a lease outlives its last renew, or `None` for "forever" (§4.3).
fn ttl(length: LeaseLength) -> Option<Duration> {
    match length {
        LeaseLength::Connected => Some(CONNECTED_GRACE),
        LeaseLength::Hours(n) => Some(Duration::from_secs(n * 3600)),
        LeaseLength::Forever => None,
    }
}

/// `adi-app`'s whole lease table: one [`Lease`] per user id (`docs/byo-credentials.md` §9 — "a
/// user is one peer, so a second unlock from the same peer replaces the lease in place").
#[derive(Default)]
pub struct LeaseTable(Mutex<HashMap<String, Lease>>);

impl std::fmt::Debug for LeaseTable {
    /// Never the values — this is the one place in the process that holds them in the clear.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.0.lock().map(|t| t.len()).unwrap_or_default();
        f.debug_struct("LeaseTable").field("leases", &count).finish()
    }
}

impl LeaseTable {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Lease>> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Install (or replace in place) `user`'s lease.
    fn unlock(&self, user: &str, petname: &str, values: BTreeMap<String, Zeroizing<String>>, length: LeaseLength) {
        self.lock().insert(
            user.to_string(),
            Lease {
                petname: petname.to_string(),
                values,
                length,
                last_seen: Instant::now(),
            },
        );
    }

    /// Move `user`'s last-seen clock forward, if their lease is still live. A renew against an
    /// already-expired (but not yet evicted) lease does **not** revive it — `lease = 0`'s grace
    /// means "gone, re-unlock", not "silently extended by whoever happens to ask next" — so the
    /// stale entry is dropped here and this answers `false`, same as no lease at all.
    fn renew(&self, user: &str) -> bool {
        let mut table = self.lock();
        match table.get_mut(user) {
            Some(lease) if lease.is_live() => {
                lease.last_seen = Instant::now();
                true
            }
            Some(_) => {
                table.remove(user);
                false
            }
            None => false,
        }
    }

    /// Drop `user`'s lease (`mesh lock`, §10.1's first step) — zeroing its values on drop.
    /// Returns whether one was there. Runs even with nothing in the table, which is itself the
    /// documented behaviour (§10.1: "it runs in full even when no lease is in the table").
    fn revoke(&self, user: &str) -> bool {
        self.lock().remove(user).is_some()
    }

    /// Every *live* lease, for `GET /api/leases` (§7) — never a value.
    fn list(&self) -> Vec<LeaseSummary> {
        self.lock()
            .iter()
            .filter(|(_, lease)| lease.is_live())
            .map(|(user, lease)| LeaseSummary {
                user: user.clone(),
                petname: lease.petname.clone(),
                names: lease.values.keys().cloned().collect(),
                lease: lease.length.to_string(),
                last_seen_secs_ago: lease.last_seen.elapsed().as_secs(),
            })
            .collect()
    }

    /// One user's own live lease, if they have one — the member-facing half of `GET /api/leases`.
    fn mine(&self, user: &str) -> Vec<LeaseSummary> {
        self.list().into_iter().filter(|s| s.user == user).collect()
    }
}

impl adi_agents::LeaseValues for LeaseTable {
    /// `user`'s live value for `name`, or `None` when their lease isn't live right now or holds
    /// nothing under that name — [`adi_agents::lease::apply`]'s own narrowing (attachment,
    /// `overridable`, `lease.names`) runs on top of this; this is purely "is there a value, right
    /// now, for exactly this pair".
    fn value(&self, user: &str, name: &str) -> Option<String> {
        let table = self.lock();
        let lease = table.get(user)?;
        if !lease.is_live() {
            return None;
        }
        lease.values.get(name).map(|v: &Zeroizing<String>| String::clone(v))
    }
}

#[derive(Debug, Serialize)]
struct LeaseSummary {
    user: String,
    petname: String,
    names: Vec<String>,
    lease: String,
    last_seen_secs_ago: u64,
}

/// `GET /api/leases/session` (§9 step 1): this boot's session public key and epoch, plus the
/// id the caller's own lease (if they unlock one) will be filed under — so a device building its
/// unlock's AAD (`docs/byo-credentials.md` §9 step 3) never has to already know its own minted
/// id out of band; the server, which just resolved it from the caller's own authenticated
/// identity, hands it over right here.
#[derive(Serialize)]
struct SessionInfo {
    user: String,
    public_key: String,
    epoch: String,
}

pub fn session(session: &SessionKey, user: Option<&str>) -> Response {
    let Some(user) = user else {
        return handlers::error(403, "a local caller, or a peer with no user scope, has no lease to open");
    };
    handlers::ok_json(&SessionInfo {
        user: user.to_string(),
        public_key: base64::engine::general_purpose::STANDARD.encode(session.public()),
        epoch: session.epoch().to_string(),
    })
}

#[derive(Deserialize)]
struct SealedEntry {
    name: String,
    device_public: String,
    nonce: String,
    ciphertext: String,
}

#[derive(Deserialize)]
struct UnlockRequest {
    epoch: String,
    #[serde(default)]
    lease: String,
    #[serde(default)]
    sealed: Vec<SealedEntry>,
}

/// `POST /api/leases/unlock` (§9 step 4). `device_key` is the caller's `EndpointId`, from the
/// gateway's own pairing record — part of every sealed value's AAD (`lease_seal`'s own doc),
/// never taken from the request body. `events` is only for `adi.secrets.lease.granted`
/// (ADI-MONO-181 P2.8) — nothing here depends on it being delivered; see [`LeaseGranted`].
#[allow(clippy::too_many_arguments)]
pub fn unlock(
    table: &LeaseTable,
    session: &SessionKey,
    config: &Config,
    events: &Events,
    user: Option<&str>,
    petname: Option<&str>,
    device_key: Option<&str>,
    body: &[u8],
) -> Response {
    let (Some(user), Some(petname), Some(device_key)) = (user, petname, device_key) else {
        return handlers::error(403, "no user scope to unlock a lease for");
    };
    let policy = UserSecretsPolicy::load(config);
    if !policy.enabled {
        return handlers::error(400, "the user-secrets feature is off on this server");
    }
    let req: UnlockRequest = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => return handlers::error(400, &format!("invalid request body: {e}")),
    };
    if !session.is_current_epoch(&req.epoch) {
        return handlers::error(409, "unknown epoch, re-unlock");
    }
    let lease_len: LeaseLength = match req.lease.parse() {
        Ok(len) => len,
        Err(e) => return handlers::error(400, &format!("invalid lease length: {e}")),
    };
    let max = policy
        .max_lease
        .as_deref()
        .and_then(|m| m.parse::<LeaseLength>().ok());
    let clamped = lease_len.clamp(max);

    let mut values = BTreeMap::new();
    for entry in req.sealed {
        if !policy.overridable.iter().any(|n| n == &entry.name) {
            continue; // §9 step 4: "names ⊆ overridable" — anything else is dropped, not refused.
        }
        let Ok(device_public) = base64_decode32(&entry.device_public) else {
            continue;
        };
        let aad = format!("user:{user}/{}|{}|{device_key}", entry.name, req.epoch);
        if let Ok(plaintext) = adi_mesh::lease_seal::open(session, &device_public, &aad, &entry.nonce, &entry.ciphertext)
            && let Ok(value) = String::from_utf8(plaintext.to_vec())
        {
            values.insert(entry.name, Zeroizing::new(value));
        }
    }
    if values.is_empty() {
        return handlers::error(400, "could not open any of the sealed values");
    }
    let names: Vec<String> = values.keys().cloned().collect();
    table.unlock(user, petname, values, clamped);
    let _ = events.emit_event(&LeaseGranted { user, names: &names });
    handlers::ok_json(&serde_json::json!({ "ok": true, "names": names, "lease": clamped.to_string() }))
}

fn base64_decode32(text: &str) -> anyhow::Result<[u8; 32]> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(text)?;
    bytes.try_into().map_err(|_| anyhow::anyhow!("expected 32 bytes"))
}

#[derive(Deserialize)]
struct RenewRequest {
    epoch: String,
}

/// `POST /api/leases/renew` (§9 step 5).
pub fn renew(table: &LeaseTable, session: &SessionKey, user: Option<&str>, body: &[u8]) -> Response {
    let Some(user) = user else {
        return handlers::error(403, "no user scope to renew");
    };
    let req: RenewRequest = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => return handlers::error(400, &format!("invalid request body: {e}")),
    };
    if !session.is_current_epoch(&req.epoch) {
        return handlers::error(409, "unknown epoch, re-unlock");
    }
    if table.renew(user) {
        handlers::ok_json(&serde_json::json!({ "ok": true }))
    } else {
        handlers::error(404, "no live lease — unlock again")
    }
}

/// The full §10.1 sequence, shared by `POST /api/leases/revoke` below and by unpairing a user's
/// last peer (`docs/byo-credentials.md` §3.2, ADI-MONO-181 P2.6's own "also on unpair"
/// requirement): drop the lease (step 1), then — through `Agents::stop_runs_for_lease_user`, which
/// voids every one of the user's `lease_cap`s and finds every session across every agent — stop
/// each one still alive (steps 2-4), and publish `adi.secrets.lease.revoked` (step 5). Runs in full
/// even when `table` holds nothing for `user`: `stop_runs_for_lease_user` only reads `sessions.db`,
/// so a restarted `adi-app` with an empty table still stops whatever was started before it came
/// back up. Returns whether a *live lease* was there to drop — the run ids actually stopped are
/// in the published event, not here.
pub fn revoke_user(table: &LeaseTable, agents: &adi_agents::Agents, events: &Events, user: &str) -> bool {
    let had_lease = table.revoke(user);
    let stopped = agents.stop_runs_for_lease_user(user);
    let _ = events.emit_event(&LeaseRevoked { user, stopped: &stopped });
    had_lease
}

/// `POST /api/leases/revoke` (`mesh lock`, §4.4, §10.1).
pub fn revoke(table: &LeaseTable, agents: &adi_agents::Agents, events: &Events, user: Option<&str>) -> Response {
    let Some(user) = user else {
        return handlers::error(403, "no user scope to revoke");
    };
    let revoked = revoke_user(table, agents, events, user);
    handlers::ok_json(&serde_json::json!({ "ok": true, "revoked": revoked }))
}

/// `GET /api/leases` (§7): every live lease for an admin, or just the caller's own for a member.
pub fn list(table: &LeaseTable, is_admin: bool, user: Option<&str>) -> Response {
    let leases = if is_admin {
        table.list()
    } else {
        match user {
            Some(user) => table.mine(user),
            None => Vec::new(),
        }
    };
    handlers::ok_json(&serde_json::json!({ "leases": leases }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-app-leases-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    fn enable(config: &Config, overridable: &[&str]) {
        UserSecretsPolicy {
            enabled: true,
            overridable: overridable.iter().map(|s| (*s).to_string()).collect(),
            ..UserSecretsPolicy::default()
        }
        .save(config)
        .expect("save policy");
    }

    fn events_for(config: &Config) -> Events {
        Events::with_config(config.clone())
    }

    fn agents_for(config: &Config) -> adi_agents::Agents {
        adi_agents::Agents::with_config(config.clone())
    }

    /// Seal one value exactly as a device would, including the AAD this server-side `unlock`
    /// reconstructs — the harness every test below shares.
    fn seal_one(
        session: &SessionKey,
        user: &str,
        device_key: &str,
        epoch: &str,
        name: &str,
        value: &str,
    ) -> serde_json::Value {
        let aad = format!("user:{user}/{name}|{epoch}|{device_key}");
        let sealed = adi_mesh::lease_seal::seal(&session.public(), &aad, value.as_bytes()).expect("seal");
        serde_json::json!({
            "name": name,
            "device_public": base64::engine::general_purpose::STANDARD.encode(sealed.device_public),
            "nonce": sealed.nonce,
            "ciphertext": sealed.ciphertext,
        })
    }

    #[test]
    fn session_requires_a_user_scope() {
        let session = SessionKey::generate();
        assert_eq!(self::session(&session, None).status, 403);
        assert_eq!(self::session(&session, Some("u-aaa")).status, 200);
    }

    #[test]
    fn unlock_opens_the_sealed_values_and_installs_a_live_lease() {
        let config = scratch("unlock-basic");
        let events = events_for(&config);
        enable(&config, &["CLAUDE_CODE_OAUTH_TOKEN"]);
        let table = LeaseTable::default();
        let session = SessionKey::generate();

        let body = serde_json::json!({
            "epoch": session.epoch(),
            "lease": "24h",
            "sealed": [seal_one(&session, "u-aaa", "dev-1", session.epoch(), "CLAUDE_CODE_OAUTH_TOKEN", "sk-mine")],
        });
        let resp = unlock(
            &table,
            &session,
            &config,
            &events,
            Some("u-aaa"),
            Some("laptop"),
            Some("dev-1"),
            body.to_string().as_bytes(),
        );
        assert_eq!(resp.status, 200, "{}", resp.body);
        assert_eq!(
            adi_agents::LeaseValues::value(&table, "u-aaa", "CLAUDE_CODE_OAUTH_TOKEN"),
            Some("sk-mine".to_string())
        );
    }

    #[test]
    fn unlock_drops_a_name_not_on_the_overridable_list() {
        let config = scratch("unlock-not-overridable");
        let events = events_for(&config);
        enable(&config, &["CLAUDE_CODE_OAUTH_TOKEN"]);
        let table = LeaseTable::default();
        let session = SessionKey::generate();

        let body = serde_json::json!({
            "epoch": session.epoch(),
            "lease": "0",
            "sealed": [seal_one(&session, "u-aaa", "dev-1", session.epoch(), "NOT_OVERRIDABLE", "v")],
        });
        let resp = unlock(&table, &session, &config, &events, Some("u-aaa"), Some("laptop"), Some("dev-1"), body.to_string().as_bytes());
        assert_eq!(resp.status, 400, "{}", resp.body);
    }

    #[test]
    fn unlock_refuses_while_the_feature_is_off() {
        let config = scratch("unlock-disabled");
        let events = events_for(&config);
        let table = LeaseTable::default();
        let session = SessionKey::generate();
        let resp = unlock(&table, &session, &config, &events, Some("u-aaa"), Some("laptop"), Some("dev-1"), b"{}");
        assert_eq!(resp.status, 400);
    }

    #[test]
    fn unlock_clamps_the_lease_to_the_servers_max() {
        let config = scratch("unlock-clamp");
        let events = events_for(&config);
        enable(&config, &["K"]);
        UserSecretsPolicy {
            enabled: true,
            overridable: vec!["K".to_string()],
            max_lease: Some("24h".to_string()),
            ..UserSecretsPolicy::default()
        }
        .save(&config)
        .expect("save");
        let table = LeaseTable::default();
        let session = SessionKey::generate();
        let body = serde_json::json!({
            "epoch": session.epoch(),
            "lease": "forever",
            "sealed": [seal_one(&session, "u-aaa", "dev-1", session.epoch(), "K", "v")],
        });
        let resp = unlock(&table, &session, &config, &events, Some("u-aaa"), Some("laptop"), Some("dev-1"), body.to_string().as_bytes());
        assert_eq!(resp.status, 200, "{}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["lease"], "24h");
    }

    #[test]
    fn unlock_refuses_a_stale_epoch() {
        let config = scratch("unlock-stale-epoch");
        let events = events_for(&config);
        enable(&config, &["K"]);
        let table = LeaseTable::default();
        let session = SessionKey::generate();
        let body = serde_json::json!({
            "epoch": "not-the-real-epoch",
            "lease": "0",
            "sealed": [seal_one(&session, "u-aaa", "dev-1", "not-the-real-epoch", "K", "v")],
        });
        let resp = unlock(&table, &session, &config, &events, Some("u-aaa"), Some("laptop"), Some("dev-1"), body.to_string().as_bytes());
        assert_eq!(resp.status, 409);
    }

    #[test]
    fn renew_moves_last_seen_and_refuses_an_unknown_epoch() {
        let config = scratch("renew");
        let events = events_for(&config);
        enable(&config, &["K"]);
        let table = LeaseTable::default();
        let session = SessionKey::generate();
        let body = serde_json::json!({
            "epoch": session.epoch(),
            "lease": "0",
            "sealed": [seal_one(&session, "u-aaa", "dev-1", session.epoch(), "K", "v")],
        });
        unlock(&table, &session, &config, &events, Some("u-aaa"), Some("laptop"), Some("dev-1"), body.to_string().as_bytes());

        let ok = renew(&table, &session, Some("u-aaa"), serde_json::json!({"epoch": session.epoch()}).to_string().as_bytes());
        assert_eq!(ok.status, 200, "{}", ok.body);

        let stale = renew(&table, &session, Some("u-aaa"), serde_json::json!({"epoch": "wrong"}).to_string().as_bytes());
        assert_eq!(stale.status, 409);

        let no_user = renew(&table, &session, None, b"{}");
        assert_eq!(no_user.status, 403);
    }

    #[test]
    fn renew_against_an_expired_lease_does_not_revive_it() {
        let table = LeaseTable::default();
        table.unlock(
            "u-aaa",
            "laptop",
            BTreeMap::from([("K".to_string(), Zeroizing::new("v".to_string()))]),
            LeaseLength::Connected,
        );
        // Force expiry without waiting out the real grace.
        table.lock().get_mut("u-aaa").unwrap().last_seen = Instant::now().checked_sub(Duration::from_secs(10_000)).unwrap();
        assert!(!table.renew("u-aaa"));
        assert!(table.lock().get("u-aaa").is_none(), "the stale entry is evicted");
    }

    #[test]
    fn revoke_drops_the_lease_and_runs_even_with_nothing_there() {
        let config = scratch("revoke-nothing");
        let events = events_for(&config);
        let agents = agents_for(&config);
        let table = LeaseTable::default();
        table.unlock(
            "u-aaa",
            "laptop",
            BTreeMap::from([("K".to_string(), Zeroizing::new("v".to_string()))]),
            LeaseLength::Forever,
        );
        let resp = revoke(&table, &agents, &events, Some("u-aaa"));
        assert_eq!(resp.status, 200);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["revoked"], true);
        assert_eq!(adi_agents::LeaseValues::value(&table, "u-aaa", "K"), None);

        // Running again with nothing in the table is not an error.
        let resp = revoke(&table, &agents, &events, Some("u-aaa"));
        assert_eq!(resp.status, 200);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["revoked"], false);
    }

    /// ADI-MONO-181 P2.6, wired for real: `revoke` doesn't just drop the table entry, it goes
    /// through `Agents::stop_runs_for_lease_user` — proven against a real spawned child, kept
    /// alive and then actually killed, in `adi_agents::tests::end_to_end_a_real_child_…` — and
    /// publishes `adi.secrets.lease.revoked` either way. This proves the wiring at this layer: a
    /// session on the user's lease that was never started (so `stop_runs_for_lease_user`
    /// correctly leaves it alone — a parked session stays parked, §10.1) still turns up in the
    /// published event's `stopped` list as *not* among them, and the whole thing runs with no
    /// lease in the table at all — the restart case §10.1 names explicitly.
    #[test]
    fn revoke_runs_the_full_sequence_even_with_nothing_in_the_table() {
        use adi_agents::store::SessionStore;
        use adi_agents::{Backend, LeaseRef};

        let config = scratch("revoke-live-session");
        let agents = agents_for(&config);
        let events = events_for(&config);
        let sessions = SessionStore::new(config.module("sessions").dir());
        let record = sessions
            .create("worker", Backend::HarnessAdi, "/tmp", "go")
            .expect("create");
        sessions
            .set_lease("worker", &record.id, &LeaseRef { user: "u-aaa".into(), names: vec!["K".into()] })
            .expect("set lease");

        // No lease in `LeaseTable` at all — the restart case §10.1 names explicitly.
        let table = LeaseTable::default();
        let resp = revoke(&table, &agents, &events, Some("u-aaa"));
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["revoked"], false, "nothing was in the table to begin with");

        let published = events.catch_up("test", 10).expect("catch up");
        let revoked = published
            .iter()
            .find(|e| e.record.name == LeaseRevoked::NAME)
            .expect("published even with nothing in the table");
        let payload: serde_json::Value = serde_json::from_str(&revoked.record.payload).unwrap();
        assert_eq!(payload["user"], "u-aaa");
        assert_eq!(
            payload["stopped"],
            serde_json::json!([]),
            "a session that never started is left alone, not reported as stopped"
        );
    }

    #[test]
    fn list_shows_everything_to_an_admin_and_only_mine_to_a_member() {
        let table = LeaseTable::default();
        table.unlock("u-aaa", "laptop-a", BTreeMap::from([("K".to_string(), Zeroizing::new("v".to_string()))]), LeaseLength::Forever);
        table.unlock("u-bbb", "laptop-b", BTreeMap::from([("K".to_string(), Zeroizing::new("v".to_string()))]), LeaseLength::Forever);

        let admin = list(&table, true, None);
        let v: serde_json::Value = serde_json::from_str(&admin.body).unwrap();
        assert_eq!(v["leases"].as_array().unwrap().len(), 2);

        let member = list(&table, false, Some("u-aaa"));
        let v: serde_json::Value = serde_json::from_str(&member.body).unwrap();
        let leases = v["leases"].as_array().unwrap();
        assert_eq!(leases.len(), 1);
        assert_eq!(leases[0]["user"], "u-aaa");

        let no_scope = list(&table, false, None);
        let v: serde_json::Value = serde_json::from_str(&no_scope.body).unwrap();
        assert!(v["leases"].as_array().unwrap().is_empty());
    }

    // -- end to end (ADI-MONO-180): a device key wraps a value, `set` uploads it, `unlock`
    // creates a lease, and the exact function `Agents::with_lease_values` wires a launched run's
    // environment through (`adi_agents::LeaseValues::value`) returns it — then `mesh lock` drops
    // it and the same call answers `None`, which is what sends the next launch back to the
    // server's own value (`docs/byo-credentials.md` §5, §10.1; already exhaustively proven at the
    // `adi_agents::lease::apply` layer by ADI-MONO-178's own tests — this test proves everything
    // *beneath* that layer is wired to it for real, not against a fake).
    #[test]
    fn end_to_end_wrap_set_unlock_reaches_a_launched_runs_lease_values_and_lock_drops_it() {
        use adi_secrets::Secrets;

        let config = scratch("e2e");
        let events = events_for(&config);
        let agents = agents_for(&config);
        let secrets = Secrets::with_config(config.clone());
        enable(&config, &["CLAUDE_CODE_OAUTH_TOKEN"]);
        let user = "u-e2e";
        let server_petname = "laptop-e2e"; // this server's name for the connecting device
        let device_label = "laptop-e2e-device"; // the device's own AAD label for this server

        // -- the device: wrap a value under its own key, exactly as `secrets set --user --node`
        // does (ADI-MONO-180 P2.1/P2.2).
        let wrap_key = adi_mesh::devicekey::load_or_create_in(&config).expect("device key");
        let dek = adi_secrets::wrap::generate_dek();
        let (value_nonce, ciphertext) =
            adi_secrets::wrap::seal_value(&dek, device_label, "CLAUDE_CODE_OAUTH_TOKEN", b"sk-mine")
                .expect("seal value");
        let (wrap_nonce, wrapped) =
            adi_secrets::wrap::wrap_dek(&wrap_key, device_label, "CLAUDE_CODE_OAUTH_TOKEN", &dek)
                .expect("wrap dek");

        // -- `set` uploads it: the real HTTP handler, which never decrypts anything (P2.1).
        let set_body = serde_json::json!({
            "name": "CLAUDE_CODE_OAUTH_TOKEN",
            "nonce": value_nonce,
            "ciphertext": ciphertext,
            "wraps": [{"kind": adi_mesh::devicekey::KIND, "nonce": wrap_nonce, "wrapped": wrapped}],
        });
        let set_resp = adi_webapp_api::handlers::set_my_secret(
            &secrets,
            set_body.to_string().as_bytes(),
            Some(user),
        );
        assert_eq!(set_resp.status, 200, "{}", set_resp.body);

        // -- `unlock`: the device fetches its own wraps, unwraps locally, reseals to the
        // server's session key, and uploads — the real handler on both ends (P2.3).
        let table = LeaseTable::default();
        let session = SessionKey::generate();
        let mine = adi_webapp_api::handlers::secrets_mine(&secrets, Some(user));
        let mine_json: serde_json::Value = serde_json::from_str(&mine.body).expect("mine json");
        let entry = &mine_json["names"][0];
        let (device_dek_nonce, device_wrap) = (
            entry["wraps"][0]["nonce"].as_str().expect("wrap nonce"),
            entry["wraps"][0]["wrapped"].as_str().expect("wrapped"),
        );
        let recovered_dek = adi_secrets::wrap::unwrap_dek(
            &wrap_key,
            device_label,
            "CLAUDE_CODE_OAUTH_TOKEN",
            device_dek_nonce,
            device_wrap,
        )
        .expect("unwrap dek");
        let plaintext = adi_secrets::wrap::open_value(
            &recovered_dek,
            device_label,
            "CLAUDE_CODE_OAUTH_TOKEN",
            entry["nonce"].as_str().expect("nonce"),
            entry["ciphertext"].as_str().expect("ciphertext"),
        )
        .expect("open value");
        assert_eq!(plaintext, b"sk-mine");

        let aad = format!("user:{user}/CLAUDE_CODE_OAUTH_TOKEN|{}|{device_label}", session.epoch());
        let sealed = adi_mesh::lease_seal::seal(&session.public(), &aad, &plaintext).expect("seal to session");
        let unlock_body = serde_json::json!({
            "epoch": session.epoch(),
            "lease": "24h",
            "sealed": [{
                "name": "CLAUDE_CODE_OAUTH_TOKEN",
                "device_public": base64::engine::general_purpose::STANDARD.encode(sealed.device_public),
                "nonce": sealed.nonce,
                "ciphertext": sealed.ciphertext,
            }],
        });
        let unlock_resp = unlock(
            &table,
            &session,
            &config,
            &events,
            Some(user),
            Some(server_petname),
            Some(device_label),
            unlock_body.to_string().as_bytes(),
        );
        assert_eq!(unlock_resp.status, 200, "{}", unlock_resp.body);

        // -- a launched run: `Agents::with_lease_values` hands the table to this exact trait
        // method, per `adi_agents::lease::apply`'s own implementation — this *is* what an agent
        // that attaches `CLAUDE_CODE_OAUTH_TOKEN` and holds this `LeaseRef` gets in its slot.
        assert_eq!(
            adi_agents::LeaseValues::value(&table, user, "CLAUDE_CODE_OAUTH_TOKEN"),
            Some("sk-mine".to_string()),
            "a launched run attaching this name gets the user's value"
        );

        // -- `mesh lock`: drop the lease and void it.
        let revoke_resp = revoke(&table, &agents, &events, Some(user));
        assert_eq!(revoke_resp.status, 200);

        // -- the next launch: no live value, so `lease::apply` leaves the server's own value in
        // its slot untouched (proven in `adi_agents::lease::tests`; this proves the source feeding
        // it is now empty).
        assert_eq!(
            adi_agents::LeaseValues::value(&table, user, "CLAUDE_CODE_OAUTH_TOKEN"),
            None,
            "the next launch gets the server value"
        );
    }

    #[test]
    fn value_is_none_once_a_connected_lease_outlives_its_grace() {
        let table = LeaseTable::default();
        table.unlock("u-aaa", "laptop", BTreeMap::from([("K".to_string(), Zeroizing::new("v".to_string()))]), LeaseLength::Connected);
        assert_eq!(adi_agents::LeaseValues::value(&table, "u-aaa", "K"), Some("v".to_string()));
        table.lock().get_mut("u-aaa").unwrap().last_seen = Instant::now().checked_sub(Duration::from_secs(10_000)).unwrap();
        assert_eq!(adi_agents::LeaseValues::value(&table, "u-aaa", "K"), None);
    }
}
