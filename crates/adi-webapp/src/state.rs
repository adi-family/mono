//! Shared application state: the signal bundles a data refresh writes to, the per-page form
//! structs, the backend-liveness/flash enums, and the `load` routine that fans a fetch into the
//! signals. Every page module reads from [`State`]; the router and view helpers thread it around.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use adi_webapp_api::types::{
    AgentPeek, AgentRunInfo, AgentsState, DashboardsState, DirListing, FileEntry, Health,
    HiveState, MeshState, MetaState, PortsState, ProjectDetail, ProjectHookLog, ProjectsState,
    SecretsState, TasksState, ToolsState, TriggerLog, TriggersState, UsedPorts, WorkspaceTerm,
    WorkspacesState,
};
use leptos::prelude::*;

use crate::fetch;
use crate::routing::{ProjectSection, Route, current_path, project_id_from_path};

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
    /// Which section of the open project is showing (`/projects/<id>/<section>`).
    pub(crate) current_section: RwSignal<ProjectSection>,
    /// The read-only task tree (`/api/tasks`), shown on the Tasks page.
    pub(crate) tasks: RwSignal<Option<TasksState>>,
    /// Agent definitions (`/api/agents`), shown on the Agents page.
    pub(crate) agents: RwSignal<Option<AgentsState>>,
    /// Tool definitions (`/api/tools`), shown on the Tools page and each project's Tools panel.
    pub(crate) tools: RwSignal<Option<ToolsState>>,
    /// Secret metadata across every scope (`/api/secrets`), shown on the Secrets page and each
    /// project's Secrets panel — never the values, which are fetched on demand by an explicit
    /// reveal.
    pub(crate) secrets: RwSignal<Option<SecretsState>>,
    /// The Meta page's state (`/api/meta`): the well-known `adi-agent`, the default system prompt
    /// to seed a new one with, and the agent form schema.
    pub(crate) meta: RwSignal<Option<MetaState>>,
    /// Trigger definitions (`/api/triggers`), shown on the Triggers page.
    pub(crate) triggers: RwSignal<Option<TriggersState>>,
    pub(crate) hive: RwSignal<Option<HiveState>>,
    /// The dashboards listing (`/dashboards`).
    pub(crate) dashboards: RwSignal<Option<DashboardsState>>,
    /// The open project's workspaces + hooks snapshot (`/api/projects/workspaces`), shown in
    /// the detail page's Workspaces panel. Refreshed by the 4s poll, so a `creating`
    /// workspace flips to `ready` on its own once the hook finishes.
    pub(crate) workspaces: RwSignal<Option<WorkspacesState>>,
    /// The project file browser/editor state (the Files panel on the detail page).
    pub(crate) files: FilesState,
    /// The store browser in the right rail — the whole `~/.adi/mono` tree, on every page.
    pub(crate) store: StoreBrowser,
}

/// The right rail's store browser: a lazily-expanded tree over `~/.adi/mono` (served through
/// the `adi-fs` jail rooted there) plus an inline editor for the selected file. `Copy` (arena
/// handles) so it threads into the view and async handlers.
///
/// The tree keeps one listing per expanded directory rather than a single "current directory",
/// so expanding a folder never collapses what is already open above it.
#[derive(Clone, Copy)]
pub(crate) struct StoreBrowser {
    /// Whether the rail is showing at all. Collapsed by default — it is a side tool, not the
    /// app's navigator (that is the left explorer).
    pub(crate) open: RwSignal<bool>,
    /// Every loaded directory listing, keyed by its path relative to the store root (`""` is
    /// the root). A key being present is what makes a directory rendered-as-expanded.
    pub(crate) dirs: RwSignal<BTreeMap<String, Vec<FileEntry>>>,
    /// The directories the user has expanded. Kept apart from `dirs` so a folder can read as
    /// expanded while its listing is still in flight.
    pub(crate) expanded: RwSignal<HashSet<String>>,
    /// The file open in the editor (its path relative to the store root), or `None`.
    pub(crate) open_file: RwSignal<Option<String>>,
    /// The open file's last-loaded/saved content — compared against `buffer` to detect edits.
    pub(crate) original: RwSignal<String>,
    /// The editable textarea buffer.
    pub(crate) buffer: RwSignal<String>,
    /// Whether a list/read/write is in flight.
    pub(crate) busy: RwSignal<bool>,
    /// Why the last list/read/write failed, or `None`. Shown in the rail, since the page's
    /// flash line can be scrolled far away from it.
    pub(crate) error: RwSignal<Option<String>>,
    /// The open right-click menu: the directory a create would land in, and the viewport point
    /// to draw at. `None` when no menu is showing.
    pub(crate) menu: RwSignal<Option<StoreMenu>>,
    /// The create in progress: which directory it lands in and whether it's a directory. The
    /// tree renders a name input inside that folder while this is set.
    pub(crate) creating: RwSignal<Option<StoreDraft>>,
    /// The name being typed into that input.
    pub(crate) draft: RwSignal<String>,
}

