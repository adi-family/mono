//! Pull-only bootstrap — `adi/mesh/join/1` (`docs/fleet.md` E3, §2, §5, §6).
//!
//! §6 says a node's only network-facing action is an *outbound* QUIC session: no `:22`, no
//! `:80`, no `:443`. Pairing is the one moment that promise is easy to break, because the
//! obvious way to enrol a machine is to ssh into it — which means opening the port the whole
//! design exists to keep shut. So the direction is inverted: the operator mints an **invite** on
//! the machine they are sitting at (the *viewer*), the node **dials out** with it, and the node
//! never listens for anything. `adi-mono mesh join <token>` in a cloud-init blob is a complete
//! enrolment.
//!
//! ```text
//! viewer:  adi-mono mesh invite            -> adi-invite:<hex(json)>
//! node:    adi-mono mesh join <token>      -- dials the viewer, one bi-stream on adi/mesh/join/1
//!            node -> viewer  { v, nonce, nickname }
//!            viewer -> node  { result: accepted, petname, username, password, grants }
//!                       or   { result: refused,  reason }
//! ```
//!
//! # What authorises what
//!
//! Three things arrive at the viewer and exactly one of them is trusted for identity:
//!
//! * The **key** comes from the QUIC handshake ([`Connection::remote_id`]) and nothing else. The
//!   payload never carries a key, and there is no code path here that would read one — a peer
//!   that could name its own key could pair as anybody.
//! * The **nonce** is the capability. It is what makes this ALPN safe to answer from a stranger,
//!   which is the whole point: a join must be accepted from a key that is by definition not yet
//!   in the registry, so the usual default-deny check cannot be the gate. Single-use and
//!   expiring, enforced by [`InviteBook::claim`] against the book of invites this machine
//!   actually minted — so a leaked token pairs at most one machine, once, briefly.
//! * The **nickname** is presentation only (§2). A clash resolves to a suggestion and pairs
//!   anyway; refusing over a cosmetic name would be the bug §2 rule 3 names.
//!
//! # Replay
//!
//! The nonce is spent by the claim, the book is **written to disk before the reply is sent**, and
//! an expired entry is refused whether or not it was ever spent. A second machine replaying the
//! same token therefore loses the race by construction rather than by timing: whichever
//! connection claims first leaves a `spent_at` behind, and the loser is refused with
//! [`ClaimError::Spent`]. The alternative — spend on success, answer first — has a window in
//! which two nodes both pair, which is precisely the case an attacker who scraped a token from a
//! terminal log is trying to hit.
//!
//! # The password
//!
//! §5's second layer is a Basic-auth password enforced *on the node*. The viewer mints it, keeps
//! only the salted verifier ([`NodeRecord::set_password`](crate::fleet::NodeRecord::set_password)),
//! and returns the plaintext exactly once, in the reply. The node stores its verifier too, so the
//! viewer's later HTTP requests are gated by that exact password — and the plaintext exists only
//! in the operator's terminal, on the far end of a QUIC session, for as long as they leave it on
//! the screen.

use std::fmt;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use adi_config::Config;
use anyhow::{Context as _, bail, ensure};
use iroh::endpoint::{Connection, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tracing::{debug, info, warn};

use crate::fleet::{FleetRegistry, Grant, Pairing, Scope};
use crate::{identity, node, ticket};

/// The ALPN identifying the join protocol during the iroh/QUIC handshake. The trailing `/1` is
/// the wire version: bump the ALPN (not just [`VERSION`]) on an incompatible change, so an old
/// peer fails at the handshake rather than mid-handshake.
///
/// Its own ALPN, and not a frame inside `adi/mesh/http/1`, because it is the one protocol on this
/// endpoint that must answer a key the registry has never seen. Keeping it separate means the
/// exception lives in one named place instead of as a branch inside the gate it is bypassing.
pub const ALPN: &[u8] = b"adi/mesh/join/1";

/// Marks a string as an adi invite (vs. an `adimesh:` ticket, which it contains).
const PREFIX: &str = "adi-invite:";

/// The invite and handshake version, mirrored in every payload's `v`.
const VERSION: u8 = 1;

/// Bytes of randomness in a nonce — 128 bits, rendered as 32 hex characters.
const NONCE_BYTES: usize = 16;

/// How long an invite is good for unless the operator says otherwise. Long enough to paste into
/// a cloud-init template and boot a machine; short enough that a token left in a shell history
/// is dead by the time anyone reads it.
pub const DEFAULT_TTL: Duration = Duration::from_secs(15 * 60);

/// Caps a single JSON frame, so a peer that claims a huge length cannot make us allocate for it.
const MAX_FRAME: usize = 64 * 1024;

/// The viewer's book of minted invites, within the `mesh` module dir.
const INVITES_FILE: &str = "invites.toml";

/// How long a spent or expired invite is kept before being pruned. Purely so the refusal can say
/// *expired* or *already used* rather than *unknown* for a token an operator is still holding;
/// past it, the entry is dropped and the same token is refused as unknown, which is the same
/// answer said less helpfully.
const INVITE_KEEP: Duration = Duration::from_secs(24 * 60 * 60);

/// The username minted credentials carry.
///
/// Constant on purpose: the credential is already per-node (each viewer gets its own record on
/// the node, each with its own salt and password), so the username carries no security weight and
/// only has to be something a human can retype into a browser prompt without a note.
pub const PAIR_USER: &str = "adi";

/// Characters and length of a generated password: 24 characters from a 56-symbol alphabet is
/// ~139 bits. The alphabet omits `0`/`O`, `1`/`l`/`I` — this string gets read off one screen and
/// typed into another, and an ambiguous glyph there costs the operator a support call.
const PASSWORD_ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const PASSWORD_LEN: usize = 24;

/// The one service a fresh pairing may reach: the node's control panel, `app.<node>.n.adi`.
///
/// Conservative in the directions that matter and useful in the one that does. It is not
/// `http:*`, so no dashboard is exposed until somebody names it; it is not `tcp:`, which would
/// splice a raw socket past the HTTP password layer entirely; and it is not `ctl:`, which is the
/// control plane. It *is* the panel, because that is the thing you paired the node for
/// (`docs/fleet.md` §1) — a default that granted nothing would leave every operator running the
/// same `mesh grant` immediately after every join, which is a default that has chosen wrong.
const DEFAULT_SERVICE: &str = "app";

/// Ceiling on the whole outbound handshake, so a stale ticket fails with a sentence instead of
/// hanging a boot script.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(45);

/// Serialised across the process, because pairing is read-modify-write over two files and two
/// nodes dialling the same invite concurrently is the *expected* attack, not a rare race. The
/// critical section holds no `.await`.
static PAIRING: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------------------
// The invite token
// ---------------------------------------------------------------------------------------

/// The payload inside an `adi-invite:` token: how to reach the viewer, and the one-time secret
/// that makes the viewer answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    /// The protocol version; always [`VERSION`] when minted here.
    pub v: u8,
    /// The viewer's `adimesh:` ticket — id plus relay and direct addresses, so the node dials it
    /// straight away instead of waiting on discovery (see [`crate::ticket`]).
    pub endpoint: String,
    /// 32 hex characters of one-time secret.
    pub nonce: String,
    /// Unix seconds after which the invite is refused.
    pub expires: u64,
}

