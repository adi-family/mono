//! The store file editor page (`/files/<path>`): one file from `~/.adi/mono` open full-width.
//!
//! The right rail picks the file; this page edits it. Keeping the editor here rather than in the
//! rail gives it the whole content pane — these are configs and JSON, and a 300px column is not
//! where you read them.

use adi_ui::{CodeEditor, CodeFrame, CodeHeight, Lang};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::{Flash, State};
use crate::ui::flash_view;

/// The editor page: the file's frame — its path, whether it is saved, Reload and Save — with the
/// buffer filling the pane under it. Shows a placeholder when no file is selected, and the failure
/// in place when one couldn't be read.
pub(crate) fn store_file_view(state: State) -> AnyView {
    let store = state.store;
    view! {
        {move || match store.open_file.get() {
            None => view! {
                <section class="adi-panel">
                    <div class="adi-empty">
                        "No file open \u{2014} pick one from the store rail on the right."
                    </div>
                </section>
            }
            .into_any(),
            Some(path) => {
                let lang = Lang::from_path(&path);
                let actions = move || view! {
                    <span class="adi-updated">
                        {move || if store.dirty() { "unsaved changes" } else { "saved" }}
                    </span>
                    <button class="adi-btn adi-btn--sm" type="button" title="Re-read from disk"
                        prop:disabled=move || store.busy.get()
                        on:click=move |_| reload(state)>"Reload"</button>
                    <button class="adi-btn adi-btn--sm adi-btn--primary" type="button"
                        prop:disabled=move || store.busy.get() || !store.dirty()
                        on:click=move |_| save(state)>"Save"</button>
                }
                .into_any();
                view! {
                <section class="adi-panel adi-panel--fill">
                    {move || store.error.get().map(|e| view! {
                        <div class="adi-flash adi-flash--card" data-kind="err">{e}</div>
                    })}
                    <CodeFrame title=path.clone() actions=actions height=CodeHeight::Fill
                        class="min-h-0 flex-1">
                        <CodeEditor value=store.buffer lang=lang id="store-file-editor"
                            height=CodeHeight::Fill/>
                    </CodeFrame>
                </section>
                }
                .into_any()
            }
        }}
        {flash_view(state.flash)}
    }
    .into_any()
}

/// Load a file into the editor buffer. Called by the rail on navigation and by Reload here.
/// A failure leaves the path selected and reports why, so the page never goes silently blank.
pub(crate) fn load_store_file(state: State, path: String) {
    let store = state.store;
    store.busy.set(true);
    store.open_file.set(Some(path.clone()));
    spawn_local(async move {
        match fetch::fs_read(&path).await {
            Ok(c) => {
                store.original.set(c.content.clone());
                store.buffer.set(c.content);
                store.error.set(None);
            }
            Err(e) => {
                store.original.set(String::new());
                store.buffer.set(String::new());
                store.error.set(Some(e));
            }
        }
        store.busy.set(false);
    });
}

/// Re-read the open file from disk, discarding the buffer.
fn reload(state: State) {
    if let Some(path) = state.store.open_file.get() {
        load_store_file(state, path);
    }
}

/// Save the buffer back through the store jail, adopting the re-read content as the new baseline.
fn save(state: State) {
    let store = state.store;
    let Some(path) = store.open_file.get() else {
        return;
    };
    let content = store.buffer.get_untracked();
    store.busy.set(true);
    spawn_local(async move {
        match fetch::fs_write(&path, content).await {
            Ok(c) => {
                store.original.set(c.content);
                state
                    .flash
                    .set(Some(Flash::ok(format!("Saved {}.", c.path))));
                store.error.set(None);
            }
            Err(e) => store.error.set(Some(e)),
        }
        store.busy.set(false);
    });
}
