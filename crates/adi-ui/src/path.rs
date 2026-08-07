//! [`PathPicker`] — the box a directory goes in: a path you can type, over a list you can
//! walk.
//!
//! The two halves are **one value**. There is no "typed path" mode and no "browse" mode to
//! switch between: the text is the state, and the list below is always showing whatever the
//! text currently points at. Paste `/Users/me/src/adi`, and the list is inside `/Users/me/src`
//! with `adi` highlighted; click a folder in the list, and the text grows a segment. Either
//! way what the caller reads back is the same signal.
//!
//! That is also what makes it filter as you type. A path is `dir` + `leaf`
//! ([`dir_of`], [`leaf_of`]) — the directory being listed and the start of a name inside it —
//! so typing narrows the list without moving it, and typing a separator steps into whatever
//! you narrowed it to. Only crossing a separator changes the directory, which is the only
//! thing that costs a read.
//!
//! **It knows nothing about a filesystem**, the way [`crate::Tree`] knows nothing about
//! files. It says which directory it wants listed, the caller lists it:
//!
//! ```ignore
//! let path = RwSignal::new(String::from("/Users/me/"));
//! // The one directory the picker needs read: the parent of whatever is typed.
//! let dir = Signal::derive(move || dir_of(&path.get()).to_string());
//! let listing = LocalResource::new(move || read_dir(dir.get()));
//!
//! view! {
//!     <PathPicker
//!         value=path
//!         entries=Signal::derive(move || listing.get().unwrap_or_default())
//!         loading=Signal::derive(move || listing.get().is_none())
//!         on_pick=Callback::new(move |dir: String| set_workdir(dir))
//!     />
//! }
//! ```
//!
//! # Windows
//!
//! Paths are split on `/` **and** `\`, and joined back with whichever the value already
//! uses, so `C:\Users\me\adi` round-trips as itself rather than turning into a mixed-slash
//! hybrid. A drive is understood as a root: `C:\` is its own parent, its crumb goes to
//! `C:\` rather than to the drive's working directory `C:`, and a path that is nothing but
//! a drive still knows it writes backslashes.
//!
//! **UNC paths (`\\server\share`) are not handled** — the leading double separator reads as
//! one empty segment and the crumbs come out a slash short. Nothing else about the picker
//! is platform-specific: it never touches a disk, so what a root is and what listing one
//! means is the caller's, as it has to be.

use core::sync::atomic::{AtomicUsize, Ordering};

use leptos::{ev, html, prelude::*, wasm_bindgen::JsCast, web_sys};

use crate::{Button, ButtonSize, ButtonVariant, Empty, Flash, FlashKind, input::FRAME, merge};

/// The inner markup of a 16×16 `<svg>`, drawn in `currentColor` at whatever size the row
/// picks. Local to the picker: a component that needs an icon to be itself owns it, rather
/// than making every call site pass one.
const FOLDER: &str = "<path d='M2 4.5A1.5 1.5 0 0 1 3.5 3h2.8l1.2 1.6h5A1.5 1.5 0 0 1 14 \
                      6.1v5.4A1.5 1.5 0 0 1 12.5 13h-9A1.5 1.5 0 0 1 2 11.5z'/>";
const FILE: &str = "<path d='M4 2h4.5L12 5.5V14H4z'/><path d='M8.5 2v3.5H12'/>";
const UP: &str = "<path d='M8 13V4'/><path d='M4.5 7.5 8 4l3.5 3.5'/>";

/// One thing inside the directory being listed.
///
/// A **directory picker lists files too**, dimmed and inert. They are how you recognise the
/// folder you are standing in — a `Cargo.toml` and a `src/` say "this is the crate" in a way
/// the folder's name alone does not — and leaving them out makes every project root look
/// alike. Only directories can be walked into or picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    /// Whether it can be entered. Files are drawn for context and nothing else.
    pub dir: bool,
}

impl DirEntry {
    /// A directory: something to walk into.
    #[must_use]
    pub fn dir(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            dir: true,
        }
    }

    /// A file: context, never a destination.
    #[must_use]
    pub fn file(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            dir: false,
        }
    }
}

/// A named place worth one click — home, the repo, the last project.
///
/// The picker has no idea what those are on this machine, so they arrive from the caller.
/// Without any, the crumbs are the only way up and a fresh field starts from wherever its
/// value points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathRoot {
    pub label: String,
    pub path: String,
}

impl PathRoot {
    /// A shortcut chip.
    #[must_use]
    pub fn new(label: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            path: path.into(),
        }
    }
}

