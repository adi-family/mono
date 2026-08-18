//! Shared application state: the signal bundles a data refresh writes to, the per-page form
//! structs, the backend-liveness/flash enums, and the `load` routine that fans a fetch into the
//! signals. Every page module reads from [`State`]; the router and view helpers thread it around.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use adi_ui::{Block, Flag, ToolDecl};
use adi_webapp_api::types::{
    AgentGoal, AgentPeek, AgentRef, AgentRunInfo, AgentRuns, AgentSimState, AgentTokens, AgentsState,
    AllAgentRuns,
    DashboardsState,
    DbExecResult, DbQueryResult, DbState, DbTablesState, DirListing, FileEntry, FleetDashboards,
    FleetState, Health,
    HiveState, KnowledgeBaseDto, KnowledgeNoteDto, KnowledgeNotes, KnowledgeResults, KnowledgeState,
    MeshState, MetaState, PortsState, ProjectDetail, ProjectHookLog, ProjectHookRef,
    ProjectsState,
    RunRef, SecretsState, TasksState, ToolsState, TriggerLog, TriggerRef, TriggersState, UsedPorts,
    WorkspaceTerm, WorkspaceTermRef, WorkspacesRef, WorkspacesState,
};
use leptos::prelude::*;

use crate::fetch;
use crate::live::Sub;
use crate::routing::{ProjectSection, Route, current_path, project_id_from_path};
use crate::ui::TableState;

/// The well-known root agent: the one onboarding sets up, the one the chat home opens on, and the
/// one a dashboard's "edit with adi-agent" launcher talks to. Every other agent is reached from the
/// chat home's agent picker.
pub(crate) const ROOT_AGENT: &str = "adi-agent";

/// How many sessions the chat rail asks for at a time — the first page, and what **Load more**
/// adds each press.
///
/// A hundred, because it is more than anyone scrolls and far less than a fleet accumulates: the
/// index reached four hundred sessions on this machine alone, and the rail is watched over the
/// live channel, so the whole of it was re-sent to every open panel each time any one of them
/// moved.
pub(crate) const SESSION_PAGE: usize = 100;

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
    /// The paired remote nodes (`/api/fleet`), shown on the Fleet page.
    pub(crate) fleet: RwSignal<Option<FleetState>>,
    pub(crate) projects: RwSignal<Option<ProjectsState>>,
    pub(crate) project_detail: RwSignal<Option<ProjectDetail>>,
    pub(crate) current_project: RwSignal<String>,
    /// Which section of the open project is showing (`/projects/<id>/<section>`).
    pub(crate) current_section: RwSignal<ProjectSection>,
    /// The read-only task tree (`/api/tasks`), shown on the Tasks page.
    pub(crate) tasks: RwSignal<Option<TasksState>>,
    /// Agent definitions (`/api/agents`), shown on the Agents page.
    pub(crate) agents: RwSignal<Option<AgentsState>>,
    /// Every agent's run history (`/api/agents/runs/all`) — the data behind the cross-agent
    /// "All chats" index shown above the Agents list and the single-agent live view.
    pub(crate) all_chats: RwSignal<Option<AllAgentRuns>>,
    /// Tool definitions (`/api/tools`), shown on the Tools page and each project's Tools panel.
    pub(crate) tools: RwSignal<Option<ToolsState>>,
    /// Secret metadata across every scope (`/api/secrets`), shown on the Secrets page and each
    /// project's Secrets panel — never the values, which are fetched on demand by an explicit
    /// reveal.
    pub(crate) secrets: RwSignal<Option<SecretsState>>,
    /// Which databases exist in the shared SQLite store (`/api/db`), shown on the Database page.
    /// Only the listing lives here — a scope's tables and any query result are page-local, fetched
    /// on demand, because they're too big and too transient to belong to the polled shell state.
    pub(crate) db: RwSignal<Option<DbState>>,
    /// The Meta page's state (`/api/meta`): the well-known `adi-agent`, the default system prompt
    /// to seed a new one with, and the agent form schema.
    pub(crate) meta: RwSignal<Option<MetaState>>,
    /// Trigger definitions (`/api/triggers`), shown on the Triggers page.
    pub(crate) triggers: RwSignal<Option<TriggersState>>,
    pub(crate) hive: RwSignal<Option<HiveState>>,
    /// The dashboards listing (`/dashboards`).
    pub(crate) dashboards: RwSignal<Option<DashboardsState>>,
    /// What the *fleet* is running (`/api/fleet/dashboards`) — one entry per paired node, asked of
    /// that node's own control panel over the mesh. Not part of the poll: every read of it leaves
    /// the machine, so it is fetched on load and when [`fleet_dashboards_busy`] says the rail asked
    /// for it again.
    ///
    /// [`fleet_dashboards_busy`]: Self::fleet_dashboards_busy
    pub(crate) fleet_dashboards: RwSignal<Option<FleetDashboards>>,
    /// Whether a fleet listing is in flight, so the rail can say it is asking rather than sit
    /// looking finished for the second or two a mesh round trip takes.
    pub(crate) fleet_dashboards_busy: RwSignal<bool>,
    /// The rail's inline "give this machine the node's password" form. See [`FleetUnlock`].
    pub(crate) fleet_unlock: FleetUnlock,
    /// The open project's workspaces + hooks snapshot (`/api/projects/workspaces`), shown in
    /// the detail page's Workspaces panel. Refreshed by the 4s poll, so a `creating`
    /// workspace flips to `ready` on its own once the hook finishes.
    pub(crate) workspaces: RwSignal<Option<WorkspacesState>>,
    /// The project file browser/editor state (the Files panel on the detail page).
    pub(crate) files: FilesState,
    /// The store browser in the right rail — the whole `~/.adi/mono` tree, on every page.
    pub(crate) store: StoreBrowser,
    /// The open table-row kebab menu, shared by every page's action columns. See [`RowMenu`].
    pub(crate) row_menu: RwSignal<Option<RowMenu>>,
    /// The open right-click menu on a chat-home session row. See [`SessionMenu`].
    pub(crate) session_menu: RwSignal<Option<SessionMenu>>,
    /// Whether the chat rail's **Hidden** band is expanded. Collapsed by default — that band exists
    /// to get a session *back*, not to be read; it is page state rather than a stored preference, so
    /// a reload closes it again.
    pub(crate) show_hidden: RwSignal<bool>,
    /// Whether the chat rail is narrowed to the sessions of **starred** agents. **Off** by default:
    /// the rail opens on every conversation, because a chat that is missing reads as a chat that is
    /// gone, and no filter should be the first thing a person has to notice and undo. Turning it on
    /// is the way to narrow to the shortlist the agent picker draws from.
    ///
    /// Page state rather than a stored preference, like the two signals around it — a reload comes
    /// back to the full list.
    ///
    /// The agent currently on screen is exempt while it is on, the same escape hatch the agent
    /// picker gives itself: a filter must never hide what the centre pane is showing.
    pub(crate) starred_only: RwSignal<bool>,
    /// How many sessions the chat rail has asked the backend for — [`SESSION_PAGE`] to begin with,
    /// another page each time its **Load more** is pressed.
    ///
    /// The rail is the one place the whole index is expensive: it is watched over the live channel,
    /// so every agent's every session used to be re-sent to every open panel whenever any one of
    /// them moved. A page is what the rail reads anyway — nobody scrolls four hundred chats — and
    /// the pages that genuinely need all of it (Analytics, the Agents index) still ask without a
    /// limit.
    ///
    /// Page state rather than a stored preference, like the rail's other two: a reload comes back
    /// to the first page.
    pub(crate) rail_limit: RwSignal<usize>,
    /// Which side rail is open as a drawer, on a viewport too narrow to seat both beside the chat.
    /// `None` on a wide one, where the rails are always in the layout and this is never read.
    ///
    /// It lives here rather than in the view for the same reason [`Self::show_hidden`] does: the
    /// chat home is re-rendered whenever `/api/meta` moves, and a signal created inside it would
    /// take the open drawer with it every time a poll landed.
    pub(crate) chat_drawer: RwSignal<Option<ChatDrawer>>,
    /// How every table on the site is sorted and arranged. See [`Tables`].
    pub(crate) tables: Tables,
}

