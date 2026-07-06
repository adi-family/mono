//! adi-webapp — the adi control-panel UI, a Leptos client-side-rendered app compiled to
//! wasm by Trunk. It talks to the `/api/*` backend using the DTO types from
//! [`adi_webapp_api`], so the wire format is shared with the server rather than duplicated.
//! Trunk's `dist/` output is embedded into [`adi-app`](../adi-app), which serves it at
//! `app.adi`.

#![allow(non_snake_case)] // Leptos components are PascalCase by convention.

use adi_webapp_api::types::{Health, LeaseRef, PortsState, UsedPorts};
use gloo_timers::callback::Interval;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::spawn_local;

fn main() {
    console_error_panic_hook::set_once();
    apply_saved_theme();
    mount_to_body(App);
}

/// The application shell: sidebar navigation, a header, and the routed page body. Shared
/// data (status, ports, health) is polled here regardless of which page is showing.
#[component]
fn App() -> impl IntoView {
    // Reactive state the whole UI reads from. `State` bundles the signals a data refresh
    // writes; `Form` bundles the reserve form's local signals.
    let status = RwSignal::new(Status::Connecting);
    let ports = RwSignal::new(None::<PortsState>);
    let health = RwSignal::new(None::<Health>);
    let flash = RwSignal::new(None::<Flash>);
    let secs_since = RwSignal::new(0u32);
    let used = RwSignal::new(None::<UsedPorts>);
    let state = State {
        status,
        ports,
        health,
        flash,
        secs_since,
        used,
    };

    let form = Form {
        svc: RwSignal::new(String::new()),
        key: RwSignal::new(String::new()),
        reserving: RwSignal::new(false),
        reserved: RwSignal::new(String::new()),
    };

    // "Ports in use" filter: defaults to ADI-managed only; toggle shows all listening ports.
    let managed_only = RwSignal::new(true);

    // The active page, derived from the URL path. Unknown paths (including `/`) resolve to
    // Overview; canonicalize the address bar so a refresh lands on the same page.
    let route = RwSignal::new(Route::from_path(&current_path()));
    if current_path() != route.get_untracked().path() {
        replace_state(route.get_untracked().path());
    }
    // Follow the browser's back/forward buttons.
    let on_pop = Closure::<dyn FnMut()>::new(move || route.set(Route::from_path(&current_path())));
    if let Some(w) = web_sys::window() {
        let _ = w.add_event_listener_with_callback("popstate", on_pop.as_ref().unchecked_ref());
    }
    on_pop.forget();

    // Load now, poll the backend every 4s, and tick the "updated Ns ago" label each second.
    spawn_local(load(state));
    Interval::new(4_000, move || spawn_local(load(state))).forget();
    Interval::new(1_000, move || {
        secs_since.update(|s| *s = s.saturating_add(1));
    })
    .forget();

    // Refresh immediately when the Ports Manager page opens, so its port scan isn't stale.
    Effect::new(move |_| {
        if route.get() == Route::PortsManager {
            spawn_local(load(state));
        }
    });

    view! {
        <div class="adi-shell">
            <aside class="adi-sidebar">
                <div class="adi-sidebar__brand">
                    <span class="adi-logo">"adi"<span class="adi-logo__dot">"."</span></span>
                    <span class="adi-bar__sub">"control panel"</span>
                </div>
                <nav class="adi-nav">
                    <a class="adi-nav__item" href=Route::Overview.path()
                        aria-current=move || aria_current(route, Route::Overview)
                        on:click=move |ev| spa_click(&ev, route, Route::Overview)>
                        <span>"Overview"</span>
                    </a>
                    <div class="adi-nav__group">
                        <div class="adi-nav__heading">"Settings"</div>
                        <a class="adi-nav__item" href=Route::PortsManager.path()
                            aria-current=move || aria_current(route, Route::PortsManager)
                            on:click=move |ev| spa_click(&ev, route, Route::PortsManager)>
                            <span>"Ports Manager"</span>
                        </a>
                    </div>
                </nav>
                <span class="adi-spacer"></span>
                <div class="adi-sidebar__foot">
                    <span class="adi-status" data-state=move || status.get().data()>
                        <span class="adi-status__led"></span>
                        <span>{move || status.get().label()}</span>
                    </span>
                    <button class="adi-btn adi-btn--icon" title="Toggle theme" aria-label="Toggle theme"
                        on:click=move |_| toggle_theme()>"◐"</button>
                </div>
            </aside>

            <main class="adi-main">
                <div class="adi-container">
                    {move || match route.get() {
                        // The Ports Manager page shows no page title — its panels are self-labelled.
                        Route::PortsManager => None,
                        other => Some(view! {
                            <header class="adi-bar">
                                <h1 class="adi-bar__title">{other.title()}</h1>
                            </header>
                        }),
                    }}

                    {move || match route.get() {
                        Route::Overview => overview_view(state),
                        Route::PortsManager => ports_manager_view(state, form, managed_only),
                    }}

                    <footer class="adi-footer">
                        "The Rust backend serves " <code>"/api"</code> "; this page is what "
                        <code>"app.adi"</code> " shows."
                    </footer>
                </div>
            </main>
        </div>
    }
}