/// Whether `c` divides one segment of a path from the next. Both spellings count, so a
/// pasted Windows path is a path here rather than one very long name.
fn is_sep(c: char) -> bool {
    c == '/' || c == '\\'
}

/// The separator to *write* with: the one the value already uses. A path only turns into
/// backslashes when it arrived that way — the default, and anything ambiguous, is `/`.
///
/// A bare drive (`C:`) has no separator to read but is not ambiguous either, so it counts.
fn sep_of(path: &str) -> char {
    let drive = path.as_bytes().get(1) == Some(&b':');
    if (path.contains('\\') || drive) && !path.contains('/') {
        '\\'
    } else {
        '/'
    }
}

/// **The directory a path is listed from** — everything up to its last separator.
///
/// This is the one function a caller needs, because it is the question the picker is
/// asking: `/Users/me/adi` and `/Users/me/` are both a view of `/Users/me`, so both list
/// the same directory and typing across those keystrokes reads it once.
///
/// A path with no separator in it has no directory (`""`); a caller that starts a field
/// empty decides for itself what to list, if anything.
#[must_use]
pub fn dir_of(path: &str) -> &str {
    match path.rfind(is_sep) {
        // The root is its own parent, and it keeps its slash: `""` is not a directory.
        Some(0) => &path[..1],
        // A drive root (`C:\`) is the same case one letter along — cutting at the separator
        // would leave `C:`, which names the drive's *working* directory, not its root.
        Some(i) if path[..i].ends_with(':') => &path[..=i],
        Some(i) => &path[..i],
        None => "",
    }
}