/// One [`TableState`] per table in the control panel: how each is sorted, which columns it shows,
/// and in what order.
///
/// These live on [`State`], beside [`State::row_menu`], rather than in the views: a page function
/// is re-run on every route render, so signals created inside it would reset the user's
/// arrangement on each redraw. Each field's storage key is what its layout persists under, so
/// renaming one forgets that table's saved arrangement — which is the right outcome when a table
/// changes enough to warrant a new name, and a bug otherwise.
///
/// A table shown in two places with different columns (the global Tools page and a project's
/// Tools panel, say) gets a state each: they are the same rows but not the same table, and one
/// arrangement can't describe both. Live and archived halves of a page are split for the same
/// reason, plus a practical one — sharing a state would open both settings menus at once.
///
/// `Copy` (every [`TableState`] is a bundle of arena handles), so it threads into views and
/// handlers as cheaply as the rest of [`State`].
#[derive(Clone, Copy)]
pub(crate) struct Tables {
    pub(crate) agents: TableState,
    /// The Global Analytics page's per-agent breakdown.
    pub(crate) analytics_agents: TableState,
    /// The cross-agent "All chats" index.
    pub(crate) chats: TableState,
    /// One agent's run history, for a backend whose runs read as a conversation…
    pub(crate) chat_runs: TableState,
    /// …and for a one-shot backend, where a run is a task.
    pub(crate) runs: TableState,
    pub(crate) dashboards: TableState,
    pub(crate) dashboards_archived: TableState,
    pub(crate) db_scopes: TableState,
    pub(crate) db_tables: TableState,
    pub(crate) hive: TableState,
    /// The port registry's active leases.
    pub(crate) leases: TableState,
    /// The scan of every listening port on the machine.
    pub(crate) used_ports: TableState,
    pub(crate) mesh_allow: TableState,
    pub(crate) mesh_peers: TableState,
    pub(crate) mesh_forwards: TableState,
    /// The paired remote nodes on the Fleet page.
    pub(crate) fleet: TableState,
    pub(crate) projects: TableState,
    pub(crate) projects_archived: TableState,
    pub(crate) secrets: TableState,
    /// The Knowledge page's base list …
    pub(crate) knowledge_bases: TableState,
    /// … and the notes of whichever base is open on it.
    pub(crate) knowledge_notes: TableState,
    pub(crate) tasks: TableState,
    pub(crate) tasks_done: TableState,
    pub(crate) tools: TableState,
    pub(crate) tools_archived: TableState,
    pub(crate) triggers: TableState,
    pub(crate) workspaces: TableState,
    pub(crate) hooks: TableState,
    // ---- a project detail page's panels ----
    pub(crate) project_agents: TableState,
    pub(crate) project_secrets: TableState,
    /// A project's Knowledge panel keeps its own layouts — its bases table drops the Level
    /// column the global one carries.
    pub(crate) project_knowledge_bases: TableState,
    pub(crate) project_knowledge_notes: TableState,
    pub(crate) project_tasks: TableState,
    pub(crate) project_tools: TableState,
    pub(crate) project_triggers: TableState,
    pub(crate) files: TableState,
    pub(crate) services: TableState,
    pub(crate) subprojects: TableState,
}

impl Tables {
    /// Restore every table from storage, each falling back to its page's declared columns.
    pub(crate) fn new() -> Self {
        use crate::pages::columns as c;

        Self {
            agents: TableState::new("agents", c::AGENT_COLS),
            analytics_agents: TableState::sorted(
                "analytics-agents",
                c::ANALYTICS_AGENT_COLS,
                c::BUSIEST_FIRST,
            ),
            chats: TableState::sorted("chats", c::CHAT_COLS, c::NEWEST_FIRST),
            chat_runs: TableState::sorted("chat-runs", c::CHAT_RUN_COLS, c::NEWEST_FIRST),
            runs: TableState::sorted("runs", c::RUN_COLS, c::NEWEST_FIRST),
            dashboards: TableState::new("dashboards", c::DASHBOARD_COLS),
            dashboards_archived: TableState::new("dashboards-archived", c::DASHBOARD_COLS),
            db_scopes: TableState::new("db-scopes", c::DB_SCOPE_COLS),
            db_tables: TableState::new("db-tables", c::DB_TABLE_COLS),
            hive: TableState::new("hive", c::HIVE_COLS),
            leases: TableState::new("leases", c::LEASE_COLS),
            used_ports: TableState::new("used-ports", c::USED_PORT_COLS),
            mesh_allow: TableState::new("mesh-allow", c::MESH_ALLOW_COLS),
            mesh_peers: TableState::new("mesh-peers", c::MESH_PEER_COLS),
            mesh_forwards: TableState::new("mesh-forwards", c::MESH_FORWARD_COLS),
            fleet: TableState::new("fleet", c::FLEET_COLS),
            projects: TableState::new("projects", c::PROJECT_COLS),
            projects_archived: TableState::new("projects-archived", c::PROJECT_ARCHIVED_COLS),
            secrets: TableState::new("secrets", c::SECRET_COLS),
            knowledge_bases: TableState::new("knowledge-bases", c::KNOWLEDGE_BASE_COLS),
            knowledge_notes: TableState::new("knowledge-notes", c::KNOWLEDGE_NOTE_COLS),
            tasks: TableState::new("tasks", c::TASK_COLS),
            tasks_done: TableState::new("tasks-done", c::TASK_COLS),
            tools: TableState::new("tools", c::TOOL_COLS),
            tools_archived: TableState::new("tools-archived", c::TOOL_COLS),
            triggers: TableState::new("triggers", c::TRIGGER_COLS),
            workspaces: TableState::new("workspaces", c::WORKSPACE_COLS),
            hooks: TableState::new("hooks", c::HOOK_COLS),
            project_agents: TableState::new("project-agents", c::PROJECT_AGENT_COLS),
            project_secrets: TableState::new("project-secrets", c::PROJECT_SECRET_COLS),
            project_knowledge_bases: TableState::new(
                "project-knowledge-bases",
                c::PROJECT_KNOWLEDGE_BASE_COLS,
            ),
            project_knowledge_notes: TableState::new(
                "project-knowledge-notes",
                c::KNOWLEDGE_NOTE_COLS,
            ),
            project_tasks: TableState::new("project-tasks", c::PROJECT_TASK_COLS),
            project_tools: TableState::new("project-tools", c::PROJECT_TOOL_COLS),
            project_triggers: TableState::new("project-triggers", c::PROJECT_TRIGGER_COLS),
            files: TableState::new("files", c::FILE_COLS),
            services: TableState::new("services", c::SERVICE_COLS),
            subprojects: TableState::new("subprojects", c::SUBPROJECT_COLS),
        }
    }
}

