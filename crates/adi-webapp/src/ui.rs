//! Shared view helpers and small utilities the pages compose from, so the repeated markup (stat
//! tiles, table shells, the flash line, segmented filters, labeled fields), the shared formatters,
//! the generic mutation runner, and the theme toggle live in one place instead of at every call
//! site.

use adi_webapp_api::types::{ProcessUsage, ServicePort, TaskRow};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::state::{Flash, RowMenu, State};

/// The table's state — sorting, the column arrangement, and its persistence — now lives in
/// [`adi_ui::table`], because none of it is about how a table *looks*. What stays here is the
/// markup it drives ([`configurable_table`] and friends), which is still written against the
/// `adi-*` layer rather than adi-ui's utilities.
///
/// Re-exported under the names the pages already import, so `use crate::ui::{Key, TableState}`
/// keeps meaning what it meant. `SortKey` comes back as `Key`: unqualified it is too vague to
/// export from a component library, and too long to write 115 times in a comparator.
pub(crate) use adi_ui::{Sort, SortKey as Key, TableState, sort_rows};

/// A single full-width placeholder row spanning `colspan` columns — the
/// `<tr><td class="adi-empty">…</td></tr>` every table body falls back to for its loading, empty,
/// or error state. A table's span is whatever its user left showing, so callers pass
/// [`adi_ui::Layout::span`] rather than a literal.
pub(crate) fn placeholder_row(colspan: usize, msg: &str) -> AnyView {
    view! { <tr><td class="adi-empty" colspan=colspan>{msg.to_string()}</td></tr> }.into_any()
}

/// The trailing action controls for a table row: any always-visible `inline` buttons, then — when
/// `items` is non-empty — a `⋮` kebab that opens an overflow menu holding them (each built with
/// [`menu_item`], so it closes the menu when chosen). With no items the kebab is dropped and only
/// `inline` shows. `key` identifies this row's menu and must be unique among the rows on screen
/// (namespace it per table, e.g. `secret:…`/`tool:…`, so two panels on one page never collide). A
/// full-viewport scrim behind the open menu makes the next click a dismiss. Shared by every page's
/// action column, backed by the single [`State::row_menu`] signal (only one menu is ever open), so
/// this replaces the old per-row `flex-end` button clusters.
pub(crate) fn row_actions(
    state: State,
    key: String,
    inline: impl IntoView + 'static,
    items: Vec<AnyView>,
) -> AnyView {
    // No overflow actions ⇒ no kebab, no menu — just the inline controls in the shared container.
    if items.is_empty() {
        return view! { <div class="adi-rowacts">{inline}</div> }.into_any();
    }
    let rm = state.row_menu;
    let toggle_key = key.clone();
    let aria_key = key.clone();
    let scrim_key = key.clone();
    // The menu and its scrim stay mounted but hidden (display:none) until this row is the open
    // one — cheaper than rebuilding the menu view on every open, and it keeps each item's click
    // handler a plain move-closure rather than something rebuildable.
    view! {
        <div class="adi-rowacts">
            {inline}
            <button class="adi-btn adi-btn--icon-sm" type="button" title="More actions"
                aria-label="More actions"
                aria-expanded=move || rm.get().is_some_and(|m| m.key == aria_key).to_string()
                on:click=move |ev: web_sys::MouseEvent| toggle_row_menu(rm, &toggle_key, &ev)>
                "\u{22ee}"
            </button>
        </div>
        <div class="adi-menu__scrim"
            style=move || if rm.get().is_some_and(|m| m.key == scrim_key) { String::new() } else { "display:none".to_string() }
            on:click=move |_| rm.set(None)
            on:contextmenu=move |ev: web_sys::MouseEvent| { ev.prevent_default(); rm.set(None); }></div>
        <div class="adi-menu"
            style=move || match rm.get() {
                Some(m) if m.key == key => format!("right:{}px; top:{}px", m.right, m.top),
                _ => "display:none".to_string(),
            }>
            {items}
        </div>
    }
    .into_any()
}

/// One item in a row's overflow [menu](row_actions): a full-width menu button that closes the menu,
/// then runs `on_select`. `danger` tints it as destructive (Remove/Delete). Wrap the call in a
/// `.then(|| …)` to make an item conditional.
pub(crate) fn menu_item(
    state: State,
    label: &str,
    danger: bool,
    on_select: impl Fn() + 'static,
) -> AnyView {
    let rm = state.row_menu;
    let class = if danger {
        "adi-menu__item adi-menu__item--danger"
    } else {
        "adi-menu__item"
    };
    let label = label.to_string();
    view! {
        <button class=class type="button" on:click=move |_| { rm.set(None); on_select(); }>
            {label}
        </button>
    }
    .into_any()
}