/// **What is being typed inside that directory** — everything after the last separator.
///
/// Empty exactly when the path ends in a separator, which is what "I am inside this
/// directory and have not started naming anything in it" looks like.
#[must_use]
pub fn leaf_of(path: &str) -> &str {
    match path.rfind(is_sep) {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

/// A path without its trailing separator — what the directory is *called*, once you have
/// stopped browsing it. This is what [`PathPicker`]'s `on_pick` hands over.
///
/// A root keeps its separator, because `""` names nothing.
#[must_use]
pub fn trim_dir(path: &str) -> &str {
    let cut = path.trim_end_matches(is_sep);
    // Both roots keep theirs. `C:` is not `C:\` on Windows — it names the drive's working
    // directory, which is a different place and usually not the one that was picked.
    if cut.is_empty() || cut.ends_with(':') {
        &path[..path.len().min(cut.len() + 1)]
    } else {
        cut
    }
}

/// The same path with exactly one trailing separator: the spelling that means "listing the
/// inside of this".
fn as_dir(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let sep = sep_of(path);
    let mut out = path.trim_end_matches(is_sep).to_string();
    out.push(sep);
    out
}

/// `dir` + `name`, with a separator between them and optionally one on the end.
fn join(dir: &str, name: &str, trailing: bool) -> String {
    let sep = sep_of(dir);
    let mut out = as_dir(dir);
    out.push_str(name);
    if trailing {
        out.push(sep);
    }
    out
}

/// Every ancestor of the directory being listed, outermost first, as `(label, path)`.
///
/// Derived from the string rather than tracked alongside it, so a pasted path grows its own
/// crumbs and the two can never disagree about where you are.
fn crumbs_of(path: &str) -> Vec<(String, String)> {
    let dir = dir_of(path);
    if dir.is_empty() {
        return Vec::new();
    }
    let sep = sep_of(dir);
    let mut out = Vec::new();
    let mut acc = String::new();
    if dir.starts_with(is_sep) {
        acc.push(sep);
        out.push((sep.to_string(), acc.clone()));
    }
    for seg in dir.split(is_sep).filter(|s| !s.is_empty()) {
        if !acc.is_empty() && !acc.ends_with(is_sep) {
            acc.push(sep);
        }
        acc.push_str(seg);
        // `acc` keeps the bare drive so the next segment joins onto it, but what the crumb
        // *goes to* is the drive's root, which is the drive plus its separator.
        let mut target = acc.clone();
        if target.ends_with(':') {
            target.push(sep);
        }
        out.push((seg.to_string(), target));
    }
    out
}

/// The rows the typed leaf leaves standing.
///
/// Matching is a case-insensitive *contains*, so `ui` finds `adi-ui` — but the ones that
/// **start** with what was typed sort first, because that is what you nearly always meant
/// and a stable sort leaves the caller's own order intact underneath.
fn filtered(entries: &[DirEntry], leaf: &str) -> Vec<DirEntry> {
    if leaf.is_empty() {
        return entries.to_vec();
    }
    let needle = leaf.to_lowercase();
    let mut out: Vec<DirEntry> = entries
        .iter()
        .filter(|e| e.name.to_lowercase().contains(&needle))
        .cloned()
        .collect();
    out.sort_by_key(|e| usize::from(!e.name.to_lowercase().starts_with(&needle)));
    out
}

/// Does `name` begin with `prefix`, ignoring case? What Tab completes against.
fn starts_with_fold(name: &str, prefix: &str) -> bool {
    name.to_lowercase().starts_with(&prefix.to_lowercase())
}

/// The longest run of characters every one of `names` opens with.
///
/// Compared exactly, not case-insensitively: `Documents` and `downloads` have nothing in
/// common to complete to, and pretending they share a `d` would type the wrong one for you.
fn common_prefix(names: &[String]) -> String {
    let mut rest = names.iter();
    let Some(first) = rest.next() else {
        return String::new();
    };
    let mut len = first.chars().count();
    for name in rest {
        len = first
            .chars()
            .zip(name.chars())
            .take(len)
            .take_while(|(a, b)| a == b)
            .count();
    }
    first.chars().take(len).collect()
}

/// Instance number, for the `id`s that tie the input to its listbox. Two pickers on one
/// screen must not both claim `aria-activedescendant="opt-0"`.
static SEQ: AtomicUsize = AtomicUsize::new(0);

fn option_id(uid: usize, index: usize) -> String {
    format!("path-{uid}-opt-{index}")
}

/// Scroll `index` into view inside the list, and only if it is not already there.
///
/// The arrow keys move a highlight the pointer is not driving, so the list has to follow it;
/// `scrollIntoView` is the obvious call and the wrong one, since it aligns unconditionally
/// and would jerk a row that was already visible to the top of the box.
fn reveal(list: NodeRef<html::Div>, index: usize) {
    let Some(box_el) = list.get_untracked() else {
        return;
    };
    let row = box_el
        .query_selector(&format!("[data-row=\"{index}\"]"))
        .ok()
        .flatten()
        .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok());
    let Some(row) = row else { return };

    // `offset_top` is measured from the nearest positioned ancestor, which is why the
    // scroll box declares itself `relative`.
    let top = row.offset_top();
    let bottom = top + row.offset_height();
    let seen_from = box_el.scroll_top();
    let seen_to = seen_from + box_el.client_height();
    if top < seen_from {
        box_el.set_scroll_top(top);
    } else if bottom > seen_to {
        box_el.set_scroll_top(bottom - box_el.client_height());
    }
}

/// A directory: typed, or browsed to.
///
/// The field is the whole control — the list hangs under it while it has focus and gets out
/// of the way when it does not, so a path lands in a [`crate::Form`] row at the height of an
/// [`crate::Input`] rather than as a panel of its own. Pass `inline` for the other case: a
/// screen or a dialog that *is* the picker, where the list is always open.
///
/// What it does with the keyboard is the point of it being custom, so it does the whole set:
/// `↓`/`↑` walk the folders (skipping the files, which are not destinations), `Enter` steps
/// into the highlighted one or picks where you are, `Tab` completes as far as the names
/// agree, and `Escape` puts the list away without touching what you typed.
///
/// ```ignore
/// <Field label="Working directory" hint="Where the agent runs.">
///     <PathPicker value=path entries=listing roots=vec![PathRoot::new("Home", "/Users/me")]/>
/// </Field>
/// ```
// Four things have already been lifted out of it — the two keyboard behaviours, the toggle
// and the rows — and what is left is one `view!` tree plus the key map that drives it.
// Cutting a markup tree in half to land under a line count costs more than it buys.
#[allow(clippy::too_many_lines)]
#[component]
pub fn PathPicker(
    /// The path, two-way. Its trailing separator is meaningful and the picker maintains it:
    /// present means "inside this directory", absent means "naming something in the one
    /// above". Read it back through [`trim_dir`], or take `on_pick`'s argument, which
    /// already is.
    value: RwSignal<String>,
    /// What is in [`dir_of`]`(value)`. The picker filters and orders these; it never asks
    /// for anything else, and it never asks twice for the same directory.
    #[prop(into)]
    entries: Signal<Vec<DirEntry>>,
    /// That read is in flight. The list says so instead of showing the previous directory's
    /// children, which would otherwise be clickable and wrong for as long as it took.
    #[prop(optional, into)]
    loading: Signal<bool>,
    /// Why the read failed — "permission denied", "no such directory". A typo'd path is the
    /// normal way to reach this, so it reads as a note in the list rather than as an alarm.
    #[prop(optional, into)]
    error: Signal<Option<String>>,
    /// One-click places, along the top of the list.
    #[prop(optional, into)]
    roots: Signal<Vec<PathRoot>>,
    /// Called with the chosen directory, already trimmed of its trailing separator. Without
    /// one the field is just bound to `value`, and the button only closes the list.
    #[prop(optional, into)]
    on_pick: Option<Callback<String>>,
    /// Keep the list open below the field, as its own block. For a dialog or a screen the
    /// picker owns, where there is nothing for it to get out of the way of.
    #[prop(optional)]
    inline: bool,
    #[prop(default = String::from("/path/to/somewhere"), into)] placeholder: String,
    /// What an empty directory says. A directory that is empty *because of the filter* says
    /// so itself instead — it names what did not match.
    #[prop(default = String::from("Nothing in here."), into)]
    empty: String,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let uid = SEQ.fetch_add(1, Ordering::Relaxed);
    // Stored rather than captured: the sheet's children are re-run on every open, so a
    // `String` moved into them would be gone the second time.
    let empty = StoredValue::new(empty);
    let list_id = format!("path-{uid}-list");
    let field = NodeRef::<html::Input>::new();
    let list = NodeRef::<html::Div>::new();
    let open = RwSignal::new(inline);
    // Which row the keyboard is on — an index into `shown`, not into `entries`, because the
    // filter is what the reader is looking at.
    let active = RwSignal::new(None::<usize>);

    let dir = Signal::derive(move || dir_of(&value.get()).to_string());
    let shown = Signal::derive(move || filtered(&entries.get(), leaf_of(&value.get())));

    // Changing directory drops the highlight: row 3 of the folder you just left means
    // nothing in the one you just entered, and keeping it would arrow off from a stranger.
    Effect::new(move |_| {
        dir.track();
        active.set(None);
    });

    // Every closure below captures nothing but `Copy` signals and plain bools, so each is
    // itself `Copy` and can be handed to as many handlers as want it.
    let focus = move || {
        if let Some(el) = field.get_untracked() {
            let _ = el.focus();
        }
    };
    let go = move |path: &str| {
        value.set(as_dir(path));
        active.set(None);
    };
    let enter = Callback::new(move |name: String| {
        value.set(join(&dir.get_untracked(), &name, true));
        active.set(None);
    });
    let confirm = move || {
        if let Some(pick) = on_pick {
            pick.run(trim_dir(&value.get_untracked()).to_string());
        }
        if !inline {
            open.set(false);
            active.set(None);
        }
    };
    let step = move |delta: isize| step_highlight(delta, shown, active, list);
    let complete = move || complete_leaf(value, entries);

    let sheet = if inline {
        "island mt-1.5 overflow-hidden bg-bar"
    } else {
        "island absolute top-[calc(100%+4px)] right-0 left-0 z-30 overflow-hidden bg-bar"
    };
    let footer = on_pick.is_some() || !inline;
    // No chevron to leave room for when the sheet cannot be closed.
    let box_class = if inline {
        format!("{FRAME} w-full pl-7 font-mono text-mini")
    } else {
        format!("{FRAME} w-full pr-7 pl-7 font-mono text-mini")
    };

    view! {
        // Two positioning contexts, not one. The outer is what the sheet hangs off; the
        // inner is the field alone, because the glyph and the chevron centre on *it* — and
        // an `inline` picker's outer box is as tall as the whole list, which would leave
        // them floating halfway down it.
        <div class=merge("relative min-w-0", class)>
            <div class="relative">
                <span
                    class="pointer-events-none absolute top-1/2 left-2 z-10 -translate-y-1/2 \
                           text-faint"
                    aria-hidden="true"
                >
                    <svg
                        class="block size-3.5"
                        viewBox="0 0 16 16"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="1.5"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        inner_html=FOLDER
                    ></svg>
                </span>

                <input
                    class=box_class
                    type="text"
                    node_ref=field
                    // A path is not prose, and every helper a browser points at prose
                    // corrupts a directory name silently.
                    spellcheck="false"
                    autocomplete="off"
                    autocapitalize="off"
                    role="combobox"
                    aria-autocomplete="list"
                    aria-controls=list_id.clone()
                    aria-expanded=move || open.get().to_string()
                    aria-activedescendant=move || active.get().map(|i| option_id(uid, i))
                    placeholder=placeholder
                    disabled=move || disabled.get()
                    prop:value=move || value.get()
                    on:input=move |ev| {
                        value.set(event_target_value(&ev));
                        // The row that was highlighted is not the row under that index any more.
                        active.set(None);
                        open.set(true);
                    }
                    on:focus=move |_| open.set(true)
                    on:blur=move |_| {
                        // Nothing inside the sheet ever takes the focus — it cancels its own
                        // mousedown — so a blur means the reader has genuinely left.
                        if !inline {
                            open.set(false);
                            active.set(None);
                        }
                    }
                    on:keydown=move |ev: ev::KeyboardEvent| {
                        match ev.key().as_str() {
                            "ArrowDown" => {
                                ev.prevent_default();
                                open.set(true);
                                step(1);
                            }
                            "ArrowUp" => {
                                ev.prevent_default();
                                open.set(true);
                                step(-1);
                            }
                            "Enter" => {
                                ev.prevent_default();
                                let rows = shown.get_untracked();
                                match active.get_untracked().and_then(|i| rows.get(i)) {
                                    Some(row) if row.dir => enter.run(row.name.clone()),
                                    _ => confirm(),
                                }
                            }
                            // Only while the list is up, and only when there is something to
                            // complete: the rest of the time Tab still has to leave the field.
                            "Tab" if !ev.shift_key() && open.get_untracked() => {
                                if complete() {
                                    ev.prevent_default();
                                }
                            }
                            "Escape" if open.get_untracked() && !inline => {
                                // A picker inside a `Modal` must close itself first rather than
                                // taking the dialog down with it, and the dialog listens on the
                                // window — which is the end of this event's own bubble path.
                                ev.stop_propagation();
                                open.set(false);
                                active.set(None);
                            }
                            _ => {}
                        }
                    }
                />

                {(!inline).then(|| browse_toggle(open, disabled, focus))}
            </div>

            <Show when=move || open.get()>
                <div
                    class=sheet
                    // One handler for the whole sheet: cancelling the mousedown anywhere in
                    // it means the field never loses the focus, so clicking a row, a crumb
                    // or the button is not also a blur. Clicks still fire.
                    on:mousedown=move |ev: ev::MouseEvent| ev.prevent_default()
                >
                    {roots_strip(roots, go)}
                    {crumbs_strip(value, go)}

                    // `relative`, so a row's `offsetTop` is measured from this box and
                    // `reveal` can compare it against this box's own scroll.
                    <div
                        class="relative max-h-64 overflow-y-auto p-1"
                        id=list_id.clone()
                        node_ref=list
                        role="listbox"
                        aria-label="Directories"
                    >
                        <Rows
                            uid=uid
                            shown=shown
                            loading=loading
                            error=error
                            empty=empty
                            value=value
                            active=active
                            enter=enter
                        />
                    </div>

                    {footer.then(|| choose_strip(value, confirm))}
                </div>
            </Show>
        </div>
    }
}

/// The chevron on the field's right edge, which puts the sheet up and takes it down.
///
/// It is out of the tab order and it never takes the focus: the field keeps that, so a
/// reader who came in by keyboard still has a caret to type into, and the field's own blur
/// stays a reliable signal that the reader has left.
fn browse_toggle(
    open: RwSignal<bool>,
    disabled: Signal<bool>,
    focus: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        <button
            class="absolute top-1/2 right-1 grid size-6 -translate-y-1/2 cursor-pointer \
                   place-items-center rounded-sm text-meta hover:text-ink \
                   disabled:cursor-not-allowed disabled:opacity-50"
            type="button"
            aria-label="Browse"
            tabindex="-1"
            disabled=move || disabled.get()
            // Let the button take the focus and the field's blur would close the sheet a
            // fraction before this click reopened it — a toggle that can never close.
            on:mousedown=move |ev: ev::MouseEvent| ev.prevent_default()
            on:click=move |_| {
                focus();
                open.update(|o| *o = !*o);
            }
        >
            <svg
                class=move || {
                    if open.get() {
                        "size-3.5 rotate-180 transition-transform duration-100"
                    } else {
                        "size-3.5 transition-transform duration-100"
                    }
                }
                viewBox="0 0 16 16"
                fill="none"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
                stroke-linejoin="round"
                aria-hidden="true"
            >
                <path d="M4 6.5 8 10.5l4-4"></path>
            </svg>
        </button>
    }
}