/// An open right-click menu on the store tree.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct StoreMenu {
    /// The directory a create from this menu lands in — the row itself for a folder, its parent
    /// for a file, so "New file" next to a file always means "beside it".
    pub(crate) dir: String,
    /// Where to draw, in viewport pixels (the menu is `position: fixed`).
    pub(crate) x: i32,
    /// See [`x`](Self::x).
    pub(crate) y: i32,
}

/// A create the tree is collecting a name for.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct StoreDraft {
    /// The directory the new entry lands in (`""` is the store root).
    pub(crate) dir: String,
    /// Whether to create a directory rather than an empty file.
    pub(crate) is_dir: bool,
}

impl StoreBrowser {
    /// Fresh signals for the store browser (collapsed, nothing loaded or open).
    pub(crate) fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            dirs: RwSignal::new(BTreeMap::new()),
            expanded: RwSignal::new(HashSet::new()),
            open_file: RwSignal::new(None),
            original: RwSignal::new(String::new()),
            buffer: RwSignal::new(String::new()),
            busy: RwSignal::new(false),
            error: RwSignal::new(None),
            menu: RwSignal::new(None),
            creating: RwSignal::new(None),
            draft: RwSignal::new(String::new()),
        }
    }

    /// Whether the editor buffer differs from what was last loaded or saved.
    pub(crate) fn dirty(self) -> bool {
        self.buffer.get() != self.original.get()
    }
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

/// The Projects page's local signals: the create-form inputs, a busy flag, and whether the
/// archive below the main table is expanded. `Copy` so it threads into the page view and handlers.
/// (The project *hierarchy* lives in the workbench explorer, not on this page.)
#[derive(Clone, Copy)]
pub(crate) struct ProjectsForm {
    pub(crate) name: RwSignal<String>,
    pub(crate) description: RwSignal<String>,
    /// The project to nest the new one under (its id), or empty for a top-level project.
    pub(crate) parent: RwSignal<String>,
    pub(crate) busy: RwSignal<bool>,
    /// Whether the collapsed archive under the main table is open. Archived projects are hidden
    /// by default; expanding is the only way to see and restore them.
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
    /// Whether the collapsed block of finished tasks at the foot of the page is open. Done and
    /// archived tasks are hidden by default so the tree shows only what is still open.
    pub(crate) show_done: RwSignal<bool>,
}

/// The Dashboards page's create form, plus whether the collapsed archive below the main table is
/// open. Archived dashboards are hidden by default; expanding is the only way to see and restore
/// them.
#[derive(Clone, Copy)]
pub(crate) struct DashboardsForm {
    pub(crate) name: RwSignal<String>,
    pub(crate) description: RwSignal<String>,
    pub(crate) busy: RwSignal<bool>,
    pub(crate) show_archived: RwSignal<bool>,
}

/// The Tools page's create/link form. `linking` flips the form between creating a new owned
/// script (name + runtime) and linking an existing file by `path`; `project` files the tool
/// under a project (empty = global). `show_archived` expands the collapsed archive at the foot.
/// `Copy` so it threads into the page view and handlers.
#[derive(Clone, Copy)]
pub(crate) struct ToolsForm {
    pub(crate) name: RwSignal<String>,
    /// The script language of a *new* tool: `sh` or `ts`.
    pub(crate) runtime: RwSignal<String>,
    pub(crate) description: RwSignal<String>,
    /// The project to file the tool under (its id), or empty for a global tool.
    pub(crate) project: RwSignal<String>,
    /// The existing file path, when linking rather than creating.
    pub(crate) path: RwSignal<String>,
    /// Whether the form is in "link an existing file" mode (vs. "create a new script").
    pub(crate) linking: RwSignal<bool>,
    pub(crate) busy: RwSignal<bool>,
    pub(crate) show_archived: RwSignal<bool>,
}

