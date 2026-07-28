//! The Files panel of the project detail page.

use adi_webapp_api::types::FileEntry;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::{Flash, State};
use crate::ui::{Key, body_row, configurable_table, fmt_date, placeholder_row, sort_rows};

use super::load_dir;

/// The file listing's columns. No action column — a row's control is its Name cell, which opens
/// the directory in place or the file in the editor below.
pub(crate) const COLS: &[&str] = &["Name", "Size", "Modified"];

/// Navigate the browser into directory `path` (a dir click or the "up" control).
fn open_dir(state: State, path: String) {
    let id = state.current_project.get_untracked();
    if !id.is_empty() {
        spawn_local(load_dir(state, id, path));
    }
}

/// Open file `path` in the editor, loading its content into the buffer (and remembering it as
/// the baseline so edits are detectable).
fn open_file(state: State, path: String) {
    let id = state.current_project.get_untracked();
    if id.is_empty() {
        return;
    }
    state.files.busy.set(true);
    spawn_local(async move {
        match fetch::read_file(&id, &path).await {
            Ok(fc) => {
                state.files.open.set(Some(fc.path.clone()));
                state.files.original.set(fc.content.clone());
                state.files.buffer.set(fc.content);
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        state.files.busy.set(false);
    });
}

/// Save the editor buffer back to the open file, then refresh the listing so its size/modified
/// update. Resets the baseline to the saved content so the dirty state clears.
fn save_file(state: State) {
    let id = state.current_project.get_untracked();
    let Some(path) = state.files.open.get_untracked() else {
        return;
    };
    if id.is_empty() {
        return;
    }
    let content = state.files.buffer.get_untracked();
    state.files.busy.set(true);
    spawn_local(async move {
        match fetch::write_file(&id, &path, &content).await {
            Ok(fc) => {
                state.files.original.set(fc.content.clone());
                state.files.buffer.set(fc.content);
                state.flash.set(Some(Flash::ok(format!("Saved {path}."))));
                load_dir(state, id, state.files.dir.get_untracked()).await;
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        state.files.busy.set(false);
    });
}

/// Close the editor, discarding the buffer (a fresh open reloads from disk anyway).
fn close_file(state: State) {
    state.files.open.set(None);
    state.files.original.set(String::new());
    state.files.buffer.set(String::new());
}

/// Join a directory path and an entry name into a project-relative path (the root is `""`).
fn join_rel(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// The Files panel on a project's detail page: a breadcrumb + directory listing scoped to the
/// project's own directory (via the isolated jail), plus an in-place editor for the selected
/// text file — so `.adi/hive.yaml` (and anything beside it) is editable here.
pub(crate) fn files_view(state: State) -> AnyView {
    let files = state.files;
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Files"</h2>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--ghost" type="button" prop:disabled=move || files.busy.get()
                    on:click=move |_| open_dir(state, files.dir.get_untracked())>"Reload"</button>
            </div>
            <div class="adi-panel__body">
                {move || crumbs_view(state)}
            </div>
            {configurable_table(state.tables.files, COLS, move || file_rows(state))}
            {move || match files.open.get() {
                None => view! {
                    <div class="adi-panel__body">
                        <span class="adi-muted">"Select a file above to view or edit it. Directories open in place; there's no going outside this project."</span>
                    </div>
                }.into_any(),
                Some(path) => editor_view(state, path),
            }}
        </section>
    }
    .into_any()
}

/// The breadcrumb trail for the file browser: the project root plus each segment of the current
/// directory, every ancestor clickable to jump straight there.
fn crumbs_view(state: State) -> AnyView {
    let dir = state.files.dir.get();
    let id = state.current_project.get();
    let mut crumbs: Vec<(String, String)> = vec![(id, String::new())]; // (label, target dir)
    let mut acc = String::new();
    if !dir.is_empty() {
        for segment in dir.split('/') {
            acc = join_rel(&acc, segment);
            crumbs.push((segment.to_string(), acc.clone()));
        }
    }
    let last = crumbs.len() - 1;
    view! {
        <div class="adi-crumbs">
            {crumbs.into_iter().enumerate().map(|(i, (label, target))| {
                let sep = (i > 0).then(|| view! { <span class="adi-crumbs__sep">"/"</span> });
                let node = if i == last {
                    view! { <span class="adi-crumbs__here">{label}</span> }.into_any()
                } else {
                    view! {
                        <a class="adi-btn adi-btn--link" href="#"
                            on:click=move |ev: web_sys::MouseEvent| {
                                ev.prevent_default();
                                open_dir(state, target.clone());
                            }>{label}</a>
                    }.into_any()
                };
                view! { {sep}{node} }
            }).collect::<Vec<_>>()}
        </div>
    }
    .into_any()
}

/// Rows for the file listing: an "up" row when not at the root, then directories (which open in
/// place) and files (which open in the editor), with size and modified date.
fn file_rows(state: State) -> AnyView {
    let files = state.files;
    let table = state.tables.files;
    let layout = table.layout.get();
    let Some(listing) = files.listing.get() else {
        return placeholder_row(layout.span(), "Loading…");
    };
    let dir = listing.path.clone();
    let shown = layout.shown();
    let mut rows: Vec<AnyView> = Vec::new();

    // The way out stays pinned at the top whatever the sort says — it is navigation, not an entry.
    if let Some(parent) = listing.parent.clone() {
        rows.push(body_row(
            &shown,
            |col| match col {
                "Name" => {
                    let parent = parent.clone();
                    view! {
                        <td>
                            <a class="adi-btn adi-btn--link adi-filerow adi-filerow--dir" href="#"
                                on:click=move |ev: web_sys::MouseEvent| {
                                    ev.prevent_default();
                                    open_dir(state, parent.clone());
                                }>
                                <span class="adi-filerow__icon">"↑"</span><span>".."</span>
                            </a>
                        </td>
                    }
                    .into_any()
                }
                _ => view! { <td class="adi-muted">"—"</td> }.into_any(),
            },
            None,
        ));
    }

    if listing.entries.is_empty() && listing.parent.is_none() {
        return placeholder_row(layout.span(), "This project directory is empty.");
    }

    let mut entries = listing.entries;
    // Directories first whichever way the sort runs — a browser you navigate reads as folders
    // then files, and a directory has no size to order by in any case.
    sort_rows(
        &mut entries,
        table.sort.get(),
        |e, col| match col {
            "Size" => Key::num(if e.is_dir { 0 } else { e.size }),
            "Modified" => Key::num(e.modified.unwrap_or(0)),
            _ => Key::text(&e.name),
        },
        |e| Key::text(&e.name),
    );
    entries.sort_by_key(|e| !e.is_dir);

    for entry in entries {
        let path = join_rel(&dir, &entry.name);
        let is_open = state.files.open.get().as_deref() == Some(path.as_str());
        rows.push(body_row(
            &shown,
            |col| file_cell(col, &entry, &path, is_open, state),
            None,
        ));
    }
    rows.into_any()
}

/// One entry's cell under `col`. Matching the header text — the same key the sort uses — is what
/// lets the user hide and reorder columns without the row builder knowing about it.
fn file_cell(col: &str, entry: &FileEntry, path: &str, is_open: bool, state: State) -> AnyView {
    match col {
        // A directory has no meaningful size, so it shows a dash rather than a misleading zero.
        "Size" if entry.is_dir => view! { <td class="adi-muted">"—"</td> }.into_any(),
        "Size" => view! { <td class="adi-mono adi-muted">{fmt_size(entry.size)}</td> }.into_any(),
        "Modified" => {
            let modified = entry.modified.map_or_else(|| "—".to_string(), fmt_date);
            view! { <td class="adi-mono adi-muted">{modified}</td> }.into_any()
        }
        // "Name", and anything the layout offers that this match doesn't name.
        _ if entry.is_dir => {
            let path = path.to_string();
            view! {
                <td>
                    <a class="adi-btn adi-btn--link adi-filerow adi-filerow--dir" href="#"
                        on:click=move |ev: web_sys::MouseEvent| {
                            ev.prevent_default();
                            open_dir(state, path.clone());
                        }>
                        <span class="adi-filerow__icon">"▸"</span>
                        <span>{entry.name.clone()}"/"</span>
                    </a>
                </td>
            }
            .into_any()
        }
        _ => {
            let path = path.to_string();
            view! {
                <td>
                    <a class="adi-btn adi-btn--link adi-filerow" href="#"
                        aria-current=move || if is_open { "true" } else { "false" }
                        on:click=move |ev: web_sys::MouseEvent| {
                            ev.prevent_default();
                            open_file(state, path.clone());
                        }>
                        <span class="adi-filerow__icon">"·"</span><span>{entry.name.clone()}</span>
                    </a>
                </td>
            }
            .into_any()
        }
    }
}

/// The in-place editor for the open file: a toolbar (path, dirty state, Save/Reload/Close) and a
/// monospace textarea bound to the buffer.
fn editor_view(state: State, path: String) -> AnyView {
    let files = state.files;
    let dirty = move || files.buffer.get() != files.original.get();
    let reload_path = path.clone();
    view! {
        <div class="adi-form adi-form--toolbar">
            <span class="adi-chip adi-mono">{path}</span>
            <span class="adi-muted" style="font-size:var(--text-md)">
                {move || if dirty() { "unsaved changes".to_string() } else { "saved".to_string() }}
            </span>
            <span class="adi-spacer"></span>
            <button class="adi-btn adi-btn--primary" type="button"
                prop:disabled=move || files.busy.get() || !dirty()
                on:click=move |_| save_file(state)>"Save"</button>
            <button class="adi-btn adi-btn--ghost" type="button"
                prop:disabled=move || files.busy.get()
                on:click=move |_| open_file(state, reload_path.clone())>"Reload"</button>
            <button class="adi-btn adi-btn--link" type="button"
                on:click=move |_| close_file(state)>"Close"</button>
        </div>
        <div class="adi-panel__body">
            <textarea class="adi-textarea" spellcheck="false" autocomplete="off"
                prop:value=move || files.buffer.get()
                on:input=move |ev| files.buffer.set(event_target_value(&ev))></textarea>
        </div>
    }
    .into_any()
}

/// Format a byte count as `N B` / `N.N KB` / `N.N MB`.
fn fmt_size(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let n = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", n / 1024.0)
    } else {
        format!("{:.1} MB", n / (1024.0 * 1024.0))
    }
}