/// What is inside the listbox: the rows, or the one line standing in for them.
///
/// A read in flight replaces the rows rather than dimming them. The alternative is showing
/// the previous directory's children while the new one loads, and those are clickable and
/// wrong — a stale row would put a path in the field that does not exist.
#[component]
fn Rows(
    uid: usize,
    shown: Signal<Vec<DirEntry>>,
    loading: Signal<bool>,
    error: Signal<Option<String>>,
    empty: StoredValue<String>,
    value: RwSignal<String>,
    active: RwSignal<Option<usize>>,
    enter: Callback<String>,
) -> impl IntoView {
    move || {
        if loading.get() {
            return view! { <Empty>"Reading\u{2026}"</Empty> }.into_any();
        }
        if let Some(why) = error.get() {
            return view! { <Flash kind=FlashKind::Err card=true class="m-1">{why}</Flash> }
                .into_any();
        }
        let rows = shown.get();
        if rows.is_empty() {
            // An empty directory and a filter that matched nothing are different facts, and
            // only one of them is worth the caller writing a sentence about.
            let leaf = leaf_of(&value.get()).to_string();
            let says = if leaf.is_empty() {
                empty.get_value()
            } else {
                format!("Nothing here matches \u{201c}{leaf}\u{201d}.")
            };
            return view! { <Empty>{says}</Empty> }.into_any();
        }
        rows.into_iter()
            .enumerate()
            .map(|(i, row)| entry_row(uid, i, row, active, enter))
            .collect::<Vec<_>>()
            .into_any()
    }
}