impl ToolsForm {
    /// Fresh signals for the create/link form (create mode, sh runtime, nothing typed).
    pub(crate) fn new() -> Self {
        Self {
            name: RwSignal::new(String::new()),
            runtime: RwSignal::new("sh".to_string()),
            description: RwSignal::new(String::new()),
            project: RwSignal::new(String::new()),
            path: RwSignal::new(String::new()),
            linking: RwSignal::new(false),
            busy: RwSignal::new(false),
            show_archived: RwSignal::new(false),
        }
    }
}

/// The Secrets page's create form plus its reveal cache. `project` files the secret under a
/// project (empty = global). `revealed` holds the values a user has explicitly revealed, keyed
/// by scope+name (see `reveal_key`), so a value is shown only after a deliberate Reveal and
/// never persists across a reload. `Copy` so it threads into the page view and handlers.
#[derive(Clone, Copy)]
pub(crate) struct SecretsForm {
    pub(crate) name: RwSignal<String>,
    pub(crate) value: RwSignal<String>,
    pub(crate) description: RwSignal<String>,
    /// The project to file the secret under (its id), or empty for a global secret.
    pub(crate) project: RwSignal<String>,
    /// Where the value comes from: `"text"` (typed) or `"oauth"` (obtained through a provider
    /// flow). Toggled in the create form.
    pub(crate) source: RwSignal<String>,
    /// The OAuth provider selected for an `oauth`-source secret (`"google"`, `"github"`).
    pub(crate) provider: RwSignal<String>,
    /// The access scopes ticked for the flow (e.g. individual Gmail permissions). What's
    /// requested; the provider returns what it actually granted, which is stored on the secret.
    pub(crate) scopes: RwSignal<Vec<String>>,
    pub(crate) busy: RwSignal<bool>,
    /// Revealed plaintext values, keyed by `reveal_key(project, name)`. Empty by default; a row
    /// masks its value until its key is present here.
    pub(crate) revealed: RwSignal<BTreeMap<String, String>>,
}

impl SecretsForm {
    /// Fresh signals for the create form (global scope, nothing typed, nothing revealed).
    pub(crate) fn new() -> Self {
        Self {
            name: RwSignal::new(String::new()),
            value: RwSignal::new(String::new()),
            description: RwSignal::new(String::new()),
            project: RwSignal::new(String::new()),
            source: RwSignal::new("text".to_string()),
            provider: RwSignal::new("google".to_string()),
            // Sensible default for the default provider (Google): read Gmail + identify the
            // account. The user ticks more in the create form.
            scopes: RwSignal::new(vec![
                "https://www.googleapis.com/auth/gmail.readonly".to_string(),
                "email".to_string(),
            ]),
            busy: RwSignal::new(false),
            revealed: RwSignal::new(BTreeMap::new()),
        }
    }

    /// Forget every revealed value — called when leaving the page so a value never lingers in
    /// memory across a navigation.
    pub(crate) fn clear_revealed(self) {
        self.revealed.set(BTreeMap::new());
    }
}

/// The Tools page's script editor panel: which tool's script is open (`None` = closed), the
/// resolved on-disk path, the runtime (for syntax highlighting), the edit buffer with its saved
/// baseline, a busy flag, and any load error. Mirrors [`AgentCodeEditor`]. `Copy` so it threads
/// into the view and async handlers.
#[derive(Clone, Copy)]
pub(crate) struct ToolEditor {
    /// The open tool's id, or `None` while the editor is closed.
    pub(crate) open: RwSignal<Option<String>>,
    /// The tool's display name, for the panel heading.
    pub(crate) name: RwSignal<String>,
    /// The resolved script path (owned file, or linked target).
    pub(crate) path: RwSignal<String>,
    /// The script runtime (`sh` | `ts`), driving the highlighter.
    pub(crate) runtime: RwSignal<String>,
    /// The last-loaded/saved content — compared against `buffer` to detect edits.
    pub(crate) original: RwSignal<String>,
    /// The editable buffer.
    pub(crate) buffer: RwSignal<String>,
    /// Whether a read/write is in flight.
    pub(crate) busy: RwSignal<bool>,
    /// Why the script couldn't be loaded, or `None`.
    pub(crate) error: RwSignal<Option<String>>,
}

