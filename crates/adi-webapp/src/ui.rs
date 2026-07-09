//! Shared view helpers and small utilities the pages compose from, so the repeated markup (stat
//! tiles, table shells, the flash line, segmented filters, labeled fields), the shared formatters,
//! the generic mutation runner, and the theme toggle live in one place instead of at every call
//! site.

use adi_webapp_api::types::{PortsState, ServicePort};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::routing::{Route, aria_current, spa_click};
use crate::state::{Flash, State};

/// A single full-width placeholder row spanning `colspan` columns — the
/// `<tr><td class="adi-empty">…</td></tr>` every table body falls back to for its loading, empty,
/// or error state.
pub(crate) fn placeholder_row(colspan: &'static str, msg: &str) -> AnyView {
    view! { <tr><td class="adi-empty" colspan=colspan>{msg.to_string()}</td></tr> }.into_any()
}

/// One stat tile in an `adi-tiles` strip: a label, a big value, and a sub-note. `value`/`note`
/// take any view, so a caller passes either a literal or a reactive `move || …` closure.
pub(crate) fn tile(
    label: &'static str,
    value: impl IntoView + 'static,
    note: impl IntoView + 'static,
) -> impl IntoView {
    view! {
        <div class="adi-tile">
            <div class="adi-tile__label">{label}</div>
            <div class="adi-tile__value">{value}</div>
            <div class="adi-tile__note">{note}</div>
        </div>
    }
}

/// The standard table shell: the `adi-tablewrap` scroll box, a header row built from `headers`
/// (an empty string yields a blank action column), and `body` as the `<tbody>`.
pub(crate) fn data_table(
    headers: &'static [&'static str],
    body: impl IntoView + 'static,
) -> impl IntoView {
    view! {
        <div class="adi-tablewrap">
            <table class="adi-table">
                <thead>
                    <tr>{headers.iter().map(|h| view! { <th>{*h}</th> }).collect::<Vec<_>>()}</tr>
                </thead>
                <tbody>{body}</tbody>
            </table>
        </div>
    }
}

/// The one-line status message shown under a form: reads the shared `flash` signal, colouring
/// itself via `data-kind`.
pub(crate) fn flash_view(flash: RwSignal<Option<Flash>>) -> impl IntoView {
    view! {
        <div class="adi-flash" data-kind=move || flash.get().map_or("none", |f| f.kind)>
            {move || flash.get().map(|f| f.msg).unwrap_or_default()}
        </div>
    }
}

/// A two-option segmented toggle bound to a `bool` signal: the left button selects `false`, the
/// right selects `true`, each reflecting the state through `aria-pressed`.
pub(crate) fn segmented(
    aria_label: &'static str,
    signal: RwSignal<bool>,
    left: &'static str,
    right: &'static str,
) -> impl IntoView {
    view! {
        <div class="adi-segmented" role="group" aria-label=aria_label>
            <button class="adi-segmented__option" type="button"
                aria-pressed=move || (!signal.get()).to_string()
                on:click=move |_| signal.set(false)>{left}</button>
            <button class="adi-segmented__option" type="button"
                aria-pressed=move || signal.get().to_string()
                on:click=move |_| signal.set(true)>{right}</button>
        </div>
    }
}

/// One sidebar nav link that navigates client-side and marks itself `aria-current` when active.
/// (The Projects link stays inline — it is also current on the project-detail route.)
pub(crate) fn nav_item(
    route: RwSignal<Route>,
    target: Route,
    label: &'static str,
) -> impl IntoView {
    view! {
        <a class="adi-nav__item" href=target.path()
            aria-current=move || aria_current(route, target)
            on:click=move |ev| spa_click(&ev, route, target)>
            <span>{label}</span>
        </a>
    }
}

