//! Shared view helpers and small utilities the pages compose from, so the repeated markup (stat
//! tiles, table shells, the flash line, segmented filters, labeled fields), the shared formatters,
//! the generic mutation runner, and the theme toggle live in one place instead of at every call
//! site.

use adi_webapp_api::types::{ProcessUsage, ServicePort, TaskRow};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::highlight::{Lang, highlight};
use crate::state::{Flash, RowMenu, State};

/// A single full-width placeholder row spanning `colspan` columns — the
/// `<tr><td class="adi-empty">…</td></tr>` every table body falls back to for its loading, empty,
/// or error state. A table's span is whatever its user left showing, so callers pass
/// [`Layout::span`] rather than a literal.
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

/// One stat tile in an `adi-tiles` strip: a label, a big value, and a sub-note. `value`/`note`
/// take any view, so a caller passes either a literal or a reactive `move || …` closure.
///
/// Currently unused: the per-page stat strips were removed in favour of a plain count chip in
/// each panel head, with the numbers moving to a dedicated analytics page. Kept for that page.
#[allow(dead_code)]
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

/// How a [`configurable_table`] is ordered: which column, and which way.
///
/// The column is named by its *header text*, not an index — indices stop meaning anything once
/// the user can reorder columns, and a page's comparator matches on the same text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Sort {
    pub(crate) col: &'static str,
    pub(crate) desc: bool,
}

impl Sort {
    /// A table's opening order: `col`, ascending.
    pub(crate) const fn new(col: &'static str) -> Self {
        Self { col, desc: false }
    }

    /// The result of clicking `col`'s header: a different column starts ascending, the active
    /// one flips direction.
    fn toggled(self, col: &'static str) -> Self {
        if self.col == col {
            Self {
                col,
                desc: !self.desc,
            }
        } else {
            Self::new(col)
        }
    }

    /// Turn a key comparison into the ordering this sort asks for. Apply it to the *sort key*
    /// only — a page's tiebreaks should stay ascending, so equal rows hold a stable order in
    /// both directions.
    pub(crate) fn dir(self, ord: std::cmp::Ordering) -> std::cmp::Ordering {
        if self.desc { ord.reverse() } else { ord }
    }

    /// The `aria-sort` value for column `col` — also what the caret styling keys off.
    fn aria(self, col: &str) -> &'static str {
        match self {
            s if s.col != col => "none",
            s if s.desc => "descending",
            _ => "ascending",
        }
    }
}

/// One configurable column: the header it renders under, and whether the user is showing it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Column {
    pub(crate) header: &'static str,
    pub(crate) shown: bool,
}

/// A table's column arrangement: every column it *can* show, in the user's order, each flagged
/// shown or hidden.
///
/// Only named columns are configurable. A trailing blank header is the action column — it holds
/// the row's controls rather than data, so it is neither reorderable nor hideable; [`Layout`]
/// just remembers that the table has one, and [`body_row`] fills it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Layout {
    columns: Vec<Column>,
    has_actions: bool,
}

