//! The fleet registry: the remote adi nodes this machine has paired with.
//!
//! # Three names, never conflated (docs/fleet.md §2)
//!
//! * **key** — the node's [`EndpointId`]. The identity of record, global, unforgeable.
//! * **nickname** — what the node calls *itself*. Only ever a *suggestion*.
//! * **petname** — what *this* machine calls it, unique here, and the thing that appears in
//!   `<service>.<petname>.n.adi`.
//!
//! The petname→key binding is **pinned** at pairing (trust on first use). A node that later
//! re-declares a different nickname gets a [`NicknameChange`] filed against it — never a
//! silent re-point. That single rule is what stops any paired node from renaming itself to
//! `main` and quietly stealing every link the operator had to the real `main`. Accepting a
//! rename is an explicit local act ([`FleetRegistry::accept_nickname`]).
//!
//! Symmetrically, a name collision must **never** refuse a connection: authorization is by
//! key, the name is presentation. So [`FleetRegistry::pair`] answers a clash with a
//! [`Pairing::NeedsPetname`] carrying a free suggestion (`<name>-2`, `-3`, …) rather than an
//! `Err` the caller would be tempted to turn into a failed handshake.
//!
//! # Default-deny (docs/fleet.md §5)
//!
//! Each record carries the [`Grant`]s the peer holds *on this machine* and the Basic-auth
//! [`Credential`] its requests must carry. **An empty grant list denies everything.** This is
//! deliberately the opposite of the legacy [`HostConfig::authorized_peers`] rule, where an
//! empty list means *every* peer (`host.rs`): on an endpoint reachable through a public relay,
//! open-by-default is a footgun. New code authorizes through [`NodeRecord::allows`], which can
//! only ever say yes because a grant said so.
//!
//! The registry lives beside `mesh.toml` and `identity.key` in the shared store's `mesh`
//! module dir (`~/.adi/mono/mesh/fleet.toml`) and is loaded/saved exactly like
//! [`MeshConfig`](crate::config::MeshConfig).
//!
//! [`HostConfig::authorized_peers`]: crate::config::HostConfig::authorized_peers

use std::collections::BTreeMap;
use std::fmt;
use std::net::SocketAddr;
use std::str::FromStr;

use adi_config::Config;
use anyhow::{Context as _, bail, ensure};
use iroh::EndpointId;
use serde::{Deserialize, Serialize};

use crate::auth::Credential;

/// The typed config file within the `mesh` module dir.
const FLEET_FILE: &str = "fleet.toml";

/// Bytes in one DNS label — the hard ceiling on a nickname or petname.
const MAX_LABEL: usize = 63;

/// What a name is sanitized to when nothing usable survives (e.g. a nickname of `"***"`).
const FALLBACK_NAME: &str = "node";

// ---------------------------------------------------------------------------------------
// B1 — names
// ---------------------------------------------------------------------------------------

/// Is `name` a usable node name — one DNS label, `^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$`?
///
/// Lowercase only (a hostname is case-insensitive, so accepting `Laptop` would make two
/// petnames that address the same host but compare unequal), 1..=63 bytes, no leading or
/// trailing hyphen, and no dots — a petname is one label, never a fragment of a domain.
///
/// One implementation, in [`crate::protocol::is_dns_label`]: the same rule decides whether a
/// petname can appear in `<service>.<petname>.n.adi` and whether a service label is legal on
/// the wire. Two copies of it would be free to drift, and the failure that drift produces —
/// a name this machine accepts but the far side rejects — surfaces only on a live connection.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    crate::protocol::is_dns_label(name)
}

/// Coerce an arbitrary offered string into a valid label: lowercase, every run of
/// non-alphanumerics collapsed to a single hyphen, trimmed and truncated to fit. Used to turn
/// a nickname we cannot accept into a suggestion we *can* — rule 3 of §2 forbids answering a
/// bad name with a refusal.
///
/// Public because the *other* side of §2 needs exactly this rule: [`crate::node`] derives this
/// machine's own nickname from its hostname, which is an arbitrary string in precisely the same
/// way an offered nickname is. A second coercion would be free to disagree, and the disagreement
/// would show up as a node whose self-chosen name its peers quietly rewrite.
#[must_use]
pub fn sanitize_name(offered: &str) -> String {
    let mut out = String::with_capacity(offered.len().min(MAX_LABEL));
    for ch in offered.chars() {
        if out.len() == MAX_LABEL {
            break;
        }
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        FALLBACK_NAME.to_string()
    } else {
        out
    }
}

/// `<base>-<n>`, with `base` shortened so the result still fits in one label.
fn numbered(base: &str, n: u32) -> String {
    let suffix = format!("-{n}");
    let room = MAX_LABEL.saturating_sub(suffix.len());
    let head = base.get(..room).unwrap_or(base).trim_end_matches('-');
    format!("{head}{suffix}")
}

// ---------------------------------------------------------------------------------------
// B6 — grants
// ---------------------------------------------------------------------------------------

/// A grant's subject: one specific name, or `*` for every name in that family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// `*` — any name.
    Any,
    /// Exactly this name.
    One(String),
}

impl Scope {
    /// Does this scope cover `label`?
    #[must_use]
    pub fn matches(&self, label: &str) -> bool {
        match self {
            Self::Any => true,
            Self::One(one) => one == label,
        }
    }

    /// A scope over one DNS label — what a `ctl:` grant names.
    fn parse(raw: &str) -> anyhow::Result<Self> {
        Self::parse_with(
            raw,
            valid_name,
            "a valid label (lowercase, 1..=63, no leading/trailing `-`)",
        )
    }