/// A labeled text input bound to a `String` signal — the `adi-field` wrapper the forms repeat.
/// Optional props toggle the mono/wide input classes, a numeric input mode, a trailing hint line,
/// and the field wrapper's flex style.
#[component]
pub(crate) fn TextField(
    /// The input's `id` (also the label's `for`).
    id: &'static str,
    /// The field's label text.
    label: &'static str,
    /// The bound value signal.
    value: RwSignal<String>,
    #[prop(optional)] placeholder: &'static str,
    #[prop(optional)] hint: &'static str,
    #[prop(optional)] mono: bool,
    #[prop(optional)] wide: bool,
    #[prop(optional)] numeric: bool,
    #[prop(optional)] field_style: &'static str,
) -> impl IntoView {
    let mut class = String::from("adi-input");
    if wide {
        class.push_str(" adi-input--wide");
    }
    if mono {
        class.push_str(" adi-mono");
    }
    let inputmode = if numeric { "numeric" } else { "text" };
    view! {
        <div class="adi-field" style=field_style>
            <label class="adi-field__label" for=id>{label}</label>
            <input class=class id=id placeholder=placeholder autocomplete="off" inputmode=inputmode
                prop:value=move || value.get()
                on:input=move |ev| value.set(event_target_value(&ev)) />
            {(!hint.is_empty()).then(|| view! { <span class="adi-field__hint">{hint}</span> })}
        </div>
    }
}

/// Run a mutation that returns fresh state `T`, hand the result to `store`, and flash success or
/// the error; toggles `busy` around the request when a form is driving it. The `apply_projects` /
/// `apply_tasks` / `apply_agents` / `apply_mesh` helpers are thin typed wrappers over this — each
/// differs only in which page-state signal receives the result.
pub(crate) fn apply_mutation<T, S, F>(
    state: State,
    busy: Option<RwSignal<bool>>,
    ok_msg: String,
    store: S,
    fut: F,
) where
    S: Fn(State, T) + 'static,
    F: std::future::Future<Output = Result<T, String>> + 'static,
{
    if let Some(b) = busy {
        b.set(true);
    }
    spawn_local(async move {
        match fut.await {
            Ok(v) => {
                store(state, v);
                state.flash.set(Some(Flash::ok(ok_msg)));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        if let Some(b) = busy {
            b.set(false);
        }
    });
}

/// The "updated Ns ago" label; empty until the first successful load.
pub(crate) fn updated_text(
    ports: RwSignal<Option<PortsState>>,
    secs_since: RwSignal<u32>,
) -> String {
    if ports.get().is_none() {
        return String::new();
    }
    match secs_since.get() {
        0 => "updated just now".to_string(),
        s => format!("updated {s}s ago"),
    }
}

/// Format a service's declared port bindings as `key:port, key:port`, or `—` when it declares none.
pub(crate) fn fmt_ports(ports: &[ServicePort]) -> String {
    if ports.is_empty() {
        "—".to_string()
    } else {
        ports
            .iter()
            .map(|p| format!("{}:{}", p.key, p.port))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// An optional string for a table cell, falling back to an em dash when it's absent.
pub(crate) fn dash(value: Option<String>) -> String {
    value.unwrap_or_else(|| "—".to_string())
}

/// Format a Unix timestamp (seconds) as a `YYYY-MM-DD` UTC date; `0` renders as `—`. Pure
/// integer arithmetic (Howard Hinnant's `civil_from_days`), so no date crate is pulled into wasm.
pub(crate) fn fmt_date(secs: u64) -> String {
    if secs == 0 {
        return "—".to_string();
    }
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

// ---- theme toggle (persisted; falls back to the OS preference) ----------------------

/// Apply the theme saved in `localStorage`, if any, to `<html data-theme>`.
pub(crate) fn apply_saved_theme() {
    if let Some(theme) = storage().and_then(|s| s.get_item("adi-theme").ok().flatten())
        && let Some(el) = document_element()
    {
        let _ = el.set_attribute("data-theme", &theme);
    }
}

/// Flip the theme and persist the choice, seeding from the OS preference when unset.
pub(crate) fn toggle_theme() {
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