impl Layout {
    /// The table as its page declared it: every named column shown, in source order.
    pub(crate) fn new(headers: &'static [&'static str]) -> Self {
        Self {
            columns: headers
                .iter()
                .filter(|h| !h.is_empty())
                .map(|h| Column {
                    header: h,
                    shown: true,
                })
                .collect(),
            has_actions: headers.iter().any(|h| h.is_empty()),
        }
    }

    /// Every configurable column, in display order — what the settings menu lists.
    pub(crate) fn columns(&self) -> &[Column] {
        &self.columns
    }

    /// The headers to render, in order: what a row builder walks to emit its cells.
    pub(crate) fn shown(&self) -> Vec<&'static str> {
        self.columns
            .iter()
            .filter(|c| c.shown)
            .map(|c| c.header)
            .collect()
    }

    /// How many cells a full-width row spans — for [`placeholder_row`], which has to match a
    /// column count the user can now change.
    pub(crate) fn span(&self) -> usize {
        self.columns.iter().filter(|c| c.shown).count() + usize::from(self.has_actions)
    }

    /// Whether `col` is the sole remaining column — the one a table may not hide, since an
    /// all-hidden table is unrecoverable without the settings menu it would also have swallowed.
    fn is_last_shown(&self, col: usize) -> bool {
        self.columns.iter().filter(|c| c.shown).count() == 1
            && self.columns.get(col).is_some_and(|c| c.shown)
    }

    fn toggle(&mut self, col: usize) {
        if self.is_last_shown(col) {
            return;
        }
        if let Some(c) = self.columns.get_mut(col) {
            c.shown = !c.shown;
        }
    }

    /// Move a column one place towards the front (`up`) or the back.
    fn shift(&mut self, col: usize, up: bool) {
        let other = if up {
            col.checked_sub(1)
        } else {
            Some(col + 1).filter(|i| *i < self.columns.len())
        };
        if let Some(other) = other {
            self.columns.swap(col, other);
        }
    }

    /// Serialize as `Header` / `!Header` (hidden), comma-separated. A compact form rather than
    /// JSON because it is written to `localStorage` on every tweak and read back on every load.
    fn encode(&self) -> String {
        self.columns
            .iter()
            .map(|c| {
                if c.shown {
                    c.header.to_string()
                } else {
                    format!("!{}", c.header)
                }
            })
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Rebuild a saved arrangement against the table's *current* headers.
    ///
    /// The two can disagree — a stored layout outlives the code that wrote it. Headers the build
    /// no longer declares are dropped, and headers the save doesn't mention are appended, shown:
    /// a column added in a later release appears for everyone instead of staying invisible until
    /// they find Reset.
    fn decode(headers: &'static [&'static str], raw: &str) -> Self {
        let mut layout = Self {
            columns: Vec::new(),
            has_actions: headers.iter().any(|h| h.is_empty()),
        };
        for entry in raw.split(',').filter(|e| !e.is_empty()) {
            let (shown, name) = match entry.strip_prefix('!') {
                Some(name) => (false, name),
                None => (true, entry),
            };
            let Some(header) = headers.iter().find(|h| **h == name && !h.is_empty()) else {
                continue;
            };
            if layout.columns.iter().any(|c| c.header == *header) {
                continue;
            }
            layout.columns.push(Column { header, shown });
        }
        for header in headers.iter().filter(|h| !h.is_empty()) {
            if !layout.columns.iter().any(|c| c.header == *header) {
                layout.columns.push(Column {
                    header,
                    shown: true,
                });
            }
        }
        layout
    }
}

/// One [`configurable_table`]'s user-owned view of itself: how it's sorted, how its columns are
/// arranged, and whether its settings menu is open.
///
/// Sort and layout are persisted to `localStorage` under `key`, so a table stays the way it was
/// left across reloads. `Copy` (arena signal handles), so it threads into views and handlers as
/// cheaply as the rest of [`State`](crate::state::State).
#[derive(Clone, Copy)]
pub(crate) struct TableState {
    key: &'static str,
    headers: &'static [&'static str],
    /// The order the table opens in before the user has sorted it, and what Reset returns to.
    default_sort: Sort,
    pub(crate) sort: RwSignal<Sort>,
    pub(crate) layout: RwSignal<Layout>,
    /// Whether the column settings menu is showing.
    open: RwSignal<bool>,
}

impl TableState {
    /// Restore this table from storage, falling back to `headers`' declared order sorted
    /// ascending by its first named column.
    pub(crate) fn new(key: &'static str, headers: &'static [&'static str]) -> Self {
        let first = headers
            .iter()
            .find(|h| !h.is_empty())
            .copied()
            .unwrap_or("");
        Self::sorted(key, headers, Sort::new(first))
    }

    /// The same, for a table whose natural opening order isn't its first column ascending — a run
    /// history reads newest-first, so it declares `When` descending rather than inheriting a rule
    /// that would silently reverse it.
    pub(crate) fn sorted(
        key: &'static str,
        headers: &'static [&'static str],
        default_sort: Sort,
    ) -> Self {
        let sort = read(key, "sort")
            .and_then(|raw| {
                let (name, dir) = raw.split_once('|')?;
                let col = headers.iter().find(|h| **h == name && !h.is_empty())?;
                Some(Sort {
                    col,
                    desc: dir == "desc",
                })
            })
            .unwrap_or(default_sort);
        let layout = read(key, "cols")
            .map_or_else(|| Layout::new(headers), |raw| Layout::decode(headers, &raw));
        Self {
            key,
            headers,
            default_sort,
            sort: RwSignal::new(sort),
            layout: RwSignal::new(layout),
            open: RwSignal::new(false),
        }
    }