/// The Overview page: system liveness at a glance.
fn overview_view(state: State) -> AnyView {
    let State { health, .. } = state;
    view! {
        <section class="adi-tiles">
            <div class="adi-tile">
                <div class="adi-tile__label">"Uptime"</div>
                <div class="adi-tile__value">
                    {move || health.get().map_or_else(|| "—".to_string(), |h| fmt_uptime(h.uptime_secs))}
                </div>
                <div class="adi-tile__note">
                    {move || health.get().map_or_else(|| "adi-app".to_string(),
                        |h| format!("{} v{}", h.service, h.version))}
                </div>
            </div>
        </section>
    }
    .into_any()
}

/// The Ports Manager page: the live registry table plus the reserve/release controls.
fn ports_manager_view(state: State, form: Form, managed_only: RwSignal<bool>) -> AnyView {
    let State {
        ports,
        flash,
        secs_since,
        used,
        ..
    } = state;
    let Form {
        svc,
        key,
        reserving,
        reserved,
    } = form;
    view! {
        <section class="adi-tiles">
            <div class="adi-tile">
                <div class="adi-tile__label">"Active leases"</div>
                <div class="adi-tile__value">
                    {move || ports.get().map_or_else(|| "—".to_string(), |p| p.leases.len().to_string())}
                </div>
                <div class="adi-tile__note">"reserved static ports"</div>
            </div>
            <div class="adi-tile">
                <div class="adi-tile__label">"Allocatable range"</div>
                <div class="adi-tile__value">
                    {move || ports.get().map_or_else(|| "—".to_string(),
                        |p| format!("{}–{}", p.range.start, p.range.end))}
                </div>
                <div class="adi-tile__note">
                    {move || ports.get().map_or_else(|| "ports handed out from here".to_string(), |p| {
                        let span = u32::from(p.range.end) - u32::from(p.range.start) + 1;
                        format!("{span} ports · {} reserved bands", p.reserved.len())
                    })}
                </div>
            </div>
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Port registry"</h2>
                <span class="adi-spacer"></span>
                <span class="adi-updated">{move || updated_text(ports, secs_since)}</span>
            </div>

            <div class="adi-tablewrap">
                <table class="adi-table">
                    <thead>
                        <tr><th>"Service"</th><th>"Key"</th><th>"Port"</th><th></th></tr>
                    </thead>
                    <tbody>
                        {move || rows_view(state)}
                    </tbody>
                </table>
            </div>

            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let service = svc.get().trim().to_string();
                let k = key.get().trim().to_string();
                if service.is_empty() || k.is_empty() {
                    return;
                }
                reserving.set(true);
                spawn_local(async move {
                    match fetch::reserve(&LeaseRef { service: service.clone(), key: k.clone() }).await {
                        Ok(r) => {
                            reserved.set(format!("{}/{} → :{}", r.service, r.key, r.port));
                            flash.set(Some(Flash::ok(
                                format!("Reserved port {} for {}/{}.", r.port, r.service, r.key),
                            )));
                            load(state).await;
                        }
                        Err(e) => flash.set(Some(Flash::err(e))),
                    }
                    reserving.set(false);
                });
            }>
                <div class="adi-field">
                    <label class="adi-field__label" for="svc">"Service"</label>
                    <input class="adi-input" id="svc" placeholder="frontend" autocomplete="off"
                        prop:value=move || svc.get()
                        on:input=move |ev| svc.set(event_target_value(&ev)) />
                </div>
                <div class="adi-field">
                    <label class="adi-field__label" for="key">"Port key"</label>
                    <input class="adi-input" id="key" placeholder="http" autocomplete="off"
                        prop:value=move || key.get()
                        on:input=move |ev| key.set(event_target_value(&ev)) />
                </div>
                <button class="adi-btn adi-btn--primary" type="submit"
                    prop:disabled=move || reserving.get()>
                    "Reserve port"
                </button>
                <span class="adi-spacer" style="flex:1"></span>
                <span class="adi-chip adi-mono">{move || reserved.get()}</span>
            </form>
            <div class="adi-flash" data-kind=move || flash.get().map_or("none", |f| f.kind)>
                {move || flash.get().map(|f| f.msg).unwrap_or_default()}
            </div>
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Ports in use"</h2>
                <span class="adi-updated">
                    {move || used.get().map_or(String::new(), |u| format!("{} listening", u.ports.len()))}
                </span>
                <span class="adi-spacer"></span>
                <div class="adi-segmented" role="group" aria-label="Filter ports">
                    <button class="adi-segmented__option" type="button"
                        aria-pressed=move || (!managed_only.get()).to_string()
                        on:click=move |_| managed_only.set(false)>"All"</button>
                    <button class="adi-segmented__option" type="button"
                        aria-pressed=move || managed_only.get().to_string()
                        on:click=move |_| managed_only.set(true)>"ADI managed"</button>
                </div>
            </div>
            <div class="adi-tablewrap">
                <table class="adi-table">
                    <thead>
                        <tr><th>"Port"</th><th>"Process"</th><th>"PID"</th><th>"Owner"</th></tr>
                    </thead>
                    <tbody>
                        {move || used_rows_view(state, managed_only)}
                    </tbody>
                </table>
            </div>
        </section>
    }
    .into_any()
}