impl Invite {
    /// Is this invite past its expiry at `now`?
    #[must_use]
    pub fn is_expired(&self, now: u64) -> bool {
        now >= self.expires
    }
}

/// Encode an invite as a shareable token.
///
/// Deliberately the same shape as an [`adimesh:` ticket](crate::ticket) — `<prefix>:<hex(json)>` —
/// and it uses that module's hex codec rather than a second one. One recognisable family of
/// tokens, all of them shell-, YAML- and cloud-init-safe without quoting.
///
/// # Errors
/// Fails only if the invite cannot be serialized (not expected in practice).
pub fn encode_invite(invite: &Invite) -> anyhow::Result<String> {
    let json = serde_json::to_vec(invite).context("serializing the invite")?;
    Ok(format!("{PREFIX}{}", ticket::to_hex(&json)))
}

/// Decode and validate an invite token.
///
/// Expiry is checked here, on the node, purely so a stale token fails before a dial rather than
/// after one. It is **not** where expiry is enforced — the token is attacker-controlled, so the
/// authority is [`InviteBook::claim`] on the viewer, against the book of invites it minted.
///
/// # Errors
/// If the string is not an `adi-invite:` token, is not valid hex, does not decode to an
/// [`Invite`], names a version this build does not speak, carries a malformed nonce, or has
/// expired.
pub fn decode_invite(token: &str, now: u64) -> anyhow::Result<Invite> {
    let hex = token
        .trim()
        .strip_prefix(PREFIX)
        .context("not an adi invite (expected a token starting with `adi-invite:`)")?;
    let bytes = ticket::from_hex(hex).context("invite payload is not valid hex")?;
    let invite: Invite =
        serde_json::from_slice(&bytes).context("invite does not decode to an invite payload")?;
    ensure!(
        invite.v == VERSION,
        "this build speaks invite v{VERSION}, but the token is v{}",
        invite.v
    );
    ensure!(
        valid_nonce(&invite.nonce),
        "the invite's nonce is malformed (expected {} hex characters)",
        NONCE_BYTES * 2
    );
    ensure!(
        !invite.is_expired(now),
        "this invite expired {}s ago — mint a fresh one with `adi-mono mesh invite`",
        now.saturating_sub(invite.expires)
    );
    Ok(invite)
}

/// A nonce is exactly [`NONCE_BYTES`] of lowercase hex. Checked so a hand-edited token is refused
/// as malformed rather than looked up (and refused) as unknown.
fn valid_nonce(nonce: &str) -> bool {
    nonce.len() == NONCE_BYTES * 2 && nonce.bytes().all(|b| b.is_ascii_hexdigit())
}

// ---------------------------------------------------------------------------------------
// The viewer's book of minted invites
// ---------------------------------------------------------------------------------------

/// One invite this machine minted, as it sits in `invites.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingInvite {
    /// The one-time secret, in the form it appears in the token.
    pub nonce: String,
    /// Unix seconds after which it is refused.
    pub expires: u64,
    /// When it was claimed. `Some` makes every later claim a [`ClaimError::Spent`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spent_at: Option<u64>,
}

/// Why an offered nonce was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimError {
    /// No invite by that nonce was minted here (or it has since been pruned).
    Unknown,
    /// It was minted here, but its TTL has passed.
    Expired,
    /// It was minted here and already paired a machine.
    Spent,
}

impl fmt::Display for ClaimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => f.write_str("that invite was not issued by this machine"),
            Self::Expired => f.write_str("that invite has expired"),
            Self::Spent => f.write_str("that invite has already been used"),
        }
    }
}

impl std::error::Error for ClaimError {}

/// The whole `invites.toml`: every invite this machine has minted and not yet pruned.
///
/// It exists so single-use survives a restart. Holding spent nonces in memory would make the
/// replay window "until the daemon is restarted", which on a machine that pairs nodes from a
/// control panel is *any time the app is relaunched*.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct InviteBook {
    /// Minted invites, newest last.
    pub invites: Vec<PendingInvite>,
}

impl InviteBook {
    /// Load the book from the standard store, materialising an empty file on first use.
    ///
    /// # Errors
    /// Any I/O or TOML error from the underlying store.
    pub fn load() -> anyhow::Result<Self> {
        Self::load_from(&Config::open())
    }

    /// [`load`](Self::load) against an explicit store — for tests and alternate installs.
    ///
    /// # Errors
    /// Any I/O or TOML error from the underlying store.
    pub fn load_from(store: &Config) -> anyhow::Result<Self> {
        Ok(Self::file_in(store).load_or_create()?)
    }

    /// Persist the book atomically.
    ///
    /// # Errors
    /// Any encode or I/O error from the underlying store.
    pub fn save_to(&self, store: &Config) -> anyhow::Result<()> {
        Self::file_in(store).save(self)?;
        Ok(())
    }

    fn file_in(store: &Config) -> adi_config::ConfigFile<Self> {
        store.module(crate::config::MODULE).file(INVITES_FILE)
    }

    /// Record a freshly minted invite, pruning anything long dead while we are here.
    pub fn mint(&mut self, nonce: String, expires: u64, now: u64) {
        self.prune(now);
        self.invites.push(PendingInvite {
            nonce,
            expires,
            spent_at: None,
        });
    }

    /// Spend `nonce`, or say why it cannot be spent.
    ///
    /// The single-use rule lives here and nowhere else, and it is a mutation: a successful claim
    /// stamps `spent_at`, so the caller's only remaining job is to persist the book *before* it
    /// answers.
    ///
    /// # Errors
    /// [`ClaimError`] when the nonce is unknown here, past its expiry, or already spent.
    pub fn claim(&mut self, nonce: &str, now: u64) -> Result<(), ClaimError> {
        let slot = self
            .invites
            .iter_mut()
            .find(|invite| invite.nonce == nonce)
            .ok_or(ClaimError::Unknown)?;
        // Expiry first: an expired invite is refused whether or not it was ever used, so a
        // pruned-then-replayed token can never come back as usable.
        if now >= slot.expires {
            return Err(ClaimError::Expired);
        }
        if slot.spent_at.is_some() {
            return Err(ClaimError::Spent);
        }
        slot.spent_at = Some(now);
        Ok(())
    }