impl ToolEditor {
    pub(crate) fn new() -> Self {
        Self {
            open: RwSignal::new(None),
            name: RwSignal::new(String::new()),
            path: RwSignal::new(String::new()),
            runtime: RwSignal::new(String::new()),
            original: RwSignal::new(String::new()),
            buffer: RwSignal::new(String::new()),
            busy: RwSignal::new(false),
            error: RwSignal::new(None),
        }
    }

    /// Close the editor and drop its buffers.
    pub(crate) fn close(self) {
        self.open.set(None);
        self.name.set(String::new());
        self.path.set(String::new());
        self.runtime.set(String::new());
        self.original.set(String::new());
        self.buffer.set(String::new());
        self.error.set(None);
    }
}

/// The Tools page's run panel: which tool was last run (`None` = closed), the args input, the
/// captured output, its exit code + success flag, and a busy flag while a run is in flight.
/// `Copy` so it threads into the view and async handlers.
#[derive(Clone, Copy)]
pub(crate) struct ToolRunView {
    /// The tool whose output is showing, or `None` while the panel is closed.
    pub(crate) id: RwSignal<Option<String>>,
    /// The tool's display name, for the panel heading.
    pub(crate) name: RwSignal<String>,
    /// The args input buffer (space-separated; passed to the tool verbatim).
    pub(crate) args: RwSignal<String>,
    /// The last run's combined output.
    pub(crate) output: RwSignal<String>,
    /// The last run's exit code, or `None` before a run / when signal-killed.
    pub(crate) code: RwSignal<Option<i32>>,
    /// Whether the last run exited cleanly.
    pub(crate) ok: RwSignal<bool>,
    /// Whether a run is in flight.
    pub(crate) busy: RwSignal<bool>,
}

impl ToolRunView {
    pub(crate) fn new() -> Self {
        Self {
            id: RwSignal::new(None),
            name: RwSignal::new(String::new()),
            args: RwSignal::new(String::new()),
            output: RwSignal::new(String::new()),
            code: RwSignal::new(None),
            ok: RwSignal::new(false),
            busy: RwSignal::new(false),
        }
    }

    /// Close the run panel and drop its output.
    pub(crate) fn close(self) {
        self.id.set(None);
        self.name.set(String::new());
        self.args.set(String::new());
        self.output.set(String::new());
        self.code.set(None);
        self.ok.set(false);
    }
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
    /// The adi tool ids enabled for this agent (its per-tool checkboxes) — each becomes a shim in
    /// the agent's own `.bin`. Distinct from `tools` above, which is the LLM `--allowed-tools` spec.
    pub(crate) bin_tools: RwSignal<BTreeSet<String>>,
    /// The secrets attached to this agent (its per-secret checkboxes), each keyed by its
    /// `(scope, name)` pair — `None` scope is a global secret. Only these are injected into the
    /// agent's runs as env vars (an allowlist).
    pub(crate) secrets: RwSignal<BTreeSet<(Option<String>, String)>>,
    pub(crate) system_prompt: RwSignal<String>,
    pub(crate) starred: RwSignal<bool>,
    /// The complete backend argument map loaded for editing, including structured values the
    /// schema-driven form does not render directly.
    pub(crate) arguments: RwSignal<BTreeMap<String, serde_json::Value>>,
    /// String representations for schema-rendered scalar backend arguments.
    pub(crate) argument_values: RwSignal<BTreeMap<String, String>>,
    pub(crate) editing: RwSignal<Option<String>>,
    pub(crate) busy: RwSignal<bool>,
}

