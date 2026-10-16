//! adi-usage — first-party usage events for the Individuals funnel (ADI-MONO-157).
//!
//! Started beside `adi_triggers::EventDispatcher` (`adi-app/src/main.rs`), this reads the
//! platform event bus with its own cursor, applies the collector's allowlist
//! (`adi-analytics:dictionary/events.json`) and queues the result to
//! `~/.adi/mono/analytics/queue.jsonl`. A timer flushes the queue to the collector
//! (`POST <relay>/v1/app`) every ten minutes and once more at shutdown.
//!
//! **On by default** (operator decision, 2026-10-16) — [`settings::DEFAULT_ENABLED`] is the one
//! constant that decides. `DO_NOT_TRACK=1`, `ADI_ANALYTICS=0` or the Settings page toggle switch
//! it off. While it (or an env override, or the panel's toggle) says off, [`dispatch::Usage`]
//! still catches up on the bus and acks every event — so switching this on later resumes from
//! "now" rather than replaying a backlog — but queues and sends nothing.
//!
//! ```text
//! bus (adi_events) ──▶ dispatch::Usage ──▶ queue.jsonl ──▶ client::flush ──▶ POST /v1/app
//!                        │                                      ▲
//!                        └─ settings::effective() gates both ───┘
//! ```

mod client;
mod dispatch;
mod queue;
mod settings;
mod state;

pub use client::{DEFAULT_RELAY_URL, FlushOutcome, RELAY_URL_ENV};
pub use dispatch::Usage;
pub use settings::{DEFAULT_ENABLED, DO_NOT_TRACK_ENV, ENV_VAR, Settings, effective, env_override, set_enabled};
pub use state::{State, forget};

/// This crate's module directory within the standard store: `~/.adi/mono/analytics`.
pub const MODULE: &str = "analytics";

/// A handle to [`MODULE`] in the given store — what the panel's `/api/analytics` handlers and
/// `adi-mono analytics` both open before calling into [`settings`] or [`state`].
#[must_use]
pub fn module(config: &adi_config::Config) -> adi_config::Module {
    config.module(MODULE)
}
