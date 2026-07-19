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
use crate::routing::{ProjectSection, Route, push_state, scroll_top, store_file_path};
use crate::state::{State, StoreDraft, StoreMenu};

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
                    // Right-clicking the tree's empty space targets the root, so the first
                    // entry in an empty store is still creatable.
                    <div class="adi-store__tree"
                        on:contextmenu=move |ev: web_sys::MouseEvent| {
                            ev.prevent_default();
                            open_menu(state, String::new(), &ev);
                        }>
                        {move || dir_rows(state, route, String::new(), 0)}
                    </div>
                </div>
            })}

            {move || menu_view(state)}
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

/// Stand the tree up at the open project: expand the chain from the store root down to
/// `projects/<id>`, one level deeper — into `workspaces/` — on the Workspaces section, and fetch
/// any listing along it that isn't cached yet. Called on navigation, so the rail shows the
/// directory behind whatever page you opened.
///
/// A collapsed rail stays collapsed. Revealing follows navigation rather than being asked for,
/// so popping the rail open would fight whoever closed it; the tree is simply already standing
/// at the project the next time it opens.
pub(crate) fn reveal_project(state: State, id: &str, section: ProjectSection) {
    if id.is_empty() {
        return;
    }
    let project = format!("projects/{id}");
    // Root first, then each ancestor: the tree renders a directory only under a loaded parent.
    let mut chain = vec![String::new(), "projects".to_string(), project.clone()];
    if section == ProjectSection::Workspaces {
        chain.push(format!("{project}/workspaces"));
    }
    // The root is rendered unconditionally, so it is loaded but never marked expanded.
    state.store.expanded.update(|set| {
        set.extend(chain.iter().filter(|p| !p.is_empty()).cloned());
    });
    let cached = state.store.dirs.get_untracked();
    for path in chain {
        if !cached.contains_key(&path) {
            load_dir(state, path);
        }
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

/// Open the right-click menu over `dir`, anchored at the pointer.
fn open_menu(state: State, dir: String, ev: &web_sys::MouseEvent) {
    state.store.menu.set(Some(StoreMenu {
        dir,
        x: ev.client_x(),
        y: ev.client_y(),
    }));
}

/// The right-click menu: which folder a create lands in, then the two creates. A full-viewport
/// scrim sits behind it so the next click anywhere dismisses it — cheaper and better-scoped than
/// a window-level listener that would outlive the menu.
fn menu_view(state: State) -> Option<AnyView> {
    let store = state.store;
    let menu = store.menu.get()?;
    let target = if menu.dir.is_empty() {
        "store root".to_string()
    } else {
        menu.dir.clone()
    };
    let (for_file, for_dir) = (menu.dir.clone(), menu.dir);
    Some(
        view! {
            <div class="adi-menu__scrim"
                on:click=move |_| store.menu.set(None)
                on:contextmenu=move |ev: web_sys::MouseEvent| {
                    ev.prevent_default();
                    store.menu.set(None);
                }></div>
            <div class="adi-menu" style=format!("left:{}px; top:{}px", menu.x, menu.y)>
                <div class="adi-menu__head adi-mono" title=target.clone()>{target.clone()}</div>
                <button class="adi-menu__item" type="button"
                    on:click=move |_| start_create(state, for_file.clone(), false)>"New file"</button>
                <button class="adi-menu__item" type="button"
                    on:click=move |_| start_create(state, for_dir.clone(), true)>"New folder"</button>
            </div>
        }
        .into_any(),
    )
}

/// Begin a create in `dir`: close the menu and put the name input inside that folder. The folder
/// is expanded and loaded first — the input renders among its rows, so a collapsed or unloaded
/// folder would swallow it.
fn start_create(state: State, dir: String, is_dir: bool) {
    let store = state.store;
    store.menu.set(None);
    store.error.set(None);
    store.draft.set(String::new());
    if !dir.is_empty() {
        store.expanded.update(|set| {
            set.insert(dir.clone());
        });
    }
    if !store.dirs.get_untracked().contains_key(&dir) {
        load_dir(state, dir.clone());
    }
    store.creating.set(Some(StoreDraft { dir, is_dir }));
}

/// Abandon the create in progress, dropping whatever was typed.
fn cancel_draft(state: State) {
    state.store.creating.set(None);
    state.store.draft.set(String::new());
}

/// Create the drafted entry. The reply is the parent's fresh listing, so the folder redraws with
/// the new row in the server's sort order rather than one guessed here.
fn submit_draft(state: State) {
    let store = state.store;
    let Some(draft) = store.creating.get_untracked() else {
        return;
    };
    let name = store.draft.get_untracked().trim().to_string();
    if name.is_empty() {
        cancel_draft(state);
        return;
    }
    // One segment only: a `/` would quietly create intermediate folders the tree never showed,
    // and `.`/`..` name something other than a new entry. The jail refuses the climb anyway —
    // this is the message that explains it.
    if name.contains('/') || name == "." || name == ".." {
        store
            .error
            .set(Some(format!("\u{201c}{name}\u{201d} is not a valid name.")));
        return;
    }
    let path = join(&draft.dir, &name);
    cancel_draft(state);
    store.busy.set(true);
    spawn_local(async move {
        match fetch::fs_create(path, draft.is_dir).await {
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

/// The name input for a create landing in `dir`, or nothing when the draft belongs to another
/// folder. Enter commits, Escape abandons, and clicking away does too — a stray input left
/// behind in the tree would be worse than losing a half-typed name.
fn draft_row(state: State, dir: &str, depth: usize) -> Option<AnyView> {
    let store = state.store;
    let draft = store.creating.get()?;
    if draft.dir != dir {
        return None;
    }
    // The row is inserted reactively, so `autofocus` never fires — focus it on mount instead.
    let input: NodeRef<leptos::html::Input> = NodeRef::new();
    Effect::new(move |_| {
        if let Some(el) = input.get() {
            let _ = el.focus();
        }
    });
    Some(
        view! {
            <div class="adi-store__row adi-store__row--new" style=indent(depth)>
                <span class="adi-store__caret">
                    {if draft.is_dir { "\u{25b8}" } else { "" }}
                </span>
                <input class="adi-store__input" type="text" node_ref=input
                    spellcheck="false" autocomplete="off"
                    placeholder=if draft.is_dir { "new-folder" } else { "new-file" }
                    prop:value=move || store.draft.get()
                    on:input=move |ev| store.draft.set(event_target_value(&ev))
                    on:blur=move |_| cancel_draft(state)
                    on:keydown=move |ev: web_sys::KeyboardEvent| match ev.key().as_str() {
                        "Enter" => { ev.prevent_default(); submit_draft(state); }
                        "Escape" => { ev.prevent_default(); cancel_draft(state); }
                        _ => {}
                    } />
            </div>
        }
        .into_any(),
    )
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
    // The name input belongs to this folder and has to render before the early returns below:
    // a still-loading or empty folder is exactly where the first entry gets created.
    let draft = draft_row(state, &path, depth);
    let Some(entries) = store.dirs.get().get(&path).cloned() else {
        // Nothing cached yet: a fetch is either in flight or about to be.
        return view! { {draft}<div class="adi-store__hint">"Loading\u{2026}"</div> }.into_any();
    };
    if entries.is_empty() {
        return view! {
            {draft}
            <div class="adi-store__hint" style=indent(depth)>"empty"</div>
        }
        .into_any();
    }
    view! {
        {draft}
        {entries
            .into_iter()
            .map(|e| entry_row(state, route, &path, e, depth))
            .collect::<Vec<_>>()}
    }
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
        // Right-clicking a folder creates *inside* it.
        let menu_path = path.clone();
        view! {
            <button class="adi-store__row" type="button" style=indent(depth)
                aria-expanded=move || expanded(&aria_path).to_string()
                on:contextmenu=move |ev: web_sys::MouseEvent| {
                    ev.prevent_default();
                    // Don't also let the tree's background handler retarget this at the root.
                    ev.stop_propagation();
                    open_menu(state, menu_path.clone(), &ev);
                }
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
        // Right-clicking a file creates beside it — in the folder holding it, not "inside" it.
        let menu_dir = parent.to_string();
        view! {
            <button class="adi-store__row adi-store__row--file" type="button" style=indent(depth)
                class:adi-store__row--on=selected
                title=fmt_size(e.size)
                on:contextmenu=move |ev: web_sys::MouseEvent| {
                    ev.prevent_default();
                    ev.stop_propagation();
                    open_menu(state, menu_dir.clone(), &ev);
                }
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