// ---- client-side routing ------------------------------------------------------------

/// The pages the sidebar navigates between, each mapped to a URL path.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Route {
    Overview,
    PortsManager,
}

impl Route {
    /// The page for a URL path; `/`, `/overview`, and anything unknown resolve to Overview.
    fn from_path(path: &str) -> Self {
        match path {
            "/settings/ports-manager" => Route::PortsManager,
            _ => Route::Overview,
        }
    }

    /// The canonical URL path for this page.
    fn path(self) -> &'static str {
        match self {
            Route::Overview => "/overview",
            Route::PortsManager => "/settings/ports-manager",
        }
    }

    /// The page title shown in the header.
    fn title(self) -> &'static str {
        match self {
            Route::Overview => "Overview",
            Route::PortsManager => "Ports Manager",
        }
    }
}

/// `aria-current` for a nav link: `"page"` when it points at the active route.
fn aria_current(route: RwSignal<Route>, target: Route) -> &'static str {
    if route.get() == target {
        "page"
    } else {
        "false"
    }
}

/// Handle a click on a nav link: navigate client-side for a plain left-click, but let
/// modified clicks (new tab/window, etc.) fall through to a normal browser navigation.
fn spa_click(ev: &web_sys::MouseEvent, route: RwSignal<Route>, target: Route) {
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
fn push_state(path: &str) {
    if let Some(h) = web_sys::window().and_then(|w| w.history().ok()) {
        let _ = h.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(path));
    }
}

/// Replace the current history entry's URL (canonicalizes the address bar on first load).
fn replace_state(path: &str) {
    if let Some(h) = web_sys::window().and_then(|w| w.history().ok()) {
        let _ = h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(path));
    }
}

/// Scroll back to the top after a page change.
fn scroll_top() {
    if let Some(w) = web_sys::window() {
        w.scroll_to_with_x_and_y(0.0, 0.0);
    }
}

/// The current URL path, e.g. `/settings/ports-manager`.
fn current_path() -> String {
    web_sys::window()
        .and_then(|w| w.location().pathname().ok())
        .unwrap_or_default()
}

/// Signals a data refresh writes to; `Copy` (each field is an arena handle) so it threads
/// cheaply through async tasks and event handlers.
#[derive(Clone, Copy)]
struct State {
    status: RwSignal<Status>,
    ports: RwSignal<Option<PortsState>>,
    health: RwSignal<Option<Health>>,
    flash: RwSignal<Option<Flash>>,
    secs_since: RwSignal<u32>,
    used: RwSignal<Option<UsedPorts>>,
}

/// The reserve form's local signals; `Copy` so it threads into the page view and handlers.
#[derive(Clone, Copy)]
struct Form {
    svc: RwSignal<String>,
    key: RwSignal<String>,
    reserving: RwSignal<bool>,
    reserved: RwSignal<String>,
}

