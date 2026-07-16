//! adi-core — the command surface for the adi platform, shared by every frontend.
//! Clients trigger platform commands through this crate instead of owning
//! launchd/config/route logic. The `adi-mono` CLI is a thin argv adapter over this API.

pub mod app;
mod commands;
pub mod dns;
pub mod launchd;
pub mod paths;
mod proc;
pub mod service;
pub mod status;

pub use app::App;
pub use commands::{Adi, Report};
pub use dns::Dns;
pub use service::{Action, Service, ServiceReport};

// Agent definitions are data/control-plane state, so adi-core exposes their store for the
// CLI and app backend without owning execution/orchestration yet.
pub use adi_agents::{Agent, AgentManifest, Agents, Error as AgentsError, Launch};

// The projects registry is pure metadata CRUD (no launchd/route machinery), so adi-core
// re-exports the [`adi_projects`] library as-is and hands out a store via [`Adi::projects`].
// Its error is re-exported as `ProjectsError` so it doesn't shadow other core error types.
pub use adi_projects::{Error as ProjectsError, Manifest, Project, Projects};

// The task tree is the shared queue/plan state. The CLI is the write-oriented command surface;
// the webapp can also create tasks but deeper mutations live in `adi-mono tasks ...`.
pub use adi_tasks::{EffectiveStatus, Error as TasksError, TaskPatch, TaskStatus, TaskView, Tasks};

// Trigger definitions (background code blocks fired by webhooks & co.) are data/control-plane
// state like agents: adi-core exposes their store — including the fire slice — for the CLI and
// app backend; live listeners (Telegram, cron) are future work.
pub use adi_triggers::{
    Error as TriggersError, Firing, Trigger, TriggerManifest, Triggers,
};

/// The CLI binary name — the single Rust-side source of truth for user-facing messages.
pub const BIN_NAME: &str = "adi-mono";
