//! Shared application state: the signal bundles a data refresh writes to, the per-page form
//! structs, the backend-liveness/flash enums, and the `load` routine that fans a fetch into the
//! signals. Every page module reads from [`State`]; the router and view helpers thread it around.

use std::collections::BTreeMap;

use adi_webapp_api::types::{
    AgentPeek, AgentsState, DirListing, Health, HiveState, MeshState, PortsState, ProjectDetail,
    ProjectsState, TasksState, TriggerLog, TriggersState, UsedPorts,
};
use leptos::prelude::*;

use crate::fetch;
use crate::routing::{Route, current_path, project_id_from_path};

/// Signals a data refresh writes to; `Copy` (each field is an arena handle) so it threads
/// cheaply through async tasks and event handlers.
#[derive(Clone, Copy)]
pub(crate) struct State {
    pub(crate) status: RwSignal<Status>,
    pub(crate) ports: RwSignal<Option<PortsState>>,
    pub(crate) health: RwSignal<Option<Health>>,
    pub(crate) flash: RwSignal<Option<Flash>>,
    pub(crate) secs_since: RwSignal<u32>,
    pub(crate) used: RwSignal<Option<UsedPorts>>,
    pub(crate) mesh: RwSignal<Option<MeshState>>,
    pub(crate) projects: RwSignal<Option<ProjectsState>>,
    pub(crate) project_detail: RwSignal<Option<ProjectDetail>>,
    pub(crate) current_project: RwSignal<String>,
    /// The read-only task tree (`/api/tasks`), shown on the Tasks page.
    pub(crate) tasks: RwSignal<Option<TasksState>>,
    /// Agent definitions (`/api/agents`), shown on the Agents page.
    pub(crate) agents: RwSignal<Option<AgentsState>>,
    /// Trigger definitions (`/api/triggers`), shown on the Triggers page.
    pub(crate) triggers: RwSignal<Option<TriggersState>>,
    pub(crate) hive: RwSignal<Option<HiveState>>,
    /// The project file browser/editor state (the Files panel on the detail page).
    pub(crate) files: FilesState,
}

/// The project detail page's file browser + editor state, scoped to the open project's own
/// directory (served through the isolated `adi-fs` jail). `Copy` (arena handles) so it threads
/// into the view and async handlers. Loading is navigation-driven, not part of the 4s poll, so
/// the poll never clobbers the editor buffer.
#[derive(Clone, Copy)]
pub(crate) struct FilesState {
    /// The directory currently being browsed, relative to the project root (`""` is the root).
    pub(crate) dir: RwSignal<String>,
    /// The listing of `dir`, or `None` while loading.
    pub(crate) listing: RwSignal<Option<DirListing>>,
    /// The file open in the editor (its path relative to the project root), or `None`.
    pub(crate) open: RwSignal<Option<String>>,
    /// The open file's last-loaded/saved content — compared against `buffer` to detect edits.
    pub(crate) original: RwSignal<String>,
    /// The editable textarea buffer.
    pub(crate) buffer: RwSignal<String>,
    /// Whether a read/write is in flight (disables the editor's buttons).
    pub(crate) busy: RwSignal<bool>,
    /// Which project id the browser currently reflects — so re-entering a fresh project reloads.
    pub(crate) loaded_for: RwSignal<String>,
}

impl FilesState {
    /// Fresh signals for the file browser (root dir, nothing loaded or open).
    pub(crate) fn new() -> Self {
        Self {
            dir: RwSignal::new(String::new()),
            listing: RwSignal::new(None),
            open: RwSignal::new(None),
            original: RwSignal::new(String::new()),
            buffer: RwSignal::new(String::new()),
            busy: RwSignal::new(false),
            loaded_for: RwSignal::new(String::new()),
        }
    }

    /// Clear the browser back to "nothing loaded" (used when leaving a project or switching to
    /// another), so the load effect re-fetches from the root next time.
    pub(crate) fn reset(self) {
        self.dir.set(String::new());
        self.listing.set(None);
        self.open.set(None);
        self.original.set(String::new());
        self.buffer.set(String::new());
        self.loaded_for.set(String::new());
    }
}

/// The Projects page's local signals: the create-form inputs, a busy flag, and the
/// active/archived filter. `Copy` so it threads into the page view and handlers.
#[derive(Clone, Copy)]
pub(crate) struct ProjectsForm {
    pub(crate) id: RwSignal<String>,
    pub(crate) name: RwSignal<String>,
    pub(crate) description: RwSignal<String>,
    /// The project to nest the new one under (its id), or empty for a top-level project.
    pub(crate) parent: RwSignal<String>,
    pub(crate) busy: RwSignal<bool>,
    pub(crate) show_archived: RwSignal<bool>,
}

