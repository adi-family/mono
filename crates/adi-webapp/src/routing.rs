//! Client-side routing: the [`Route`] enum mapping URL paths to pages, the click/history plumbing
//! that navigates without a page reload, and the project-detail navigation helpers.

use leptos::prelude::*;

use crate::state::State;

/// The path prefix every workbench route lives under. The bare root (`/`) is the simple
/// launcher (see `main`); the whole control panel hangs off `/extended/…`. This is the one
/// place the prefix is written for the dynamic path helpers — the fixed routes mirror it as
/// literals in [`Route::path`]/[`Route::from_path`].
pub(crate) const BASE: &str = "/extended";

/// The pages the sidebar navigates between, each mapped to a URL path.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    /// The default ADI agent — set up and run `adi-agent` (`/meta`).
    Meta,
    /// What every agent on this machine has actually run (`/analytics`).
    Analytics,
    Projects,
    /// A single project's detail page (`/projects/<id>`); the id lives in `State::current_project`.
    ProjectDetail,
    /// The read-only task tree (`/tasks`).
    Tasks,
    /// Agent definitions (`/agents`).
    Agents,
    /// One agent's editor — the full definition form on a page of its own, rather than under the
    /// list (`/agents/new`, `/agents/<name>/edit`). Which agent lives in `State::current_agent`,
    /// empty for a definition that doesn't exist yet.
    AgentDetail,
    /// Tool definitions — user CLIs (`/tools`).
    Tools,
    /// Encrypted secrets — global & per-project key-values (`/secrets`).
    Secrets,
    /// Knowledge bases — scoped text notes, searched by meaning (`/knowledge`).
    Knowledge,
    /// The facts base — plain sentences, and the queue of pair decisions that keeps it
    /// honest (`/facts`).
    Facts,
    /// The shared SQLite store — browse tables and run SQL (`/database`).
    Database,
    /// Trigger definitions (`/triggers`).
    Triggers,
    /// Agent-authored dashboards (`/dashboards`).
    Dashboards,
    /// The apps marketplace (`/marketplace`) — apps from manifests this machine trusts, listed
    /// from the cache and installed as git clones pinned to a commit, inert.
    Marketplace,
    Hive,
    PortsManager,
    Mesh,
    /// The paired remote adi nodes — `<service>.<node>.n.adi` (`/settings/fleet`).
    Fleet,
    /// One file from the ADI store, open in the full-width editor (`/files/<path>`). The path
    /// lives in `StoreBrowser::open_file`, the way a project id lives in `current_project`.
    StoreFile,
}

impl Route {
    /// Every page that can be opened by name alone, in the order the explorer lists them.
    ///
    /// This is what the ⌘K menu navigates by (see [`crate::menu`]) and the one place to add a page
    /// to it. [`Route::ProjectDetail`], [`Route::AgentDetail`] and [`Route::StoreFile`] are
    /// deliberately absent: each needs a subject the menu has no way to supply, so a row for one
    /// would open an error rather than a page.
    pub(crate) const NAV: [Route; 17] = [
        Route::Meta,
        Route::Analytics,
        Route::Projects,
        Route::Tasks,
        Route::Agents,
        Route::Tools,
        Route::Secrets,
        Route::Knowledge,
        Route::Facts,
        Route::Database,
        Route::Triggers,
        Route::Dashboards,
        Route::Marketplace,
        Route::Hive,
        Route::PortsManager,
        Route::Mesh,
        Route::Fleet,
    ];

    /// The page for a URL path; `/` and anything unknown resolve to Projects.
    pub(crate) fn from_path(path: &str) -> Self {
        if project_id_from_path(path).is_some() {
            return Route::ProjectDetail;
        }
        if store_path_from_path(path).is_some() {
            return Route::StoreFile;
        }
        if agent_form_from_path(path).is_some() {
            return Route::AgentDetail;
        }
        // Match the remainder after the `/extended` prefix, so the arms stay the short,
        // canonical names. A path without the prefix falls through to Projects.
        match path.strip_prefix(BASE).unwrap_or(path) {
            "/meta" => Route::Meta,
            "/analytics" => Route::Analytics,
            "/tasks" => Route::Tasks,
            "/agents" => Route::Agents,
            "/tools" => Route::Tools,
            "/secrets" => Route::Secrets,
            "/knowledge" => Route::Knowledge,
            "/facts" => Route::Facts,
            "/database" => Route::Database,
            "/triggers" => Route::Triggers,
            "/dashboards" => Route::Dashboards,
            "/marketplace" => Route::Marketplace,
            "/settings/hive" => Route::Hive,
            "/settings/ports-manager" => Route::PortsManager,
            "/settings/mesh" => Route::Mesh,
            "/settings/fleet" => Route::Fleet,
            _ => Route::Projects,
        }
    }