/// Fetch `/api/health` + `/api/ports` together and fan the result into the signals.
async fn load(s: State) {
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
    // The system port scan is only needed on the Ports Manager page; skip it elsewhere.
    if current_path() == Route::PortsManager.path()
        && let Ok(u) = fetch::used().await
    {
        s.used.set(Some(u));
    }
}

/// Render the port table body: a loading/empty placeholder, or one row per lease sorted
/// by port. Reads `ports` reactively, so it re-renders on every refresh.
fn rows_view(state: State) -> AnyView {
    match state.ports.get() {
        None => view! { <tr><td class="adi-empty" colspan="4">"Loading…"</td></tr> }.into_any(),
        Some(p) if p.leases.is_empty() => view! {
            <tr><td class="adi-empty" colspan="4">"No ports reserved yet — reserve one below."</td></tr>
        }
        .into_any(),
        Some(p) => {
            let mut leases = p.leases;
            leases.sort_by_key(|l| l.port);
            leases
                .into_iter()
                .map(|l| {
                    let service = l.service.clone();
                    let key = l.key.clone();
                    view! {
                        <tr>
                            <td class="adi-mono">{l.service}</td>
                            <td class="adi-mono">{l.key}</td>
                            <td class="adi-mono adi-table__port">{l.port.to_string()}</td>
                            <td style="text-align:right">
                                <button class="adi-btn adi-btn--link" on:click=move |_| {
                                    let service = service.clone();
                                    let key = key.clone();
                                    spawn_local(async move {
                                        let req = LeaseRef { service, key };
                                        match fetch::release(&req).await {
                                            Ok(r) => {
                                                let msg = match r.freed {
                                                    Some(port) => format!("Released port {port}."),
                                                    None => "Nothing to release.".to_string(),
                                                };
                                                state.flash.set(Some(Flash::ok(msg)));
                                                load(state).await;
                                            }
                                            Err(e) => state.flash.set(Some(Flash::err(e))),
                                        }
                                    });
                                }>"Release"</button>
                            </td>
                        </tr>
                    }
                })
                .collect::<Vec<_>>()
                .into_any()
        }
    }
}

/// Render the "ports in use" table body: every listening port, or only the ADI-managed
/// ones when `managed_only`. A port is ADI-managed when a registry lease binds it.
fn used_rows_view(state: State, managed_only: RwSignal<bool>) -> AnyView {
    let Some(used) = state.used.get() else {
        return view! { <tr><td class="adi-empty" colspan="4">"Scanning…"</td></tr> }.into_any();
    };
    let leases = state.ports.get().map(|p| p.leases).unwrap_or_default();
    let managed = managed_only.get();

    let rows: Vec<_> = used
        .ports
        .into_iter()
        .filter_map(|u| {
            let lease = leases.iter().find(|l| l.port == u.port).cloned();
            // ADI-managed: bound by a registry lease, or owned by an `adi-*` service process.
            let is_adi =
                lease.is_some() || u.process.as_deref().is_some_and(|p| p.starts_with("adi"));
            if managed && !is_adi {
                return None;
            }
            Some((u, lease))
        })
        .collect();

    if rows.is_empty() {
        let msg = if managed {
            "No ADI-managed ports are listening."
        } else {
            "No listening ports found."
        };
        return view! { <tr><td class="adi-empty" colspan="4">{msg}</td></tr> }.into_any();
    }

    rows.into_iter()
        .map(|(u, lease)| {
            let owner = match lease {
                Some(l) => view! {
                    <td><span class="adi-chip">{format!("{}/{}", l.service, l.key)}</span></td>
                }
                .into_any(),
                None => view! { <td class="adi-muted">"—"</td> }.into_any(),
            };
            let process = u.process.unwrap_or_else(|| "—".to_string());
            let pid = u.pid.map_or_else(|| "—".to_string(), |p| p.to_string());
            view! {
                <tr>
                    <td class="adi-mono adi-table__port">{u.port.to_string()}</td>
                    <td>{process}</td>
                    <td class="adi-mono adi-muted">{pid}</td>
                    {owner}
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// The "updated Ns ago" label; empty until the first successful load.
fn updated_text(ports: RwSignal<Option<PortsState>>, secs_since: RwSignal<u32>) -> String {
    if ports.get().is_none() {
        return String::new();
    }
    match secs_since.get() {
        0 => "updated just now".to_string(),
        s => format!("updated {s}s ago"),
    }
}

/// Backend liveness as shown by the status pill.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Connecting,
    Online,
    Down,
}

impl Status {
    /// The `data-state` value the CSS keys the LED colour off.
    fn data(self) -> &'static str {
        match self {
            Status::Connecting => "unknown",
            Status::Online => "online",
            Status::Down => "down",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Status::Connecting => "connecting…",
            Status::Online => "online",
            Status::Down => "offline",
        }
    }
}

/// A one-line status message under the form; `kind` drives its colour via `data-kind`.
#[derive(Clone)]
struct Flash {
    kind: &'static str,
    msg: String,
}

impl Flash {
    fn ok(msg: String) -> Self {
        Self { kind: "ok", msg }
    }

    fn err(msg: String) -> Self {
        Self { kind: "err", msg }
    }
}

/// Format an uptime in seconds as `Ns` / `Nm Ss` / `Nh Mm`.
fn fmt_uptime(s: u64) -> String {
    if s < 60 {
        format!("{s}s")
    } else if s < 3_600 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}h {}m", s / 3_600, (s % 3_600) / 60)
    }
}

