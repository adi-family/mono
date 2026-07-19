//! The store file editor page (`/files/<path>`): one file from `~/.adi/mono` open full-width.
//!
//! The right rail picks the file; this page edits it. Keeping the editor here rather than in the
//! rail gives it the whole content pane — these are configs and JSON, and a 300px column is not
//! where you read them.

use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::highlight::{Lang, highlight};
use crate::state::{Flash, State};
use crate::ui::flash_view;

/// The editor page: a header with the path and Save, then the buffer. Shows a placeholder when
/// no file is selected, and the failure in place when one couldn't be read.
pub(crate) fn store_file_view(state: State) -> AnyView {
    let store = state.store;
    view! {
        {move || match store.open_file.get() {
            None => view! {
                <section class="adi-panel">
                    <div class="adi-empty">
                        "No file open \u{2014} pick one from the Store rail on the right."
                    </div>
                </section>
            }
            .into_any(),
            Some(path) => {
                let lang = Lang::from_path(&path);
                let area = NodeRef::<leptos::html::Textarea>::new();
                let paint = NodeRef::<leptos::html::Pre>::new();
                view! {
                <section class="adi-panel">
                    <div class="adi-panel__head">
                        <h2 class="adi-panel__title adi-mono">{path.clone()}</h2>
                        <span class="adi-updated">
                            {move || if store.dirty() { "unsaved changes" } else { "saved" }}
                        </span>
                        <span class="adi-spacer"></span>
                        <button class="adi-btn adi-btn--ghost" type="button" title="Re-read from disk"
                            prop:disabled=move || store.busy.get()
                            on:click=move |_| reload(state)>"Reload"</button>
                        <button class="adi-btn adi-btn--primary" type="button"
                            prop:disabled=move || store.busy.get() || !store.dirty()
                            on:click=move |_| save(state)>"Save"</button>
                    </div>

                    {move || store.error.get().map(|e| view! {
                        <div class="adi-flash" data-kind="err">{e}</div>
                    })}

                    // The editor is two stacked layers sharing one box: a highlighted <pre>
                    // underneath, and a transparent-text textarea on top that still owns the
                    // caret, selection, and typing. They must use identical font metrics and
                    // padding or the two drift apart — see `.adi-code` / `.adi-fileedit`.
                    <div class="adi-code">
                        <pre class="adi-code__paint adi-mono" aria-hidden="true" node_ref=paint>
                            {move || highlighted(&lang, &store.buffer.get())}
                        </pre>
                        <textarea class="adi-textarea adi-mono adi-fileedit" spellcheck="false"
                            autocomplete="off" node_ref=area
                            prop:value=move || store.buffer.get()
                            on:scroll=move |_| sync_scroll(area, paint)
                            on:input=move |ev| {
                                store.buffer.set(event_target_value(&ev));
                                sync_scroll(area, paint);
                            }></textarea>
                    </div>
                </section>
                }
                .into_any()
            }
        }}
        {flash_view(state.flash)}
    }
    .into_any()
}

/// The buffer painted as coloured spans. A trailing newline gets a space appended so the last
/// line still has height — otherwise the painted layer is one line shorter than the textarea
/// and the two scroll out of step at the bottom.
fn highlighted(lang: &Lang, src: &str) -> AnyView {
    let mut text = src.to_string();
    if text.ends_with('\n') {
        text.push(' ');
    }
    highlight(*lang, &text)
        .into_iter()
        .map(|(tok, run)| view! { <span class=tok.class()>{run}</span> })
        .collect::<Vec<_>>()
        .into_any()
}

/// Keep the painted layer aligned with the textarea's scroll position. The textarea is the one
/// that actually scrolls; the <pre> is dragged along behind it.
fn sync_scroll(
    area: NodeRef<leptos::html::Textarea>,
    paint: NodeRef<leptos::html::Pre>,
) {
    if let (Some(a), Some(p)) = (area.get_untracked(), paint.get_untracked()) {
        p.set_scroll_top(a.scroll_top());
        p.set_scroll_left(a.scroll_left());
    }
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
                state.flash.set(Some(Flash::ok(format!("Saved {}.", c.path))));
                store.error.set(None);
            }
            Err(e) => store.error.set(Some(e)),
        }
        store.busy.set(false);
    });
}
