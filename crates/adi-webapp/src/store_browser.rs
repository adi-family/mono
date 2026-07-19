//! The right rail: a browser over the ADI store (`~/.adi/mono`), on every page.
//!
//! Where the left explorer is the app's *navigator* — it routes you to a page — this rail is a
//! *file* view of the same machine: the raw manifests, JSON, and YAML the pages render. It is
//! served through the `adi-fs` jail rooted at the store, so nothing outside `~/.adi/mono` is
//! reachable, and it is collapsed by default because it is a side tool, not the way around.
//!
//! Directories load lazily, one listing per expanded folder, so opening a deep path never
//! re-fetches or collapses what is already open above it.

use adi_webapp_api::types::FileEntry;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::pages::load_store_file;
use crate::routing::{Route, push_state, scroll_top, store_file_path};
use crate::state::State;

/// The rail itself: a header with the toggle, then either the tree or nothing. Rendered beside
/// `<main>` in the shell, so it spans the full height next to whatever page is showing.
pub(crate) fn store_rail(state: State, route: RwSignal<Route>) -> AnyView {
    let store = state.store;
    view! {
        <aside class="adi-store" class:adi-store--open=move || store.open.get()>
            <div class="adi-store__head">
                <button class="adi-btn adi-btn--icon-sm" type="button"
                    title=move || if store.open.get() { "Hide the store browser" } else { "Show the store browser" }
                    aria-expanded=move || store.open.get().to_string()
                    on:click=move |_| toggle_rail(state)>
                    {move || if store.open.get() { "\u{25b8}" } else { "\u{25c2}" }}
                </button>
                {move || store.open.get().then(|| view! {
                    <span class="adi-store__title">"Store"</span>
                    <span class="adi-spacer"></span>
                    <button class="adi-btn adi-btn--icon-sm" type="button" title="Reload the tree"
                        prop:disabled=move || store.busy.get()
                        on:click=move |_| reload_root(state)>"\u{21bb}"</button>
                })}
            </div>

            {move || store.open.get().then(|| view! {
                <div class="adi-store__body">
                    {move || store.error.get().map(|e| view! {
                        <div class="adi-flash" data-kind="err">{e}</div>
                    })}
                    <div class="adi-store__tree">{move || dir_rows(state, route, String::new(), 0)}</div>
                </div>
            })}
        </aside>
    }
    .into_any()
}

/// Open or close the rail. The first open loads the root listing; closing keeps the tree, so
/// re-opening is instant and lands where you left off.
fn toggle_rail(state: State) {
    let store = state.store;
    let opening = !store.open.get();
    store.open.set(opening);
    if opening && !store.dirs.get().contains_key("") {
        load_dir(state, String::new());
    }
}

/// Drop every cached listing and re-fetch the root — the tree's refresh button. Expanded
/// folders stay expanded, so each re-loads as it re-renders.
fn reload_root(state: State) {
    let store = state.store;
    store.dirs.update(std::collections::BTreeMap::clear);
    store.error.set(None);
    load_dir(state, String::new());
}

/// Fetch one directory listing into `dirs`. Failures land in the rail's own error line rather
/// than the page flash, which can be scrolled far away from here.
fn load_dir(state: State, path: String) {
    let store = state.store;
    store.busy.set(true);
    spawn_local(async move {
        match fetch::fs_list(&path).await {
            Ok(listing) => {
                store.dirs.update(|d| {
                    d.insert(listing.path.clone(), listing.entries);
                });
                store.error.set(None);
            }
            Err(e) => store.error.set(Some(e)),
        }
        store.busy.set(false);
    });
}

/// Expand or collapse a directory. Expanding fetches its listing the first time only.
fn toggle_dir(state: State, path: String) {
    let store = state.store;
    let was_open = store.expanded.get().contains(&path);
    store.expanded.update(|set| {
        if was_open {
            set.remove(&path);
        } else {
            set.insert(path.clone());
        }
    });
    if !was_open && !store.dirs.get().contains_key(&path) {
        load_dir(state, path);
    }
}

/// Select a file: navigate to its editor page, which loads it. A dirty buffer is protected —
/// switching files with unsaved edits is refused rather than silently discarding them.
fn open_file(state: State, route: RwSignal<Route>, path: String) {
    let store = state.store;
    if store.open_file.get().is_some() && store.dirty() {
        store.error.set(Some(
            "Unsaved changes \u{2014} save the open file first.".to_string(),
        ));
        return;
    }
    store.error.set(None);
    push_state(&store_file_path(&path));
    route.set(Route::StoreFile);
    scroll_top();
    load_store_file(state, path);
}

/// The rows for one directory: its entries, each indented by `depth`, with expanded directories
/// immediately followed by their own rows. Recurses, so the whole open tree renders in order.
fn dir_rows(state: State, route: RwSignal<Route>, path: String, depth: usize) -> AnyView {
    let store = state.store;
    let Some(entries) = store.dirs.get().get(&path).cloned() else {
        // Nothing cached yet: a fetch is either in flight or about to be.
        return view! { <div class="adi-store__hint">"Loading\u{2026}"</div> }.into_any();
    };
    if entries.is_empty() {
        return view! { <div class="adi-store__hint" style=indent(depth)>"empty"</div> }.into_any();
    }
    entries
        .into_iter()
        .map(|e| entry_row(state, route, &path, e, depth))
        .collect::<Vec<_>>()
        .into_any()
}

/// One row — a directory with its caret, or a file that opens in the editor.
fn entry_row(
    state: State,
    route: RwSignal<Route>,
    parent: &str,
    e: FileEntry,
    depth: usize,
) -> AnyView {
    let store = state.store;
    let path = join(parent, &e.name);
    let name = e.name.clone();
    if e.is_dir {
        // One clone per closure: the row's caret, its aria state, its click, and its children
        // each outlive this call, so none of them can share a borrow of `path`.
        let (aria_path, caret_path, click_path, kids_path) =
            (path.clone(), path.clone(), path.clone(), path.clone());
        let expanded = move |p: &str| store.expanded.get().contains(p);
        view! {
            <button class="adi-store__row" type="button" style=indent(depth)
                aria-expanded=move || expanded(&aria_path).to_string()
                on:click=move |_| toggle_dir(state, click_path.clone())>
                <span class="adi-store__caret">
                    {move || if expanded(&caret_path) { "\u{25be}" } else { "\u{25b8}" }}
                </span>
                <span class="adi-store__name">{name}</span>
            </button>
            {move || expanded(&kids_path).then(|| dir_rows(state, route, kids_path.clone(), depth + 1))}
        }
        .into_any()
    } else {
        let sel_path = path.clone();
        let selected = move || store.open_file.get().as_deref() == Some(sel_path.as_str());
        view! {
            <button class="adi-store__row adi-store__row--file" type="button" style=indent(depth)
                class:adi-store__row--on=selected
                title=fmt_size(e.size)
                on:click={
                    let p = path.clone();
                    move |_| open_file(state, route, p.clone())
                }>
                <span class="adi-store__caret"></span>
                <span class="adi-store__name">{name}</span>
            </button>
        }
        .into_any()
    }
}

/// Join a parent directory and an entry name into a store-relative path (`""` is the root).
fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

/// The per-row indent for a tree depth. Rows are buttons, so this is padding, not a margin.
fn indent(depth: usize) -> String {
    format!("padding-left:{}px", 6 + depth * 12)
}

/// A file size for the row tooltip, in whichever unit keeps it short.
fn fmt_size(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
    }
}