/// Move the highlight `delta` rows and scroll it into view, wrapping at both ends.
///
/// It steps **over the files**: they are drawn so you can recognise the folder you are
/// standing in, but they are not anywhere you can go, and an arrow key that lands on one
/// would be a keypress that did nothing.
fn step_highlight(
    delta: isize,
    shown: Signal<Vec<DirEntry>>,
    active: RwSignal<Option<usize>>,
    list: NodeRef<html::Div>,
) {
    let rows = shown.get_untracked();
    let dirs: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, e)| e.dir)
        .map(|(i, _)| i)
        .collect();
    if dirs.is_empty() {
        return;
    }
    let at = active
        .get_untracked()
        .and_then(|cur| dirs.iter().position(|&i| i == cur));
    let next = match (at, delta > 0) {
        (Some(p), true) => (p + 1) % dirs.len(),
        (Some(p), false) => (p + dirs.len() - 1) % dirs.len(),
        (None, true) => 0,
        (None, false) => dirs.len() - 1,
    };
    active.set(Some(dirs[next]));
    reveal(list, dirs[next]);
}

/// Shell completion, in a text box: one match fills itself in and opens, several fill in as
/// far as they agree.
///
/// Returns whether it did anything, which is what decides whether Tab was ours to swallow —
/// a Tab that completed nothing still has to move the focus out of the field.
fn complete_leaf(value: RwSignal<String>, entries: Signal<Vec<DirEntry>>) -> bool {
    let path = value.get_untracked();
    let leaf = leaf_of(&path);
    let here = dir_of(&path).to_string();
    let names: Vec<String> = entries
        .get_untracked()
        .iter()
        .filter(|e| e.dir && starts_with_fold(&e.name, leaf))
        .map(|e| e.name.clone())
        .collect();
    match names.as_slice() {
        [] => false,
        [only] => {
            value.set(join(&here, only, true));
            true
        }
        many => {
            let prefix = common_prefix(many);
            if prefix.len() > leaf.len() {
                value.set(join(&here, &prefix, false));
                true
            } else {
                false
            }
        }
    }
}