/// The Tasks page's local signals: the create-form inputs (title, optional project/parent/tag,
/// optional details) and a busy flag. A tag matching an agent name is the future dispatch hook
/// (see docs/adi-agents.md). `Copy` so it threads into the page view and handlers.
#[derive(Clone, Copy)]
pub(crate) struct TasksForm {
    pub(crate) title: RwSignal<String>,
    /// The project to file the task under (its id), or empty for a project-less task. A
    /// project-scoped task gets a Jira-style `<KEY>-<n>` id.
    pub(crate) project: RwSignal<String>,
    pub(crate) parent: RwSignal<String>,
    pub(crate) tag: RwSignal<String>,
    pub(crate) details: RwSignal<String>,
    pub(crate) busy: RwSignal<bool>,
}

/// The Agents page's local create/edit form. Numeric fields (`temperature`, `max_turns`) are held
/// as strings and parsed on submit; `editing` is `Some(name)` while an existing agent is loaded
/// into the form (drives the header + a "New agent" reset). `Copy` so it threads into handlers.
#[derive(Clone, Copy)]
pub(crate) struct AgentsForm {
    pub(crate) name: RwSignal<String>,
    pub(crate) backend: RwSignal<String>,
    /// The project to file the agent under (its id), or empty for a global agent.
    pub(crate) project: RwSignal<String>,
    pub(crate) model: RwSignal<String>,
    pub(crate) permission_mode: RwSignal<String>,
    pub(crate) temperature: RwSignal<String>,
    pub(crate) max_turns: RwSignal<String>,
    pub(crate) tags: RwSignal<String>,
    pub(crate) tools: RwSignal<String>,
    pub(crate) system_prompt: RwSignal<String>,
    pub(crate) starred: RwSignal<bool>,
    pub(crate) extra: RwSignal<BTreeMap<String, String>>,
    pub(crate) editing: RwSignal<Option<String>>,
    pub(crate) busy: RwSignal<bool>,
}

/// The Triggers page's local create/edit form. `editing` is `Some(name)` while an existing
/// trigger is loaded into the form (drives the header + a "New trigger" reset); `extra` holds
/// the kind-specific settings (secret, schedule, …). `Copy` so it threads into handlers.
#[derive(Clone, Copy)]
pub(crate) struct TriggersForm {
    pub(crate) name: RwSignal<String>,
    pub(crate) kind: RwSignal<String>,
    /// The project to file the trigger under (its id), or empty for a global trigger.
    pub(crate) project: RwSignal<String>,
    pub(crate) description: RwSignal<String>,
    pub(crate) code: RwSignal<String>,
    pub(crate) enabled: RwSignal<bool>,
    pub(crate) extra: RwSignal<BTreeMap<String, String>>,
    pub(crate) editing: RwSignal<Option<String>>,
    pub(crate) busy: RwSignal<bool>,
}

/// The Triggers page's log view: which trigger's fire log is open (`None` = closed) and the
/// latest snapshot. The shell re-polls it every second while open (a fired code block may still
/// be appending); leaving the page closes it. `Copy` so it threads into the poll closure.
#[derive(Clone, Copy)]
pub(crate) struct TriggersLogView {
    /// The watched trigger's name, or `None` while the log view is closed.
    pub(crate) name: RwSignal<Option<String>>,
    /// The last log snapshot received, or `None` before the first one lands.
    pub(crate) log: RwSignal<Option<TriggerLog>>,
}

impl TriggersLogView {
    pub(crate) fn new() -> Self {
        Self {
            name: RwSignal::new(None),
            log: RwSignal::new(None),
        }
    }

    /// Close the log view (stops the polling; the poll no-ops while `name` is `None`).
    pub(crate) fn close(self) {
        self.name.set(None);
        self.log.set(None);
    }
}

/// The Agents page's live view: which agent's tmux pane is being watched (`None` = closed), the
/// latest snapshot, and the send-bar input buffer. The shell polls a fresh peek every second
/// while open; leaving the page closes it. `Copy` so it threads into the poll closure and
/// handlers.
#[derive(Clone, Copy)]
pub(crate) struct AgentsWatch {
    /// The watched agent's name, or `None` while the live view is closed.
    pub(crate) name: RwSignal<Option<String>>,
    /// The last snapshot received, or `None` before the first one lands.
    pub(crate) peek: RwSignal<Option<AgentPeek>>,
    /// The send bar's text buffer (typed into the session on submit).
    pub(crate) input: RwSignal<String>,
}

impl AgentsWatch {
    pub(crate) fn new() -> Self {
        Self {
            name: RwSignal::new(None),
            peek: RwSignal::new(None),
            input: RwSignal::new(String::new()),
        }
    }

    /// Close the live view (stops the polling; `poll_watch` no-ops while `name` is `None`).
    pub(crate) fn close(self) {
        self.name.set(None);
        self.peek.set(None);
        self.input.set(String::new());
    }
}