    fn set_sort(self, sort: Sort) {
        self.sort.set(sort);
        write(
            self.key,
            "sort",
            &format!("{}|{}", sort.col, if sort.desc { "desc" } else { "asc" }),
        );
    }

    /// Apply `edit` to the layout and persist the result.
    fn edit_layout(self, edit: impl FnOnce(&mut Layout)) {
        self.layout.update(edit);
        write(self.key, "cols", &self.layout.get_untracked().encode());
    }

    fn reset(self) {
        self.edit_layout(|l| *l = Layout::new(self.headers));
        self.set_sort(self.default_sort);
    }
}

/// One row's value under one column, reduced to something orderable.
///
/// A page's comparator is a single `fn(&Row, &str) -> Key` naming the header it is asked about,
/// which is what lets [`sort_rows`] be shared: every table differs only in that mapping. Sorting
/// on the key rather than the rendered cell is the point — `4 GB` belongs after `900 KB`, and
/// `http:9` before `http:80`.
#[derive(Clone, Debug)]
pub(crate) enum Key {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

impl Key {
    /// A text key from anything string-like.
    pub(crate) fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    /// An optional text cell, absent sorting as empty — which puts the dashes at one end rather
    /// than scattering them.
    pub(crate) fn maybe(value: Option<&str>) -> Self {
        Self::Text(value.unwrap_or_default().to_string())
    }

    /// A count. Saturates rather than wrapping, so an absurd length can't invert an ordering.
    pub(crate) fn count(n: usize) -> Self {
        Self::Int(i64::try_from(n).unwrap_or(i64::MAX))
    }

    /// An unsigned quantity — a timestamp, a byte count, a row count.
    pub(crate) fn num(n: u64) -> Self {
        Self::Int(i64::try_from(n).unwrap_or(i64::MAX))
    }
}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Self::Bool(a), Self::Bool(b)) => a.cmp(b),
            (Self::Int(a), Self::Int(b)) => a.cmp(b),
            (Self::Float(a), Self::Float(b)) => a.total_cmp(b),
            (Self::Text(a), Self::Text(b)) => a.cmp(b),
            // One column always yields one kind of key, so mismatched variants mean the two rows
            // were asked different questions — nothing sensible to compare.
            _ => std::cmp::Ordering::Equal,
        }
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for Key {}

