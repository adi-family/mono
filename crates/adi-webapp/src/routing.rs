//! Client-side routing: the [`Route`] enum mapping URL paths to pages, the click/history plumbing
//! that navigates without a page reload, and the project-detail navigation helpers.

use leptos::prelude::*;

use crate::state::State;

/// The pages the sidebar navigates between, each mapped to a URL path.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    Projects,
    /// A single project's detail page (`/projects/<id>`); the id lives in `State::current_project`.
    ProjectDetail,
    /// The read-only task tree (`/tasks`).
    Tasks,
    /// Agent definitions (`/agents`).
    Agents,
    /// Trigger definitions (`/triggers`).
    Triggers,
    /// Agent-authored dashboards (`/dashboards`).
    Dashboards,
    Hive,
    PortsManager,
    Mesh,
}

impl Route {
    /// The page for a URL path; `/` and anything unknown resolve to Projects.
    pub(crate) fn from_path(path: &str) -> Self {
        if project_id_from_path(path).is_some() {
            return Route::ProjectDetail;
        }
        match path {
            "/tasks" => Route::Tasks,
            "/agents" => Route::Agents,
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
            Route::Projects | Route::ProjectDetail => "/projects",
            Route::Tasks => "/tasks",
            Route::Agents => "/agents",
            Route::Triggers => "/triggers",
            Route::Dashboards => "/dashboards",
            Route::Hive => "/settings/hive",
            Route::PortsManager => "/settings/ports-manager",
            Route::Mesh => "/settings/mesh",
        }
    }

    /// The page title shown in the header.
    pub(crate) fn title(self) -> &'static str {
        match self {
            Route::Projects => "Projects",
            Route::ProjectDetail => "Project",
            Route::Tasks => "Tasks",
            Route::Agents => "Agents",
            Route::Triggers => "Triggers",
            Route::Dashboards => "Dashboards",
            Route::Hive => "Hive",
            Route::PortsManager => "Ports Manager",
            Route::Mesh => "Mesh",
        }
    }
}

/// `aria-current` for a nav link: `"page"` when it points at the active route.
pub(crate) fn aria_current(route: RwSignal<Route>, target: Route) -> &'static str {
    if route.get() == target {
        "page"
    } else {
        "false"
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

/// The project id in a `/projects/<id>` detail path, or `None` for any other path (including
/// the bare `/projects` list). The trailing segment must be non-empty and slash-free.
pub(crate) fn project_id_from_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/projects/")?;
    if rest.is_empty() || rest.contains('/') {
        None
    } else {
        Some(rest.to_string())
    }
}

/// Navigate to a project's detail page, clearing any stale detail so it shows a loading state.
pub(crate) fn open_project(state: State, route: RwSignal<Route>, id: String) {
    state.project_detail.set(None);
    // Clear the file browser so the load effect re-fetches from this project's root.
    state.files.reset();
    state.current_project.set(id.clone());
    push_state(&format!("/projects/{id}"));
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