impl State {
    /// A fresh state with every signal empty. Used by the standalone dashboard-agent embed page,
    /// which reuses the agent chat components but not the main App shell (which seeds its own
    /// project/section from the path).
    pub(crate) fn fresh() -> Self {
        Self {
            status: RwSignal::new(Status::Connecting),
            ports: RwSignal::new(None),
            health: RwSignal::new(None),
            flash: RwSignal::new(None),
            secs_since: RwSignal::new(0),
            used: RwSignal::new(None),
            mesh: RwSignal::new(None),
            fleet: RwSignal::new(None),
            projects: RwSignal::new(None),
            project_detail: RwSignal::new(None),
            current_project: RwSignal::new(String::new()),
            current_section: RwSignal::new(ProjectSection::Overview),
            tasks: RwSignal::new(None),
            agents: RwSignal::new(None),
            all_chats: RwSignal::new(None),
            tools: RwSignal::new(None),
            secrets: RwSignal::new(None),
            db: RwSignal::new(None),
            meta: RwSignal::new(None),
            triggers: RwSignal::new(None),
            hive: RwSignal::new(None),
            dashboards: RwSignal::new(None),
            fleet_dashboards: RwSignal::new(None),
            fleet_dashboards_busy: RwSignal::new(false),
            fleet_unlock: FleetUnlock::new(),
            workspaces: RwSignal::new(None),
            files: FilesState::new(),
            store: StoreBrowser::new(),
            row_menu: RwSignal::new(None),
            session_menu: RwSignal::new(None),
            show_hidden: RwSignal::new(false),
            starred_only: RwSignal::new(false),
            rail_limit: RwSignal::new(SESSION_PAGE),
            chat_drawer: RwSignal::new(None),
            tables: Tables::new(),
        }
    }
}

/// A side rail of the chat home, when it is showing as a drawer over the conversation.
///
/// Only one is ever open: they come in from opposite edges and each covers most of the screen, so
/// two at once would be two scrims and a chat you cannot see either way.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ChatDrawer {
    /// The left rail: the agent picker and its sessions.
    Sessions,
    /// The right rail: the open conversation's analytics, or the live dashboards when none is open.
    Right,
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

/// An open row-actions (kebab) menu, shared by every table: which row it belongs to (a caller-
/// supplied key, unique among the rows on screen) and where to anchor. `right`/`top` are distances
/// from the viewport's right/top edges, so the menu opens leftward from the right-aligned kebab and
/// never spills off the right edge. `None` when no menu is showing.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RowMenu {
    pub(crate) key: String,
    pub(crate) right: i32,
    pub(crate) top: i32,
}

/// An open right-click menu on one of the chat rail's session rows. Carries which session was
/// right-clicked — the rail spans every agent, so a row is named by its agent *and* run id — and
/// both of the row's flags, which are what decide between Hide and Unhide and between Star and
/// Unstar.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SessionMenu {
    pub(crate) agent: String,
    pub(crate) run_id: String,
    /// The row's title, shown as the menu's heading so it is unmistakable which chat is being acted
    /// on — a rail can hold several rows that read alike.
    pub(crate) title: String,
    pub(crate) hidden: bool,
    pub(crate) starred: bool,
    /// Where to draw, in viewport pixels (the menu is `position: fixed`).
    pub(crate) x: i32,
    /// See [`x`](Self::x).
    pub(crate) y: i32,
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
    /// Where the task's work happens — the directory a run picking it up starts in. Blank leaves
    /// it to the agent's own home; a subtask inherits its parent's.
    pub(crate) cwd: RwSignal<String>,
    pub(crate) details: RwSignal<String>,
    pub(crate) busy: RwSignal<bool>,
    /// Whether the collapsed block of finished tasks at the foot of the page is open. Done and
    /// archived tasks are hidden by default so the tree shows only what is still open.
    pub(crate) show_done: RwSignal<bool>,
}

/// The Dashboards page's create form, plus whether the collapsed archive below the main table is
/// open. Archived dashboards are hidden by default; expanding is the only way to see and restore
/// them.
///
/// The `transfer_*` half is the "run this on a node" panel (`docs/fleet.md` §10). It is a *page*
/// form rather than per-row state because only one transfer is ever being set up at a time:
/// `transfer_id` naming the dashboard is what opens the panel, and clearing it is what closes it.
#[derive(Clone, Copy)]
pub(crate) struct DashboardsForm {
    pub(crate) name: RwSignal<String>,
    pub(crate) description: RwSignal<String>,
    pub(crate) busy: RwSignal<bool>,
    pub(crate) show_archived: RwSignal<bool>,
    /// The dashboard being transferred (its id), or empty when the panel is closed.
    pub(crate) transfer_id: RwSignal<String>,
    /// The destination node's petname.
    pub(crate) transfer_node: RwSignal<String>,
    /// Whether the local copy is stood down once the node has it — `false` is a plain copy.
    pub(crate) transfer_move: RwSignal<bool>,
    /// With a move, also delete the local directory. Off by default: the node's copy would then be
    /// the only one in existence.
    pub(crate) transfer_delete: RwSignal<bool>,
    /// The node's own Basic-auth password, typed per transfer. Never persisted — not here, and not
    /// on the server (`docs/fleet.md` §8).
    pub(crate) transfer_password: RwSignal<String>,
    /// Set while the upload is in flight; a transfer crosses a relay and carries files, so it is
    /// the one action on this page that is visibly slow.
    pub(crate) transfer_busy: RwSignal<bool>,
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

/// Everything the Knowledge page holds that isn't on the server: the search box and its last
/// answer, which base is open, and the two create forms.
///
/// Page-local, like [`DbConsole`]: a base's counts are a status pass over its storage, so they
/// are fetched when the page is looked at rather than polled into the shell state every four
/// seconds whatever page is open.
#[derive(Clone, Copy)]
pub(crate) struct KnowledgeConsole {
    /// Every base and the providers this build offers, or `None` before the first load.
    pub(crate) state: RwSignal<Option<KnowledgeState>>,
    /// The search box.
    pub(crate) query: RwSignal<String>,
    /// Rank by words instead of by meaning — no model, no wait, and no idea what a synonym is.
    pub(crate) words: RwSignal<bool>,
    /// The base the search is narrowed to; empty searches every one of them.
    pub(crate) scope: RwSignal<String>,
    /// The last search's answer, or `None` before anything was asked.
    pub(crate) results: RwSignal<Option<KnowledgeResults>>,
    /// Which base's notes are open below the search, or empty for none.
    pub(crate) open_base: RwSignal<String>,
    pub(crate) notes: RwSignal<Option<KnowledgeNotes>>,
    /// The note open in the reader — the whole body, which a list never shows.
    pub(crate) open_note: RwSignal<Option<KnowledgeNoteDto>>,
    /// The new-base form.
    pub(crate) new_base: RwSignal<String>,
    pub(crate) new_provider: RwSignal<String>,
    /// The new-note form.
    pub(crate) title: RwSignal<String>,
    pub(crate) body: RwSignal<String>,
    pub(crate) tags: RwSignal<String>,
    /// Why the last action failed, kept beside the page rather than in the shared flash: a
    /// search that could not load the model is about this page and nothing else.
    pub(crate) error: RwSignal<Option<String>>,
    /// Whether a request is in flight — a search may be loading a model, and the page says so
    /// rather than looking broken for the several seconds that takes.
    pub(crate) busy: RwSignal<bool>,
}

impl KnowledgeConsole {
    pub(crate) fn new() -> Self {
        Self {
            state: RwSignal::new(None),
            query: RwSignal::new(String::new()),
            words: RwSignal::new(false),
            scope: RwSignal::new(String::new()),
            results: RwSignal::new(None),
            open_base: RwSignal::new(String::new()),
            notes: RwSignal::new(None),
            open_note: RwSignal::new(None),
            new_base: RwSignal::new(String::new()),
            new_provider: RwSignal::new(String::new()),
            title: RwSignal::new(String::new()),
            body: RwSignal::new(String::new()),
            tags: RwSignal::new(String::new()),
            error: RwSignal::new(None),
            busy: RwSignal::new(false),
        }
    }