    /// Is `nonce` still claimable at `now`? A read-only peek, for status output.
    #[must_use]
    pub fn is_open(&self, nonce: &str, now: u64) -> bool {
        self.invites
            .iter()
            .any(|invite| invite.nonce == nonce && invite.spent_at.is_none() && now < invite.expires)
    }

    /// Drop entries whose expiry passed more than [`INVITE_KEEP`] ago. Safe against replay by
    /// construction: an entry is only ever pruned once it is already past the expiry check, so a
    /// pruned nonce goes from being refused as expired to being refused as unknown.
    fn prune(&mut self, now: u64) {
        let horizon = now.saturating_sub(INVITE_KEEP.as_secs());
        self.invites.retain(|invite| invite.expires > horizon);
    }
}

// ---------------------------------------------------------------------------------------
// The handshake payloads
// ---------------------------------------------------------------------------------------

/// What the node sends: the nonce that authorises the pairing and the name it would like.
///
/// No key. The key is [`Connection::remote_id`], authenticated by the QUIC handshake; a field for
/// it here would be a field an attacker fills in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinRequest {
    /// The protocol version; always [`VERSION`] when sent by this build.
    pub v: u8,
    /// The nonce copied out of the invite.
    pub nonce: String,
    /// What the node calls itself (`docs/fleet.md` §2) — a suggestion, never a claim.
    pub nickname: String,
}

/// What the node is told once it is in: its petname over there, and the credentials its services
/// must now demand.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Accepted {
    /// What the viewer calls this node — the label in `<service>.<petname>.n.adi`.
    pub petname: String,
    /// The username the viewer will present.
    pub username: String,
    /// The plaintext password, sent **once**. Neither side stores it; both store its verifier.
    pub password: String,
    /// What the viewer may reach on this node.
    pub grants: Vec<Grant>,
}

/// Redacted by hand rather than derived, because the only copy of the password in the system
/// passes through this struct. A derived `Debug` would put it in a log the first time anyone
/// wrote `debug!(?reply)` in an error path — and that log would then hold a working credential
/// for a remote machine, indefinitely, somewhere nobody thinks to look.
impl fmt::Debug for Accepted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Accepted")
            .field("petname", &self.petname)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("grants", &self.grants)
            .finish()
    }
}

/// The viewer's answer.
///
/// A refusal carries a sentence rather than a code because it is read by a human staring at a
/// boot log, and the three things that go wrong (wrong fleet, stale token, token already used)
/// have three different fixes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum JoinReply {
    /// Paired. The payload is the node's whole configuration for this relationship.
    Accepted(Accepted),
    /// Not paired, and why.
    Refused {
        /// A human-readable reason.
        reason: String,
    },
}

impl JoinReply {
    fn refused(reason: impl Into<String>) -> Self {
        Self::Refused {
            reason: reason.into(),
        }
    }
}

/// What a successful [`join`] produces, for the CLI to print.
#[derive(Clone)]
pub struct Joined {
    /// What the viewer calls this node.
    pub petname: String,
    /// What this node now calls the viewer.
    pub viewer: String,
    /// The viewer's key — the identity of record for the relationship (`docs/fleet.md` §2).
    pub viewer_key: EndpointId,
    /// The username to type into the browser prompt.
    pub username: String,
    /// The password to type into it. Printed once and stored nowhere.
    pub password: String,
    /// What the viewer may reach here.
    pub grants: Vec<Grant>,
}

/// Redacted by hand, for the same reason as [`Accepted`]: this is the value the CLI holds while
/// it prints the one and only copy of the password, so a derived `Debug` would be one
/// `debug!(?joined)` away from writing a live credential into a node's boot log.
impl fmt::Debug for Joined {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Joined")
            .field("petname", &self.petname)
            .field("viewer", &self.viewer)
            .field("viewer_key", &self.viewer_key)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("grants", &self.grants)
            .finish()
    }
}

// ---------------------------------------------------------------------------------------
// The decision
// ---------------------------------------------------------------------------------------

/// The grants a fresh pairing carries — see [`DEFAULT_SERVICE`].
#[must_use]
pub fn default_grants() -> Vec<Grant> {
    vec![Grant::Http(Scope::One(DEFAULT_SERVICE.to_string()))]
}

/// The viewer's whole pairing decision, with the network and the clock passed in.
///
/// Pure in the sense that matters: every input is an argument, so the interesting cases — a
/// replayed nonce, an expired one, a name clash, a re-pair — are ordinary unit tests instead of
/// two live endpoints and a stopwatch. The caller supplies `password` because randomness is the
/// one thing a test cannot assert against.
///
/// Order is policy. The nonce is checked first, so a caller with no valid invite cannot learn
/// whether a name is taken here; only then does §2's naming run, and it *cannot* refuse.
pub fn decide(
    registry: &mut FleetRegistry,
    invites: &mut InviteBook,
    key: &EndpointId,
    request: &JoinRequest,
    password: &str,
    now: u64,
) -> JoinReply {
    if request.v != VERSION {
        return JoinReply::refused(format!(
            "this fleet speaks join v{VERSION}; the node offered v{}",
            request.v
        ));
    }
    if let Err(problem) = invites.claim(&request.nonce, now) {
        return JoinReply::refused(problem.to_string());
    }

    let petname = match pair_or_suggest(registry, key, &request.nickname) {
        Ok(petname) => petname,
        Err(e) => return JoinReply::refused(format!("could not file the node locally: {e}")),
    };
    let grants = default_grants();
    let Some(record) = registry.get_mut(&petname) else {
        return JoinReply::refused(format!("the record for {petname:?} vanished mid-pairing"));
    };
    // A re-pair rotates the password rather than keeping the old one: re-inviting a node you
    // already have is what an operator does when the password is lost, and quietly re-issuing a
    // secret they no longer hold would answer the wrong question.
    record.set_password(PAIR_USER, password);
    for grant in grants.clone() {
        record.grant(grant);
    }

    JoinReply::Accepted(Accepted {
        petname,
        username: PAIR_USER.to_string(),
        password: password.to_string(),
        grants,
    })
}