    /// The canonical URL path for this page. `ProjectDetail`'s real path carries an id, so this
    /// returns the list base for it (used only for nav; detail canonicalization is skipped).
    pub(crate) fn path(self) -> &'static str {
        // Full literals under `/extended` (mirrors the prefix in [`BASE`]); the arms in
        // [`Route::from_path`] match these with the prefix stripped back off.
        match self {
            Route::Meta => "/extended/meta",
            Route::Analytics => "/extended/analytics",
            Route::Projects | Route::ProjectDetail => "/extended/projects",
            Route::Tasks => "/extended/tasks",
            // The editor's real path carries the agent name; this base is only used for nav.
            Route::Agents | Route::AgentDetail => "/extended/agents",
            Route::Tools => "/extended/tools",
            Route::Secrets => "/extended/secrets",
            Route::Knowledge => "/extended/knowledge",
            Route::Facts => "/extended/facts",
            Route::Database => "/extended/database",
            Route::Triggers => "/extended/triggers",
            Route::Dashboards => "/extended/dashboards",
            Route::Marketplace => "/extended/marketplace",
            Route::Hive => "/extended/settings/hive",
            Route::PortsManager => "/extended/settings/ports-manager",
            Route::Mesh => "/extended/settings/mesh",
            Route::Fleet => "/extended/settings/fleet",
            // The real path carries the file path; this base is only used for nav fallbacks.
            Route::StoreFile => "/extended/files",
        }
    }

    /// The page title shown in the header.
    pub(crate) fn title(self) -> &'static str {
        match self {
            Route::Meta => "Meta",
            Route::Analytics => "Global analytics",
            Route::Projects => "Projects",
            Route::ProjectDetail => "Project",
            Route::Tasks => "Tasks",
            Route::Agents => "Agents",
            Route::AgentDetail => "Agent",
            Route::Tools => "Tools",
            Route::Secrets => "Secrets",
            Route::Knowledge => "Knowledge",
            Route::Facts => "Facts",
            Route::Database => "Database",
            Route::Triggers => "Triggers",
            Route::Dashboards => "Dashboards",
            Route::Marketplace => "Marketplace",
            Route::Hive => "Hive",
            Route::PortsManager => "Ports manager",
            Route::Mesh => "Mesh",
            Route::Fleet => "Fleet",
            Route::StoreFile => "File",
        }
    }

    /// One line about what is on the page, shown dim beside its name in the ⌘K menu.
    ///
    /// Also *searched* — the menu filters on this as well as on the title (see
    /// [`crate::launcher::Action`]) — so each line spends its words on what somebody would type
    /// while looking for the page without remembering what we called it: "keys" for Secrets,
    /// "sql" for Database, "device" for Fleet.
    pub(crate) fn blurb(self) -> &'static str {
        match self {
            Route::Meta => "The default adi-agent — its model and prompt",
            Route::Analytics => "What every agent has run, and what it cost",
            Route::Projects | Route::ProjectDetail => "Every project on this machine",
            Route::Tasks => "The task tree, across every project",
            Route::Agents | Route::AgentDetail => "Agent definitions, backends and prompts",
            Route::Tools => "The CLIs an agent may run",
            Route::Secrets => "Encrypted keys and API keys",
            Route::Knowledge => "Notes, searched by meaning",
            Route::Facts => "Sentences, and the pairs still to decide",
            Route::Database => "Browse the store's tables, run SQL",
            Route::Triggers => "What runs when something happens",
            Route::Dashboards => "Create, archive, transfer",
            Route::Marketplace => "Install an app someone else published",
            Route::Hive => "Services, and the .adi names in front of them",
            Route::PortsManager => "Reserved ports and what holds them",
            Route::Mesh => "Peers, allowed ports and forwards",
            Route::Fleet => "Paired devices and what they may reach",
            Route::StoreFile => "One file from the ADI store",
        }
    }
}

/// One section of a project — a sub-page under `/projects/<id>/<slug>`. The explorer nests
/// these under each project, so a project is browsed the way a directory is rather than as
/// one long page of stacked panels.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectSection {
    Overview,
    Tasks,
    Agents,
    Triggers,
    Tools,
    Secrets,
    Knowledge,
    Services,
    Workspaces,
    Files,
}

impl ProjectSection {
    /// Every section, in the order the explorer lists them.
    pub(crate) const ALL: [ProjectSection; 10] = [
        ProjectSection::Overview,
        ProjectSection::Tasks,
        ProjectSection::Agents,
        ProjectSection::Triggers,
        ProjectSection::Tools,
        ProjectSection::Secrets,
        ProjectSection::Knowledge,
        ProjectSection::Services,
        ProjectSection::Workspaces,
        ProjectSection::Files,
    ];