/// The Meta page's setup form for the default `adi-agent`: the chosen backend and the (editable)
/// system prompt, a busy flag while a save is in flight, and `editing` — true while reconfiguring
/// an agent that already exists (the same form doubles as create and edit). `Copy` so it threads
/// into the page view and handlers. Seeded from the server's default prompt on first load (see
/// [`crate::App`]).
#[derive(Clone, Copy)]
pub(crate) struct MetaForm {
    /// The selected backend id (`tmux:claude`, `process:codex`, …).
    pub(crate) backend: RwSignal<String>,
    /// The system prompt buffer — prefilled with the server's default, then editable.
    pub(crate) prompt: RwSignal<String>,
    pub(crate) busy: RwSignal<bool>,
    /// True while reconfiguring an agent that already exists, so the setup form shows in place of
    /// the ready view.
    pub(crate) editing: RwSignal<bool>,
}

impl MetaForm {
    pub(crate) fn new() -> Self {
        Self {
            backend: RwSignal::new(String::new()),
            prompt: RwSignal::new(String::new()),
            busy: RwSignal::new(false),
            editing: RwSignal::new(false),
        }
    }
}

/// The Triggers page's local create/edit form. `editing` is `Some(name)` while an existing
/// trigger is loaded into the form (drives the header + a "New trigger" reset); `extra` holds
/// the kind-specific settings (secret, schedule, …). `Copy` so it threads into handlers.
#[derive(Clone, Copy)]
pub(crate) struct TriggersForm {
    pub(crate) name: RwSignal<String>,
    /// How the trigger launches: `webhook` or `background`.
    pub(crate) kind: RwSignal<String>,
    /// The language of the code block: `sh` or `ts`.
    pub(crate) runtime: RwSignal<String>,
    /// The preset the form was prefilled from, which decides the settings inputs it offers.
    /// `None` once the user starts from scratch.
    pub(crate) preset: RwSignal<Option<String>>,
    /// The project to file the trigger under (its id), or empty for a global trigger.
    pub(crate) project: RwSignal<String>,
    pub(crate) description: RwSignal<String>,
    pub(crate) code: RwSignal<String>,
    pub(crate) enabled: RwSignal<bool>,
    pub(crate) extra: RwSignal<BTreeMap<String, String>>,
    /// For an event trigger: the subscription patterns, one per line (`adi.tasks.*`). Held as raw
    /// text and split on save; irrelevant to the other kinds.
    pub(crate) events: RwSignal<String>,
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

/// The project detail page's hook-log view: which hook's run log is open (`None` = closed) —
/// keyed by (project id, hook name), since hook logs are project-scoped — and the latest
/// snapshot. The shell re-polls it every second while open (a running hook may still be
/// appending); leaving the page closes it. `Copy` so it threads into the poll closure.
#[derive(Clone, Copy)]
pub(crate) struct HookLogView {
    /// The watched (project id, hook name), or `None` while the log view is closed.
    pub(crate) watched: RwSignal<Option<(String, String)>>,
    /// The last log snapshot received, or `None` before the first one lands.
    pub(crate) log: RwSignal<Option<ProjectHookLog>>,
}

impl HookLogView {
    pub(crate) fn new() -> Self {
        Self {
            watched: RwSignal::new(None),
            log: RwSignal::new(None),
        }
    }

    /// Close the log view (stops the polling; the poll no-ops while `watched` is `None`).
    pub(crate) fn close(self) {
        self.watched.set(None);
        self.log.set(None);
    }
}

/// The project detail page's workspace terminal view: which workspace's tmux terminal is
/// being watched (`None` = closed) — keyed by (project id, workspace name) — the latest pane
/// snapshot, and the send-bar input buffer. The shell polls a fresh peek every second while
/// open; leaving the page closes it. The workspace twin of [`AgentsWatch`]. `Copy` so it
/// threads into the poll closure and handlers.
#[derive(Clone, Copy)]
pub(crate) struct TermWatch {
    /// The watched (project id, workspace name), or `None` while the terminal view is closed.
    pub(crate) watched: RwSignal<Option<(String, String)>>,
    /// The last snapshot received, or `None` before the first one lands.
    pub(crate) peek: RwSignal<Option<WorkspaceTerm>>,
    /// The send bar's text buffer (typed into the session on submit).
    pub(crate) input: RwSignal<String>,
}

impl TermWatch {
    pub(crate) fn new() -> Self {
        Self {
            watched: RwSignal::new(None),
            peek: RwSignal::new(None),
            input: RwSignal::new(String::new()),
        }
    }