/// Open (or close, if already this row's) the shared kebab menu for `key`, anchored to the click
/// point. Anchored from the viewport's right edge so it opens leftward from the right-aligned kebab
/// and never spills off-screen.
fn toggle_row_menu(rm: RwSignal<Option<RowMenu>>, key: &str, ev: &web_sys::MouseEvent) {
    if rm.get_untracked().is_some_and(|m| m.key == key) {
        rm.set(None);
        return;
    }
    let inner_w = web_sys::window()
        .and_then(|w| w.inner_width().ok())
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    #[allow(clippy::cast_possible_truncation)]
    rm.set(Some(RowMenu {
        key: key.to_string(),
        right: (inner_w - f64::from(ev.client_x())) as i32,
        top: ev.client_y(),
    }));
}

/// Format an uptime in seconds as `Ns` / `Nm Ss` / `Nh Mm`.
pub(crate) fn fmt_uptime(s: u64) -> String {
    if s < 60 {
        format!("{s}s")
    } else if s < 3_600 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}h {}m", s / 3_600, (s % 3_600) / 60)
    }
}

/// One body row: a cell per shown column, in the user's order, then the row's controls in the
/// trailing action cell when the table declares one (`action` of `None` for a table without).
///
/// `shown` is [`adi_ui::Layout::shown`], hoisted out of the row loop by the caller. Building cells by
/// asking for a header rather than emitting a fixed sequence is what makes hiding and reordering
/// invisible to a page's row builder.
pub(crate) fn body_row(
    shown: &[&'static str],
    cell: impl Fn(&'static str) -> AnyView,
    action: Option<AnyView>,
) -> AnyView {
    let cells: Vec<AnyView> = shown.iter().map(|col| cell(col)).collect();
    view! {
        <tr>
            {cells}
            {action.map(|a| view! { <td class="adi-table__actions">{a}</td> })}
        </tr>
    }
    .into_any()
}

/// The app's one table: the `adi-tablewrap` scroll box, click-to-sort headers, and a gear that
/// opens a menu for showing, hiding, and reordering columns.
///
/// This owns the chrome and the state, never the data: the page reads the same `table.sort` and
/// `table.layout` signals when it builds `body`, emitting one cell per [`adi_ui::Layout::shown`] header
/// (see [`body_row`]). `headers` is the full set the table can ever show — the user's subset and
/// order live in the layout.
///
/// A table with a single named column gets sorting but no gear: there is nothing to reorder, and
/// its one column may not be hidden, so the menu would offer only disabled controls.
pub(crate) fn configurable_table(
    table: TableState,
    headers: &'static [&'static str],
    body: impl IntoView + 'static,
) -> AnyView {
    let sort = table.sort;
    let cells = move || {
        let mut cells: Vec<AnyView> = table
            .layout
            .get()
            .shown()
            .into_iter()
            .map(|header| {
                view! {
                    <th class="adi-table__sort" aria-sort=move || sort.get().aria(header)>
                        // A real button, so the header is reachable and operable by keyboard.
                        <button type="button" title=format!("Sort by {header}")
                            on:click=move |_| table.set_sort(sort.get_untracked().toggled(header))>
                            {header}
                        </button>
                    </th>
                }
                .into_any()
            })
            .collect();
        if headers.iter().any(|h| h.is_empty()) {
            cells.push(view! { <th></th> }.into_any());
        }
        cells
    };
    // Without a gear there is no absolute box to anchor and no header padding to reserve, so the
    // shell is rendered bare rather than wrapped in a `.adi-tablebox` that does nothing.
    if headers.iter().filter(|h| !h.is_empty()).count() < 2 {
        return table_shell(cells, body);
    }
    view! {
        <div class="adi-tablebox">
            {column_menu(table)}
            {table_shell(cells, body)}
        </div>
    }
    .into_any()
}

/// The shared scroll box + table markup, so the shell exists once.
fn table_shell(header_cells: impl IntoView + 'static, body: impl IntoView + 'static) -> AnyView {
    view! {
        <div class="adi-tablewrap">
            <table class="adi-table">
                <thead><tr>{header_cells}</tr></thead>
                <tbody>{body}</tbody>
            </table>
        </div>
    }
    .into_any()
}

/// The gear and the panel it opens: one row per column with a show/hide toggle and a pair of
/// move buttons, plus Reset. Buttons rather than drag-and-drop — the list is short, and this
/// works from the keyboard without a custom drop target.
fn column_menu(table: TableState) -> AnyView {
    let open = table.open;
    let rows = move || {
        let layout = table.layout.get();
        let last = layout.columns().len().saturating_sub(1);
        layout
            .columns()
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let (header, shown) = (c.header, c.shown);
                let locked = layout.is_last_shown(i);
                view! {
                    <div class="adi-colmenu__row">
                        <button class="adi-colmenu__pick" type="button"
                            role="checkbox" aria-checked=shown.to_string()
                            prop:disabled=locked
                            title=if locked { "A table keeps at least one column" }
                                  else if shown { "Hide this column" } else { "Show this column" }
                            on:click=move |_| table.edit_layout(|l| l.toggle(i))>
                            <span class="adi-colmenu__box" data-on=shown.to_string()>
                                {if shown { "\u{2713}" } else { "" }}
                            </span>
                            <span>{header}</span>
                        </button>
                        <button class="adi-btn adi-btn--icon-sm" type="button"
                            title="Move up" aria-label=format!("Move {header} up")
                            prop:disabled=i == 0
                            on:click=move |_| table.edit_layout(|l| l.shift(i, true))>"\u{2191}"</button>
                        <button class="adi-btn adi-btn--icon-sm" type="button"
                            title="Move down" aria-label=format!("Move {header} down")
                            prop:disabled=i == last
                            on:click=move |_| table.edit_layout(|l| l.shift(i, false))>"\u{2193}"</button>
                    </div>
                }
                .into_any()
            })
            .collect::<Vec<_>>()
    };
    view! {
        <button class="adi-btn adi-btn--icon-sm adi-tablebox__cog" type="button"
            title="Columns" aria-label="Configure columns"
            aria-expanded=move || open.get().to_string()
            on:click=move |_| open.update(|o| *o = !*o)>
            <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor"
                stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"
                inner_html=crate::icons::Icon::Gear.path()></svg>
        </button>
        // Mounted but hidden until opened, matching the row kebab: the scrim makes the next
        // click anywhere a dismiss.
        <div class="adi-menu__scrim"
            style=move || if open.get() { String::new() } else { "display:none".to_string() }
            on:click=move |_| open.set(false)></div>
        <div class="adi-menu adi-colmenu"
            style=move || if open.get() { String::new() } else { "display:none".to_string() }>
            <div class="adi-menu__head">
                <span>"Columns"</span>
                <button class="adi-btn adi-btn--link" type="button"
                    on:click=move |_| table.reset()>"Reset"</button>
            </div>
            {rows}
        </div>
    }
    .into_any()
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