    /// Forget the open base, its notes, and the reader — what leaving the page does, so coming
    /// back shows the store as it is now rather than as it was.
    pub(crate) fn close(self) {
        self.open_base.set(String::new());
        self.notes.set(None);
        self.open_note.set(None);
        self.error.set(None);
    }
}

/// The Database page's console: which scope is open, that scope's tables, the SQL buffer, and
/// whatever the last run produced.
///
/// Reading and writing are separate actions here for the same reason they're separate endpoints —
/// `Run` holds a read-only connection server-side, so browsing can't mutate — and the two results
/// are different shapes, hence both `rows` and `exec`. `Copy` so it threads into the view and
/// async handlers.
#[derive(Clone, Copy)]
pub(crate) struct DbConsole {
    /// The open scope: a project id, or empty for the global database.
    pub(crate) project: RwSignal<String>,
    /// The open scope's tables and views, or `None` before the first load.
    pub(crate) tables: RwSignal<Option<DbTablesState>>,
    /// The `create` statements for the open scope (or one table), or empty when the schema panel
    /// is closed. Reading this before querying a table someone else made is the advice the guide
    /// gives agents, so the panel offers it too.
    pub(crate) schema: RwSignal<String>,
    /// The SQL buffer.
    pub(crate) sql: RwSignal<String>,
    /// The last `Run`'s result set, or `None` if the last action wrote (or nothing ran yet).
    pub(crate) rows: RwSignal<Option<DbQueryResult>>,
    /// The last `Execute`'s counts, or `None` if the last action read.
    pub(crate) exec: RwSignal<Option<DbExecResult>>,
    /// Why the last statement failed, or `None`. Kept beside the results rather than in the shared
    /// flash: a SQL error belongs next to the SQL that caused it.
    pub(crate) error: RwSignal<Option<String>>,
    /// Whether a statement is in flight.
    pub(crate) busy: RwSignal<bool>,
}

impl DbConsole {
    pub(crate) fn new() -> Self {
        Self {
            project: RwSignal::new(String::new()),
            tables: RwSignal::new(None),
            schema: RwSignal::new(String::new()),
            sql: RwSignal::new(String::new()),
            rows: RwSignal::new(None),
            exec: RwSignal::new(None),
            error: RwSignal::new(None),
            busy: RwSignal::new(false),
        }
    }

    /// The open scope as the API wants it — `None` for global.
    pub(crate) fn scope(self) -> Option<String> {
        let project = self.project.get_untracked();
        (!project.is_empty()).then_some(project)
    }

    /// Drop whatever the last statement produced, before running the next one.
    pub(crate) fn clear_result(self) {
        self.rows.set(None);
        self.exec.set(None);
        self.error.set(None);
    }
}

/// The Tools page's script editor panel: which tool's script is open (`None` = closed), the
/// resolved on-disk path, the runtime (for syntax highlighting), the edit buffer with its saved
/// baseline, a busy flag, and any load error. `Copy` so it threads into the view and async
/// handlers.
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
    /// The knowledge bases this agent works with (its per-base checkboxes), by written id. A
    /// wish list rather than a grant: what it actually reaches is decided by the three isolation
    /// levels at read time, which is why the checkbox list marks the ones out of its scope.
    pub(crate) knowledge: RwSignal<BTreeSet<String>>,
    /// Whether the agent keeps a memory of its own — the `agent:<name>/memory` base, which it
    /// alone writes and every other agent may read.
    pub(crate) memory: RwSignal<bool>,
    /// The bases the checkbox list offers, fetched once when the form first renders them. Held on
    /// the form rather than in the shell state because a base's counts are a status pass over its
    /// storage — not something the 4s poll should run on every page.
    pub(crate) knowledge_bases: RwSignal<Option<Vec<KnowledgeBaseDto>>>,
    /// Commands every run of this agent really runs before its first message reaches the model,
    /// one per line — its standing orientation read. Parsed on submit.
    pub(crate) prelude: RwSignal<String>,
    /// Extra `PATH` dirs for the agent's runs, one per line — how an agent pins a toolchain the
    /// machine's default `PATH` doesn't point at. Parsed on submit.
    pub(crate) path: RwSignal<String>,
    /// Extra environment variables for the agent's runs, one `KEY=VALUE` per line. Parsed on submit.
    pub(crate) env: RwSignal<String>,
    pub(crate) system_prompt: RwSignal<String>,
    pub(crate) starred: RwSignal<bool>,
    /// Whether the agent runs with nobody watching — the `Ask` tool refuses on an unattended agent,
    /// so a question can never leave the work stopped in silence.
    pub(crate) unattended: RwSignal<bool>,
    /// The complete backend argument map loaded for editing, including structured values the
    /// schema-driven form does not render directly.
    pub(crate) arguments: RwSignal<BTreeMap<String, serde_json::Value>>,
    /// String representations for schema-rendered scalar backend arguments.
    pub(crate) argument_values: RwSignal<BTreeMap<String, String>>,
    pub(crate) editing: RwSignal<Option<String>>,
    pub(crate) busy: RwSignal<bool>,
}

impl AgentsForm {
    /// A blank form — "New agent" on the Agents page, and the starting point the onboarding
    /// wizard seeds from `/api/meta`.
    pub(crate) fn new() -> Self {
        Self {
            name: RwSignal::new(String::new()),
            backend: RwSignal::new(String::new()),
            project: RwSignal::new(String::new()),
            model: RwSignal::new(String::new()),
            permission_mode: RwSignal::new(String::new()),
            temperature: RwSignal::new(String::new()),
            max_turns: RwSignal::new(String::new()),
            tags: RwSignal::new(String::new()),
            tools: RwSignal::new(String::new()),
            bin_tools: RwSignal::new(BTreeSet::new()),
            knowledge: RwSignal::new(BTreeSet::new()),
            memory: RwSignal::new(false),
            knowledge_bases: RwSignal::new(None),
            secrets: RwSignal::new(BTreeSet::new()),
            prelude: RwSignal::new(String::new()),
            path: RwSignal::new(String::new()),
            env: RwSignal::new(String::new()),
            system_prompt: RwSignal::new(String::new()),
            starred: RwSignal::new(false),
            unattended: RwSignal::new(false),
            arguments: RwSignal::new(BTreeMap::new()),
            argument_values: RwSignal::new(BTreeMap::new()),
            editing: RwSignal::new(None),
            busy: RwSignal::new(false),
        }
    }
}