    /// The URL segment for this section. Overview is the project's own root, so it has none.
    pub(crate) fn slug(self) -> &'static str {
        match self {
            ProjectSection::Overview => "",
            ProjectSection::Tasks => "tasks",
            ProjectSection::Agents => "agents",
            ProjectSection::Triggers => "triggers",
            ProjectSection::Tools => "tools",
            ProjectSection::Secrets => "secrets",
            ProjectSection::Knowledge => "knowledge",
            ProjectSection::Services => "services",
            ProjectSection::Workspaces => "workspaces",
            ProjectSection::Files => "files",
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            ProjectSection::Overview => "Overview",
            ProjectSection::Tasks => "Tasks",
            ProjectSection::Agents => "Agents",
            ProjectSection::Triggers => "Triggers",
            ProjectSection::Tools => "Tools",
            ProjectSection::Secrets => "Secrets",
            ProjectSection::Knowledge => "Knowledge",
            ProjectSection::Services => "Services",
            ProjectSection::Workspaces => "Workspaces",
            ProjectSection::Files => "Files",
        }
    }

    /// The section for a URL segment; an unknown or empty segment is the overview.
    pub(crate) fn from_slug(slug: &str) -> Self {
        ProjectSection::ALL
            .into_iter()
            .find(|s| s.slug() == slug)
            .unwrap_or(ProjectSection::Overview)
    }

    /// This section's path within a project.
    pub(crate) fn path(self, project: &str) -> String {
        match self.slug() {
            "" => format!("{BASE}/projects/{project}"),
            slug => format!("{BASE}/projects/{project}/{slug}"),
        }
    }
}

/// The canonical href for a project's detail page (`/extended/projects/<id>`). Keeps the base
/// prefix in one place for the plain project links scattered across the pages.
pub(crate) fn project_href(id: &str) -> String {
    ProjectSection::Overview.path(id)
}

/// Whether a click on a link should be taken over and navigated client-side. A modified click
/// (new tab, new window, download) is left to the browser, which is why these links are real
/// `<a href>`s rather than buttons — so `href` still means what it means everywhere else.
///
/// Calling it *consumes* the click (`preventDefault`) when it returns `true`.
pub(crate) fn spa_nav(ev: &web_sys::MouseEvent) -> bool {
    if ev.default_prevented()
        || ev.button() != 0
        || ev.meta_key()
        || ev.ctrl_key()
        || ev.shift_key()
        || ev.alt_key()
    {
        return false;
    }
    ev.prevent_default();
    true
}

/// Handle a click on a nav link: navigate client-side for a plain left-click, but let
/// modified clicks (new tab/window, etc.) fall through to a normal browser navigation.
pub(crate) fn spa_click(ev: &web_sys::MouseEvent, route: RwSignal<Route>, target: Route) {
    if !spa_nav(ev) {
        return;
    }
    if route.get_untracked() != target {
        push_state(target.path());
        route.set(target);
        scroll_top();
    }
}

/// Push a new history entry for `path` without reloading the page.
pub(crate) fn push_state(path: &str) {
    if let Some(h) = web_sys::window().and_then(|w| w.history().ok()) {
        let _ = h.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(path));
    }
}

/// Replace the current history entry's URL (canonicalizes the address bar on first load).
pub(crate) fn replace_state(path: &str) {
    if let Some(h) = web_sys::window().and_then(|w| w.history().ok()) {
        let _ = h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(path));
    }
}

/// Scroll back to the top after a page change.
pub(crate) fn scroll_top() {
    if let Some(w) = web_sys::window() {
        w.scroll_to_with_x_and_y(0.0, 0.0);
    }
}

/// The current URL path, e.g. `/settings/ports-manager`.
pub(crate) fn current_path() -> String {
    web_sys::window()
        .and_then(|w| w.location().pathname().ok())
        .unwrap_or_default()
}

/// A query-string parameter of the current URL (e.g. `?dashboard=<id>`), or `None` if absent.
pub(crate) fn query_param(name: &str) -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    web_sys::UrlSearchParams::new_with_str(&search)
        .ok()?
        .get(name)
        .filter(|v| !v.is_empty())
}

/// The project id in a `/projects/<id>` or `/projects/<id>/<section>` path, or `None` for any
/// other path (including the bare `/projects` list). The id segment must be non-empty.
pub(crate) fn project_id_from_path(path: &str) -> Option<String> {
    let rest = path
        .strip_prefix(BASE)
        .and_then(|p| p.strip_prefix("/projects/"))?;
    let id = rest.split('/').next().unwrap_or_default();
    (!id.is_empty()).then(|| id.to_string())
}