/// The one-click places, across the top of the sheet. Nothing when the caller named none.
fn roots_strip(roots: Signal<Vec<PathRoot>>, go: impl Fn(&str) + Copy + Send + Sync + 'static) -> impl IntoView {
    move || {
        let places = roots.get();
        (!places.is_empty()).then(|| {
            view! {
                <div class="flex flex-wrap gap-1 border-b border-divider bg-panel px-2 py-1.5">
                    {places
                        .into_iter()
                        .map(|root| {
                            let path = root.path.clone();
                            view! {
                                <button
                                    class="cursor-pointer rounded-sm border border-edge \
                                           bg-card px-1.5 py-0.5 text-mini text-secondary \
                                           hover:border-accent-soft-edge hover:text-accent"
                                    type="button"
                                    title=root.path.clone()
                                    on:click=move |_| go(&path)
                                >
                                    {root.label}
                                </button>
                            }
                        })
                        .collect::<Vec<_>>()}
                </div>
            }
        })
    }
}

/// The way back out: one step up, then every ancestor by name.
///
/// It scrolls sideways rather than wrapping, because a deep path would otherwise push the
/// list itself off the bottom of the sheet — and the end that matters is the end it is
/// already scrolled to.
fn crumbs_strip(
    value: RwSignal<String>,
    go: impl Fn(&str) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    move || {
        let path = value.get();
        let crumbs = crumbs_of(&path);
        (!crumbs.is_empty()).then(|| {
            let deepest = crumbs.len() - 1;
            let parent = dir_of(trim_dir(&path)).to_string();
            view! {
                <div class="flex items-center gap-0.5 overflow-x-auto border-b border-divider \
                            bg-panel px-1.5 py-1 whitespace-nowrap">
                    <button
                        class="mr-0.5 grid size-5 shrink-0 cursor-pointer place-items-center \
                               rounded-sm text-meta hover:bg-card hover:text-ink \
                               disabled:cursor-not-allowed disabled:opacity-40 \
                               disabled:hover:bg-transparent"
                        type="button"
                        aria-label="Up one level"
                        disabled=deepest == 0
                        on:click=move |_| go(&parent)
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
                            inner_html=UP
                        ></svg>
                    </button>
                    {crumbs
                        .into_iter()
                        .enumerate()
                        .map(|(i, (label, target))| {
                            // The one you are in is not a place to go, so it does not offer.
                            let tone = if i == deepest {
                                "shrink-0 cursor-pointer rounded-sm px-1 py-0.5 font-mono \
                                 text-mini font-medium text-ink"
                            } else {
                                "shrink-0 cursor-pointer rounded-sm px-1 py-0.5 font-mono \
                                 text-mini text-meta hover:bg-card hover:text-ink"
                            };
                            view! {
                                {(i > 0)
                                    .then(|| view! {
                                        <span class="shrink-0 text-fainter" aria-hidden="true">
                                            "\u{203a}"
                                        </span>
                                    })}
                                <button class=tone type="button" on:click=move |_| go(&target)>
                                    {label}
                                </button>
                            }
                        })
                        .collect::<Vec<_>>()}
                </div>
            }
        })
    }
}