// ---- theme toggle (persisted; falls back to the OS preference) ----------------------

/// Apply the theme saved in `localStorage`, if any, to `<html data-theme>`.
fn apply_saved_theme() {
    if let Some(theme) = storage().and_then(|s| s.get_item("adi-theme").ok().flatten())
        && let Some(el) = document_element()
    {
        let _ = el.set_attribute("data-theme", &theme);
    }
}

/// Flip the theme and persist the choice, seeding from the OS preference when unset.
fn toggle_theme() {
    let Some(el) = document_element() else {
        return;
    };
    let current = match el.get_attribute("data-theme") {
        Some(t) if !t.is_empty() => t,
        _ if prefers_dark() => "dark".to_string(),
        _ => "light".to_string(),
    };
    let next = if current == "dark" { "light" } else { "dark" };
    let _ = el.set_attribute("data-theme", next);
    if let Some(s) = storage() {
        let _ = s.set_item("adi-theme", next);
    }
}

fn document_element() -> Option<web_sys::Element> {
    web_sys::window()?.document()?.document_element()
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn prefers_dark() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten())
        .is_some_and(|m| m.matches())
}

/// Thin fetch layer over the `/api/*` endpoints, deserializing into the shared DTOs.
mod fetch {
    use adi_webapp_api::types::{
        ApiError, Health, PortsState, ReleaseResponse, ReserveResponse, UsedPorts,
    };
    use gloo_net::http::{Request, Response};
    use serde::Serialize;
    use serde::de::DeserializeOwned;

    use super::LeaseRef;

    pub async fn health() -> Result<Health, String> {
        get("/api/health").await
    }

    pub async fn ports() -> Result<PortsState, String> {
        get("/api/ports").await
    }

    pub async fn used() -> Result<UsedPorts, String> {
        get("/api/ports/used").await
    }

    pub async fn reserve(body: &LeaseRef) -> Result<ReserveResponse, String> {
        post("/api/ports/reserve", body).await
    }

    pub async fn release(body: &LeaseRef) -> Result<ReleaseResponse, String> {
        post("/api/ports/release", body).await
    }

    async fn get<T: DeserializeOwned>(url: &str) -> Result<T, String> {
        let resp = Request::get(url).send().await.map_err(stringify)?;
        finish(resp).await
    }

    async fn post<B: Serialize, T: DeserializeOwned>(url: &str, body: &B) -> Result<T, String> {
        let resp = Request::post(url)
            .json(body)
            .map_err(stringify)?
            .send()
            .await
            .map_err(stringify)?;
        finish(resp).await
    }

    /// Turn a response into `T`, or a message: the API's `{ error }` if present, else the
    /// HTTP status line.
    async fn finish<T: DeserializeOwned>(resp: Response) -> Result<T, String> {
        let status = resp.status();
        let text = resp.text().await.map_err(stringify)?;
        if !(200..300).contains(&status) {
            let msg = serde_json::from_str::<ApiError>(&text)
                .map_or_else(|_| format!("{status} {}", resp.status_text()), |e| e.error);
            return Err(msg);
        }
        serde_json::from_str(&text).map_err(stringify)
    }

    fn stringify<E: std::fmt::Display>(e: E) -> String {
        e.to_string()
    }
}