/// The reserve form's local signals; `Copy` so it threads into the page view and handlers.
#[derive(Clone, Copy)]
pub(crate) struct Form {
    pub(crate) svc: RwSignal<String>,
    pub(crate) key: RwSignal<String>,
    pub(crate) reserving: RwSignal<bool>,
    pub(crate) reserved: RwSignal<String>,
}

/// The Mesh page's local signals: the three add-forms' inputs, a shared busy flag, and node
/// refs to the id/ticket fields so the Copy buttons can select their text. `Copy` so it
/// threads into the page view and handlers.
#[derive(Clone, Copy)]
pub(crate) struct MeshForm {
    pub(crate) allow_port: RwSignal<String>,
    pub(crate) peer: RwSignal<String>,
    pub(crate) fwd_listen: RwSignal<String>,
    pub(crate) fwd_peer: RwSignal<String>,
    pub(crate) fwd_port: RwSignal<String>,
    pub(crate) busy: RwSignal<bool>,
    pub(crate) id_ref: NodeRef<leptos::html::Input>,
    pub(crate) ticket_ref: NodeRef<leptos::html::Input>,
}

/// Backend liveness as shown by the status pill.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Connecting,
    Online,
    Down,
}

impl Status {
    /// The `data-state` value the CSS keys the LED colour off.
    pub(crate) fn data(self) -> &'static str {
        match self {
            Status::Connecting => "unknown",
            Status::Online => "online",
            Status::Down => "down",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Status::Connecting => "connecting…",
            Status::Online => "online",
            Status::Down => "offline",
        }
    }
}

/// A one-line status message under the form; `kind` drives its colour via `data-kind`.
#[derive(Clone)]
pub(crate) struct Flash {
    pub(crate) kind: &'static str,
    pub(crate) msg: String,
}

impl Flash {
    pub(crate) fn ok(msg: String) -> Self {
        Self { kind: "ok", msg }
    }

    pub(crate) fn err(msg: String) -> Self {
        Self { kind: "err", msg }
    }
}

/// Fetch `/api/health` + `/api/ports` together and fan the result into the signals.
pub(crate) async fn load(s: State) {
    match (fetch::health().await, fetch::ports().await) {
        (Ok(h), Ok(p)) => {
            s.health.set(Some(h));
            s.ports.set(Some(p));
            s.status.set(Status::Online);
            s.secs_since.set(0);
        }
        (Err(e), _) | (_, Err(e)) => {
            s.status.set(Status::Down);
            s.flash
                .set(Some(Flash::err(format!("Couldn't reach the backend: {e}"))));
        }
    }
    // Page-specific data, fetched only where it's shown.
    let path = current_path();
    if path == Route::Projects.path() {
        if let Ok(p) = fetch::projects().await {
            s.projects.set(Some(p));
        }
        // The list shows a per-project open-task count, so it needs the task tree too.
        if let Ok(t) = fetch::tasks().await {
            s.tasks.set(Some(t));
        }
    }
    if let Some(id) = project_id_from_path(&path) {
        if let Ok(d) = fetch::project_detail(&id).await {
            s.project_detail.set(Some(d));
        }
        // The detail page's Tasks panel filters the shared task tree to this project.
        if let Ok(t) = fetch::tasks().await {
            s.tasks.set(Some(t));
        }
        // Likewise its Triggers panel filters the shared trigger list.
        if let Ok(t) = fetch::triggers().await {
            s.triggers.set(Some(t));
        }
        // And its Agents panel filters the shared agent list.
        if let Ok(a) = fetch::agents().await {
            s.agents.set(Some(a));
        }
    }
    if path == Route::Tasks.path() {
        if let Ok(t) = fetch::tasks().await {
            s.tasks.set(Some(t));
        }
        // The create form's project picker is populated from the registered projects.
        if let Ok(p) = fetch::projects().await {
            s.projects.set(Some(p));
        }
    }
    if path == Route::Agents.path() {
        if let Ok(a) = fetch::agents().await {
            s.agents.set(Some(a));
        }
        // The create form's project picker is populated from the registered projects.
        if let Ok(p) = fetch::projects().await {
            s.projects.set(Some(p));
        }
    }
    if path == Route::Triggers.path() {
        if let Ok(t) = fetch::triggers().await {
            s.triggers.set(Some(t));
        }
        // The create form's project picker is populated from the registered projects.
        if let Ok(p) = fetch::projects().await {
            s.projects.set(Some(p));
        }
    }
    if path == Route::Hive.path()
        && let Ok(h) = fetch::hive().await
    {
        s.hive.set(Some(h));
    }
    if path == Route::PortsManager.path()
        && let Ok(u) = fetch::used().await
    {
        s.used.set(Some(u));
    }
    if path == Route::Mesh.path()
        && let Ok(m) = fetch::mesh().await
    {
        s.mesh.set(Some(m));
    }
}
