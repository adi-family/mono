//! adi-core — the command surface for the adi platform, shared by every frontend.
//! Clients trigger platform commands through this crate instead of owning
//! launchd/config/route logic. The `adi-mono` CLI is a thin argv adapter over this API.

pub mod app;
mod commands;
pub mod dashboards;
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

/// The version this build was cut from — the release tag, via `scripts/version.sh`. Every
/// frontend reports this one number so `adi-mono --version`, the bundle's `Info.plist` and
/// the published manifest can never drift apart.
pub use adi_update::BUILT_VERSION as VERSION;

pub use adi_agents::arguments::AgentSummaryArguments;
pub use adi_agents::goals;
pub use adi_agents::store::{Ask, Goal, GoalClosed, GoalState, SetBy};
pub use adi_agents::{
    Agent, AgentManifest, Agents, Backend, DEFAULT_MAX_CONCURRENT_RUNS, Error as AgentsError,
    Launch, RawAgentArguments, RunInfo, RunLimits, SecretAttachment, Sent, StoredAgent,
    StoredAgentManifest, contains_json_null, event_catalog,
};

pub use adi_db::{
    ColumnInfo, Db, DbInfo, Error as DbError, ExecResult, QueryResult, TableInfo,
};

pub use adi_knowledge::{
    Access as KnowledgeAccess, Base as KnowledgeBase, BaseId, BaseStatus, Error as KnowledgeError,
    Knowledge, KnowledgePatch, KnowledgeStore, NewKnowledge, Reader as KnowledgeReader,
    Scope as KnowledgeScope, resolve_agent_bases,
};

pub use adi_projects::{Error as ProjectsError, Manifest, Project, Projects};

pub use adi_secrets::{Error as SecretsError, Secret, Secrets};

// Project hooks + workspaces are rooted at a project's directory rather than a global
// store, so there is no `Adi` accessor: compose `projects().project_dir(id)` with
// `ProjectHooks::new(dir)` / `Workspaces::new(dir)`.
pub use adi_hooks::{
    Error as HooksError, HOOK_INIT, HOOK_WORKSPACE, Hook, HookRun, HookRunStatus,
    Hooks as ProjectHooks, WorkspaceEntry, WorkspaceKind, WorkspaceStatus, Workspaces,
    hook_template, is_lifecycle, terminal as workspace_terminal,
};

pub use adi_tasks::{EffectiveStatus, Error as TasksError, TaskPatch, TaskStatus, TaskView, Tasks};

pub use adi_tools::{
    Error as ToolsError, Manifest as ToolManifest, RunOutput as ToolRunOutput, Tool, Tools,
};

pub use adi_events::{
    ENVELOPE as EVENT_ENVELOPE, Error as EventsError, EventRecord, EventType, Events, SpooledEvent,
    matches as event_matches,
};

pub use adi_triggers::{
    Error as TriggersError, Firing, KIND_BACKGROUND, KIND_EVENT, KIND_WEBHOOK, RUNTIME_SH,
    clean_trigger_on,
    RUNTIME_TS, RunState, Trigger, TriggerManifest, Triggers, presets as trigger_presets,
};

/// The CLI binary name — the single Rust-side source of truth for user-facing messages.
pub const BIN_NAME: &str = "adi-mono";
