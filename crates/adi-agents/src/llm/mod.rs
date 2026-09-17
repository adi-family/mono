//! LLM backends — one agent, many ways to answer a turn.
//!
//! An agent is an identity: a prompt, a set of tools, a working directory, a memory. A **backend**
//! is one complete way to answer a turn — a login, a model, and the dials that model runs with,
//! under a name somebody chose. The two are separate because they change for different reasons:
//! the prompt is edited when the job changes, the model when a subscription runs out.
//!
//! The whole design is `docs/llm-backends.md`; the short version is three layers and no fourth:
//!
//! ```text
//! LAUNCH     start_at · overrides                              per run
//! AGENT      prompt · tools · cwd · memory · backends = [ … ]   identity + the order
//! BACKEND    runtime · login · model · dials · limits · probe   one flat object
//! ```
//!
//! There is no preset, no chain object and no login record, and **a backend is never built on
//! another backend** — no inheritance. Reuse happens in the agent's ordered list, where a row may
//! override a backend's model and dials but never its runtime or its credential.
//!
//! What lives where:
//!
//! * [`backend`] — the backend definition and its on-disk store (`llm/backends/<id>.toml`).
//! * [`chain`] — an agent's ordered list, and resolving it into the concrete configurations a run
//!   will try in order.
//! * [`holds`] — the shared record of "this credential is spent until T", in `llm/holds.db`, so
//!   sixteen runs do not discover one limit sixteen times.
//! * [`classify`] — reading a backend's own limit rules against an error to decide what happened.
//! * [`failover`] — and what to *do* about it: hold it, move down the chain, or stop and ask.
//! * [`migrate`] — the one-time upgrade that lifts each agent's own model configuration out into a
//!   backend and puts that backend at the head of its list.
//! * [`prober`] — the background sweep that finds out a held backend is *back*, so that no chat turn
//!   ever has to be the thing that discovers it.
//! * [`ondemand`] — the human-triggered counterpart: a "Test" button or `llm test <id>`, reaching
//!   every runtime rather than only `harness:adi`, and writing nothing.
//! * [`settings`] — `llm/settings.toml`, the handful of global switches.

pub mod backend;
pub mod chain;
pub mod classify;
pub mod failover;
pub mod holds;
pub mod migrate;
pub mod ondemand;
pub mod prober;
pub mod settings;

pub use backend::{
    DEFAULT_HOLD, HoldScope, LimitClass, LimitRule, LlmBackend, LlmBackendManifest, LlmBackends,
    Probe, Resume,
};
pub use chain::{
    AgentBackendEntry, PinnedChain, ResolvedBackend, ResolvedChain, StartAt, catalog, validate_rows,
};
pub use classify::{Classification, classify};
pub use failover::{Decision, Failure, decide};
pub use holds::{Hold, HoldKey, Holds};
pub use migrate::{Move, Plan, Skip};
pub use ondemand::{TestResult, TestVerdict, test_backend, test_manifest};
pub use prober::{Checked, Prober, Verdict};
pub use settings::LlmSettings;

/// The store module every LLM file lives under: `llm/` in the mono store.
pub const LLM_MODULE: &str = "llm";

/// The subdirectory of [`LLM_MODULE`] holding one `<id>.toml` per backend.
pub const BACKENDS_DIR: &str = "backends";