/// Order `rows` by the column `sort` names, using the page's `key` mapping, then `tiebreak`.
///
/// Only the key flips with the direction — `tiebreak` stays ascending — so a descending sort is
/// the exact mirror of its ascending self and a mostly-tied table doesn't reshuffle on every poll.
pub(crate) fn sort_rows<T>(
    rows: &mut [T],
    sort: Sort,
    key: impl Fn(&T, &'static str) -> Key,
    tiebreak: impl Fn(&T) -> Key,
) {
    rows.sort_by(|a, b| {
        sort.dir(key(a, sort.col).cmp(&key(b, sort.col)))
            .then_with(|| tiebreak(a).cmp(&tiebreak(b)))
    });
}

/// One body row: a cell per shown column, in the user's order, then the row's controls in the
/// trailing action cell when the table declares one (`action` of `None` for a table without).
///
/// `shown` is [`Layout::shown`], hoisted out of the row loop by the caller. Building cells by
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

/// Read one of a table's persisted settings. Storage being unavailable (private mode, a
/// disabled origin) is not an error — the table just opens in its declared state.
fn read(key: &str, field: &str) -> Option<String> {
    storage()?
        .get_item(&format!("adi-table:{key}:{field}"))
        .ok()
        .flatten()
        .filter(|v| !v.is_empty())
}

fn write(key: &str, field: &str, value: &str) {
    if let Some(s) = storage() {
        let _ = s.set_item(&format!("adi-table:{key}:{field}"), value);
    }
}

/// The app's one table: the `adi-tablewrap` scroll box, click-to-sort headers, and a gear that
/// opens a menu for showing, hiding, and reordering columns.
///
/// This owns the chrome and the state, never the data: the page reads the same `table.sort` and
/// `table.layout` signals when it builds `body`, emitting one cell per [`Layout::shown`] header
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

/// A code editor over `buffer`: a syntax-highlighted view of the text with a real textarea on
/// top of it. Two stacked layers sharing one box — a painted `<pre>` underneath, and a
/// transparent-text `<textarea>` above that still owns the caret, selection, and typing. They
/// must keep identical font metrics and padding or the two drift apart; `.adi-code` /
/// `.adi-fileedit` enforce that in one place.
///
/// `modifier` is an extra class on the wrapper, for callers that need a different height than
/// the full-pane default (`adi-code--form` sizes it for a form).
pub(crate) fn code_editor(
    lang: impl Fn() -> Lang + Copy + Send + Sync + 'static,
    buffer: RwSignal<String>,
    modifier: &'static str,
    id: &'static str,
) -> AnyView {
    let area = NodeRef::<leptos::html::Textarea>::new();
    let paint = NodeRef::<leptos::html::Pre>::new();
    let class = if modifier.is_empty() {
        "adi-code".to_string()
    } else {
        format!("adi-code {modifier}")
    };
    view! {
        <div class=class>
            <pre class="adi-code__paint adi-mono" aria-hidden="true" node_ref=paint>
                {move || painted(lang(), &buffer.get())}
            </pre>
            <textarea class="adi-textarea adi-mono adi-fileedit" spellcheck="false"
                autocomplete="off" id=id node_ref=area
                prop:value=move || buffer.get()
                on:scroll=move |_| sync_code_scroll(area, paint)
                on:input=move |ev| {
                    buffer.set(event_target_value(&ev));
                    sync_code_scroll(area, paint);
                }></textarea>
        </div>
    }
    .into_any()
}

/// A read-only, syntax-highlighted viewer over `buffer` that follows the tail. Unlike
/// [`code_editor`], this is a single scrollable `<pre>` with no editing textarea: appending output
/// never resets the reader's scroll the way setting a textarea's `value` does, so a log that grows
/// under the poll reads cleanly. It stays pinned to the newest line while the reader is at the
/// bottom; scrolling up pauses the follow, scrolling back to the end resumes it — `tail -f`, inline.
pub(crate) fn code_viewer(
    lang: impl Fn() -> Lang + Copy + Send + Sync + 'static,
    buffer: RwSignal<String>,
    modifier: &'static str,
    id: &'static str,
) -> AnyView {
    let pre = NodeRef::<leptos::html::Pre>::new();
    // Whether the viewer is pinned to the bottom. Starts true so a freshly opened log lands on its
    // newest line; the reader's own scrolling flips it (see the scroll handler below).
    let follow = RwSignal::new(true);
    // Follow the tail: when the buffer grows, drop to the newest line — but only while pinned, so
    // scrolling up to read history isn't yanked away. Reads `buffer`, so it also runs on mount.
    Effect::new(move |_| {
        let _ = buffer.get();
        if follow.get_untracked()
            && let Some(p) = pre.get_untracked()
        {
            p.set_scroll_top(p.scroll_height());
        }
    });
    let class = if modifier.is_empty() {
        "adi-logview adi-mono".to_string()
    } else {
        format!("adi-logview adi-mono {modifier}")
    };
    view! {
        <pre class=class id=id node_ref=pre tabindex="0"
            on:scroll=move |_| {
                if let Some(p) = pre.get_untracked() {
                    // Within a couple of lines of the end still counts as "following".
                    let dist = p.scroll_height() - p.scroll_top() - p.client_height();
                    follow.set(dist <= 24);
                }
            }>
            {move || painted(lang(), &buffer.get())}
        </pre>
    }
    .into_any()
}

/// The buffer painted as coloured spans. A trailing newline gets a space appended so the last
/// line still has height — otherwise the painted layer is one line shorter than the textarea
/// and the two scroll out of step at the bottom.
fn painted(lang: Lang, src: &str) -> AnyView {
    let mut text = src.to_string();
    if text.ends_with('\n') {
        text.push(' ');
    }
    highlight(lang, &text)
        .into_iter()
        .map(|(tok, run)| view! { <span class=tok.class()>{run}</span> })
        .collect::<Vec<_>>()
        .into_any()
}

/// Keep the painted layer aligned with the textarea's scroll position. The textarea is the one
/// that actually scrolls; the `<pre>` is dragged along behind it.
fn sync_code_scroll(area: NodeRef<leptos::html::Textarea>, paint: NodeRef<leptos::html::Pre>) {
    if let (Some(a), Some(p)) = (area.get_untracked(), paint.get_untracked()) {
        p.set_scroll_top(a.scroll_top());
        p.set_scroll_left(a.scroll_left());
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A table with an action column, so the tests cover the blank-header rule too.
    const COLS: &[&str] = &["Source", "Service", "CPU", "Memory", ""];

    fn headers(layout: &Layout) -> Vec<&'static str> {
        layout.columns().iter().map(|c| c.header).collect()
    }

    /// The action column is the row's controls, not data: it is never offered in the menu, never
    /// moves, and never hides — but it still counts towards a placeholder's span.
    #[test]
    fn the_action_column_is_not_configurable_but_is_still_a_column() {
        let layout = Layout::new(COLS);
        assert_eq!(headers(&layout), ["Source", "Service", "CPU", "Memory"]);
        assert_eq!(layout.shown().len(), 4);
        assert_eq!(layout.span(), 5, "four shown, plus the action column");

        // A table without one spans only what it shows.
        assert_eq!(Layout::new(&["A", "B"]).span(), 2);
    }

    #[test]
    fn hiding_a_column_drops_it_from_the_row_and_the_span() {
        let mut layout = Layout::new(COLS);
        layout.toggle(1); // Service
        assert_eq!(layout.shown(), ["Source", "CPU", "Memory"]);
        assert_eq!(layout.span(), 4);
        assert_eq!(
            headers(&layout),
            ["Source", "Service", "CPU", "Memory"],
            "a hidden column keeps its place in the menu, so re-showing it returns it there"
        );

        layout.toggle(1);
        assert_eq!(layout.shown().len(), 4, "and toggles back");
    }

    /// An all-hidden table would swallow its own settings menu, so the last one is pinned.
    #[test]
    fn the_final_visible_column_cannot_be_hidden() {
        let mut layout = Layout::new(&["A", "B"]);
        layout.toggle(1);
        assert_eq!(layout.shown(), ["A"]);
        layout.toggle(0);
        assert_eq!(layout.shown(), ["A"], "the last column ignores the toggle");
    }

    #[test]
    fn moving_a_column_reorders_it_and_stops_at_the_ends() {
        let mut layout = Layout::new(COLS);
        layout.shift(3, true); // Memory up, past CPU
        assert_eq!(layout.shown(), ["Source", "Service", "Memory", "CPU"]);

        layout.shift(0, true);
        assert_eq!(
            layout.shown()[0],
            "Source",
            "the first column can't move further up"
        );
        layout.shift(3, false);
        assert_eq!(layout.shown()[3], "CPU", "nor the last one further down");
    }

    #[test]
    fn an_arrangement_survives_a_round_trip_through_storage() {
        let mut layout = Layout::new(COLS);
        layout.shift(3, true);
        layout.toggle(0);

        let restored = Layout::decode(COLS, &layout.encode());
        assert_eq!(restored, layout);
        assert_eq!(restored.shown(), ["Service", "Memory", "CPU"]);
    }

    /// A stored arrangement outlives the build that wrote it. Neither a dropped column nor a new
    /// one may strand the user: the first is forgotten, the second appears rather than hiding
    /// until someone finds Reset.
    #[test]
    fn a_saved_arrangement_reconciles_with_changed_headers() {
        // Saved when the table still had a "Ports" column and no "Memory".
        let saved = "Ports,!Service,Source,CPU";
        let layout = Layout::decode(COLS, saved);

        assert_eq!(
            headers(&layout),
            ["Service", "Source", "CPU", "Memory"],
            "`Ports` is gone; `Memory` is appended"
        );
        assert_eq!(
            layout.shown(),
            ["Source", "CPU", "Memory"],
            "the saved hidden flag survives, and the new column arrives shown"
        );
    }

    #[test]
    fn a_corrupt_saved_arrangement_still_yields_every_column() {
        for raw in ["", ",,,", "!!!", "Nonsense,Source,Source"] {
            let layout = Layout::decode(COLS, raw);
            assert_eq!(
                headers(&layout).len(),
                4,
                "{raw:?} must still produce the full column set"
            );
        }
    }

    /// A row of the toy table the sort tests order.
    #[derive(Debug, PartialEq)]
    struct Row {
        name: &'static str,
        bytes: u64,
        cpu: f64,
        up: bool,
    }

    fn row(name: &'static str, bytes: u64, cpu: f64, up: bool) -> Row {
        Row {
            name,
            bytes,
            cpu,
            up,
        }
    }

    fn rows() -> Vec<Row> {
        vec![
            row("web", 900_000, 4.0, false),
            row("api", 4_000_000_000, 0.5, true),
            row("app", 30_000_000, 45.0, true),
        ]
    }

    fn key(r: &Row, col: &str) -> Key {
        match col {
            "Memory" => Key::num(r.bytes),
            "CPU" => Key::Float(r.cpu),
            "Status" => Key::Bool(r.up),
            _ => Key::text(r.name),
        }
    }

    fn names(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.name).collect()
    }

    /// The sort a second click on `col` produces: that column, descending.
    fn desc(col: &'static str) -> Sort {
        Sort { col, desc: true }
    }

    /// The point of sorting on a key rather than the rendered cell: `4 GB` belongs after
    /// `900 KB`, however the two are spelled in their column.
    #[test]
    fn a_column_orders_by_its_value_not_its_formatted_cell() {
        let mut rows = rows();
        sort_rows(&mut rows, Sort::new("Memory"), key, |r| Key::text(r.name));
        assert_eq!(names(&rows), ["web", "app", "api"]);

        sort_rows(&mut rows, desc("Memory"), key, |r| Key::text(r.name));
        assert_eq!(
            names(&rows),
            ["api", "app", "web"],
            "descending is the exact mirror"
        );
    }

    /// Only the key flips with the direction — the tiebreak stays ascending, so a mostly-tied
    /// table holds one order instead of reshuffling as the poll redraws it.
    #[test]
    fn ties_hold_the_same_order_in_both_directions() {
        // Every row's Status ties within its group, so the tiebreak decides each group's order.
        let mut ascending = rows();
        sort_rows(&mut ascending, Sort::new("Status"), key, |r| {
            Key::text(r.name)
        });
        assert_eq!(names(&ascending), ["web", "api", "app"]);

        let mut descending = rows();
        sort_rows(&mut descending, desc("Status"), key, |r| Key::text(r.name));
        assert_eq!(
            names(&descending),
            ["api", "app", "web"],
            "the groups swap, but each keeps its ascending tiebreak"
        );
    }

    /// A header the comparator doesn't name — one a page hides, renames, or has yet to key —
    /// falls to its catch-all rather than leaving the rows in an arbitrary order.
    #[test]
    fn an_unkeyed_column_falls_through_to_the_comparators_default() {
        let mut rows = rows();
        sort_rows(&mut rows, Sort::new("Nonsense"), key, |r| Key::text(r.name));
        assert_eq!(names(&rows), ["api", "app", "web"], "by name, the default");
    }

    /// Floats order by value, including against each other — `total_cmp`, so a comparator can't
    /// be handed a partial ordering.
    #[test]
    fn every_key_kind_orders_within_itself() {
        let mut rows = rows();
        sort_rows(&mut rows, Sort::new("CPU"), key, |r| Key::text(r.name));
        assert_eq!(names(&rows), ["api", "web", "app"]);

        assert!(Key::Bool(false) < Key::Bool(true));
        assert!(Key::num(0) < Key::count(1));
        assert!(Key::maybe(None) < Key::text("a"), "absent sorts as empty");
        assert_eq!(
            Key::text("a").cmp(&Key::Int(1)),
            std::cmp::Ordering::Equal,
            "two kinds means two different questions — nothing to compare"
        );
    }

    /// A table opens the way its page declared, and Reset returns it there — not to the
    /// first-column-ascending rule, which would silently reverse a newest-first history.
    #[test]
    fn reset_restores_the_declared_default_sort() {
        let newest_first = Sort {
            col: "Memory",
            desc: true,
        };
        let table = TableState::sorted("test-reset", COLS, newest_first);
        assert_eq!(table.sort.get_untracked(), newest_first);

        table.set_sort(Sort::new("Source"));
        table.edit_layout(|l| l.toggle(1));
        table.reset();

        assert_eq!(table.sort.get_untracked(), newest_first);
        assert_eq!(table.layout.get_untracked(), Layout::new(COLS));
    }

    #[test]
    fn sort_toggles_direction_only_on_the_active_column() {
        let sort = Sort::new("CPU");
        assert!(!sort.desc);
        assert!(sort.toggled("CPU").desc, "a second click flips it");
        assert!(
            !sort.toggled("CPU").toggled("Memory").desc,
            "a different column starts ascending again"
        );
        assert_eq!(sort.aria("CPU"), "ascending");
        assert_eq!(sort.aria("Memory"), "none");
        assert_eq!(sort.toggled("CPU").aria("CPU"), "descending");
    }
}
