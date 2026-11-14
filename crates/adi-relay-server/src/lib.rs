//! The webhook relay service (ADI-MONO-222): a small self-hostable binary answering
//! unauthenticated sender webhooks at `/h/<endpoint-id>` and a signed, long-polled pull API for
//! the one node that owns them. One binary, one `relay.toml`, three modes differing only in
//! whose key is accepted.
//!
//! Every header map in this crate is the one concrete `HashMap<String, String>` [`http::parse`]
//! builds — there's no caller who'd ever hand in a different hasher — so `clippy::implicit_hasher`
//! is noise here, not a real API gap.
#![allow(clippy::implicit_hasher)]

pub mod auth;
pub mod cidr;
pub mod config;
pub mod http;
pub mod licence;
pub mod routes;
pub mod store;
pub mod sweep;

use std::sync::Arc;

use auth::AuthorizedKeys;
use config::Config;
use licence::LicenceClient;
use store::Store;

/// Everything a route handler reads: the store, the resolved config, and the two optional
/// mode-specific gates — present only in the mode that needs them, so a route never has to ask
/// "which mode am I" to know whether to consult one.
#[derive(Debug)]
pub struct AppState {
    pub store: Store,
    pub config: Config,
    pub licence: Option<LicenceClient>,
    pub authorized_keys: Option<AuthorizedKeys>,
    /// Woken on every accepted ingress so a long-polling `pull` notices new data without
    /// sleeping through its own window.
    pub notify: tokio::sync::Notify,
}

impl AppState {
    /// Build the state one running instance needs from its resolved [`Config`] — opens the
    /// store at `config.db_path` and wires up the licence client / `authorized_keys` watcher
    /// the config's own mode calls for.
    ///
    /// # Errors
    /// Any `rusqlite` error opening the store, or a licence client the config names but can't
    /// build (a malformed `public_key`).
    pub fn open(config: Config) -> anyhow::Result<Self> {
        let store = Store::open(&config.db_path)?;
        let licence = config
            .licence
            .as_ref()
            .map(|l| LicenceClient::new(l.service_url.clone(), &l.public_key))
            .transpose()?;
        let authorized_keys = config
            .self_host
            .as_ref()
            .map(|sh| AuthorizedKeys::new(sh.authorized_keys_path.clone()));
        Ok(Self {
            store,
            config,
            licence,
            authorized_keys,
            notify: tokio::sync::Notify::new(),
        })
    }
}

pub type SharedState = Arc<AppState>;

/// An HTTP response: a status paired with its JSON body — every route builds one exclusively
/// through [`ok`] or [`err`], except `/healthz`, which is plain text and has no body shape to
/// share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

/// The error body: `{ "error": "<code>", "message": "<detail>" }`, for every non-2xx response.
#[must_use]
pub fn err(status: u16, code: &str, message: impl Into<String>) -> Response {
    let body = serde_json::json!({ "error": code, "message": message.into() });
    Response {
        status,
        body: body.to_string(),
    }
}

/// A success body, serialized compactly; a serialization failure (not expected for any type
/// this crate's routes build) degrades to a 500.
#[must_use]
pub fn ok<T: serde::Serialize>(status: u16, value: &T) -> Response {
    match serde_json::to_string(value) {
        Ok(body) => Response { status, body },
        Err(e) => err(500, "internal_error", e.to_string()),
    }
}

#[must_use]
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// The calendar day `now` (unix seconds, UTC) falls on, as `registrations_by_ip.day` stores it
/// (e.g. `"2026-11-13"`).
#[must_use]
pub fn today(now: u64) -> String {
    let secs = i64::try_from(now).unwrap_or(0);
    chrono::DateTime::from_timestamp(secs, 0)
        .unwrap_or_default()
        .format("%Y-%m-%d")
        .to_string()
}