/// A labeled text input bound to a `String` signal — the `adi-field` wrapper the forms repeat.
/// Optional props toggle the mono/wide input classes, a numeric input mode, a trailing hint line,
/// and extra classes on the field wrapper (e.g. `adi-field--grow`).
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
    #[prop(optional)] field_class: &'static str,
) -> impl IntoView {
    let mut class = String::from("adi-input");
    if wide {
        class.push_str(" adi-input--wide");
    }
    if mono {
        class.push_str(" adi-mono");
    }
    let mut field = String::from("adi-field");
    if !field_class.is_empty() {
        field.push(' ');
        field.push_str(field_class);
    }
    let inputmode = if numeric { "numeric" } else { "text" };
    view! {
        <div class=field>
            <label class="adi-field__label" for=id>{label}</label>
            <input class=class id=id placeholder=placeholder autocomplete="off" inputmode=inputmode
                prop:value=move || value.get()
                on:input=move |ev| value.set(event_target_value(&ev)) />
            {(!hint.is_empty()).then(|| field_hint(hint))}
        </div>
    }
}

/// A field's explanation, rendered as a “?” beside its label that opens the text on hover or
/// keyboard focus. Written after the control (where it reads naturally in the markup); the
/// field's grid places it up next to the title, so it costs the form no vertical space.
pub(crate) fn field_hint(text: impl IntoView + 'static) -> AnyView {
    view! {
        <span class="adi-field__hint" tabindex="0" role="note">
            <span class="adi-field__hint-text">{text}</span>
        </span>
    }
    .into_any()
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

/// A native confirm dialog, returning `true` only when the user accepts. Gates the irreversible
/// row actions (permanent deletes) behind an explicit yes; a browser that has no `confirm`
/// (or denies it) reads as "cancelled", so nothing is destroyed by accident.
pub(crate) fn confirm(message: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(message).ok())
        .unwrap_or(false)
}

