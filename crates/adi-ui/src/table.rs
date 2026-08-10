//! A sortable, user-configurable table: the state, and the markup that wears it.
//!
//! The state half is what is fiddly and worth getting right exactly once, and all of it is
//! invisible: what "sorted by Size descending" means when the cell says `4 GB`, what happens
//! to a saved column arrangement when a release adds a column, and which column a table
//! refuses to let you hide. [`TableState`] owns that, and is usable on its own.
//!
//! The markup half is [`Table`] — the scroll box, the click-to-sort header row, and the gear
//! that opens the show/hide/reorder menu. What it deliberately does *not* own is a cell:
//! rows are the caller's, because a cell holds badges, links and buttons that no table API
//! can anticipate.
//!
//! ```ignore
//! let table = TableState::new("ports", &["Port", "Service", "PID", ""]);
//! let mut rows = load();
//! sort_rows(&mut rows, table.sort.get(), key_of, |r| SortKey::text(&r.name));
//!
//! view! {
//!     <Table state=table>
//!         {rows.into_iter().map(|r| view! {
//!             <Row state=table
//!                 cell=move |col| match col {
//!                     "Port" => view! { <span class="text-accent">{r.port}</span> }.into_any(),
//!                     _ => view! { {r.service.clone()} }.into_any(),
//!                 }
//!                 actions=move || view! { <Button size=ButtonSize::Small>"Free"</Button> }.into_any()
//!             />
//!         }).collect_view()}
//!     </Table>
//! }
//! ```
//!
//! # A cell is asked for, never assumed
//!
//! [`Row`] walks [`Layout::shown`] and asks `cell` for each header by name, rather than
//! taking a fixed sequence of cells. That indirection is the whole reason hiding and
//! reordering work: a row builder that answers questions cannot fall out of step with a
//! column order the user has since rearranged.
//!
//! # Sorting on keys, not on cells
//!
//! Every comparison goes through [`SortKey`], which a page produces from a row and a header
//! name. That indirection is the whole reason the ordering is right: `4 GB` sorts after
//! `900 KB` and `http:9` before `http:80` because what is compared is the number, never the
//! string that was rendered from it.
//!
//! # A saved layout outlives the code that saved it
//!
//! Arrangements persist to `localStorage`, so a table stays how it was left. [`Layout::decode`]
//! therefore rebuilds a save against the *current* headers rather than trusting it: headers
//! the build no longer declares are dropped, and headers the save never heard of are appended
//! and shown — so a column added in a later release appears for everybody, instead of being
//! invisible until they find Reset.

use leptos::prelude::*;

use crate::merge;

/// How a [`TableState`] is ordered: which column, and which way.
///
/// The column is named by its *header text*, not an index — indices stop meaning anything once
/// the user can reorder columns, and a page's comparator matches on the same text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sort {
    pub col: &'static str,
    pub desc: bool,
}

impl Sort {
    /// A table's opening order: `col`, ascending.
    #[must_use]
    pub const fn new(col: &'static str) -> Self {
        Self { col, desc: false }
    }

    /// The result of clicking `col`'s header: a different column starts ascending, the active
    /// one flips direction.
    #[must_use]
    pub fn toggled(self, col: &'static str) -> Self {
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
    #[must_use]
    pub fn dir(self, ord: std::cmp::Ordering) -> std::cmp::Ordering {
        if self.desc { ord.reverse() } else { ord }
    }

    /// The `aria-sort` value for column `col` — also what the caret styling keys off.
    #[must_use]
    pub fn aria(self, col: &str) -> &'static str {
        match self {
            s if s.col != col => "none",
            s if s.desc => "descending",
            _ => "ascending",
        }
    }
}

/// One configurable column: the header it renders under, and whether the user is showing it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Column {
    pub header: &'static str,
    pub shown: bool,
}