    /// Close the terminal view (stops the polling; the poll no-ops while `watched` is
    /// `None`). The tmux session itself keeps running — closing the view never kills it.
    pub(crate) fn close(self) {
        self.watched.set(None);
        self.peek.set(None);
        self.input.set(String::new());
    }
}

/// The project detail page's hook editor: which hook script is open (`None` = closed) —
/// keyed by (project id, hook name) so save/reload always target the project the file was
/// read from — plus the edit buffer and its saved baseline. Rendered as its own panel next
/// to the Workspaces panel; a navigation builds a fresh (closed) one. `Copy` so it threads
/// into the view and async handlers.
#[derive(Clone, Copy)]
pub(crate) struct HookEditor {
    /// The open (project id, hook name), or `None` while the editor is closed.
    pub(crate) open: RwSignal<Option<(String, String)>>,
    /// The last-loaded/saved content — compared against `buffer` to detect edits.
    pub(crate) original: RwSignal<String>,
    /// The editable textarea buffer.
    pub(crate) buffer: RwSignal<String>,
    /// Whether a read/write is in flight (disables the editor's buttons).
    pub(crate) busy: RwSignal<bool>,
}

impl HookEditor {
    pub(crate) fn new() -> Self {
        Self {
            open: RwSignal::new(None),
            original: RwSignal::new(String::new()),
            buffer: RwSignal::new(String::new()),
            busy: RwSignal::new(false),
        }
    }

    /// Close the editor and drop its buffers.
    pub(crate) fn close(self) {
        self.open.set(None);
        self.original.set(String::new());
        self.buffer.set(String::new());
    }
}

/// The Agents page's employee-code editor: which wasm agent's TypeScript source is open
/// (`None` = closed), the file path it was read from, the edit buffer with its saved baseline,
/// and the last build's outcome. `Copy` so it threads into the view and async handlers.
#[derive(Clone, Copy)]
pub(crate) struct AgentCodeEditor {
    /// The open agent's name, or `None` while the editor is closed.
    pub(crate) open: RwSignal<Option<String>>,
    /// The source file path the server resolved from the agent's `src` argument.
    pub(crate) path: RwSignal<String>,
    /// The last-loaded/saved content — compared against `buffer` to detect edits.
    pub(crate) original: RwSignal<String>,
    /// The editable textarea buffer.
    pub(crate) buffer: RwSignal<String>,
    /// Whether a read/write/build is in flight (disables the editor's buttons).
    pub(crate) busy: RwSignal<bool>,
    /// The last build's (succeeded, combined output), or `None` before the first build.
    pub(crate) build: RwSignal<Option<(bool, String)>>,
    /// Why the source couldn't be loaded, or `None` when it loaded fine. The panel opens either
    /// way: an unreadable `src` has to say so in place, since the action scrolls here.
    pub(crate) error: RwSignal<Option<String>>,
}

impl AgentCodeEditor {
    pub(crate) fn new() -> Self {
        Self {
            open: RwSignal::new(None),
            path: RwSignal::new(String::new()),
            original: RwSignal::new(String::new()),
            buffer: RwSignal::new(String::new()),
            busy: RwSignal::new(false),
            build: RwSignal::new(None),
            error: RwSignal::new(None),
        }
    }