/// The "updated Ns ago" label; empty until the first successful load. Generic over the loaded
/// payload — every page has its own state type and only emptiness matters here.
pub(crate) fn updated_text<T>(loaded: RwSignal<Option<T>>, secs_since: RwSignal<u32>) -> String
where
    T: Send + Sync + 'static,
{
    // `with` rather than `get`, so testing emptiness never clones the payload.
    if loaded.with(Option::is_none) {
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

/// Format a byte count as `N KB` / `N.N MB` / `N.NN GB` — decimal units, so the numbers read the
/// same as Activity Monitor's.
pub(crate) fn fmt_bytes(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let n = bytes as f64;
    if bytes < 1_000_000 {
        format!("{:.0} KB", n / 1_000.0)
    } else if bytes < 1_000_000_000 {
        format!("{:.1} MB", n / 1_000_000.0)
    } else {
        format!("{:.2} GB", n / 1_000_000_000.0)
    }
}

/// Format a CPU share (percent of one core, so it can exceed 100) for a table cell: whole
/// percents once it's past 10, one decimal below that, where the difference between idle and
/// slightly busy actually matters.
pub(crate) fn fmt_cpu(percent: f32) -> String {
    if percent >= 10.0 {
        format!("{percent:.0}%")
    } else {
        format!("{percent:.1}%")
    }
}

/// The CPU cell for a service row. Separate from [`memory_cell`] rather than one two-`<td>`
/// helper, because a [`configurable_table`] lets the user hide or reorder either independently.
pub(crate) fn cpu_cell(usage: Option<&ProcessUsage>) -> AnyView {
    usage_cell(usage, |u| fmt_cpu(u.cpu_percent))
}

/// The memory cell for a service row. See [`cpu_cell`].
pub(crate) fn memory_cell(usage: Option<&ProcessUsage>) -> AnyView {
    usage_cell(usage, |u| fmt_bytes(u.memory_bytes))
}

/// One sampled-usage cell: `value` of the sample, or a muted em dash when the service is down
/// (or the host could not sample it). The `title` spells out what the number covers, since it
/// rolls up the listener's whole process tree.
fn usage_cell(usage: Option<&ProcessUsage>, value: impl Fn(&ProcessUsage) -> String) -> AnyView {
    let Some(u) = usage else {
        return view! { <td class="adi-mono adi-muted">"—"</td> }.into_any();
    };
    let procs = if u.processes == 1 {
        format!("pid {}", u.pid)
    } else {
        format!("pid {} + {} child processes", u.pid, u.processes - 1)
    };
    let title = format!("{procs}, up {}", fmt_uptime(u.uptime_secs));
    view! { <td class="adi-mono" title=title>{value(u)}</td> }.into_any()
}

/// The capitalized display label for a task's computed effective status (`ready`/`blocked`/
/// `done`/`archived`), used with the `adi-tstatus` pill on both the Tasks page and a project's
/// detail panel.
pub(crate) fn effective_label_title(effective: &str) -> &'static str {
    match effective {
        "ready" => "Ready",
        "blocked" => "Blocked",
        "done" => "Done",
        "archived" => "Archived",
        _ => "—",
    }
}

/// Flatten a flat task list into depth-annotated tree order: each task is immediately followed by
/// its subtree (children in their incoming order), so a caller renders one row per task and indents
/// by the returned depth. A task whose `parent` isn't in the set is treated as a root, so nothing is
/// ever dropped. Depth is **unbounded** — the tree may nest arbitrarily deep. Shared by the global
/// Tasks page and a project's detail panel.
pub(crate) fn task_tree_rows(rows: Vec<TaskRow>) -> Vec<(usize, TaskRow)> {
    use std::collections::{HashMap, HashSet};

    let ids: HashSet<String> = rows.iter().map(|r| r.id.clone()).collect();
    let mut children: HashMap<String, Vec<TaskRow>> = HashMap::new();
    let mut roots: Vec<TaskRow> = Vec::new();
    for r in rows {
        match &r.parent {
            Some(p) if ids.contains(p) => children.entry(p.clone()).or_default().push(r),
            _ => roots.push(r),
        }
    }

    fn walk(
        node: TaskRow,
        depth: usize,
        children: &mut HashMap<String, Vec<TaskRow>>,
        out: &mut Vec<(usize, TaskRow)>,
    ) {
        let id = node.id.clone();
        out.push((depth, node));
        if let Some(kids) = children.remove(&id) {
            for kid in kids {
                walk(kid, depth + 1, children, out);
            }
        }
    }

    let mut out = Vec::new();
    for root in roots {
        walk(root, 0, &mut children, &mut out);
    }
    out
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

/// The origin's `localStorage`, when there is one. `None` covers private mode and a disabled
/// origin — and, off wasm, the absence of a browser at all, which is what lets the unit tests
/// build a [`TableState`] without reaching for a `window` that isn't there.
fn storage() -> Option<web_sys::Storage> {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()?.local_storage().ok().flatten()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

fn prefers_dark() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten())
        .is_some_and(|m| m.matches())
}
