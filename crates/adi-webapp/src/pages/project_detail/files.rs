//! The Files panel of the project detail page.

use adi_ui::{
    CodeEditor, CodeFrame, CodeHeight, EmptyRow, Icon, IconSize, Lang, Lucide, Row as TableRow,
    Table,
};
use adi_webapp_api::types::FileEntry;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::{Flash, State};
use crate::ui::{Key, fmt_date, sort_rows};

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
                {move || crumbs_view(state)}
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--quiet" type="button" prop:disabled=move || files.busy.get()
                    on:click=move |_| open_dir(state, files.dir.get_untracked())>"Reload"</button>
            </div>
            <Table state=state.tables.files>{move || file_rows(state)}</Table>
            {move || match files.open.get() {
                None => view! {
                    <div class="adi-hint">
                        "Select a file above to view or edit it. Directories open in place; there's no going outside this project."
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
                        <a href="#"
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
    let Some(listing) = files.listing.get() else {
        return view! { <EmptyRow state=table>"Loading…"</EmptyRow> }.into_any();
    };
    let dir = listing.path.clone();
    let mut rows: Vec<AnyView> = Vec::new();

    // The way out stays pinned at the top whatever the sort says — it is navigation, not an entry.
    if let Some(parent) = listing.parent.clone() {
        rows.push(
            view! { <TableRow state=table cell=move |col| match col {
                "Name" => {
                    let parent = parent.clone();
                    view! {
                        <span>
                            <a class="adi-filerow adi-filerow--dir" href="#"
                                on:click=move |ev: web_sys::MouseEvent| {
                                    ev.prevent_default();
                                    open_dir(state, parent.clone());
                                }>
                                <Icon icon=Lucide::ArrowUp size=IconSize::Md class="adi-filerow__icon"/>
                                <span>".."</span>
                            </a>
                        </span>
                    }
                    .into_any()
                }
                _ => view! { <span class="adi-muted">"—"</span> }.into_any(),
            }/> }
            .into_any(),
        );
    }

    if listing.entries.is_empty() && listing.parent.is_none() {
        return view! { <EmptyRow state=table>"This project directory is empty."</EmptyRow> }
            .into_any();
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
        rows.push(view! { <TableRow state=table cell=move |col| file_cell(col, &entry, &path, is_open, state)/> }.into_any());
    }
    rows.into_any()
}

/// One entry's cell under `col`. Matching the header text — the same key the sort uses — is what
/// lets the user hide and reorder columns without the row builder knowing about it.
fn file_cell(col: &str, entry: &FileEntry, path: &str, is_open: bool, state: State) -> AnyView {
    match col {
        // A directory has no meaningful size, so it shows a dash rather than a misleading zero.
        "Size" if entry.is_dir => view! { <span class="adi-muted">"—"</span> }.into_any(),
        "Size" => {
            view! { <span class="adi-muted adi-tabnums">{fmt_size(entry.size)}</span> }.into_any()
        }
        "Modified" => {
            let modified = entry.modified.map_or_else(|| "—".to_string(), fmt_date);
            view! { <span class="adi-muted adi-tabnums">{modified}</span> }.into_any()
        }
        // "Name", and anything the layout offers that this match doesn't name.
        _ if entry.is_dir => {
            let path = path.to_string();
            view! {
                <span>
                    <a class="adi-filerow adi-filerow--dir" href="#"
                        on:click=move |ev: web_sys::MouseEvent| {
                            ev.prevent_default();
                            open_dir(state, path.clone());
                        }>
                        <Icon icon=Lucide::Folder size=IconSize::Md class="adi-filerow__icon"/>
                        <span>{entry.name.clone()}"/"</span>
                    </a>
                </span>
            }
            .into_any()
        }
        _ => {
            let path = path.to_string();
            view! {
                <span>
                    <a class="adi-filerow" href="#"
                        aria-current=move || if is_open { "true" } else { "false" }
                        on:click=move |ev: web_sys::MouseEvent| {
                            ev.prevent_default();
                            open_file(state, path.clone());
                        }>
                        <Icon icon=Lucide::File size=IconSize::Md class="adi-filerow__icon"/>
                        <span>{entry.name.clone()}</span>
                    </a>
                </span>
            }
            .into_any()
        }
    }
}

/// The in-place editor for the open file: the file's frame — its path, whether it is saved, and
/// Save / Reload / Close — with the highlighted buffer under it.
fn editor_view(state: State, path: String) -> AnyView {
    let files = state.files;
    let dirty = move || files.buffer.get() != files.original.get();
    let lang = Lang::from_path(&path);
    let reload_path = path.clone();
    // The frame asks for its controls on every render, so the path is cloned into each click
    // handler rather than moved into the first one.
    let actions = move || {
        let reload_path = reload_path.clone();
        view! {
            <span class="adi-updated">
                {move || if dirty() { "unsaved changes" } else { "saved" }}
            </span>
            <button class="adi-btn adi-btn--sm" type="button"
                prop:disabled=move || files.busy.get()
                on:click=move |_| open_file(state, reload_path.clone())>"Reload"</button>
            <button class="adi-btn adi-btn--sm adi-btn--primary" type="button"
                prop:disabled=move || files.busy.get() || !dirty()
                on:click=move |_| save_file(state)>"Save"</button>
            <button class="adi-btn adi-btn--sm adi-btn--quiet" type="button"
                on:click=move |_| close_file(state)>"Close"</button>
        }
        .into_any()
    };
    view! {
        <div class="adi-panel__body">
            <CodeFrame title=path actions=actions height=CodeHeight::Form>
                <CodeEditor value=files.buffer lang=lang height=CodeHeight::Form id="project-file-editor"/>
            </CodeFrame>
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