/// The bottom of the sheet: what you are about to hand over, spelled out, and the button
/// that hands it over. The path is written out because the field above it may be scrolled,
/// truncated, or still holding the half-name you typed before clicking.
fn choose_strip(value: RwSignal<String>, confirm: impl Fn() + Copy + Send + Sync + 'static) -> impl IntoView {
    view! {
        <div class="flex items-center gap-2 border-t border-divider bg-panel px-2 py-1.5">
            <span class="min-w-0 flex-1 truncate font-mono text-mini text-meta">
                {move || {
                    let path = value.get();
                    let chosen = trim_dir(&path);
                    if chosen.is_empty() { "Nowhere yet".to_string() } else { chosen.to_string() }
                }}
            </span>
            <Button
                size=ButtonSize::Small
                variant=ButtonVariant::Primary
                on:click=move |_| confirm()
            >
                "Use this folder"
            </Button>
        </div>
    }
}

/// One row of the list. A directory is an option you can land on; a file is furniture.
fn entry_row(
    uid: usize,
    index: usize,
    entry: DirEntry,
    active: RwSignal<Option<usize>>,
    enter: Callback<String>,
) -> AnyView {
    let icon = |markup: &'static str| {
        view! {
            <svg
                class="block size-3.5 shrink-0"
                viewBox="0 0 16 16"
                fill="none"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
                stroke-linejoin="round"
                aria-hidden="true"
                inner_html=markup
            ></svg>
        }
    };

    let name = entry.name;
    if !entry.dir {
        let label = name.clone();
        return view! {
            // Still carries `data-row`, so the indices the keyboard skips over and the
            // indices in the DOM stay the same numbers.
            <div
                class="flex cursor-default items-center gap-2 rounded-sm px-2 py-1 text-row \
                       text-faint"
                data-row=index.to_string()
                title=name
            >
                {icon(FILE)}
                <span class="truncate">{label}</span>
            </div>
        }
        .into_any();
    }

    let label = name.clone();
    let target = name.clone();
    let is_active = move || active.get() == Some(index);
    view! {
        <div
            class=move || {
                if is_active() {
                    "flex cursor-pointer items-center gap-2 rounded-sm bg-accent-soft px-2 \
                     py-1 text-row text-accent"
                } else {
                    "flex cursor-pointer items-center gap-2 rounded-sm px-2 py-1 text-row \
                     text-body hover:bg-card"
                }
            }
            id=option_id(uid, index)
            data-row=index.to_string()
            role="option"
            aria-selected=move || is_active().to_string()
            title=name
            on:click=move |_| enter.run(target.clone())
        >
            {icon(FOLDER)}
            <span class="truncate">{label}</span>
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The split the whole control is built on: what gets listed, and what filters it.
    #[test]
    fn a_path_splits_into_a_directory_and_a_leaf() {
        for (path, dir, leaf) in [
            ("/Users/me/adi", "/Users/me", "adi"),
            ("/Users/me/", "/Users/me", ""),
            ("/Users", "/", "Users"),
            ("/", "/", ""),
            ("", "", ""),
            ("adi-ui", "", "adi-ui"),
            ("crates/adi-ui", "crates", "adi-ui"),
        ] {
            assert_eq!(dir_of(path), dir, "dir_of({path:?})");
            assert_eq!(leaf_of(path), leaf, "leaf_of({path:?})");
        }
    }

    /// Both separators are read; the one already in the path is the one written back.
    #[test]
    fn a_windows_path_survives_a_round_trip() {
        assert_eq!(dir_of(r"C:\Users\me\adi"), r"C:\Users\me");
        assert_eq!(leaf_of(r"C:\Users\me\adi"), "adi");
        assert_eq!(join(r"C:\Users\me", "adi", true), r"C:\Users\me\adi\");
        // A drive root keeps its separator: `C:` is the drive's working directory, which is
        // a different place from `C:\`.
        assert_eq!(dir_of(r"C:\Users"), r"C:\");
        assert_eq!(as_dir(r"C:\Users\me"), r"C:\Users\me\");
    }

    /// A drive is only a root with its separator on, and every way of arriving at one has to
    /// put it back: `C:` is the drive's *working* directory, which is not where you clicked.
    #[test]
    fn a_drive_root_keeps_its_separator() {
        assert_eq!(trim_dir(r"C:\"), r"C:\");
        assert_eq!(trim_dir(r"C:\Users\"), r"C:\Users");
        assert_eq!(as_dir("C:"), r"C:\");
        assert_eq!(join("C:", "Users", true), r"C:\Users\");
        // The crumb for the drive goes to `C:\`, not to `C:`.
        assert_eq!(
            crumbs_of(r"C:\Users\me\adi"),
            vec![
                ("C:".to_string(), "C:\\".to_string()),
                ("Users".to_string(), r"C:\Users".to_string()),
                ("me".to_string(), r"C:\Users\me".to_string()),
            ]
        );
    }

    #[test]
    fn entering_a_directory_appends_one_segment() {
        assert_eq!(join("/", "Users", true), "/Users/");
        assert_eq!(join("/Users/me", "adi", true), "/Users/me/adi/");
        assert_eq!(join("/Users/me/", "adi", false), "/Users/me/adi");
        assert_eq!(join("", "adi", true), "adi/");
    }

    /// What lands in `on_pick`. The root is the one path that keeps its separator, because
    /// the alternative is the empty string, which is not a directory.
    #[test]
    fn a_chosen_directory_loses_its_trailing_separator() {
        assert_eq!(trim_dir("/Users/me/"), "/Users/me");
        assert_eq!(trim_dir("/Users/me"), "/Users/me");
        assert_eq!(trim_dir("/"), "/");
        assert_eq!(trim_dir(""), "");
    }

    #[test]
    fn crumbs_walk_out_to_the_root() {
        let crumbs = crumbs_of("/Users/me/adi");
        assert_eq!(
            crumbs,
            vec![
                ("/".to_string(), "/".to_string()),
                ("Users".to_string(), "/Users".to_string()),
                ("me".to_string(), "/Users/me".to_string()),
            ]
        );
        // Inside a directory rather than naming something in it: same crumbs, one deeper.
        assert_eq!(crumbs_of("/Users/me/adi/").len(), 4);
        assert!(crumbs_of("adi-ui").is_empty());
    }

    #[test]
    fn the_filter_puts_what_you_typed_first() {
        let entries = [
            DirEntry::dir("adi-css"),
            DirEntry::dir("legacy-ui"),
            DirEntry::dir("ui-kit"),
            DirEntry::file("Cargo.toml"),
        ];
        let names: Vec<String> = filtered(&entries, "ui")
            .into_iter()
            .map(|e| e.name)
            .collect();
        // `ui-kit` starts with it, `legacy-ui` merely contains it, `adi-css` is out.
        assert_eq!(names, vec!["ui-kit", "legacy-ui"]);
        // An empty leaf is every row, in the order the caller gave them.
        assert_eq!(filtered(&entries, "").len(), 4);
    }

    #[test]
    fn tab_completes_as_far_as_the_names_agree() {
        let names = |v: &[&str]| v.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        assert_eq!(common_prefix(&names(&["adi-ui", "adi-app", "adi-agents"])), "adi-");
        assert_eq!(common_prefix(&names(&["adi-ui"])), "adi-ui");
        // Nothing shared, nothing typed for you — including across a case difference.
        assert_eq!(common_prefix(&names(&["Documents", "downloads"])), "");
        assert_eq!(common_prefix(&[]), "");
    }

    #[test]
    fn completion_matches_without_regard_to_case() {
        assert!(starts_with_fold("Documents", "doc"));
        assert!(starts_with_fold("adi-ui", ""));
        assert!(!starts_with_fold("adi-ui", "ui"));
    }
}