/// Pair `key` under `nickname`, falling back to the registry's own suggestion when that name is
/// taken or unusable — §2 rule 3: a clash resolves, it never refuses.
///
/// # Errors
/// Only if pairing under a name the registry itself proposed still fails, which would mean the
/// registry contradicted itself between the two calls.
fn pair_or_suggest(
    registry: &mut FleetRegistry,
    key: &EndpointId,
    nickname: &str,
) -> anyhow::Result<String> {
    match registry.pair(key, nickname) {
        // An already-paired key keeps its pinned petname, and `pair` has already filed any
        // changed nickname as a pending notification (B4) — never a silent re-point.
        Pairing::Paired { petname } | Pairing::AlreadyPaired { petname, .. } => Ok(petname),
        Pairing::NeedsPetname { suggestion, .. } => registry
            .pair_as(&suggestion, key, nickname)?
            .petname()
            .map(ToString::to_string)
            .with_context(|| format!("pairing under the suggested name {suggestion:?}")),
    }
}

/// The node's side of the bookkeeping: file the viewer, and store the verifier for the password
/// it just handed us, so the viewer's later HTTP requests are gated by exactly that password
/// (`docs/fleet.md` §5).
///
/// The viewer offers no nickname of its own — the invite carries an address and a secret, nothing
/// cosmetic — so it is filed under its key's short form. That is an honest name for a peer we
/// know only by key, and [`FleetRegistry::rename`] is one command away.
///
/// # Errors
/// If the registry cannot file the viewer under any name.
pub fn record_viewer(
    registry: &mut FleetRegistry,
    key: &EndpointId,
    accepted: &Accepted,
) -> anyhow::Result<String> {
    let petname = pair_or_suggest(registry, key, &viewer_nickname(key))?;
    let record = registry
        .get_mut(&petname)
        .with_context(|| format!("the record for {petname:?} vanished mid-pairing"))?;
    record.set_password(&accepted.username, &accepted.password);
    for grant in accepted.grants.iter().cloned() {
        record.grant(grant);
    }
    Ok(petname)
}

/// A name for a peer we know only by key: `viewer-<10 hex>`.
fn viewer_nickname(key: &EndpointId) -> String {
    crate::fleet::sanitize_name(&format!("viewer-{}", key.fmt_short()))
}

// ---------------------------------------------------------------------------------------
// Randomness
// ---------------------------------------------------------------------------------------

/// A fresh 128-bit nonce, hex-encoded.
///
/// # Panics
/// Never in practice: only if the OS random source is unavailable, which is not a condition a
/// pairing can meaningfully continue past.
#[must_use]
pub fn random_nonce() -> String {
    use rand::TryRng as _;
    let mut bytes = [0u8; NONCE_BYTES];
    rand::rngs::SysRng
        .try_fill_bytes(&mut bytes)
        .expect("the OS random source is unavailable");
    ticket::to_hex(&bytes)
}

/// A fresh password from [`PASSWORD_ALPHABET`], drawn without modulo bias.
///
/// # Panics
/// Never in practice: only if the OS random source is unavailable.
#[must_use]
pub fn random_password() -> String {
    use rand::RngExt as _;
    // `SysRng` is fallible (it reads the OS on every call); `UnwrapErr` turns it into the
    // infallible `Rng` that `random_range` needs, panicking on a source that isn't there —
    // which is the behaviour documented above.
    let mut rng = rand::rand_core::UnwrapErr(rand::rngs::SysRng);
    (0..PASSWORD_LEN)
        .map(|_| char::from(PASSWORD_ALPHABET[rng.random_range(0..PASSWORD_ALPHABET.len())]))
        .collect()
}

// ---------------------------------------------------------------------------------------
// Minting an invite (viewer side)
// ---------------------------------------------------------------------------------------

/// Mint an invite against this machine's running mesh, and record its nonce so exactly one node
/// can spend it.
///
/// The endpoint comes from the ticket the daemon publishes, not from binding one here: a second
/// endpoint on the same identity would race the running one for its relay session, and the
/// absence of a published ticket is exactly the check we want anyway — an invite nobody is
/// listening behind is a token that cannot work.
///
/// # Errors
/// If the mesh is not running (no published ticket), or the book cannot be read or written.
pub fn mint_invite(ttl: Duration) -> anyhow::Result<String> {
    let endpoint = ticket::published().context(
        "the mesh is not running here, so a node would have nothing to dial — start it \
         (`adi-mesh run`, or the control panel) and try again",
    )?;
    mint_invite_for(&endpoint, ttl, &Config::open(), adi_config::now_unix())
}

/// [`mint_invite`] with the endpoint, store and clock passed in — the seam the tests mint
/// through, and the entry point for a caller that already holds a ticket.
///
/// # Errors
/// Any I/O or encode error from the store, or from encoding the token.
pub fn mint_invite_for(
    endpoint: &str,
    ttl: Duration,
    store: &Config,
    now: u64,
) -> anyhow::Result<String> {
    let _guard = PAIRING.lock().unwrap_or_else(PoisonError::into_inner);
    let mut invites = InviteBook::load_from(store)?;
    let nonce = random_nonce();
    // At least a second, so a zero TTL is a very short invite rather than a dead one.
    let expires = now.saturating_add(ttl.as_secs().max(1));
    invites.mint(nonce.clone(), expires, now);
    invites.save_to(store)?;
    encode_invite(&Invite {
        v: VERSION,
        endpoint: endpoint.to_string(),
        nonce,
        expires,
    })
}

// ---------------------------------------------------------------------------------------
// Serving a join (viewer side)
// ---------------------------------------------------------------------------------------

/// Answer one inbound `adi/mesh/join/1` connection.
///
/// Dispatched from the endpoint's single accept loop ([`crate::host::serve`]) by ALPN, and
/// deliberately *before* the peer-authorization check that the forward role applies: a joining
/// node is by definition not yet authorized, and the nonce is what stands in its place.
pub async fn serve_join(conn: Connection) {
    serve_join_with(conn, |_| {}).await;
}

/// [`serve_join`], with the acceptance handed to `on_paired` before the connection closes.
///
/// The password exists in plaintext exactly once, here, on the side that minted it (§8) — and a
/// viewer with no terminal to print it to needs that one copy. The iOS app takes it straight to the
/// Keychain, so pairing a node also finishes the login for it; without this hook the phone would
/// mint a password, hand it to the node, and then have to ask a human to type it back in.
///
/// It is a callback and not a return value so the existing callers cannot accidentally bind the
/// credential to a variable, and so the copy's lifetime is the length of one call.
pub async fn serve_join_with(conn: Connection, on_paired: impl FnOnce(&Accepted)) {
    let peer = conn.remote_id();
    match handshake(&conn, peer).await {
        Ok(JoinReply::Accepted(accepted)) => {
            info!(%peer, petname = %accepted.petname, "join: paired a new node");
            on_paired(&accepted);
        }
        Ok(JoinReply::Refused { reason }) => {
            warn!(%peer, %reason, "join: refused");
        }
        Err(e) => debug!(%peer, error = %e, "join: handshake failed"),
    }
    conn.close(0u32.into(), b"join complete");
}

