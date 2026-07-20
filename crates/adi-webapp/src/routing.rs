//! Client-side routing: the [`Route`] enum mapping URL paths to pages, the click/history plumbing
//! that navigates without a page reload, and the project-detail navigation helpers.

use leptos::prelude::*;

use crate::state::State;

/// The pages the sidebar navigates between, each mapped to a URL path.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    /// The default ADI agent — set up and run `adi-agent` (`/meta`).
    Meta,
    Projects,
    /// A single project's detail page (`/projects/<id>`); the id lives in `State::current_project`.
    ProjectDetail,
    /// The read-only task tree (`/tasks`).
    Tasks,
    /// Agent definitions (`/agents`).
    Agents,
    /// Tool definitions — user CLIs (`/tools`).
    Tools,
    /// Encrypted secrets — global & per-project key-values (`/secrets`).
    Secrets,
    /// Trigger definitions (`/triggers`).
    Triggers,
    /// Agent-authored dashboards (`/dashboards`).
    Dashboards,
    Hive,
    PortsManager,
    Mesh,
    /// One file from the ADI store, open in the full-width editor (`/files/<path>`). The path
    /// lives in `StoreBrowser::open_file`, the way a project id lives in `current_project`.
    StoreFile,
}

impl Route {
    /// The page for a URL path; `/` and anything unknown resolve to Projects.
    pub(crate) fn from_path(path: &str) -> Self {
        if project_id_from_path(path).is_some() {
            return Route::ProjectDetail;
        }
        if store_path_from_path(path).is_some() {
            return Route::StoreFile;
        }
        match path {
            "/meta" => Route::Meta,
            "/tasks" => Route::Tasks,
            "/agents" => Route::Agents,
            "/tools" => Route::Tools,
            "/secrets" => Route::Secrets,
            "/triggers" => Route::Triggers,
            "/dashboards" => Route::Dashboards,
            "/settings/hive" => Route::Hive,
            "/settings/ports-manager" => Route::PortsManager,
            "/settings/mesh" => Route::Mesh,
            _ => Route::Projects,
        }
    }

    /// The canonical URL path for this page. `ProjectDetail`'s real path carries an id, so this
    /// returns the list base for it (used only for nav; detail canonicalization is skipped).
    pub(crate) fn path(self) -> &'static str {
        match self {
            Route::Meta => "/meta",
            Route::Projects | Route::ProjectDetail => "/projects",
            Route::Tasks => "/tasks",
            Route::Agents => "/agents",
            Route::Tools => "/tools",
            Route::Secrets => "/secrets",
            Route::Triggers => "/triggers",
            Route::Dashboards => "/dashboards",
            Route::Hive => "/settings/hive",
            Route::PortsManager => "/settings/ports-manager",
            Route::Mesh => "/settings/mesh",
            // The real path carries the file path; this base is only used for nav fallbacks.
            Route::StoreFile => "/files",
        }
    }

    /// The page title shown in the header.
    pub(crate) fn title(self) -> &'static str {
        match self {
            Route::Meta => "Meta",
            Route::Projects => "Projects",
            Route::ProjectDetail => "Project",
            Route::Tasks => "Tasks",
            Route::Agents => "Agents",
            Route::Tools => "Tools",
            Route::Secrets => "Secrets",
            Route::Triggers => "Triggers",
            Route::Dashboards => "Dashboards",
            Route::Hive => "Hive",
            Route::PortsManager => "Ports Manager",
            Route::Mesh => "Mesh",
            Route::StoreFile => "File",
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
    Services,
    Workspaces,
    Files,
}

impl ProjectSection {
    /// Every section, in the order the explorer lists them.
    pub(crate) const ALL: [ProjectSection; 9] = [
        ProjectSection::Overview,
        ProjectSection::Tasks,
        ProjectSection::Agents,
        ProjectSection::Triggers,
        ProjectSection::Tools,
        ProjectSection::Secrets,
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
            "" => format!("/projects/{project}"),
            slug => format!("/projects/{project}/{slug}"),
        }
    }
}

/// Handle a click on a nav link: navigate client-side for a plain left-click, but let
/// modified clicks (new tab/window, etc.) fall through to a normal browser navigation.
pub(crate) fn spa_click(ev: &web_sys::MouseEvent, route: RwSignal<Route>, target: Route) {
    if ev.default_prevented()
        || ev.button() != 0
        || ev.meta_key()
        || ev.ctrl_key()
        || ev.shift_key()
        || ev.alt_key()
    {
        return;
    }
    ev.prevent_default();
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

/// The project id in a `/projects/<id>` or `/projects/<id>/<section>` path, or `None` for any
/// other path (including the bare `/projects` list). The id segment must be non-empty.
pub(crate) fn project_id_from_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/projects/")?;
    let id = rest.split('/').next().unwrap_or_default();
    (!id.is_empty()).then(|| id.to_string())
}

/// The store-relative file path in a `/files/<path>` URL, or `None` for any other path. Each
/// segment is percent-decoded, so a name with a space or `#` round-trips through the address bar.
pub(crate) fn store_path_from_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/files/")?;
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
    format!("/files/{}", encoded.join("/"))
}

/// The section in a `/projects/<id>/<section>` path; the bare project path is its overview.
pub(crate) fn project_section_from_path(path: &str) -> ProjectSection {
    let Some(rest) = path.strip_prefix("/projects/") else {
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

/// Navigate back to the projects list.
pub(crate) fn go_projects(state: State, route: RwSignal<Route>) {
    state.current_project.set(String::new());
    state.files.reset();
    push_state(Route::Projects.path());
    route.set(Route::Projects);
    scroll_top();
}