    /// A scope over a **service name**, which is one or more labels: a node serves `app.nosh.adi`
    /// beside `nosh.adi`, and `http:app.nosh` is the grant that names it (`docs/fleet.md` §5).
    /// A single label still parses, so every grant written before deep names existed still does.
    fn parse_service(raw: &str) -> anyhow::Result<Self> {
        Self::parse_with(
            raw,
            crate::protocol::is_service_name,
            "a valid service name (one or more lowercase DNS labels, `app.nosh`)",
        )
    }

    fn parse_with(raw: &str, valid: fn(&str) -> bool, expected: &str) -> anyhow::Result<Self> {
        if raw == "*" {
            return Ok(Self::Any);
        }
        ensure!(valid(raw), "{raw:?} is neither `*` nor {expected}");
        Ok(Self::One(raw.to_string()))
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Any => f.write_str("*"),
            Self::One(one) => f.write_str(one),
        }
    }
}

/// One thing a paired peer may reach on this machine.
///
/// The string form is what lives in `fleet.toml` and what an operator types:
/// `http:*`, `http:nosh`, `tcp:127.0.0.1:22`, `ctl:read`, `ctl:*`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Grant {
    /// An HTTP service served here by name (`http:nosh`, `http:app.nosh`), or every one of
    /// them (`http:*`).
    Http(Scope),
    /// A raw TCP forward to exactly this local address (`tcp:127.0.0.1:22`).
    Tcp(SocketAddr),
    /// A control-plane scope (`ctl:read`), or every one of them (`ctl:*`).
    Ctl(Scope),
}

/// A request to authorize, as the caller sees it at the moment of the check.
///
/// Borrowed and [`Copy`] because it is built per request in the gateway's hot path; the
/// registry never stores one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target<'a> {
    /// The peer asked for the HTTP service with this label.
    Http(&'a str),
    /// The peer asked to be spliced to this local address.
    Tcp(SocketAddr),
    /// The peer asked for this control-plane scope.
    Ctl(&'a str),
}

impl Grant {
    /// Does this single grant cover `target`?
    #[must_use]
    pub fn allows(&self, target: Target<'_>) -> bool {
        match (self, target) {
            (Self::Http(scope), Target::Http(label)) | (Self::Ctl(scope), Target::Ctl(label)) => {
                scope.matches(label)
            }
            (Self::Tcp(granted), Target::Tcp(wanted)) => *granted == wanted,
            _ => false,
        }
    }
}

impl FromStr for Grant {
    type Err = anyhow::Error;

    fn from_str(raw: &str) -> anyhow::Result<Self> {
        let raw = raw.trim();
        let (family, rest) = raw
            .split_once(':')
            .with_context(|| format!("grant {raw:?} is missing its `<family>:` prefix"))?;
        match family {
            "http" => Ok(Self::Http(Scope::parse_service(rest)?)),
            "ctl" => Ok(Self::Ctl(Scope::parse(rest)?)),
            "tcp" => Ok(Self::Tcp(rest.parse().with_context(|| {
                format!("grant {raw:?}: {rest:?} is not an `<ip>:<port>` address")
            })?)),
            other => bail!("unknown grant family {other:?} (expected `http`, `tcp` or `ctl`)"),
        }
    }
}

impl fmt::Display for Grant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(scope) => write!(f, "http:{scope}"),
            Self::Tcp(addr) => write!(f, "tcp:{addr}"),
            Self::Ctl(scope) => write!(f, "ctl:{scope}"),
        }
    }
}

impl TryFrom<String> for Grant {
    type Error = anyhow::Error;

    fn try_from(raw: String) -> anyhow::Result<Self> {
        raw.parse()
    }
}

impl From<Grant> for String {
    fn from(grant: Grant) -> Self {
        grant.to_string()
    }
}

// ---------------------------------------------------------------------------------------
// B2 — the stored credential lives in `crate::auth`
// ---------------------------------------------------------------------------------------
//
// The verifier a node's requests must satisfy is [`crate::auth::Credential`], not a type of
// our own. A registry entry only *holds* one; the module that parses `Authorization` headers
// is the module that should own what it compares against, and one implementation cannot drift
// from the other on the two things that matter here — that the digest is salted, and that the
// comparison is constant time in both halves.

// ---------------------------------------------------------------------------------------
// B2 — the record and the registry
// ---------------------------------------------------------------------------------------

/// Everything this machine knows about one paired node.
///
/// The naming half (`key`, `nickname`) says how we *reach* it; the policy half (`grants`,
/// `auth`) says what it may reach *here*. Both live in one record because both are keyed by
/// the same thing — the peer's [`EndpointId`] — and pairing establishes them together.
///
/// Field order matters: TOML requires every plain value before any sub-table, so `auth` is
/// last.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeRecord {
    /// The node's [`EndpointId`], string form — stored as text for the same reason
    /// [`authorized_peers`](crate::config::HostConfig::authorized_peers) is: the file parses
    /// and tests standalone, without pulling iroh into the config layer.
    pub key: String,
    /// The nickname this machine has **acknowledged** for the node.
    pub nickname: String,
    /// Unix seconds at which the petname was bound to the key.
    pub paired_at: u64,
    /// A newer nickname the node declared that has *not* been acknowledged. Purely a
    /// notification: it never moves the petname. See [`FleetRegistry::accept_nickname`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_nickname: Option<String>,
    /// What this peer may reach here. **Empty denies everything.**
    pub grants: Vec<Grant>,
    /// The Basic-auth credential this node's requests must carry.
    pub auth: Credential,
}

impl NodeRecord {
    /// The node's key, or `None` if the stored string does not parse. A record whose key is
    /// unreadable can never be authorized — the same fail-closed choice `host.rs` makes for an
    /// unparseable `authorized_peers` entry.
    #[must_use]
    pub fn endpoint_id(&self) -> Option<EndpointId> {
        self.key.parse().ok()
    }

