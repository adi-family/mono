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
pub mod update;

pub use app::App;
pub use commands::{Adi, Report};
pub use dns::Dns;
pub use service::{Action, Service, ServiceReport};
pub use update::{RunOutcome, Update, Updater};

pub use adi_update::{Check as UpdateCheck, Error as UpdateError, State as UpdateState};

pub use adi_agents::arguments::AgentSummaryArguments;
pub use adi_agents::{
    Agent, AgentManifest, Agents, Error as AgentsError, Launch, RawAgentArguments, StoredAgent,
    StoredAgentManifest,
};

pub use adi_projects::{Error as ProjectsError, Manifest, Project, Projects};

// Project hooks + workspaces are rooted at a project's directory rather than a global
// store, so there is no `Adi` accessor: compose `projects().project_dir(id)` with
// `ProjectHooks::new(dir)` / `Workspaces::new(dir)`.
pub use adi_hooks::{
    Error as HooksError, HOOK_INIT, HOOK_WORKSPACE, Hook, HookRun, HookRunStatus,
    Hooks as ProjectHooks, WorkspaceEntry, WorkspaceKind, WorkspaceStatus, Workspaces,
    hook_template, is_lifecycle, terminal as workspace_terminal,
};

pub use adi_tasks::{EffectiveStatus, Error as TasksError, TaskPatch, TaskStatus, TaskView, Tasks};

pub use adi_triggers::{Error as TriggersError, Firing, Trigger, TriggerManifest, Triggers};

/// The CLI binary name — the single Rust-side source of truth for user-facing messages.
pub const BIN_NAME: &str = "adi-mono";