    /// Close the editor and drop its buffers.
    pub(crate) fn close(self) {
        self.open.set(None);
        self.path.set(String::new());
        self.original.set(String::new());
        self.buffer.set(String::new());
        self.build.set(None);
        self.error.set(None);
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
    /// Whether the watched agent is interactive (tmux) — it then shows a live pane and a send bar.
    /// A headless agent shows its run history and a task composer instead.
    pub(crate) interactive: RwSignal<bool>,
    /// For a headless agent, the run whose log the view is showing (or `None` = none selected yet).
    pub(crate) run_id: RwSignal<Option<String>>,
    /// For a headless agent, its run history (newest first), refreshed by the poll.
    pub(crate) runs: RwSignal<Vec<AgentRunInfo>>,
    /// The last snapshot received, or `None` before the first one lands.
    pub(crate) peek: RwSignal<Option<AgentPeek>>,
    /// The selected run's log tail, kept apart from `peek` so the inline viewer binds to a plain
    /// `String` signal and the poll only touches it when the log actually grew — the log follows
    /// (`tail -f`) without the whole panel re-rendering each second.
    pub(crate) log: RwSignal<String>,
    /// Text buffer: the send bar (tmux) or the run composer's task (headless).
    pub(crate) input: RwSignal<String>,
}

impl AgentsWatch {
    pub(crate) fn new() -> Self {
        Self {
            name: RwSignal::new(None),
            interactive: RwSignal::new(false),
            run_id: RwSignal::new(None),
            runs: RwSignal::new(Vec::new()),
            peek: RwSignal::new(None),
            log: RwSignal::new(String::new()),
            input: RwSignal::new(String::new()),
        }
    }

    /// Close the live view (stops the polling; `poll_watch` no-ops while `name` is `None`).
    pub(crate) fn close(self) {
        self.name.set(None);
        self.interactive.set(false);
        self.run_id.set(None);
        self.runs.set(Vec::new());
        self.peek.set(None);
        self.log.set(String::new());
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
    // The explorer renders the project tree on every route, so the project list is shell
    // data rather than something an individual page opts into.
    if let Ok(p) = fetch::projects().await {
        s.projects.set(Some(p));
    }

    // Page-specific data, fetched only where it's shown.
    let path = current_path();
    if path == Route::Projects.path() {
        // The list shows a per-project open-task count, so it needs the task tree too.
        if let Ok(t) = fetch::tasks().await {
            s.tasks.set(Some(t));
        }
    }
    if let Some(id) = project_id_from_path(&path) {
        if let Ok(d) = fetch::project_detail(&id).await {
            s.project_detail.set(Some(d));
        }
        if let Ok(t) = fetch::tasks().await {
            s.tasks.set(Some(t));
        }
        if let Ok(t) = fetch::triggers().await {
            s.triggers.set(Some(t));
        }
        if let Ok(a) = fetch::agents().await {
            s.agents.set(Some(a));
        }
        // The project's Tools panel lists the tools filed under it (from the shared list).
        if let Ok(t) = fetch::tools().await {
            s.tools.set(Some(t));
        }
        // The project's Secrets panel filters the shared secrets list to this project.
        if let Ok(sec) = fetch::secrets().await {
            s.secrets.set(Some(sec));
        }
        // The Workspaces panel's snapshot; polling it flips `creating` → `ready` live.
        if let Ok(w) = fetch::workspaces(&id).await {
            s.workspaces.set(Some(w));
        }
    }
    if path == Route::Tasks.path() {
        if let Ok(t) = fetch::tasks().await {
            s.tasks.set(Some(t));
        }
    }
    if path == Route::Meta.path()
        && let Ok(m) = fetch::meta().await
    {
        s.meta.set(Some(m));
    }
    if path == Route::Agents.path() {
        if let Ok(a) = fetch::agents().await {
            s.agents.set(Some(a));
        }
        // The agent form's per-tool checkboxes are populated from the tools list.
        if let Ok(t) = fetch::tools().await {
            s.tools.set(Some(t));
        }
        // The agent form's per-secret checkboxes are populated from the secrets list (metadata
        // only — values are never fetched here).
        if let Ok(sec) = fetch::secrets().await {
            s.secrets.set(Some(sec));
        }
    }
    if path == Route::Tools.path()
        && let Ok(t) = fetch::tools().await
    {
        s.tools.set(Some(t));
    }
    if path == Route::Secrets.path()
        && let Ok(sec) = fetch::secrets().await
    {
        s.secrets.set(Some(sec));
    }
    if path == Route::Triggers.path() {
        if let Ok(t) = fetch::triggers().await {
            s.triggers.set(Some(t));
        }
    }
    if path == Route::Hive.path()
        && let Ok(h) = fetch::hive().await
    {
        s.hive.set(Some(h));
    }
    if path == Route::Dashboards.path()
        && let Ok(d) = fetch::dashboards().await
    {
        s.dashboards.set(Some(d));
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