    /// May this node reach `target`?
    ///
    /// **Default-deny**: with no grants this is always `false`. There is no "empty means
    /// everything" case, unlike [`HostConfig`](crate::config::HostConfig)'s peer list.
    #[must_use]
    pub fn allows(&self, target: Target<'_>) -> bool {
        self.grants.iter().any(|grant| grant.allows(target))
    }

    /// Add a grant; `false` if it was already held.
    pub fn grant(&mut self, grant: Grant) -> bool {
        if self.grants.contains(&grant) {
            return false;
        }
        self.grants.push(grant);
        true
    }

    /// Drop a grant; `true` if it was held.
    pub fn revoke(&mut self, grant: &Grant) -> bool {
        let before = self.grants.len();
        self.grants.retain(|held| held != grant);
        self.grants.len() != before
    }

    /// Replace this node's Basic-auth credential with a freshly salted one.
    pub fn set_password(&mut self, username: &str, password: &str) {
        self.auth = Credential::from_password(username, password);
    }

    /// Constant-time check of a `username`/`password` pair against this node's credential.
    /// The header-parsing half of Basic auth lives in [`crate::auth`]; this is the only place
    /// that touches the stored digest.
    #[must_use]
    pub fn verify_password(&self, username: &str, password: &str) -> bool {
        self.auth.verify(username, password)
    }
}

/// Why an offered nickname could not become the petname.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameProblem {
    /// A *different* key already goes by that name here.
    Taken,
    /// Not one lowercase DNS label (see [`valid_name`]).
    Invalid,
}

impl fmt::Display for NameProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Taken => f.write_str("already taken by another node"),
            Self::Invalid => f.write_str("not a valid node name"),
        }
    }
}

/// The outcome of a pairing attempt.
///
/// Deliberately an outcome and not a `Result`: a cosmetic name clash must never be reported as
/// a failure, because a caller holding an `Err` at handshake time will refuse the connection —
/// exactly the bug §2 rule 3 names.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum Pairing {
    /// A new key, filed under this petname. The registry changed.
    Paired {
        /// The petname the node now answers to on this machine.
        petname: String,
    },
    /// This key was already paired. Its petname is pinned and unchanged.
    AlreadyPaired {
        /// The petname bound at the original pairing.
        petname: String,
        /// Set when the node declared a *different* nickname this time: recorded as pending,
        /// never applied. See [`FleetRegistry::accept_nickname`].
        nickname_change: Option<String>,
    },
    /// The offered nickname is unusable. **Nothing was written** — ask the operator to confirm
    /// `suggestion` (or supply their own) and call [`FleetRegistry::pair_as`].
    NeedsPetname {
        /// The nickname the node offered.
        offered: String,
        /// A free, valid name to propose.
        suggestion: String,
        /// What was wrong with `offered`.
        reason: NameProblem,
    },
}

impl Pairing {
    /// The petname the node ended up with, or `None` when the operator still has to choose.
    #[must_use]
    pub fn petname(&self) -> Option<&str> {
        match self {
            Self::Paired { petname } | Self::AlreadyPaired { petname, .. } => Some(petname),
            Self::NeedsPetname { .. } => None,
        }
    }
}

/// One node's unacknowledged rename — "the node you call `petname` now calls itself
/// `declared`".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NicknameChange {
    /// What this machine calls the node (unchanged, and it stays that way until accepted).
    pub petname: String,
    /// The nickname this machine had acknowledged.
    pub acknowledged: String,
    /// The nickname the node now declares.
    pub declared: String,
}

impl fmt::Display for NicknameChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            petname,
            acknowledged,
            declared,
        } = self;
        write!(
            f,
            "node `{petname}` (known as `{acknowledged}`) now calls itself `{declared}`"
        )
    }
}

/// The whole `fleet.toml`: every node this machine has paired with, keyed by petname.
///
/// A [`BTreeMap`] so the saved file has a stable, diffable order and so a petname lookup — the
/// gateway's per-request operation — is a map hit rather than a scan.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FleetRegistry {
    /// Paired nodes by petname.
    pub nodes: BTreeMap<String, NodeRecord>,
}

impl FleetRegistry {
    // -- store ---------------------------------------------------------------------------

    /// Load the registry from the standard store, materializing an empty `fleet.toml` on first
    /// use so the operator has a file to edit.
    ///
    /// # Errors
    /// Any I/O or TOML error from the underlying store, including a grant that does not parse.
    pub fn load() -> anyhow::Result<Self> {
        Self::load_from(&Config::open())
    }