/// The agent an editor URL names: `Some("")` for `/agents/new` — a definition that doesn't exist
/// yet — and `Some(name)` for `/agents/<name>/edit`. `None` for anything else, including the bare
/// `/agents` list.
///
/// Create and edit are told apart by *shape* rather than by reserving a word, so an agent actually
/// named `new` still edits at `/agents/new/edit` instead of reopening the create form.
pub(crate) fn agent_form_from_path(path: &str) -> Option<String> {
    let rest = path
        .strip_prefix(BASE)
        .and_then(|p| p.strip_prefix("/agents/"))?;
    let mut segs = rest.split('/').filter(|seg| !seg.is_empty());
    match (segs.next(), segs.next(), segs.next()) {
        (Some("new"), None, None) => Some(String::new()),
        (Some(name), Some("edit"), None) => Some(name.to_string()),
        _ => None,
    }
}

/// The editor URL for an agent — the create page when `name` is empty, that agent's edit page
/// otherwise. No escaping: an agent name is one segment of letters, digits, `.`, `-` and `_`
/// (`adi_agents::Error::InvalidName`), all of which stand for themselves in a path.
pub(crate) fn agent_form_path(name: &str) -> String {
    match name.trim() {
        "" => format!("{BASE}/agents/new"),
        name => format!("{BASE}/agents/{name}/edit"),
    }
}

/// The store-relative file path in a `/files/<path>` URL, or `None` for any other path. Each
/// segment is percent-decoded, so a name with a space or `#` round-trips through the address bar.
pub(crate) fn store_path_from_path(path: &str) -> Option<String> {
    let rest = path
        .strip_prefix(BASE)
        .and_then(|p| p.strip_prefix("/files/"))?;
    let decoded: Vec<String> = rest
        .split('/')
        .filter(|seg| !seg.is_empty())
        .map(|seg| {
            js_sys::decode_uri_component(seg)
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_else(|| seg.to_string())
        })
        .collect();
    (!decoded.is_empty()).then(|| decoded.join("/"))
}

/// The `/files/<path>` URL for a store-relative file path, percent-encoding each segment so a
/// name containing `?`, `#`, or a space cannot break the address bar.
pub(crate) fn store_file_path(rel: &str) -> String {
    let encoded: Vec<String> = rel
        .split('/')
        .filter(|seg| !seg.is_empty())
        .map(|seg| {
            js_sys::encode_uri_component(seg)
                .as_string()
                .unwrap_or_else(|| seg.to_string())
        })
        .collect();
    format!("{BASE}/files/{}", encoded.join("/"))
}

/// The section in a `/projects/<id>/<section>` path; the bare project path is its overview.
pub(crate) fn project_section_from_path(path: &str) -> ProjectSection {
    let Some(rest) = path
        .strip_prefix(BASE)
        .and_then(|p| p.strip_prefix("/projects/"))
    else {
        return ProjectSection::Overview;
    };
    ProjectSection::from_slug(rest.split('/').nth(1).unwrap_or_default())
}

/// Navigate to a project's detail page, clearing any stale detail so it shows a loading state.
pub(crate) fn open_project(state: State, route: RwSignal<Route>, id: String) {
    open_project_section(state, route, id, ProjectSection::Overview);
}

/// Navigate to one section of a project. Re-entering a different project clears the file
/// browser so it re-fetches from the new root; switching sections within one project does not.
pub(crate) fn open_project_section(
    state: State,
    route: RwSignal<Route>,
    id: String,
    section: ProjectSection,
) {
    if state.current_project.get_untracked() != id {
        state.project_detail.set(None);
        state.files.reset();
        state.current_project.set(id.clone());
    }
    state.current_section.set(section);
    push_state(&section.path(&id));
    route.set(Route::ProjectDetail);
    scroll_top();
}

/// Navigate to a page that is not about a particular project — anything in [`Route::NAV`].
///
/// Dropping the open project is the point: leaving its id set would leave the explorer
/// highlighting a project you are no longer looking at, and the file browser standing in its
/// directory. Every way into a global page goes through here — the explorer's tree, the ⌘K
/// menu, and [`go_projects`] — so none of them can grow its own idea of what a page change
/// clears.
pub(crate) fn go_global(state: State, route: RwSignal<Route>, target: Route) {
    state.current_project.set(String::new());
    state.files.reset();
    // No history entry for the page you are already on: Back would then be a press that
    // visibly does nothing, which is worse than no entry at all.
    if route.get_untracked() == target {
        return;
    }
    push_state(target.path());
    route.set(target);
    scroll_top();
}

/// Navigate back to the projects list.
pub(crate) fn go_projects(state: State, route: RwSignal<Route>) {
    go_global(state, route, Route::Projects);
}