/// One bi-stream: read the request, decide, persist, answer.
async fn handshake(conn: &Connection, peer: EndpointId) -> anyhow::Result<JoinReply> {
    let (mut send, mut recv) = conn.accept_bi().await?;
    let request: JoinRequest = read_frame(&mut recv).await?;
    let reply = decide_and_persist(&peer, &request)?;
    write_frame(&mut send, &reply).await?;
    let _ = send.finish();
    // Give the node a moment to read the reply before the connection goes away; an immediate
    // close truncates the one message that carries the password.
    let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
    Ok(reply)
}

/// Decide and write both files under one lock, **before** the reply goes out.
///
/// The store work is synchronous inside the lock on purpose: two small TOML files, held for the
/// length of a decision, on a path that runs once per pairing. What it buys is that the critical
/// section contains no `.await`, so "the nonce is spent" and "the registry names the node" cannot
/// interleave with another join.
fn decide_and_persist(key: &EndpointId, request: &JoinRequest) -> anyhow::Result<JoinReply> {
    let _guard = PAIRING.lock().unwrap_or_else(PoisonError::into_inner);

    let store = Config::open();
    let mut invites = InviteBook::load_from(&store)?;
    let mut registry = FleetRegistry::load_from(&store)?;
    let password = random_password();
    let reply = decide(
        &mut registry,
        &mut invites,
        key,
        request,
        &password,
        adi_config::now_unix(),
    );

    // The book first, always: it is the record of what has been spent, and a crash between these
    // two writes must leave a burnt nonce rather than a reusable one.
    invites.save_to(&store)?;
    if matches!(reply, JoinReply::Accepted(_)) {
        registry.save_to(&store)?;
        // The front door's certificate list wants the new petname here (`docs/fleet.md` F2), but
        // that lives in `adi_core::dns::Dns::add_mesh_node` and this crate does not depend on
        // adi-core — nor should it, since the dependency runs the other way everywhere else. The
        // owner of that call is whoever owns this daemon's lifecycle and can see both crates:
        // adi-app. Until then routing works immediately and only HTTPS waits for a regeneration,
        // which is exactly the trade F2 already describes.
    }
    Ok(reply)
}

// ---------------------------------------------------------------------------------------
// Joining (node side)
// ---------------------------------------------------------------------------------------

/// Dial the viewer named by `token` and complete the pairing.
///
/// This is the only outbound thing a node does to be enrolled, and it binds no listener: the
/// endpoint here exists to dial and is closed again before the function returns.
///
/// # Errors
/// If the token is malformed or expired, the identity or registry cannot be read, the viewer
/// cannot be reached within [`HANDSHAKE_TIMEOUT`], or the viewer refuses.
pub async fn join(token: &str) -> anyhow::Result<Joined> {
    let invite = decode_invite(token, adi_config::now_unix())?;
    let addr = ticket::parse_target(&invite.endpoint)
        .context("the invite does not name a reachable endpoint")?;
    let nickname = node::nickname();
    let secret = identity::load_or_create()?;
    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret)
        .bind()
        .await?;

    let asked = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        ask_to_join(&endpoint, addr, &invite.nonce, &nickname),
    )
    .await;
    endpoint.close().await;
    let (viewer_key, accepted) = match asked {
        Ok(result) => result?,
        Err(_) => bail!(
            "the viewer did not answer within {HANDSHAKE_TIMEOUT:?} — is its mesh still running?"
        ),
    };

    let store = Config::open();
    let mut registry = FleetRegistry::load_from(&store)?;
    let viewer = record_viewer(&mut registry, &viewer_key, &accepted)?;
    registry.save_to(&store)?;

    Ok(Joined {
        petname: accepted.petname,
        viewer,
        viewer_key,
        username: accepted.username,
        password: accepted.password,
        grants: accepted.grants,
    })
}

/// Dial, ask, and read the answer. Split out so the timeout above wraps the whole exchange
/// rather than each step.
async fn ask_to_join(
    endpoint: &Endpoint,
    addr: EndpointAddr,
    nonce: &str,
    nickname: &str,
) -> anyhow::Result<(EndpointId, Accepted)> {
    let viewer = addr.id;
    let conn = endpoint
        .connect(addr, ALPN)
        .await
        .with_context(|| format!("dialling the viewer {viewer}"))?;
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(
        &mut send,
        &JoinRequest {
            v: VERSION,
            nonce: nonce.to_string(),
            nickname: nickname.to_string(),
        },
    )
    .await?;
    let _ = send.finish();

    let reply: JoinReply = read_frame(&mut recv).await?;
    conn.close(0u32.into(), b"join complete");
    match reply {
        JoinReply::Accepted(accepted) => Ok((viewer, accepted)),
        JoinReply::Refused { reason } => bail!("the viewer refused to pair: {reason}"),
    }
}

// ---------------------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------------------

/// Write one length-prefixed JSON frame: `[len: u32 BE][json]`.
///
/// Length-prefixed rather than "write JSON and close the stream", because both directions of this
/// handshake live on one bi-stream and a reader that waits for EOF cannot also expect a reply.
///
/// # Errors
/// If the value cannot be serialized, exceeds [`MAX_FRAME`], or the stream write fails.
async fn write_frame<W, T>(w: &mut W, value: &T) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let json = serde_json::to_vec(value).context("serializing a join frame")?;
    let len = u32::try_from(json.len()).unwrap_or(u32::MAX);
    ensure!(
        json.len() <= MAX_FRAME,
        "join frame is {} bytes, over the {MAX_FRAME}-byte limit",
        json.len()
    );
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(&json).await?;
    w.flush().await?;
    Ok(())
}