    /// Persist the registry to the standard store, atomically.
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
        store.module(crate::config::MODULE).file(FLEET_FILE)
    }

    // -- B5: lookup ----------------------------------------------------------------------

    /// The record filed under `petname`.
    #[must_use]
    pub fn get(&self, petname: &str) -> Option<&NodeRecord> {
        self.nodes.get(petname)
    }

    /// The record filed under `petname`, mutably — to edit grants or set a password.
    pub fn get_mut(&mut self, petname: &str) -> Option<&mut NodeRecord> {
        self.nodes.get_mut(petname)
    }

    /// Petname → key. `None` if the name is unknown or its stored key does not parse.
    #[must_use]
    pub fn key_of(&self, petname: &str) -> Option<EndpointId> {
        self.nodes.get(petname)?.endpoint_id()
    }

    /// Key → petname.
    #[must_use]
    pub fn petname_of(&self, key: &EndpointId) -> Option<&str> {
        self.petname_for_id(&key.to_string())
    }

    /// Key → `(petname, record)`. The lookup the node side does on an inbound connection: the
    /// peer's key is the only thing it can trust, and the record carries both the grants and
    /// the credential it needs.
    #[must_use]
    pub fn by_key(&self, key: &EndpointId) -> Option<(&str, &NodeRecord)> {
        let id = key.to_string();
        self.nodes
            .iter()
            .find(|(_, record)| record.key == id)
            .map(|(petname, record)| (petname.as_str(), record))
    }

    /// Every petname, in sorted order.
    pub fn petnames(&self) -> impl Iterator<Item = &str> {
        self.nodes.keys().map(String::as_str)
    }

    /// How many nodes are paired.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Is nothing paired yet?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    fn petname_for_id(&self, id: &str) -> Option<&str> {
        self.nodes
            .iter()
            .find(|(_, record)| record.key == id)
            .map(|(petname, _)| petname.as_str())
    }

    // -- B3: pairing ---------------------------------------------------------------------

    /// A free, valid name derived from `offered` — `offered` itself when it is usable,
    /// otherwise `<base>-2`, `-3`, … until one is free.
    #[must_use]
    pub fn suggest(&self, offered: &str) -> String {
        let base = sanitize_name(offered);
        if !self.nodes.contains_key(&base) {
            return base;
        }
        let mut n = 2u32;
        loop {
            let candidate = numbered(&base, n);
            if !self.nodes.contains_key(&candidate) {
                return candidate;
            }
            n += 1;
        }
    }

    /// Pair `key` under the nickname it offers.
    ///
    /// * key already paired → [`Pairing::AlreadyPaired`]; the petname is pinned, and a changed
    ///   nickname is filed as pending (B4).
    /// * nickname free and valid → it becomes the petname ([`Pairing::Paired`]).
    /// * nickname taken by another key, or not a valid label → [`Pairing::NeedsPetname`] with a
    ///   suggestion. Nothing is written, and this is **not** an error: the caller confirms a
    ///   name and calls [`pair_as`](Self::pair_as). A clash must never refuse the connection.
    pub fn pair(&mut self, key: &EndpointId, nickname: &str) -> Pairing {
        let id = key.to_string();
        if let Some(petname) = self.petname_for_id(&id).map(ToString::to_string) {
            let nickname_change = self.note_nickname(&petname, nickname);
            return Pairing::AlreadyPaired {
                petname,
                nickname_change,
            };
        }

        let reason = if valid_name(nickname) {
            if self.nodes.contains_key(nickname) {
                NameProblem::Taken
            } else {
                self.insert(nickname, id, nickname);
                return Pairing::Paired {
                    petname: nickname.to_string(),
                };
            }
        } else {
            NameProblem::Invalid
        };

        Pairing::NeedsPetname {
            offered: nickname.to_string(),
            suggestion: self.suggest(nickname),
            reason,
        }
    }

    /// Pair `key` under a petname the operator chose — the answer to
    /// [`Pairing::NeedsPetname`]. `nickname` is recorded as what the node calls itself.
    ///
    /// An already-paired key yields [`Pairing::AlreadyPaired`] and is *not* moved: re-pointing
    /// a pinned petname is [`rename`](Self::rename)'s job, never a pairing side effect.
    ///
    /// # Errors
    /// If `petname` is not a valid label, or already names a different node.
    pub fn pair_as(
        &mut self,
        petname: &str,
        key: &EndpointId,
        nickname: &str,
    ) -> anyhow::Result<Pairing> {
        let id = key.to_string();
        if let Some(existing) = self.petname_for_id(&id).map(ToString::to_string) {
            let nickname_change = self.note_nickname(&existing, nickname);
            return Ok(Pairing::AlreadyPaired {
                petname: existing,
                nickname_change,
            });
        }
        ensure!(
            valid_name(petname),
            "{petname:?} is not a valid node name (one lowercase DNS label, 1..=63 bytes)"
        );
        ensure!(
            !self.nodes.contains_key(petname),
            "the name {petname:?} already belongs to another node here"
        );
        self.insert(petname, id, nickname);
        Ok(Pairing::Paired {
            petname: petname.to_string(),
        })
    }

    fn insert(&mut self, petname: &str, key: String, nickname: &str) {
        self.nodes.insert(
            petname.to_string(),
            NodeRecord {
                key,
                nickname: nickname.to_string(),
                paired_at: adi_config::now_unix(),
                pending_nickname: None,
                grants: Vec::new(),
                auth: Credential::default(),
            },
        );
    }

    // -- B4: pinning ---------------------------------------------------------------------

    /// File `declared` against `petname` if it differs from the acknowledged nickname,
    /// returning it. Re-declaring the acknowledged nickname clears a stale pending change.
    fn note_nickname(&mut self, petname: &str, declared: &str) -> Option<String> {
        let record = self.nodes.get_mut(petname)?;
        if declared.is_empty() {
            return None;
        }
        if declared == record.nickname {
            record.pending_nickname = None;
            return None;
        }
        record.pending_nickname = Some(declared.to_string());
        Some(declared.to_string())
    }

    /// Every node that now calls itself something other than what we acknowledged.
    ///
    /// This is the whole visible effect of a node renaming itself: a line the operator can act
    /// on, and no change to any URL until they do.
    #[must_use]
    pub fn pending_nicknames(&self) -> Vec<NicknameChange> {
        self.nodes
            .iter()
            .filter_map(|(petname, record)| {
                record
                    .pending_nickname
                    .as_ref()
                    .map(|declared| NicknameChange {
                        petname: petname.clone(),
                        acknowledged: record.nickname.clone(),
                        declared: declared.clone(),
                    })
            })
            .collect()
    }

    /// Adopt a node's declared nickname: acknowledge it **and** move the petname to it.
    /// Returns the new petname.
    ///
    /// This is the only path by which a node's own declaration can change a petname, and it
    /// runs the same checks as a local [`rename`](Self::rename) — so a node still cannot land
    /// on a name another node holds, even with the operator's blessing.
    ///
    /// # Errors
    /// If `petname` is unknown, has no pending change, or the declared nickname is not a valid
    /// label or already names a different node.
    pub fn accept_nickname(&mut self, petname: &str) -> anyhow::Result<String> {
        let declared = self
            .nodes
            .get(petname)
            .with_context(|| format!("no node is named {petname:?} here"))?
            .pending_nickname
            .clone()
            .with_context(|| format!("node {petname:?} has not declared a new nickname"))?;
        ensure!(
            valid_name(&declared),
            "the declared nickname {declared:?} is not a valid node name"
        );
        ensure!(
            declared == petname || !self.nodes.contains_key(&declared),
            "cannot adopt {declared:?}: another node already goes by that name here"
        );

        let Some(mut record) = self.nodes.remove(petname) else {
            bail!("no node is named {petname:?} here");
        };
        record.nickname.clone_from(&declared);
        record.pending_nickname = None;
        self.nodes.insert(declared.clone(), record);
        Ok(declared)
    }

    /// Acknowledge the declared nickname **without** moving the petname — "yes, it calls itself
    /// that now; here it stays `<petname>`". Stops the change being reported again.
    ///
    /// Returns the acknowledged nickname, or `None` if there was nothing pending.
    pub fn dismiss_nickname(&mut self, petname: &str) -> Option<String> {
        let record = self.nodes.get_mut(petname)?;
        let declared = record.pending_nickname.take()?;
        record.nickname.clone_from(&declared);
        Some(declared)
    }

    // -- B5: rename and unpair -----------------------------------------------------------

    /// Rename a node locally, without the far side's involvement — the escape hatch for
    /// belonging to two fleets that both use `main` (§2 rule 5).
    ///
    /// # Errors
    /// If `to` is not a valid label, `from` is unknown, or `to` already names a different node.
    pub fn rename(&mut self, from: &str, to: &str) -> anyhow::Result<()> {
        ensure!(
            valid_name(to),
            "{to:?} is not a valid node name (one lowercase DNS label, 1..=63 bytes)"
        );
        ensure!(
            self.nodes.contains_key(from),
            "no node is named {from:?} here"
        );
        if from == to {
            return Ok(());
        }
        ensure!(
            !self.nodes.contains_key(to),
            "the name {to:?} already belongs to another node here"
        );
        let Some(record) = self.nodes.remove(from) else {
            bail!("no node is named {from:?} here");
        };
        self.nodes.insert(to.to_string(), record);
        Ok(())
    }

    /// Unpair a node, returning the record that was dropped.
    pub fn unpair(&mut self, petname: &str) -> Option<NodeRecord> {
        self.nodes.remove(petname)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn some_key() -> EndpointId {
        iroh::SecretKey::generate().public()
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "adi-mesh-fleet-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ))
    }

    /// Pair a key and return the petname it landed on, failing the test on any other outcome.
    fn pair_ok(registry: &mut FleetRegistry, key: &EndpointId, nickname: &str) -> String {
        match registry.pair(key, nickname) {
            Pairing::Paired { petname } => petname,
            other => panic!("expected a fresh pairing, got {other:?}"),
        }
    }

    // -- B1 ------------------------------------------------------------------------------

    #[test]
    fn valid_name_accepts_one_lowercase_label() {
        for name in ["a", "1", "laptop-b", "node-2", "a-b-c-9", &"x".repeat(63)] {
            assert!(valid_name(name), "{name:?} should be valid");
        }
    }

    #[test]
    fn valid_name_rejects_case_edges_dots_and_over_long() {
        for name in [
            "",
            "-a",
            "a-",
            "-",
            "Laptop",
            "laptop.b",
            "lap top",
            "laptop_b",
            "node/1",
            &"x".repeat(64),
        ] {
            assert!(!valid_name(name), "{name:?} should be rejected");
        }
    }

    #[test]
    fn sanitize_coerces_anything_into_a_label() {
        assert_eq!(sanitize_name("Laptop.B"), "laptop-b");
        assert_eq!(sanitize_name("  mac mini  "), "mac-mini");
        assert_eq!(sanitize_name("***"), FALLBACK_NAME);
        assert_eq!(sanitize_name("--a--"), "a");
        assert!(valid_name(&sanitize_name(&"x".repeat(200))));
        assert_eq!(sanitize_name(&"x".repeat(200)).len(), MAX_LABEL);
    }

    #[test]
    fn numbered_keeps_the_result_inside_one_label() {
        assert_eq!(numbered("laptop", 2), "laptop-2");
        let long = numbered(&"x".repeat(63), 12);
        assert!(valid_name(&long), "{long:?}");
        assert!(long.ends_with("-12"));
    }

    // -- B2 ------------------------------------------------------------------------------

    #[test]
    fn parses_a_full_fleet_toml() {
        let registry: FleetRegistry = toml::from_str(
            r#"
[nodes.laptop-b]
key = "abc123"
nickname = "laptop-b"
paired_at = 1700000000
grants = ["http:*", "tcp:127.0.0.1:22", "ctl:read"]

[nodes.laptop-b.auth]
user = "igor"
salt = "c2FsdA=="
digest = "aGFzaA=="

[nodes.desk]
key = "def456"
nickname = "desk"
paired_at = 1700000001
pending_nickname = "main"
grants = []
"#,
        )
        .expect("parses");

        let laptop = registry.get("laptop-b").expect("laptop-b");
        assert_eq!(laptop.key, "abc123");
        assert_eq!(laptop.paired_at, 1_700_000_000);
        assert_eq!(
            laptop.grants,
            vec![
                Grant::Http(Scope::Any),
                Grant::Tcp("127.0.0.1:22".parse().unwrap()),
                Grant::Ctl(Scope::One("read".into())),
            ]
        );
        assert_eq!(laptop.auth.user, "igor");
        assert!(laptop.pending_nickname.is_none());

        let desk = registry.get("desk").expect("desk");
        assert_eq!(desk.pending_nickname.as_deref(), Some("main"));
        assert!(desk.grants.is_empty());
        assert!(!desk.auth.is_set(), "an absent [auth] table is unset");
    }

    #[test]
    fn empty_toml_is_an_empty_registry() {
        let registry: FleetRegistry = toml::from_str("").expect("empty parses");
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(registry.petnames().next().is_none());
    }

    #[test]
    fn an_unparseable_grant_fails_the_load_loudly() {
        let err = toml::from_str::<FleetRegistry>(
            r#"
[nodes.a]
key = "k"
grants = ["ftp:*"]
"#,
        )
        .expect_err("unknown family");
        assert!(err.to_string().contains("ftp"), "{err}");
    }

    #[test]
    fn registry_round_trips_through_the_store() {
        let dir = scratch("roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        let store = Config::with_root(&dir);

        // First load materializes an empty file rather than failing.
        let mut registry = FleetRegistry::load_from(&store).expect("first load");
        assert!(registry.is_empty());

        let key = some_key();
        let petname = pair_ok(&mut registry, &key, "laptop-b");
        let record = registry.get_mut(&petname).expect("record");
        record.grant(Grant::Http(Scope::One("nosh".into())));
        record.set_password("igor", "hunter2");
        registry.save_to(&store).expect("save");

        // The file reads the way §2/§5 describe it: a table per petname, grants in their
        // string form, the credential in its own sub-table.
        let rendered = toml::to_string_pretty(&registry).expect("render");
        for expected in ["[nodes.laptop-b]", "\"http:nosh\"", "[nodes.laptop-b.auth]"] {
            assert!(rendered.contains(expected), "missing {expected}:\n{rendered}");
        }
        assert!(
            !rendered.contains("pending_nickname"),
            "no pending change, so no key:\n{rendered}"
        );

        let reloaded = FleetRegistry::load_from(&store).expect("reload");
        let record = reloaded.get("laptop-b").expect("laptop-b survived");
        assert_eq!(record.endpoint_id(), Some(key));
        assert_eq!(record.grants, vec![Grant::Http(Scope::One("nosh".into()))]);
        assert!(record.verify_password("igor", "hunter2"));
        assert!(record.paired_at > 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- B3 ------------------------------------------------------------------------------

    #[test]
    fn a_free_nickname_becomes_the_petname() {
        let mut registry = FleetRegistry::default();
        let key = some_key();
        assert_eq!(
            registry.pair(&key, "laptop-b"),
            Pairing::Paired {
                petname: "laptop-b".to_string()
            }
        );
        assert_eq!(
            registry.get("laptop-b").expect("stored").nickname,
            "laptop-b"
        );
    }

    #[test]
    fn a_name_collision_suggests_instead_of_failing() {
        let mut registry = FleetRegistry::default();
        pair_ok(&mut registry, &some_key(), "main");

        let intruder = some_key();
        let outcome = registry.pair(&intruder, "main");
        assert_eq!(
            outcome,
            Pairing::NeedsPetname {
                offered: "main".to_string(),
                suggestion: "main-2".to_string(),
                reason: NameProblem::Taken,
            }
        );
        assert_eq!(registry.len(), 1, "a clash writes nothing");
        assert!(
            registry.petname_of(&intruder).is_none(),
            "and pairs nobody — but it is not an error either"
        );

        // The operator confirms the suggestion.
        let confirmed = registry
            .pair_as("main-2", &intruder, "main")
            .expect("confirmed name");
        assert_eq!(confirmed.petname(), Some("main-2"));
        assert_eq!(registry.petname_of(&intruder), Some("main-2"));

        // A third `main` walks past the taken `main-2`.
        let third = some_key();
        match registry.pair(&third, "main") {
            Pairing::NeedsPetname { suggestion, .. } => assert_eq!(suggestion, "main-3"),
            other => panic!("expected a suggestion, got {other:?}"),
        }
    }

    #[test]
    fn an_invalid_nickname_falls_back_to_a_suggestion() {
        let mut registry = FleetRegistry::default();
        let key = some_key();
        assert_eq!(
            registry.pair(&key, "Laptop.B"),
            Pairing::NeedsPetname {
                offered: "Laptop.B".to_string(),
                suggestion: "laptop-b".to_string(),
                reason: NameProblem::Invalid,
            }
        );
        assert!(registry.is_empty());

        // Even a nickname with nothing usable in it yields a name, never an error.
        match registry.pair(&key, "***") {
            Pairing::NeedsPetname { suggestion, .. } => assert_eq!(suggestion, FALLBACK_NAME),
            other => panic!("expected a suggestion, got {other:?}"),
        }
    }

    #[test]
    fn pair_as_refuses_an_invalid_or_taken_petname() {
        let mut registry = FleetRegistry::default();
        pair_ok(&mut registry, &some_key(), "main");

        let key = some_key();
        assert!(registry.pair_as("Main", &key, "main").is_err(), "invalid");
        assert!(registry.pair_as("main", &key, "main").is_err(), "taken");
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn re_pairing_the_same_key_keeps_the_petname() {
        let mut registry = FleetRegistry::default();
        let key = some_key();
        pair_ok(&mut registry, &key, "laptop-b");
        registry
            .get_mut("laptop-b")
            .expect("record")
            .grant(Grant::Http(Scope::Any));

        assert_eq!(
            registry.pair(&key, "laptop-b"),
            Pairing::AlreadyPaired {
                petname: "laptop-b".to_string(),
                nickname_change: None,
            }
        );
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry.get("laptop-b").expect("record").grants,
            vec![Grant::Http(Scope::Any)],
            "a re-pair keeps the node's grants"
        );

        // Even an explicitly chosen petname does not move a pinned one.
        let outcome = registry
            .pair_as("something-else", &key, "laptop-b")
            .expect("re-pair");
        assert_eq!(outcome.petname(), Some("laptop-b"));
        assert!(registry.get("something-else").is_none());
    }

    // -- B4 ------------------------------------------------------------------------------

    #[test]
    fn a_declared_rename_is_reported_and_never_repoints_the_petname() {
        let mut registry = FleetRegistry::default();
        let key = some_key();
        pair_ok(&mut registry, &key, "laptop-b");

        assert_eq!(
            registry.pair(&key, "desk"),
            Pairing::AlreadyPaired {
                petname: "laptop-b".to_string(),
                nickname_change: Some("desk".to_string()),
            }
        );
        assert_eq!(registry.petname_of(&key), Some("laptop-b"));
        assert!(registry.get("desk").is_none(), "the petname did not move");
        assert_eq!(
            registry.pending_nicknames(),
            vec![NicknameChange {
                petname: "laptop-b".to_string(),
                acknowledged: "laptop-b".to_string(),
                declared: "desk".to_string(),
            }]
        );
        assert!(
            registry.pending_nicknames()[0]
                .to_string()
                .contains("now calls itself `desk`")
        );

        // Declaring the acknowledged name again clears the notification.
        assert_eq!(
            registry.pair(&key, "laptop-b").petname(),
            Some("laptop-b")
        );
        assert!(registry.pending_nicknames().is_empty());
    }

    #[test]
    fn a_node_cannot_take_over_another_nodes_petname_by_renaming() {
        let mut registry = FleetRegistry::default();
        let honest = some_key();
        let attacker = some_key();
        pair_ok(&mut registry, &honest, "main");
        pair_ok(&mut registry, &attacker, "laptop-b");

        // The attacker re-declares itself as `main`.
        assert_eq!(
            registry.pair(&attacker, "main").petname(),
            Some("laptop-b"),
            "a re-declaration is still an already-paired node"
        );
        assert_eq!(
            registry.key_of("main"),
            Some(honest),
            "`main` still resolves to the honest node"
        );
        assert_eq!(registry.petname_of(&attacker), Some("laptop-b"));

        // And even the operator accepting the change cannot land it on a taken name.
        let err = registry
            .accept_nickname("laptop-b")
            .expect_err("must not steal `main`");
        assert!(
            err.to_string().contains("already goes by that name"),
            "{err}"
        );
        assert_eq!(registry.key_of("main"), Some(honest));
    }

    #[test]
    fn accepting_a_declared_nickname_moves_the_petname() {
        let mut registry = FleetRegistry::default();
        let key = some_key();
        pair_ok(&mut registry, &key, "laptop-b");
        let _ = registry.pair(&key, "desk");

        assert_eq!(registry.accept_nickname("laptop-b").expect("accept"), "desk");
        assert_eq!(registry.petname_of(&key), Some("desk"));
        assert_eq!(registry.get("desk").expect("record").nickname, "desk");
        assert!(registry.pending_nicknames().is_empty());
        assert!(registry.get("laptop-b").is_none());

        assert!(
            registry.accept_nickname("desk").is_err(),
            "nothing pending any more"
        );
        assert!(registry.accept_nickname("nobody").is_err(), "unknown node");
    }

    #[test]
    fn dismissing_a_declared_nickname_keeps_the_petname() {
        let mut registry = FleetRegistry::default();
        let key = some_key();
        pair_ok(&mut registry, &key, "laptop-b");
        let _ = registry.pair(&key, "desk");

        assert_eq!(
            registry.dismiss_nickname("laptop-b").as_deref(),
            Some("desk")
        );
        assert_eq!(registry.petname_of(&key), Some("laptop-b"));
        assert_eq!(
            registry.get("laptop-b").expect("record").nickname,
            "desk",
            "we acknowledge what it calls itself, we just don't use it"
        );
        assert!(registry.pending_nicknames().is_empty());
        assert!(registry.dismiss_nickname("laptop-b").is_none());

        // A later re-pair under the acknowledged nickname is not a change any more.
        assert_eq!(
            registry.pair(&key, "desk"),
            Pairing::AlreadyPaired {
                petname: "laptop-b".to_string(),
                nickname_change: None,
            }
        );
    }

    // -- B5 ------------------------------------------------------------------------------

    #[test]
    fn lookup_resolves_petname_to_key_and_back() {
        let mut registry = FleetRegistry::default();
        let key = some_key();
        let other = some_key();
        pair_ok(&mut registry, &key, "laptop-b");

        assert_eq!(registry.key_of("laptop-b"), Some(key));
        assert_eq!(registry.key_of("nobody"), None);
        assert_eq!(registry.petname_of(&key), Some("laptop-b"));
        assert_eq!(registry.petname_of(&other), None);

        let (petname, record) = registry.by_key(&key).expect("by key");
        assert_eq!(petname, "laptop-b");
        assert_eq!(record.endpoint_id(), Some(key));
        assert!(registry.by_key(&other).is_none());

        assert_eq!(registry.petnames().collect::<Vec<_>>(), vec!["laptop-b"]);
    }

    #[test]
    fn rename_moves_a_node_and_refuses_a_taken_name() {
        let mut registry = FleetRegistry::default();
        let key = some_key();
        let other = some_key();
        pair_ok(&mut registry, &key, "main");
        pair_ok(&mut registry, &other, "laptop-b");

        assert!(registry.rename("main", "Main").is_err(), "invalid label");
        assert!(
            registry.rename("main", "laptop-b").is_err(),
            "taken by another key"
        );
        assert!(registry.rename("nobody", "somebody").is_err(), "unknown");
        assert_eq!(
            registry.key_of("main"),
            Some(key),
            "a refusal changes nothing"
        );

        registry.rename("main", "main").expect("renaming to itself");
        registry.rename("main", "work").expect("rename");
        assert_eq!(registry.key_of("work"), Some(key));
        assert!(registry.get("main").is_none());
        assert_eq!(registry.petname_of(&key), Some("work"));
    }

    #[test]
    fn unpair_removes_the_node() {
        let mut registry = FleetRegistry::default();
        let key = some_key();
        pair_ok(&mut registry, &key, "laptop-b");

        let dropped = registry.unpair("laptop-b").expect("removed");
        assert_eq!(dropped.endpoint_id(), Some(key));
        assert!(registry.is_empty());
        assert!(registry.unpair("laptop-b").is_none());

        // Unpaired, the same key pairs afresh under its nickname.
        assert_eq!(pair_ok(&mut registry, &key, "laptop-b"), "laptop-b");
    }

    // -- B6 ------------------------------------------------------------------------------

    #[test]
    fn no_grants_denies_everything() {
        let record = NodeRecord::default();
        assert!(!record.allows(Target::Http("nosh")));
        assert!(!record.allows(Target::Http("app")));
        assert!(!record.allows(Target::Tcp("127.0.0.1:22".parse().unwrap())));
        assert!(!record.allows(Target::Ctl("read")));
    }

    #[test]
    fn http_wildcard_and_exact_service_grants() {
        let mut exact = NodeRecord::default();
        assert!(exact.grant(Grant::Http(Scope::One("nosh".into()))));
        assert!(!exact.grant(Grant::Http(Scope::One("nosh".into()))), "dedup");
        assert!(exact.allows(Target::Http("nosh")));
        assert!(!exact.allows(Target::Http("app")));
        assert!(!exact.allows(Target::Ctl("nosh")), "families do not cross");
        assert!(!exact.allows(Target::Tcp("127.0.0.1:22".parse().unwrap())));

        let mut any = NodeRecord::default();
        any.grant(Grant::Http(Scope::Any));
        assert!(any.allows(Target::Http("nosh")));
        assert!(any.allows(Target::Http("app")));
        assert!(!any.allows(Target::Ctl("read")));

        assert!(any.revoke(&Grant::Http(Scope::Any)));
        assert!(!any.revoke(&Grant::Http(Scope::Any)));
        assert!(!any.allows(Target::Http("nosh")), "revoked is denied again");
    }

    #[test]
    fn tcp_grant_matches_only_its_exact_address() {
        let mut record = NodeRecord::default();
        record.grant(Grant::Tcp("127.0.0.1:22".parse().unwrap()));
        assert!(record.allows(Target::Tcp("127.0.0.1:22".parse().unwrap())));
        assert!(!record.allows(Target::Tcp("127.0.0.1:23".parse().unwrap())));
        assert!(!record.allows(Target::Tcp("10.0.0.1:22".parse().unwrap())));
        assert!(!record.allows(Target::Http("nosh")));
    }

    #[test]
    fn ctl_grants_scope_the_control_plane() {
        let mut record = NodeRecord::default();
        record.grant(Grant::Ctl(Scope::One("read".into())));
        assert!(record.allows(Target::Ctl("read")));
        assert!(!record.allows(Target::Ctl("write")));

        record.grant(Grant::Ctl(Scope::Any));
        assert!(record.allows(Target::Ctl("write")));
        assert!(!record.allows(Target::Http("app")));
    }

    #[test]
    fn grants_round_trip_through_their_string_form() {
        for raw in [
            "http:*",
            "http:nosh",
            // A service is a *name*, so a node's `app.nosh.adi` is grantable as itself.
            "http:app.nosh",
            "tcp:127.0.0.1:22",
            "tcp:[::1]:5432",
            "ctl:read",
            "ctl:*",
        ] {
            let grant: Grant = raw.parse().expect(raw);
            assert_eq!(grant.to_string(), raw);
        }

        for raw in [
            "http",
            "",
            "http:",
            "http:NOSH",
            "http:app..nosh",
            "http:.nosh",
            // The control plane's scopes are single words, not names.
            "ctl:read.write",
            "ftp:*",
            "tcp:127.0.0.1",
            "tcp:*",
        ] {
            assert!(raw.parse::<Grant>().is_err(), "{raw:?} should not parse");
        }
    }

    #[test]
    fn a_deep_service_grant_covers_only_that_name() {
        let mut record = NodeRecord::default();
        record.grant("http:app.nosh".parse().expect("a valid grant"));
        assert!(record.allows(Target::Http("app.nosh")));
        // Not its parent, and not a sibling: `nosh.adi` and `app.nosh.adi` are two services.
        assert!(!record.allows(Target::Http("nosh")));
        assert!(!record.allows(Target::Http("app")));
    }

    // -- B2/B6: the stored credential ----------------------------------------------------

    #[test]
    fn password_verify_accepts_only_the_right_credential() {
        let mut record = NodeRecord::default();
        record.set_password("igor", "hunter2");

        assert!(record.verify_password("igor", "hunter2"));
        assert!(!record.verify_password("igor", "hunter3"), "wrong password");
        assert!(!record.verify_password("igor", "hunter"), "prefix only");
        assert!(!record.verify_password("igor", ""), "empty password");
        assert!(!record.verify_password("root", "hunter2"), "wrong username");

        assert!(!record.auth.digest.contains("hunter2"), "no plaintext at rest");
        assert!(record.auth.is_set());
    }

    #[test]
    fn an_unset_or_corrupt_credential_denies() {
        let record = NodeRecord::default();
        assert!(!record.auth.is_set());
        assert!(!record.verify_password("igor", "hunter2"));
        assert!(!record.verify_password("", ""), "empty is not a password");

        let corrupt = Credential {
            user: "igor".to_string(),
            salt: "not base64!!".to_string(),
            digest: "also not base64!!".to_string(),
        };
        assert!(!corrupt.verify("igor", "hunter2"));
    }

    #[test]
    fn each_credential_gets_its_own_salt() {
        let a = Credential::from_password("igor", "hunter2");
        let b = Credential::from_password("igor", "hunter2");
        assert_ne!(a.salt, b.salt, "salts are per credential");
        assert_ne!(a.digest, b.digest, "so identical passwords store differently");
        assert!(a.verify("igor", "hunter2") && b.verify("igor", "hunter2"));
    }
}