/// The Meta page's setup form for the default `adi-agent`: the chosen backend and the (editable)
/// system prompt, a busy flag while a save is in flight, and `editing` — true while reconfiguring
/// an agent that already exists (the same form doubles as create and edit). `Copy` so it threads
/// into the page view and handlers. Seeded from the server's default prompt on first load (see
/// [`crate::App`]).
#[derive(Clone, Copy)]
pub(crate) struct MetaForm {
    /// The selected backend id (`pty:claude`, `process:codex`, …).
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
    /// Restrict which projects may fire this trigger — the checked project ids. Empty means
    /// unrestricted (fires for every project). For event/webhook triggers the project is read
    /// from the fire's payload.
    pub(crate) trigger_on: RwSignal<Vec<String>>,
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

/// The project detail page's workspace terminal view: which workspace's pty terminal is
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
    /// `None`). The pty session itself keeps running — closing the view never kills it.
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

/// The Agents page's live view: which agent's pty pane is being watched (`None` = closed), the
/// latest snapshot, and the send-bar input buffer. The shell polls a fresh peek every second
/// while open; leaving the page closes it. `Copy` so it threads into the poll closure and
/// handlers.
#[derive(Clone, Copy)]
pub(crate) struct AgentsWatch {
    /// The watched agent's name, or `None` while the live view is closed.
    pub(crate) name: RwSignal<Option<String>>,
    /// Whether the watched agent is interactive (pty) — it then shows a live pane and a send bar.
    /// A headless agent shows its run history and a task composer instead.
    pub(crate) interactive: RwSignal<bool>,
    /// For a headless agent, the run whose log the view is showing (or `None` = none selected yet).
    pub(crate) run_id: RwSignal<Option<String>>,
    /// For a headless agent, its run history (newest first), refreshed by the poll.
    pub(crate) runs: RwSignal<Vec<AgentRunInfo>>,
    /// Whether the watched agent's runs are answerable conversations (harness backends): the run
    /// detail shows a chat transcript + reply box rather than a plain log. Set by the poll.
    pub(crate) answerable: RwSignal<bool>,
    /// The last snapshot received, or `None` before the first one lands.
    pub(crate) peek: RwSignal<Option<AgentPeek>>,
    /// The selected run's log tail, kept apart from `peek` so the inline viewer binds to a plain
    /// `String` signal and the poll only touches it when the log actually grew — the log follows
    /// (`tail -f`) without the whole panel re-rendering each second.
    pub(crate) log: RwSignal<String>,
    /// Text buffer: the send bar (pty) or the run composer's task (headless).
    pub(crate) input: RwSignal<String>,
    /// Text buffer for the chat reply box under a selected answerable (harness) conversation —
    /// kept apart from `input` (the new-conversation composer) so the two don't clobber each other.
    pub(crate) reply: RwSignal<String>,
    /// True while an answer to the open conversation's question is in flight, so the card cannot be
    /// sent twice by an impatient second click on a request that is already gone.
    pub(crate) answering: RwSignal<bool>,
    /// A context line prepended to every message this view sends (new run *and* reply). Empty in the
    /// normal app; the dashboard-agent embed sets it to "editing dashboard <id> …" so the one global
    /// `adi-agent` session always knows which dashboard it was opened from.
    pub(crate) context_prefix: RwSignal<String>,
    /// Where the *next* run started from this composer should begin, when the human wants one run
    /// somewhere other than the agent's usual home — a recon agent pointed at this target, a
    /// reviewer pointed at that checkout. Empty means "as the agent is defined", which is the
    /// normal case; it applies to the launch only, and the conversation then keeps that directory
    /// for its replies.
    pub(crate) run_dir: RwSignal<String>,
    /// The open conversation's token itemization: what its context went on, and what it sent twice.
    ///
    /// The one thing in this struct the poll never touches. Everything else here is refreshed each
    /// second because it is free once the peek has landed; this costs a tokenizer pass over the whole
    /// transcript, so it is fetched when a reader asks for it and then left alone.
    pub(crate) tokens: RwSignal<Option<AgentTokens>>,
    /// Which run the report in `tokens` is *of*. Opening another conversation must not leave the
    /// previous one's numbers on screen under a new title, and a report carries no such claim itself.
    pub(crate) tokens_of: RwSignal<Option<String>>,
    /// Whether a report is in flight, and what the last attempt failed with (empty when it didn't).
    pub(crate) tokens_busy: RwSignal<bool>,
    pub(crate) tokens_error: RwSignal<String>,
    /// Whether a hand-off to the reviewing agent is in flight. Not `_of`-stamped like the token
    /// report: a review produces no state to leave on screen — the reply is a place to go, and the
    /// screen has gone there by the time it matters.
    pub(crate) review_busy: RwSignal<bool>,
    /// The open conversation's goals — what it is for, open and closed alike.
    ///
    /// Fetched when a conversation is opened and after every write, not on the poll: a goal changes
    /// when somebody changes it, and the once-a-second snapshot has no reason to carry a list that
    /// is nearly always the same three rows.
    pub(crate) goals: RwSignal<Vec<AgentGoal>>,
    /// Which conversation the list in `goals` is *of*, for the reason `tokens_of` exists: opening
    /// another chat must not leave the last one's goals on screen under a new title.
    pub(crate) goals_of: RwSignal<Option<String>>,
    /// The goal input, and whether a goal write is in flight.
    pub(crate) goal_input: RwSignal<String>,
    pub(crate) goal_busy: RwSignal<bool>,
    /// Whether the goal editor is open. Closed is the normal state and it costs one line — most
    /// conversations never have a goal, and a permanently open text box for something rarely used
    /// takes more of the screen than the composer it sits above.
    pub(crate) goal_editor: RwSignal<bool>,
    /// The goal the open editor is rewording, or `None` when it is writing a new one.
    pub(crate) goal_editing: RwSignal<Option<String>>,
    /// Images attached to the message the *new-conversation* composer is holding.
    ///
    /// Two trays for the same reason there are two text buffers: the box that starts a conversation
    /// and the box that answers in one are different messages, and a screenshot pasted into one must
    /// not turn up attached to the other.
    pub(crate) input_files: RwSignal<Vec<adi_ui::Attached>>,
    /// Images attached to the reply being typed into the open conversation.
    pub(crate) reply_files: RwSignal<Vec<adi_ui::Attached>>,
}

impl AgentsWatch {
    pub(crate) fn new() -> Self {
        Self {
            name: RwSignal::new(None),
            interactive: RwSignal::new(false),
            run_id: RwSignal::new(None),
            runs: RwSignal::new(Vec::new()),
            answerable: RwSignal::new(false),
            peek: RwSignal::new(None),
            log: RwSignal::new(String::new()),
            input: RwSignal::new(String::new()),
            reply: RwSignal::new(String::new()),
            answering: RwSignal::new(false),
            context_prefix: RwSignal::new(String::new()),
            run_dir: RwSignal::new(String::new()),
            tokens: RwSignal::new(None),
            tokens_of: RwSignal::new(None),
            tokens_busy: RwSignal::new(false),
            tokens_error: RwSignal::new(String::new()),
            review_busy: RwSignal::new(false),
            goals: RwSignal::new(Vec::new()),
            goals_of: RwSignal::new(None),
            goal_input: RwSignal::new(String::new()),
            goal_busy: RwSignal::new(false),
            goal_editor: RwSignal::new(false),
            goal_editing: RwSignal::new(None),
            input_files: RwSignal::new(Vec::new()),
            reply_files: RwSignal::new(Vec::new()),
        }
    }