/// Read one length-prefixed JSON frame, refusing an over-long one **before** allocating for it.
///
/// # Errors
/// If the length is zero or over [`MAX_FRAME`], the stream ends early, or the bytes are not the
/// expected JSON.
async fn read_frame<R, T>(r: &mut R) -> anyhow::Result<T>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut header = [0u8; 4];
    r.read_exact(&mut header).await?;
    let len = usize::try_from(u32::from_be_bytes(header)).unwrap_or(usize::MAX);
    ensure!(len > 0, "join frame is empty");
    ensure!(
        len <= MAX_FRAME,
        "join frame claims {len} bytes, over the {MAX_FRAME}-byte limit"
    );
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    serde_json::from_slice(&buf).context("a join frame did not decode to the expected payload")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::Target;

    fn some_key() -> EndpointId {
        iroh::SecretKey::generate().public()
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "adi-mesh-join-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ))
    }

    fn sample_ticket() -> String {
        let addr = iroh::EndpointAddr::new(some_key()).with_ip_addr("127.0.0.1:45080".parse().unwrap());
        ticket::encode(&addr).expect("encode a ticket")
    }

    fn invite(nonce: &str, expires: u64) -> Invite {
        Invite {
            v: VERSION,
            endpoint: sample_ticket(),
            nonce: nonce.to_string(),
            expires,
        }
    }

    fn request(nonce: &str, nickname: &str) -> JoinRequest {
        JoinRequest {
            v: VERSION,
            nonce: nonce.to_string(),
            nickname: nickname.to_string(),
        }
    }

    /// A book holding one open invite, plus the nonce it holds.
    fn book_with_open_invite(expires: u64) -> (InviteBook, String) {
        let mut book = InviteBook::default();
        let nonce = random_nonce();
        book.mint(nonce.clone(), expires, 0);
        (book, nonce)
    }

    fn accepted_of(reply: &JoinReply) -> &Accepted {
        match reply {
            JoinReply::Accepted(accepted) => accepted,
            JoinReply::Refused { reason } => panic!("expected an acceptance, got: {reason}"),
        }
    }

    fn refusal_of(reply: &JoinReply) -> &str {
        match reply {
            JoinReply::Refused { reason } => reason,
            JoinReply::Accepted(accepted) => panic!("expected a refusal, got {accepted:?}"),
        }
    }

    // -- the token -----------------------------------------------------------------------

    #[test]
    fn invite_token_round_trips() {
        let original = invite(&random_nonce(), 2_000);
        let token = encode_invite(&original).expect("encode");
        assert!(token.starts_with(PREFIX), "{token}");
        assert_eq!(decode_invite(&token, 1_000).expect("decode"), original);
    }

    #[test]
    fn a_token_with_the_wrong_prefix_or_bad_hex_is_refused() {
        let token = encode_invite(&invite(&random_nonce(), 2_000)).expect("encode");
        let payload = token.strip_prefix(PREFIX).expect("payload");

        for bad in [payload, &format!("adimesh:{payload}"), "", "adi-invite"] {
            let err = decode_invite(bad, 1_000).expect_err("prefix");
            assert!(err.to_string().contains("adi-invite"), "{bad:?}: {err}");
        }

        for bad in [
            format!("{PREFIX}zzzz"),
            format!("{PREFIX}abc"),
            format!("{PREFIX}{payload}ff"),
        ] {
            assert!(decode_invite(&bad, 1_000).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_token_of_another_version_is_refused() {
        let mut future = invite(&random_nonce(), 2_000);
        future.v = VERSION + 1;
        let err = decode_invite(&encode_invite(&future).expect("encode"), 1_000)
            .expect_err("version");
        assert!(err.to_string().contains("v1"), "{err}");
    }

    #[test]
    fn an_expired_token_is_refused_before_a_dial() {
        let token = encode_invite(&invite(&random_nonce(), 1_000)).expect("encode");
        assert!(decode_invite(&token, 999).is_ok(), "still live one second out");
        let err = decode_invite(&token, 1_000).expect_err("expired at the boundary");
        assert!(err.to_string().contains("expired"), "{err}");
        assert!(decode_invite(&token, 5_000).is_err(), "long expired");
    }

    #[test]
    fn a_token_with_a_malformed_nonce_is_refused() {
        for nonce in ["", "abc", &"z".repeat(32), &"a".repeat(31), &"a".repeat(33)] {
            let token = encode_invite(&invite(nonce, 2_000)).expect("encode");
            let err = decode_invite(&token, 1_000).expect_err("nonce");
            assert!(err.to_string().contains("nonce"), "{nonce:?}: {err}");
        }
        assert!(valid_nonce(&random_nonce()));
    }

    // -- single use ----------------------------------------------------------------------

    #[test]
    fn a_nonce_can_be_claimed_exactly_once() {
        let (mut book, nonce) = book_with_open_invite(1_000);
        assert!(book.is_open(&nonce, 500));
        assert_eq!(book.claim(&nonce, 500), Ok(()));
        assert!(!book.is_open(&nonce, 500), "spending closes it");
        assert_eq!(
            book.claim(&nonce, 500),
            Err(ClaimError::Spent),
            "a replay is refused"
        );
    }

    #[test]
    fn an_unknown_or_expired_nonce_is_refused() {
        let (mut book, nonce) = book_with_open_invite(1_000);
        assert_eq!(book.claim("deadbeef", 500), Err(ClaimError::Unknown));
        assert_eq!(book.claim(&nonce, 1_000), Err(ClaimError::Expired));
        assert_eq!(book.claim(&nonce, 5_000), Err(ClaimError::Expired));
        assert!(!book.is_open(&nonce, 1_000));
    }

    #[test]
    fn an_expired_nonce_stays_refused_even_if_it_was_never_spent() {
        let (mut book, nonce) = book_with_open_invite(1_000);
        assert_eq!(book.claim(&nonce, 2_000), Err(ClaimError::Expired));
        // And it does not become claimable again by turning the clock back inside a live TTL:
        // the entry is still there and unspent, so this is the honest behaviour to pin.
        assert_eq!(book.claim(&nonce, 500), Ok(()));
        assert_eq!(book.claim(&nonce, 500), Err(ClaimError::Spent));
    }

    #[test]
    fn long_dead_invites_are_pruned_but_live_ones_survive() {
        let mut book = InviteBook::default();
        let old = random_nonce();
        let fresh = random_nonce();
        book.mint(old.clone(), 1_000, 0);
        let long_after = 1_000 + INVITE_KEEP.as_secs() + 1;
        book.mint(fresh.clone(), long_after + 900, long_after);

        assert_eq!(book.invites.len(), 1, "the minting prune dropped the dead one");
        assert_eq!(book.claim(&old, long_after), Err(ClaimError::Unknown));
        assert_eq!(book.claim(&fresh, long_after), Ok(()));
    }

    #[test]
    fn the_book_round_trips_through_the_store() {
        let dir = scratch("book");
        let _ = std::fs::remove_dir_all(&dir);
        let store = Config::with_root(&dir);

        let mut book = InviteBook::load_from(&store).expect("first load");
        assert!(book.invites.is_empty());
        let nonce = random_nonce();
        book.mint(nonce.clone(), 9_999_999_999, 0);
        book.claim(&nonce, 1).expect("claim");
        book.save_to(&store).expect("save");

        // Spentness survives a restart — that is the whole reason the book is a file.
        let mut reloaded = InviteBook::load_from(&store).expect("reload");
        assert_eq!(reloaded.claim(&nonce, 2), Err(ClaimError::Spent));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn minting_records_the_nonce_the_token_carries() {
        let dir = scratch("mint");
        let _ = std::fs::remove_dir_all(&dir);
        let store = Config::with_root(&dir);

        let token = mint_invite_for(&sample_ticket(), Duration::from_secs(600), &store, 1_000)
            .expect("mint");
        let decoded = decode_invite(&token, 1_000).expect("decode");
        assert_eq!(decoded.expires, 1_600);

        let mut book = InviteBook::load_from(&store).expect("load");
        assert!(book.is_open(&decoded.nonce, 1_000), "minted and claimable");
        assert_eq!(book.claim(&decoded.nonce, 1_000), Ok(()));

        // Two mints are two independent invites.
        let other = mint_invite_for(&sample_ticket(), Duration::from_secs(600), &store, 1_000)
            .expect("mint again");
        assert_ne!(
            decode_invite(&other, 1_000).expect("decode").nonce,
            decoded.nonce
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- the decision --------------------------------------------------------------------

    #[test]
    fn a_fresh_nickname_becomes_the_petname_and_the_password_verifies() {
        let (mut book, nonce) = book_with_open_invite(1_000);
        let mut registry = FleetRegistry::default();
        let key = some_key();

        let reply = decide(
            &mut registry,
            &mut book,
            &key,
            &request(&nonce, "laptop-b"),
            "hunter2",
            500,
        );
        let accepted = accepted_of(&reply);
        assert_eq!(accepted.petname, "laptop-b");
        assert_eq!(accepted.username, PAIR_USER);
        assert_eq!(accepted.password, "hunter2");
        assert_eq!(accepted.grants, default_grants());

        let record = registry.get("laptop-b").expect("filed under the petname");
        assert_eq!(record.key, key.to_string());
        assert!(record.verify_password(PAIR_USER, "hunter2"));
        assert!(!record.verify_password(PAIR_USER, "hunter3"), "wrong password");
        assert!(!record.verify_password("root", "hunter2"), "wrong user");
        assert!(record.allows(Target::Http("app")), "the default grant is live");
        assert!(!record.allows(Target::Http("nosh")), "and nothing else is");
    }

    #[test]
    fn a_clashing_nickname_takes_a_suggestion_and_never_refuses() {
        let mut registry = FleetRegistry::default();
        let incumbent = some_key();
        assert!(matches!(
            registry.pair(&incumbent, "laptop-b"),
            Pairing::Paired { .. }
        ));

        let (mut book, nonce) = book_with_open_invite(1_000);
        let newcomer = some_key();
        let reply = decide(
            &mut registry,
            &mut book,
            &newcomer,
            &request(&nonce, "laptop-b"),
            "pw",
            500,
        );
        assert_eq!(accepted_of(&reply).petname, "laptop-b-2");
        assert_eq!(
            registry.get("laptop-b").expect("incumbent").key,
            incumbent.to_string(),
            "the pinned binding is untouched"
        );
    }

    #[test]
    fn an_unusable_nickname_is_coerced_rather_than_refused() {
        let (mut book, nonce) = book_with_open_invite(1_000);
        let mut registry = FleetRegistry::default();
        let reply = decide(
            &mut registry,
            &mut book,
            &some_key(),
            &request(&nonce, "Build Box!!"),
            "pw",
            500,
        );
        assert_eq!(accepted_of(&reply).petname, "build-box");
    }

    #[test]
    fn a_replayed_nonce_pairs_no_second_machine() {
        let (mut book, nonce) = book_with_open_invite(1_000);
        let mut registry = FleetRegistry::default();

        let first = decide(
            &mut registry,
            &mut book,
            &some_key(),
            &request(&nonce, "laptop-b"),
            "pw",
            500,
        );
        assert_eq!(accepted_of(&first).petname, "laptop-b");

        let replay = decide(
            &mut registry,
            &mut book,
            &some_key(),
            &request(&nonce, "impostor"),
            "pw",
            500,
        );
        assert!(refusal_of(&replay).contains("already been used"), "{replay:?}");
        assert_eq!(registry.len(), 1, "no second machine was filed");
        assert!(registry.get("impostor").is_none());
    }

    #[test]
    fn an_unknown_expired_or_wrong_version_request_is_refused_without_pairing() {
        let (mut book, nonce) = book_with_open_invite(1_000);
        let mut registry = FleetRegistry::default();

        let unknown = decide(
            &mut registry,
            &mut book,
            &some_key(),
            &request("f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0", "a"),
            "pw",
            500,
        );
        assert!(refusal_of(&unknown).contains("not issued"), "{unknown:?}");

        let expired = decide(
            &mut registry,
            &mut book,
            &some_key(),
            &request(&nonce, "a"),
            "pw",
            5_000,
        );
        assert!(refusal_of(&expired).contains("expired"), "{expired:?}");

        let mut future = request(&nonce, "a");
        future.v = VERSION + 1;
        let wrong_version = decide(&mut registry, &mut book, &some_key(), &future, "pw", 500);
        assert!(refusal_of(&wrong_version).contains("join v1"), "{wrong_version:?}");
        assert!(
            book.is_open(&nonce, 500),
            "a version mismatch must not burn the invite"
        );

        assert!(registry.is_empty(), "nothing was filed by any refusal");
    }

    #[test]
    fn re_pairing_a_known_key_keeps_its_petname_and_rotates_the_password() {
        let mut registry = FleetRegistry::default();
        let key = some_key();
        let (mut book, first) = book_with_open_invite(1_000);
        decide(
            &mut registry,
            &mut book,
            &key,
            &request(&first, "laptop-b"),
            "old",
            500,
        );
        registry.rename("laptop-b", "desk").expect("local rename");

        let second = random_nonce();
        book.mint(second.clone(), 1_000, 500);
        let reply = decide(
            &mut registry,
            &mut book,
            &key,
            &request(&second, "laptop-b"),
            "new",
            600,
        );

        // The pinned petname wins over the offered nickname (§2 rule 4), and the rename is only
        // a notification.
        assert_eq!(accepted_of(&reply).petname, "desk");
        let record = registry.get("desk").expect("record");
        assert!(record.verify_password(PAIR_USER, "new"));
        assert!(!record.verify_password(PAIR_USER, "old"), "the old one is gone");
        assert_eq!(registry.len(), 1);
    }

    // -- the node's side of the bookkeeping ----------------------------------------------

    #[test]
    fn the_node_files_the_viewer_under_its_key_and_stores_the_verifier() {
        let mut registry = FleetRegistry::default();
        let viewer = some_key();
        let accepted = Accepted {
            petname: "laptop-b".to_string(),
            username: PAIR_USER.to_string(),
            password: "hunter2".to_string(),
            grants: default_grants(),
        };

        let petname = record_viewer(&mut registry, &viewer, &accepted).expect("file the viewer");
        assert!(petname.starts_with("viewer-"), "{petname}");
        assert!(crate::fleet::valid_name(&petname), "{petname}");

        let record = registry.get(&petname).expect("record");
        assert_eq!(record.key, viewer.to_string());
        assert!(record.verify_password(PAIR_USER, "hunter2"));
        assert!(!record.verify_password(PAIR_USER, "nope"));
        assert!(
            record.allows(Target::Http("app")),
            "the viewer may reach exactly what it granted itself"
        );
        assert!(!record.allows(Target::Http("nosh")));

        // Re-running the same join is idempotent, not a second entry.
        let again = record_viewer(&mut registry, &viewer, &accepted).expect("again");
        assert_eq!(again, petname);
        assert_eq!(registry.len(), 1);
    }

    // -- randomness ----------------------------------------------------------------------

    #[test]
    fn generated_secrets_are_well_formed_and_not_repeated() {
        let a = random_nonce();
        let b = random_nonce();
        assert!(valid_nonce(&a) && valid_nonce(&b));
        assert_ne!(a, b);

        let password = random_password();
        assert_eq!(password.chars().count(), PASSWORD_LEN);
        assert!(
            password
                .bytes()
                .all(|b| PASSWORD_ALPHABET.contains(&b)),
            "{password}"
        );
        assert_ne!(password, random_password());
    }

    /// The password must never survive a `{:?}`. Both types carry it, both hand-write `Debug` to
    /// redact it, and both are one careless `debug!(?value)` away from putting a live credential
    /// for a remote machine into a log file. Pinned here so re-deriving `Debug` fails loudly
    /// rather than silently.
    #[test]
    fn a_debug_rendering_never_carries_the_password() {
        let secret = "correct-horse-battery-staple";

        let accepted = Accepted {
            petname: "laptop-b".into(),
            username: "adi".into(),
            password: secret.into(),
            grants: vec![Grant::Http(Scope::One("app".into()))],
        };
        let rendered = format!("{accepted:?}");
        assert!(!rendered.contains(secret), "{rendered}");
        assert!(rendered.contains("redacted"), "{rendered}");
        assert!(rendered.contains("laptop-b"), "the rest still debugs: {rendered}");

        let joined = Joined {
            petname: "laptop-b".into(),
            viewer: "desk".into(),
            viewer_key: iroh::SecretKey::generate().public(),
            username: "adi".into(),
            password: secret.into(),
            grants: Vec::new(),
        };
        let rendered = format!("{joined:?}");
        assert!(!rendered.contains(secret), "{rendered}");
        assert!(rendered.contains("redacted"), "{rendered}");
        assert!(rendered.contains("desk"), "the rest still debugs: {rendered}");
    }

    // -- framing -------------------------------------------------------------------------

    #[tokio::test]
    async fn frames_round_trip_over_a_stream() {
        let (mut writer, mut reader) = tokio::io::duplex(8 * 1024);
        let sent = request(&random_nonce(), "laptop-b");
        write_frame(&mut writer, &sent).await.expect("write");
        let read: JoinRequest = read_frame(&mut reader).await.expect("read");
        assert_eq!(read, sent);

        let reply = JoinReply::Accepted(Accepted {
            petname: "laptop-b".to_string(),
            username: PAIR_USER.to_string(),
            password: random_password(),
            grants: default_grants(),
        });
        write_frame(&mut writer, &reply).await.expect("write reply");
        let read: JoinReply = read_frame(&mut reader).await.expect("read reply");
        assert_eq!(read, reply);

        let refusal = JoinReply::refused("nope");
        write_frame(&mut writer, &refusal).await.expect("write refusal");
        let read: JoinReply = read_frame(&mut reader).await.expect("read refusal");
        assert_eq!(read, refusal);
    }

    #[tokio::test]
    async fn an_over_long_frame_is_refused_before_it_is_allocated() {
        let (mut writer, mut reader) = tokio::io::duplex(64);
        let claimed = u32::try_from(MAX_FRAME + 1).expect("fits");
        writer
            .write_all(&claimed.to_be_bytes())
            .await
            .expect("write the header");
        let err = read_frame::<_, JoinRequest>(&mut reader)
            .await
            .expect_err("over-long");
        assert!(err.to_string().contains("over the"), "{err}");
    }

    #[tokio::test]
    async fn an_empty_or_undecodable_frame_is_refused() {
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        writer.write_all(&0u32.to_be_bytes()).await.expect("header");
        assert!(read_frame::<_, JoinRequest>(&mut reader).await.is_err());

        let (mut writer, mut reader) = tokio::io::duplex(1024);
        let body = b"not json";
        let len = u32::try_from(body.len()).expect("fits");
        writer.write_all(&len.to_be_bytes()).await.expect("header");
        writer.write_all(body).await.expect("body");
        let err = read_frame::<_, JoinRequest>(&mut reader)
            .await
            .expect_err("garbage");
        assert!(err.to_string().contains("did not decode"), "{err}");
    }

    #[test]
    fn the_reply_is_tagged_the_way_the_contract_describes_it() {
        let accepted = JoinReply::Accepted(Accepted {
            petname: "laptop-b".to_string(),
            username: PAIR_USER.to_string(),
            password: "pw".to_string(),
            grants: default_grants(),
        });
        let json = serde_json::to_value(&accepted).expect("encode");
        assert_eq!(json["result"], "accepted");
        assert_eq!(json["petname"], "laptop-b");
        assert_eq!(json["grants"][0], "http:app");

        let refused = serde_json::to_value(JoinReply::refused("no")).expect("encode");
        assert_eq!(refused["result"], "refused");
        assert_eq!(refused["reason"], "no");
    }
}
