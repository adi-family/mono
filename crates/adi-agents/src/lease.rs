//! The lease reference a session carries — `docs/byo-credentials.md` §11.
//!
//! Phase 1 ships this and nothing that acts on it: the reference is computed, written once, and
//! read back, but no turn is actually provisioned with a user's value yet (that is phase 1's own
//! layering item, P1.8, and phases 2–3 beyond it). A session with a lease reference and
//! `[user_secrets] enabled = false` behaves exactly like one with none.

use serde::{Deserialize, Serialize};

/// Whose lease a session's turns look for, and which names they may use from it — resolved once
/// at [`crate::store::SessionStore::create_as`] and never re-derived, the same reason
/// [`crate::RunOverrides`] and a pinned backend chain aren't either: a conversation is answered by
/// whatever *started* it, not by whatever the agent or the server policy says today.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRef {
    /// The minted user id whose lease this is (`docs/byo-credentials.md` §3.2).
    pub user: String,
    /// `attached ∩ overridable` at creation — the agent's `[[secrets]]` names that were also on
    /// the server's `overridable` list the moment this session opened. A later turn narrows this
    /// further (§5: also against the agent's *current* attachments, the server's *current*
    /// `overridable`, and the live lease's shared names) but never widens it — a name absent here
    /// can never later apply to this session.
    pub names: Vec<String>,
}