    /// Close the live view (stops the polling; `poll_watch` no-ops while `name` is `None`).
    pub(crate) fn close(self) {
        self.name.set(None);
        self.interactive.set(false);
        self.run_id.set(None);
        self.runs.set(Vec::new());
        self.answerable.set(false);
        self.peek.set(None);
        self.log.set(String::new());
        self.input.set(String::new());
        self.reply.set(String::new());
        self.context_prefix.set(String::new());
        self.tokens.set(None);
        self.tokens_of.set(None);
        self.tokens_busy.set(false);
        self.tokens_error.set(String::new());
        self.review_busy.set(false);
        self.goals.set(Vec::new());
        self.goals_of.set(None);
        self.goal_input.set(String::new());
        self.goal_busy.set(false);
        self.goal_editor.set(false);
        self.goal_editing.set(None);
        self.input_files.set(Vec::new());
        self.reply_files.set(Vec::new());
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

/// The Fleet page's local signals: the grant form's node picker and grant text, plus a shared
/// busy flag. Renaming and unpairing are row actions rather than form fields — they name their
/// node by the row you clicked — so nothing here holds a petname of its own. `Copy`, so it
/// threads into the page view and its handlers.
#[derive(Clone, Copy)]
pub(crate) struct FleetForm {
    /// Which node the grant lands on (a petname, empty until picked).
    pub(crate) grant_node: RwSignal<String>,
    /// The grant in its string form: `http:*`, `http:nosh`, `tcp:127.0.0.1:22`, `ctl:read`.
    pub(crate) grant: RwSignal<String>,
    pub(crate) busy: RwSignal<bool>,
}

impl FleetForm {
    pub(crate) fn new() -> Self {
        Self {
            grant_node: RwSignal::new(String::new()),
            grant: RwSignal::new(String::new()),
            busy: RwSignal::new(false),
        }
    }
}

/// The dashboards rail's unlock form: the one node whose password is being typed, and what has
/// been typed into it.
///
/// One form and not one per node, deliberately — only one can be open at a time, so a password
/// typed for `laptop-b` can never be submitted against `studio` because the rail re-rendered under
/// it. Clearing [`node`](Self::node) is what closes the form, and it takes the password with it:
/// nothing typed here outlives the row it was typed into.
#[derive(Clone, Copy)]
pub(crate) struct FleetUnlock {
    /// The petname whose form is open, or empty when none is.
    pub(crate) node: RwSignal<String>,
    pub(crate) password: RwSignal<String>,
    pub(crate) busy: RwSignal<bool>,
    /// What the node said when it refused, shown under the form. `None` until it has.
    pub(crate) error: RwSignal<Option<String>>,
}

impl FleetUnlock {
    pub(crate) fn new() -> Self {
        Self {
            node: RwSignal::new(String::new()),
            password: RwSignal::new(String::new()),
            busy: RwSignal::new(false),
            error: RwSignal::new(None),
        }
    }

    /// Open the form on `node`, from a clean slate: the previous node's password and refusal must
    /// never carry over into this one.
    pub(crate) fn open(self, node: &str) {
        self.node.set(node.to_string());
        self.password.set(String::new());
        self.error.set(None);
    }

    /// Close it, dropping what was typed — in particular the password, which has no reason to
    /// outlive the form.
    pub(crate) fn close(self) {
        self.node.set(String::new());
        self.password.set(String::new());
        self.error.set(None);
    }
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

/// The simulator's own state: which run is open, what has been staged into the turn that is still
/// open, and the flags taken while reading.
///
/// Deliberately apart from [`AgentsWatch`]. That view watches a run somebody else is doing; this one
/// *is* the run. Sharing a struct would mean one poll clobbering the other's idea of which run is on
/// screen, and the two are never the same run.
#[derive(Clone, Copy)]
pub(crate) struct Simulate {
    /// The agent being simulated, or `None` while the simulator is closed.
    pub(crate) name: RwSignal<Option<String>>,
    /// The run as the server last reported it — the composed prompt, its split, its tools, its
    /// turns. `None` between opening the simulator and the run existing.
    pub(crate) run: RwSignal<Option<AgentSimState>>,
    /// The declared tools, with a signal-backed field per parameter. Rebuilt on every landing, so a
    /// call that has been made does not leave its arguments sitting in the form.
    pub(crate) tools: RwSignal<Vec<ToolDecl>>,
    /// What has been emitted into the open turn. Held here rather than server-side because nothing
    /// has happened yet: a staged block is a thing a person may still drop.
    pub(crate) blocks: RwSignal<Vec<Block>>,
    /// Passages marked while reading, each with its note. These are the point of the feature.
    pub(crate) flags: RwSignal<Vec<Flag>>,
    /// A request is in flight — a turn executing, or a reply being sent.
    pub(crate) busy: RwSignal<bool>,
}

impl Simulate {
    pub(crate) fn new() -> Self {
        Self {
            name: RwSignal::new(None),
            run: RwSignal::new(None),
            tools: RwSignal::new(Vec::new()),
            blocks: RwSignal::new(Vec::new()),
            flags: RwSignal::new(Vec::new()),
            busy: RwSignal::new(false),
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

/// Write `value` into `sig` only when it differs from what's already there. The 4s poll re-fetches
/// every page's data and, without this, re-set each signal unconditionally — notifying subscribers
/// and tearing down their DOM every tick even when nothing changed, which dropped input focus and
/// reset scroll offsets mid-interaction. Gating on real change keeps a settled page's reactive graph
/// perfectly still between polls; only genuinely new data re-renders. Reads untracked so `load`
/// itself never subscribes to anything.
fn set_if_changed<T: PartialEq + Send + Sync + 'static>(sig: RwSignal<Option<T>>, value: T) {
    if sig.with_untracked(|current| current.as_ref() != Some(&value)) {
        sig.set(Some(value));
    }
}

/// Ask every paired node what it is running, and fold the answer into the dashboards rail.
///
/// **Deliberately not a subscription.** Every other list on the page is local state a poll can read
/// for free; this one is an authenticated HTTP call to each node over the mesh, which is a third of
/// a second before any payload (`docs/fleet.md` §9) and wakes a relay to get there. So it is asked
/// when the page loads and when a person asks again — never on a four-second timer.
///
/// A refusal is a flash and nothing else: the rail keeps whatever it last knew, because a fleet
/// that was there a moment ago is better information than an empty list.
pub(crate) fn refresh_fleet_dashboards(s: State) {
    if s.fleet_dashboards_busy.get_untracked() {
        return;
    }
    s.fleet_dashboards_busy.set(true);
    wasm_bindgen_futures::spawn_local(async move {
        match fetch::fleet_dashboards().await {
            Ok(f) => set_if_changed(s.fleet_dashboards, f),
            Err(e) => s.flash.set(Some(Flash::err(e))),
        }
        s.fleet_dashboards_busy.set(false);
    });
}

/// What the live channel should be watching for the page that is open, and where each answer
/// goes — the subscription form of [`load`] plus the per-second watches the shell used to poll.
///
/// This is deliberately the same shape as [`load`], read the same way: an entry here is the very
/// request the line beside it in `load` makes. The two are kept side by side so a page that starts
/// needing something new is one edit in each, and the fallback path can never fetch something the
/// live path forgets to watch.
///
/// Reads its signals *tracked*, so the effect that calls it re-runs — and re-subscribes — the
/// moment the page moves: a route change, a different project, another chat opened.
pub(crate) fn subscriptions(
    s: State,
    route: Route,
    watch: AgentsWatch,
    triggers_log: TriggersLogView,
    hook_log: HookLogView,
    term: TermWatch,
) -> Vec<Sub> {
    let mut subs = vec![
        // Liveness and uptime. Every message is a sign of life, but this is the one that arrives
        // whether or not anything on the page is changing.
        Sub::get("/api/health", move |health: Health| {
            set_if_changed(s.health, health);
            s.status.set(Status::Online);
        }),
        // The explorer renders the project tree on every route, so the project list is shell data
        // rather than something an individual page opts into.
        Sub::get("/api/projects", move |p: ProjectsState| {
            set_if_changed(s.projects, p);
        }),
    ];

    // Page-specific data, watched only where it's shown.
    if route == Route::Projects {
        // The list shows a per-project open-task count, so it needs the task tree too.
        subs.push(Sub::get("/api/tasks", move |t: TasksState| {
            set_if_changed(s.tasks, t);
        }));
    }
    if route == Route::ProjectDetail {
        let id = s.current_project.get();
        if !id.is_empty() {
            subs.push(Sub::get(
                format!("/api/projects/{id}"),
                move |d: ProjectDetail| set_if_changed(s.project_detail, d),
            ));
            subs.push(Sub::get("/api/tasks", move |t: TasksState| {
                set_if_changed(s.tasks, t);
            }));
            subs.push(Sub::get("/api/triggers", move |t: TriggersState| {
                set_if_changed(s.triggers, t);
            }));
            subs.push(Sub::get("/api/agents", move |a: AgentsState| {
                set_if_changed(s.agents, a);
            }));
            // The cross-agent "All chats" index above the project's Agents panel.
            subs.push(Sub::get("/api/agents/runs/all", move |c: AllAgentRuns| {
                set_if_changed(s.all_chats, c);
            }));
            // The project's Tools panel lists the tools filed under it (from the shared list).
            subs.push(Sub::get("/api/tools", move |t: ToolsState| {
                set_if_changed(s.tools, t);
            }));
            // The project's Secrets panel filters the shared secrets list to this project.
            subs.push(Sub::get("/api/secrets", move |sec: SecretsState| {
                set_if_changed(s.secrets, sec);
            }));
            // The Workspaces panel's snapshot; watching it flips `creating` → `ready` live.
            subs.push(Sub::post(
                "/api/projects/workspaces",
                &WorkspacesRef { id },
                move |w: WorkspacesState| set_if_changed(s.workspaces, w),
            ));
        }
    }
    if route == Route::Tasks {
        subs.push(Sub::get("/api/tasks", move |t: TasksState| {
            set_if_changed(s.tasks, t);
        }));
    }
    if route == Route::Meta {
        subs.push(Sub::get("/api/meta", move |m: MetaState| {
            set_if_changed(s.meta, m);
        }));
    }
    if route == Route::Analytics {
        // The whole page is these two listings joined: what is defined, and what it has run.
        subs.push(Sub::get("/api/agents", move |a: AgentsState| {
            set_if_changed(s.agents, a);
        }));
        subs.push(Sub::get("/api/agents/runs/all", move |c: AllAgentRuns| {
            set_if_changed(s.all_chats, c);
        }));
    }
    if route == Route::Agents {
        subs.push(Sub::get("/api/agents", move |a: AgentsState| {
            set_if_changed(s.agents, a);
        }));
        // The cross-agent "All chats" index at the top of the Agents page.
        subs.push(Sub::get("/api/agents/runs/all", move |c: AllAgentRuns| {
            set_if_changed(s.all_chats, c);
        }));
        // The agent form's per-tool and per-secret checkboxes (metadata only — a secret's value
        // is never fetched here).
        subs.push(Sub::get("/api/tools", move |t: ToolsState| {
            set_if_changed(s.tools, t);
        }));
        subs.push(Sub::get("/api/secrets", move |sec: SecretsState| {
            set_if_changed(s.secrets, sec);
        }));
    }
    if route == Route::Tools {
        subs.push(Sub::get("/api/tools", move |t: ToolsState| {
            set_if_changed(s.tools, t);
        }));
    }
    if route == Route::Secrets {
        subs.push(Sub::get("/api/secrets", move |sec: SecretsState| {
            set_if_changed(s.secrets, sec);
        }));
    }
    if route == Route::Database {
        subs.push(Sub::get("/api/db", move |d: DbState| {
            set_if_changed(s.db, d);
        }));
    }
    if route == Route::Triggers {
        subs.push(Sub::get("/api/triggers", move |t: TriggersState| {
            set_if_changed(s.triggers, t);
        }));
    }
    if route == Route::Hive {
        subs.push(Sub::get("/api/hive", move |h: HiveState| {
            set_if_changed(s.hive, h);
        }));
        // The Hive table lists dashboard services too, and names their source — which needs the
        // dashboards' own listing, since a service carries only its dashboard's id.
        subs.push(Sub::get("/api/dashboards", move |d: DashboardsState| {
            set_if_changed(s.dashboards, d);
        }));
    }
    if route == Route::Dashboards {
        subs.push(Sub::get("/api/dashboards", move |d: DashboardsState| {
            set_if_changed(s.dashboards, d);
        }));
        // The transfer panel's node picker. Every destination a dashboard can be sent to is a
        // paired node, so the page needs the fleet to offer any of them.
        subs.push(Sub::get("/api/fleet", move |f: FleetState| {
            set_if_changed(s.fleet, f);
        }));
    }
    if route == Route::PortsManager {
        // The registry's leases, and the scan of what is actually listening.
        subs.push(Sub::get("/api/ports", move |p: PortsState| {
            set_if_changed(s.ports, p);
        }));
        subs.push(Sub::get("/api/ports/used", move |u: UsedPorts| {
            set_if_changed(s.used, u);
        }));
    }
    if route == Route::Mesh {
        subs.push(Sub::get("/api/mesh", move |m: MeshState| {
            set_if_changed(s.mesh, m);
        }));
    }
    if route == Route::Fleet {
        subs.push(Sub::get("/api/fleet", move |f: FleetState| {
            set_if_changed(s.fleet, f);
        }));
    }

    // The views that used to have a poll each: an open chat, an open log, an open terminal.
    subs.extend(chat_subscriptions(watch));
    if let Some(name) = triggers_log.name.get() {
        subs.push(Sub::post(
            "/api/triggers/log",
            &TriggerRef { name },
            move |snapshot: TriggerLog| set_if_changed(triggers_log.log, snapshot),
        ));
    }
    if let Some((id, name)) = hook_log.watched.get() {
        subs.push(Sub::post(
            "/api/projects/hook/log",
            &ProjectHookRef { id, name },
            move |snapshot: ProjectHookLog| set_if_changed(hook_log.log, snapshot),
        ));
    }
    if let Some((id, name)) = term.watched.get() {
        subs.push(Sub::post(
            "/api/projects/workspaces/terminal/peek",
            &WorkspaceTermRef { id, name },
            move |peek: WorkspaceTerm| set_if_changed(term.peek, peek),
        ));
    }
    subs
}

/// What an open chat watches: the agent's live pane, or its run history and the transcript of
/// whichever run is selected. Shared by the shell, the chat home and the dashboard embed, which
/// all show the same live view.
pub(crate) fn chat_subscriptions(watch: AgentsWatch) -> Vec<Sub> {
    let Some(name) = watch.name.get() else {
        return Vec::new();
    };
    // An interactive (pty) agent keeps no run history — the pane is the whole of it.
    if watch.interactive.get() {
        return vec![Sub::post(
            "/api/agents/peek",
            &AgentRef { name },
            move |peek: AgentPeek| set_if_changed(watch.peek, peek),
        )];
    }

    let mut subs = vec![Sub::post(
        "/api/agents/runs",
        &AgentRef { name: name.clone() },
        move |runs: AgentRuns| {
            // Whether these runs are answerable conversations — drives the chat vs. log view.
            if watch.answerable.get_untracked() != runs.answerable {
                watch.answerable.set(runs.answerable);
            }
            if watch.runs.get_untracked() != runs.runs {
                watch.runs.set(runs.runs);
            }
        },
    )];
    // …and the selected run's transcript, if one is open. The tail feeds a dedicated `log` signal
    // that the inline viewer follows; both are written only on real change, so a finished run's
    // viewer sits perfectly still while a live one still grows.
    if let Some(run_id) = watch.run_id.get() {
        subs.push(Sub::post(
            "/api/agents/run/peek",
            &RunRef { name, run_id },
            move |peek: AgentPeek| {
                if watch.log.get_untracked() != peek.output {
                    watch.log.set(peek.output.clone());
                }
                set_if_changed(watch.peek, peek);
            },
        ));
    }
    subs
}

/// Fetch `/api/health` + `/api/ports` together and fan the result into the signals.
///
/// The fallback path: with the live channel up this never runs (see [`subscriptions`]) — the
/// shell's timers check [`crate::live::connected`] first. It is what keeps the panel working
/// against a backend too old to speak `/api/ws`, or while a socket is down between reconnects.
pub(crate) async fn load(s: State) {
    match (fetch::health().await, fetch::ports().await) {
        (Ok(h), Ok(p)) => {
            set_if_changed(s.health, h);
            set_if_changed(s.ports, p);
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
        set_if_changed(s.projects, p);
    }

    // Page-specific data, fetched only where it's shown.
    let path = current_path();
    if path == Route::Projects.path() {
        // The list shows a per-project open-task count, so it needs the task tree too.
        if let Ok(t) = fetch::tasks().await {
            set_if_changed(s.tasks, t);
        }
    }
    if let Some(id) = project_id_from_path(&path) {
        if let Ok(d) = fetch::project_detail(&id).await {
            set_if_changed(s.project_detail, d);
        }
        if let Ok(t) = fetch::tasks().await {
            set_if_changed(s.tasks, t);
        }
        if let Ok(t) = fetch::triggers().await {
            set_if_changed(s.triggers, t);
        }
        if let Ok(a) = fetch::agents().await {
            set_if_changed(s.agents, a);
        }
        // The cross-agent "All chats" index above the project's Agents panel.
        if let Ok(c) = fetch::all_agent_runs(None).await {
            set_if_changed(s.all_chats, c);
        }
        // The project's Tools panel lists the tools filed under it (from the shared list).
        if let Ok(t) = fetch::tools().await {
            set_if_changed(s.tools, t);
        }
        // The project's Secrets panel filters the shared secrets list to this project.
        if let Ok(sec) = fetch::secrets().await {
            set_if_changed(s.secrets, sec);
        }
        // The Workspaces panel's snapshot; polling it flips `creating` → `ready` live.
        if let Ok(w) = fetch::workspaces(&id).await {
            set_if_changed(s.workspaces, w);
        }
    }
    if path == Route::Tasks.path() {
        if let Ok(t) = fetch::tasks().await {
            set_if_changed(s.tasks, t);
        }
    }
    if path == Route::Analytics.path() {
        // The whole page is these two listings joined: what is defined, and what it has run.
        if let Ok(a) = fetch::agents().await {
            set_if_changed(s.agents, a);
        }
        if let Ok(c) = fetch::all_agent_runs(None).await {
            set_if_changed(s.all_chats, c);
        }
    }
    if path == Route::Meta.path()
        && let Ok(m) = fetch::meta().await
    {
        set_if_changed(s.meta, m);
    }
    if path == Route::Agents.path() {
        if let Ok(a) = fetch::agents().await {
            set_if_changed(s.agents, a);
        }
        // The cross-agent "All chats" index at the top of the Agents page.
        if let Ok(c) = fetch::all_agent_runs(None).await {
            set_if_changed(s.all_chats, c);
        }
        // The agent form's per-tool checkboxes are populated from the tools list.
        if let Ok(t) = fetch::tools().await {
            set_if_changed(s.tools, t);
        }
        // The agent form's per-secret checkboxes are populated from the secrets list (metadata
        // only — values are never fetched here).
        if let Ok(sec) = fetch::secrets().await {
            set_if_changed(s.secrets, sec);
        }
    }
    if path == Route::Tools.path()
        && let Ok(t) = fetch::tools().await
    {
        set_if_changed(s.tools, t);
    }
    if path == Route::Secrets.path()
        && let Ok(sec) = fetch::secrets().await
    {
        set_if_changed(s.secrets, sec);
    }
    if path == Route::Database.path()
        && let Ok(d) = fetch::db().await
    {
        set_if_changed(s.db, d);
    }
    if path == Route::Triggers.path() {
        if let Ok(t) = fetch::triggers().await {
            set_if_changed(s.triggers, t);
        }
    }
    if path == Route::Hive.path() {
        if let Ok(h) = fetch::hive().await {
            set_if_changed(s.hive, h);
        }
        // The Hive table lists dashboard services too, and names their source — which needs the
        // dashboards' own listing, since a service carries only its dashboard's id.
        if let Ok(d) = fetch::dashboards().await {
            set_if_changed(s.dashboards, d);
        }
    }
    if path == Route::Dashboards.path()
        && let Ok(d) = fetch::dashboards().await
    {
        set_if_changed(s.dashboards, d);
    }
    if path == Route::PortsManager.path()
        && let Ok(u) = fetch::used().await
    {
        set_if_changed(s.used, u);
    }
    if path == Route::Mesh.path()
        && let Ok(m) = fetch::mesh().await
    {
        set_if_changed(s.mesh, m);
    }
    if path == Route::Fleet.path()
        && let Ok(f) = fetch::fleet().await
    {
        set_if_changed(s.fleet, f);
    }
}