/// A table's column arrangement: every column it *can* show, in the user's order, each flagged
/// shown or hidden.
///
/// Only named columns are configurable. A trailing blank header is the action column — it holds
/// the row's controls rather than data, so it is neither reorderable nor hideable; [`Layout`]
/// just remembers that the table has one, and the row builder fills it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Layout {
    columns: Vec<Column>,
    has_actions: bool,
}

impl Layout {
    /// The table as its page declared it: every named column shown, in source order.
    #[must_use]
    pub fn new(headers: &'static [&'static str]) -> Self {
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
    #[must_use]
    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    /// The headers to render, in order: what a row builder walks to emit its cells.
    #[must_use]
    pub fn shown(&self) -> Vec<&'static str> {
        self.columns
            .iter()
            .filter(|c| c.shown)
            .map(|c| c.header)
            .collect()
    }

    /// How many cells a full-width row spans — for a placeholder row, which has to match a
    /// column count the user can now change.
    #[must_use]
    pub fn span(&self) -> usize {
        self.columns.iter().filter(|c| c.shown).count() + usize::from(self.has_actions)
    }

    /// Whether the table declared a trailing action column (a blank header). The header row
    /// owes it an empty `<th>` and every body row a cell, so both halves of the markup ask.
    #[must_use]
    pub const fn has_actions(&self) -> bool {
        self.has_actions
    }

    /// Whether `col` is the sole remaining column — the one a table may not hide, since an
    /// all-hidden table is unrecoverable without the settings menu it would also have swallowed.
    #[must_use]
    pub fn is_last_shown(&self, col: usize) -> bool {
        self.columns.iter().filter(|c| c.shown).count() == 1
            && self.columns.get(col).is_some_and(|c| c.shown)
    }

    pub fn toggle(&mut self, col: usize) {
        if self.is_last_shown(col) {
            return;
        }
        if let Some(c) = self.columns.get_mut(col) {
            c.shown = !c.shown;
        }
    }

    /// Move a column one place towards the front (`up`) or the back.
    pub fn shift(&mut self, col: usize, up: bool) {
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
    #[must_use]
    pub fn encode(&self) -> String {
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
    #[must_use]
    pub fn decode(headers: &'static [&'static str], raw: &str) -> Self {
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

/// One table's user-owned view of itself: how it's sorted, how its columns are arranged,
/// and whether its settings menu is open.
///
/// Sort and layout are persisted to `localStorage` under `key`, so a table stays the way it
/// was left across reloads. `Copy` (arena signal handles), so it threads into views and
/// handlers as cheaply as the rest of a page's state.
///
/// The three signals are public because the markup is the caller's: a page reads `sort` and
/// `layout` to build its header and its rows, and binds `open` to whatever it uses for a
/// settings menu. Everything that *changes* them goes through the methods below, so the
/// write to storage can never be forgotten at a call site.
#[derive(Clone, Copy, Debug)]
pub struct TableState {
    key: &'static str,
    headers: &'static [&'static str],
    /// The order the table opens in before the user has sorted it, and what Reset returns to.
    default_sort: Sort,
    /// The column being sorted on, and which way.
    pub sort: RwSignal<Sort>,
    /// Which columns are showing, in the user's order.
    pub layout: RwSignal<Layout>,
    /// Whether the column settings menu is showing. Not persisted — a menu left open is not
    /// a preference.
    pub open: RwSignal<bool>,
}

impl TableState {
    /// Restore this table from storage, falling back to `headers`' declared order sorted
    /// ascending by its first named column.
    #[must_use]
    pub fn new(key: &'static str, headers: &'static [&'static str]) -> Self {
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
    #[must_use]
    pub fn sorted(
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

    pub fn set_sort(self, sort: Sort) {
        self.sort.set(sort);
        write(
            self.key,
            "sort",
            &format!("{}|{}", sort.col, if sort.desc { "desc" } else { "asc" }),
        );
    }

    /// Apply `edit` to the layout and persist the result.
    pub fn edit_layout(self, edit: impl FnOnce(&mut Layout)) {
        self.layout.update(edit);
        write(self.key, "cols", &self.layout.get_untracked().encode());
    }

    pub fn reset(self) {
        self.edit_layout(|l| *l = Layout::new(self.headers));
        self.set_sort(self.default_sort);
    }
}

/// One row's value under one column, reduced to something orderable.
///
/// A page's comparator is a single `fn(&Row, &str) -> SortKey` naming the header it is asked about,
/// which is what lets [`sort_rows`] be shared: every table differs only in that mapping. Sorting
/// on the key rather than the rendered cell is the point — `4 GB` belongs after `900 KB`, and
/// `http:9` before `http:80`.
#[derive(Clone, Debug)]
pub enum SortKey {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

impl SortKey {
    /// A text key from anything string-like.
    #[must_use]
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    /// An optional text cell, absent sorting as empty — which puts the dashes at one end rather
    /// than scattering them.
    #[must_use]
    pub fn maybe(value: Option<&str>) -> Self {
        Self::Text(value.unwrap_or_default().to_string())
    }

    /// A count. Saturates rather than wrapping, so an absurd length can't invert an ordering.
    #[must_use]
    pub fn count(n: usize) -> Self {
        Self::Int(i64::try_from(n).unwrap_or(i64::MAX))
    }

    /// An unsigned quantity — a timestamp, a byte count, a row count.
    #[must_use]
    pub fn num(n: u64) -> Self {
        Self::Int(i64::try_from(n).unwrap_or(i64::MAX))
    }
}

impl Ord for SortKey {
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

impl PartialOrd for SortKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for SortKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for SortKey {}

/// Order `rows` by the column `sort` names, using the page's `key` mapping, then `tiebreak`.
///
/// Only the key flips with the direction — `tiebreak` stays ascending — so a descending sort is
/// the exact mirror of its ascending self and a mostly-tied table doesn't reshuffle on every poll.
pub fn sort_rows<T>(
    rows: &mut [T],
    sort: Sort,
    key: impl Fn(&T, &'static str) -> SortKey,
    tiebreak: impl Fn(&T) -> SortKey,
) {
    rows.sort_by(|a, b| {
        sort.dir(key(a, sort.col).cmp(&key(b, sort.col)))
            .then_with(|| tiebreak(a).cmp(&tiebreak(b)))
    });
}

/// Read one of a table's persisted settings. Storage being unavailable (private mode, a
/// disabled origin) is not an error — the table just opens in its declared state.
fn read(key: &str, field: &str) -> Option<String> {
    storage()?
        .get_item(&format!("adi-table:{key}:{field}"))
        .ok()
        .flatten()
        .filter(|v: &String| !v.is_empty())
}

fn write(key: &str, field: &str, value: &str) {
    if let Some(s) = storage() {
        let _ = s.set_item(&format!("adi-table:{key}:{field}"), value);
    }
}

/// The browser's `localStorage`, when there is one.
///
/// Private mode and a disabled origin both land here as `None`, which every caller above
/// already treats as "nothing was saved".
///
/// **The `cfg` is load-bearing, not defensive.** Off wasm there is no window to ask, and the
/// binding does not politely say so — it panics on the imported static. That is the host test
/// run, which is where the rest of this module is actually tested, so a `TableState` has to
/// be constructible there and simply remember nothing.
fn storage() -> Option<web_sys::Storage> {
    #[cfg(target_arch = "wasm32")]
    {
        window().local_storage().ok().flatten()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

/// The table: a scroll box, a click-to-sort header row, and a gear that opens the column
/// menu.
///
/// It owns the chrome and the state, never the data — `children` is the caller's rows, which
/// are built with [`Row`] so that hiding and reordering reach them.
///
/// The gear appears only when there are at least two columns to arrange. One column cannot be
/// reordered and may not be hidden, so the menu would open onto nothing but disabled controls.
///
/// ```ignore
/// <Panel title="Ports" flush=true>
///     <Table state=table>{rows}</Table>
/// </Panel>
/// ```
#[component]
pub fn Table(
    state: TableState,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    // The gear floats over the header's right edge. It cannot live inside the scroll box —
    // that scrolls away from it — nor in a column of its own, which every row would then owe
    // a matching empty cell. So the box it is pinned to is this outer one.
    let has_gear = move || state.layout.get().columns().len() > 1;

    view! {
        <div class=merge("relative", class)>
            <Show when=has_gear>{move || column_menu(state)}</Show>
            <div class="overflow-x-auto">
                <table class="w-full border-collapse">
                    <thead>
                        <tr>
                            {move || {
                                let layout = state.layout.get();
                                let shown = layout.shown();
                                let last = shown.len().saturating_sub(1);
                                let gear = has_gear();
                                let mut cells: Vec<AnyView> = shown
                                    .iter()
                                    .enumerate()
                                    // Only the final header reserves room, and only when
                                    // there is a gear to reserve it for — and only if the
                                    // table has no action column already holding that corner.
                                    .map(|(i, h)| {
                                        header_cell(
                                            state,
                                            h,
                                            gear && !layout.has_actions() && i == last,
                                        )
                                    })
                                    .collect();
                                if layout.has_actions() {
                                    cells.push(
                                        view! {
                                            <th class=if gear {
                                                "w-px bg-card pr-8"
                                            } else {
                                                "w-px bg-card"
                                            }></th>
                                        }
                                            .into_any(),
                                    );
                                }
                                cells
                            }}
                        </tr>
                    </thead>
                    <tbody>{children()}</tbody>
                </table>
            </div>
        </div>
    }
}

/// One click-to-sort header. The button fills the cell — hence the cell's own padding moves
/// onto it — so the whole header is the target, and the keyboard reaches it without a custom
/// handler.
fn header_cell(state: TableState, header: &'static str, pad_for_gear: bool) -> AnyView {
    let sort = state.sort;
    let active = move || sort.get().col == header;

    view! {
        <th
            class=if pad_for_gear {
                "bg-card p-0 pr-8 text-left align-middle whitespace-nowrap"
            } else {
                "bg-card p-0 text-left align-middle whitespace-nowrap"
            }
            aria-sort=move || sort.get().aria(header)
        >
            <button
                // The active column stays lit while the pointer is on a neighbour, so the
                // hover colour is only in the inactive arm.
                class=move || {
                    if active() {
                        "caps group flex w-full cursor-pointer items-center gap-1 px-3.5 \
                         py-[7px] text-left text-accent"
                    } else {
                        "caps group flex w-full cursor-pointer items-center gap-1 px-3.5 \
                         py-[7px] text-left text-faint hover:text-ink"
                    }
                }
                type="button"
                title=format!("Sort by {header}")
                on:click=move |_| state.set_sort(sort.get_untracked().toggled(header))
            >
                {header}
                // The caret always occupies its space and only its ink changes, so switching
                // the sorted column never reflows the header row.
                <span class=move || {
                    if active() { "opacity-100" } else { "opacity-0 group-hover:opacity-40" }
                }>
                    {move || if active() && sort.get().desc { "\u{2193}" } else { "\u{2191}" }}
                </span>
            </button>
        </th>
    }
    .into_any()
}

/// One body row: a cell per shown column, in the user's order, then the row's controls in the
/// trailing action cell when the table declared one.
///
/// `cell` is asked for a header by name rather than handed an index, which is what keeps a row
/// builder in step with a column order the user has rearranged. A header the builder does not
/// recognise is its own business — return an empty view for it.
/// # Why the data cells are reactive and the action cell is not
///
/// Only the *data* cells depend on the layout, so only they are rebuilt when it changes. The
/// action column cannot be hidden or reordered — [`Layout`] tracks it as a flag fixed at
/// construction, never as a configurable column — so its cell is rendered once. That is also
/// what lets `actions` be a plain view instead of a closure: it is never asked for twice.
#[component]
pub fn Row<F>(
    state: TableState,
    /// This row's content under one header.
    cell: F,
    /// The row's controls, for a table whose headers end in a blank.
    #[prop(optional, into)]
    actions: Option<AnyView>,
    #[prop(optional, into)] class: String,
) -> impl IntoView
where
    // `Send` because Leptos requires it of anything inside a reactive view, even under CSR
    // where nothing ever crosses a thread.
    F: Fn(&'static str) -> AnyView + Send + 'static,
{
    view! {
        <tr class=merge("hover:bg-bubble", class)>
            {move || {
                state
                    .layout
                    .get()
                    .shown()
                    .into_iter()
                    .map(|col| {
                        view! {
                            <td class="border-t border-divider px-3.5 py-[7px] text-row \
                                       whitespace-nowrap">
                                {cell(col)}
                            </td>
                        }
                            .into_any()
                    })
                    .collect::<Vec<_>>()
            }}
            // Right-aligned and shrink-wrapped, so the data columns keep the width.
            {actions
                .map(|a| {
                    view! {
                        <td class="w-px border-t border-divider px-3.5 py-[7px] text-right \
                                   whitespace-nowrap">
                            {a}
                        </td>
                    }
                })}
        </tr>
    }
}

/// The row a table shows instead of rows. It spans the columns the table is *currently*
/// showing, which is why it takes the state rather than a count.
#[component]
pub fn EmptyRow(state: TableState, children: Children) -> impl IntoView {
    view! {
        <tr>
            <td
                class="border-t border-divider px-3.5 py-6 text-center text-mini text-meta"
                colspan=move || state.layout.get().span()
            >
                {children()}
            </td>
        </tr>
    }
}

/// The gear and the panel it opens: one row per column with a show/hide toggle and a pair of
/// move buttons, plus Reset.
///
/// Buttons rather than drag-and-drop — the list is short, and this works from the keyboard
/// without a custom drop target.
fn column_menu(state: TableState) -> AnyView {
    let open = state.open;
    let rows = move || {
        let layout = state.layout.get();
        let last = layout.columns().len().saturating_sub(1);
        layout
            .columns()
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let (header, shown) = (c.header, c.shown);
                let locked = layout.is_last_shown(i);
                view! {
                    <div class="flex items-center gap-1 px-1.5 py-0.5">
                        <button
                            class="flex flex-1 cursor-pointer items-center gap-2 rounded-sm \
                                   px-1.5 py-1 text-left text-row text-body hover:bg-bubble \
                                   disabled:cursor-default disabled:opacity-50"
                            type="button"
                            role="checkbox"
                            aria-checked=shown.to_string()
                            prop:disabled=locked
                            title=if locked {
                                "A table keeps at least one column"
                            } else if shown {
                                "Hide this column"
                            } else {
                                "Show this column"
                            }
                            on:click=move |_| state.edit_layout(|l| l.toggle(i))
                        >
                            <span class=if shown {
                                "grid size-3.5 place-items-center rounded-sm border \
                                 border-transparent bg-accent-fill text-[9px] text-on-accent"
                            } else {
                                "grid size-3.5 place-items-center rounded-sm border \
                                 border-edge bg-card text-[9px]"
                            }>{if shown { "\u{2713}" } else { "" }}</span>
                            <span>{header}</span>
                        </button>
                        {move_button(state, i, true, i == 0)}
                        {move_button(state, i, false, i == last)}
                    </div>
                }
                .into_any()
            })
            .collect::<Vec<_>>()
    };

    view! {
        <button
            class="absolute top-1 right-1.5 z-20 grid size-6 cursor-pointer place-items-center \
                   rounded-sm text-meta hover:bg-card hover:text-ink"
            type="button"
            title="Columns"
            aria-label="Configure columns"
            aria-expanded=move || open.get().to_string()
            on:click=move |_| open.update(|o| *o = !*o)
        >
            <svg
                class="size-3.5"
                viewBox="0 0 16 16"
                fill="none"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
                stroke-linejoin="round"
                aria-hidden="true"
            >
                <circle cx="8" cy="8" r="2.25"></circle>
                <path d="M8 1.5v1.75M8 12.75v1.75M1.5 8h1.75M12.75 8h1.75M3.4 3.4l1.25 1.25M11.35 11.35l1.25 1.25M12.6 3.4l-1.25 1.25M4.65 11.35L3.4 12.6"></path>
            </svg>
        </button>
        <Show when=move || open.get()>
            // The scrim makes the next click anywhere a dismiss, so it has to be its own
            // element under the panel rather than a handler on the page.
            <div class="fixed inset-0 z-20" on:click=move |_| open.set(false)></div>
            // Anchored under the gear rather than at the click point: this control sits in a
            // fixed corner, so the panel can too.
            <div class="island absolute top-7 right-1.5 z-30 min-w-55 bg-card py-1">
                <div class="flex items-center justify-between gap-2 border-b border-divider \
                            px-3 py-1.5">
                    <span class="caps text-faint">"Columns"</span>
                    <button
                        class="cursor-pointer text-row text-accent hover:underline"
                        type="button"
                        on:click=move |_| state.reset()
                    >
                        "Reset"
                    </button>
                </div>
                {rows}
            </div>
        </Show>
    }
    .into_any()
}

/// One of the pair of arrows that moves a column in the settings menu.
fn move_button(state: TableState, col: usize, up: bool, at_end: bool) -> AnyView {
    let (label, glyph) = if up {
        ("Move up", "\u{2191}")
    } else {
        ("Move down", "\u{2193}")
    };
    view! {
        <button
            class="grid size-6 shrink-0 cursor-pointer place-items-center rounded-sm text-meta \
                   hover:bg-bubble hover:text-ink disabled:cursor-default disabled:opacity-30 \
                   disabled:hover:bg-transparent"
            type="button"
            title=label
            aria-label=label
            prop:disabled=at_end
            on:click=move |_| state.edit_layout(|l| l.shift(col, up))
        >
            {glyph}
        </button>
    }
    .into_any()
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

    fn key(r: &Row, col: &str) -> SortKey {
        match col {
            "Memory" => SortKey::num(r.bytes),
            "CPU" => SortKey::Float(r.cpu),
            "Status" => SortKey::Bool(r.up),
            _ => SortKey::text(r.name),
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
        sort_rows(&mut rows, Sort::new("Memory"), key, |r| SortKey::text(r.name));
        assert_eq!(names(&rows), ["web", "app", "api"]);

        sort_rows(&mut rows, desc("Memory"), key, |r| SortKey::text(r.name));
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
            SortKey::text(r.name)
        });
        assert_eq!(names(&ascending), ["web", "api", "app"]);

        let mut descending = rows();
        sort_rows(&mut descending, desc("Status"), key, |r| SortKey::text(r.name));
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
        sort_rows(&mut rows, Sort::new("Nonsense"), key, |r| SortKey::text(r.name));
        assert_eq!(names(&rows), ["api", "app", "web"], "by name, the default");
    }

    /// Floats order by value, including against each other — `total_cmp`, so a comparator can't
    /// be handed a partial ordering.
    #[test]
    fn every_key_kind_orders_within_itself() {
        let mut rows = rows();
        sort_rows(&mut rows, Sort::new("CPU"), key, |r| SortKey::text(r.name));
        assert_eq!(names(&rows), ["api", "web", "app"]);

        assert!(SortKey::Bool(false) < SortKey::Bool(true));
        assert!(SortKey::num(0) < SortKey::count(1));
        assert!(SortKey::maybe(None) < SortKey::text("a"), "absent sorts as empty");
        assert_eq!(
            SortKey::text("a").cmp(&SortKey::Int(1)),
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
