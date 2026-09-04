//! Attaching files to a message: the tray the [`Composer`](crate::Composer) shows above what you
//! are typing, and the three ways one gets into it.
//!
//! Presentational, exactly as [`MicButton`](crate::MicButton) is. Nothing here uploads anything,
//! reads a file, or knows an endpoint: the composer collects the files a person pasted, dropped or
//! picked and hands them to whoever mounted it. In this tree that is `adi-webapp`, because *where*
//! a file is stored is a question about the app.
//!
//! A picture and a PDF are the same act here and differ only in what the tray can draw: one has a
//! thumbnail, the other has its name in a frame. Which it is, is the caller's answer — see
//! [`Attached::kind`].
//!
//! # Why a thumbnail is drawn before the upload finishes
//!
//! An attachment appears in the tray the instant it is chosen, in [`AttachState::Uploading`], with
//! whatever preview the caller could produce locally. Waiting for the round trip would mean pasting
//! a screenshot and seeing nothing happen — the one moment a person needs to be told that the paste
//! worked. The state is what says the picture is not sendable *yet*, and [`Attached::sendable`] is
//! what the send button reads.

use leptos::{ev, html, prelude::*};
use web_sys::wasm_bindgen::JsCast;

use crate::icon::{Icon, IconSize, Lucide};
use crate::merge;

/// Where one attachment has got to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachState {
    /// Chosen, and on its way to wherever the caller keeps it.
    Uploading,
    /// Stored, and carrying the id a send names it by.
    Ready(String),
    /// It did not get there, for the reason carried. It stays in the tray rather than vanishing:
    /// a picture that silently disappeared is one you attach again, and watch disappear again.
    Failed(String),
}

/// One attachment in the composer's tray.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attached {
    /// This attachment's identity *in the tray*, minted by the caller before any upload — an id
    /// from the server cannot be it, because the row exists before there is one, and the ✕ has to
    /// work in that window too.
    pub key: String,
    /// What to call it. A pasted screenshot has no name of its own and gets one from the caller.
    pub name: String,
    /// Where to draw the thumbnail from: a local object URL while it uploads, or the stored
    /// image's own URL afterwards. Empty draws the frame with no picture in it, which is what a
    /// caller with nothing to show yet should pass rather than a broken source. Ignored entirely
    /// for an [`AttachKind::File`], which has no thumbnail to draw.
    pub preview: String,
    /// Picture or file. The caller decides — the tray has no opinion about media types, and one
    /// held here would be a second list to keep in step with the store's.
    pub kind: AttachKind,
    pub state: AttachState,
}

/// What a tray row draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AttachKind {
    /// A picture, shown as a thumbnail.
    #[default]
    Image,
    /// Anything else — a PDF, a CSV, a log — shown as its name in the same frame. There is nothing
    /// to preview, and a file whose row said nothing at all would look like an upload that failed.
    File,
}

impl Attached {
    /// Whether this one can go with a message right now.
    #[must_use]
    pub fn sendable(&self) -> bool {
        matches!(self.state, AttachState::Ready(_))
    }

    /// The stored id, once there is one.
    #[must_use]
    pub fn id(&self) -> Option<&str> {
        match &self.state {
            AttachState::Ready(id) => Some(id),
            _ => None,
        }
    }
}

/// Everything the composer needs in order to take images, in one prop.
///
/// One struct rather than five parallel props because they are one decision: a composer either
/// attaches or it does not, and a caller that supplied the list but forgot the handler would have a
/// tray nothing can ever be added to.
///
/// `Copy`, because every field already is (a Leptos signal and a callback are handles). That is what
/// lets the composer's event closures each hold one — a `Clone`-only bundle would make the `send`
/// closure non-`Copy`, and a `send` that cannot be shared between the keyboard and the button is
/// two sends that have to agree.
#[derive(Debug, Clone, Copy)]
pub struct Attaching {
    /// What is attached right now, two-way. The caller owns it — it uploads into it, and reads it
    /// on send — and the composer only draws it and removes from it.
    pub files: RwSignal<Vec<Attached>>,
    /// Files a person just pasted, dropped, or picked. Called with at least one; the caller
    /// decides what to accept, what to name it, and where it goes.
    pub on_files: Callback<Vec<web_sys::File>>,
    /// Whether this conversation can take an attachment at all. False draws the refusal instead of
    /// the paperclip — some engines take text and nothing else, and accepting a file there would
    /// silently drop it.
    pub can_attach: Signal<bool>,
    /// Why not, shown when `can_attach` is false. One short clause: it sits under the box.
    pub refusal: Signal<String>,
}

impl Attaching {
    /// Whether anything is still on its way. A send has to wait for it — a message that left
    /// without its screenshot is the failure this exists to prevent.
    #[must_use]
    pub fn uploading(&self) -> bool {
        self.files
            .get()
            .iter()
            .any(|a| a.state == AttachState::Uploading)
    }

    /// Whether there is at least one image ready to send. What lets the send button light up for a
    /// message with a picture and no words — which is a whole message, not an empty one.
    #[must_use]
    pub fn has_sendable(&self) -> bool {
        self.files.get().iter().any(Attached::sendable)
    }
}

/// Every file on a drop or a paste, in order. Empty when the event carried none — a dropped link,
/// or text pasted from another document.
#[must_use]
pub fn files_of(transfer: Option<web_sys::DataTransfer>) -> Vec<web_sys::File> {
    let Some(list) = transfer.and_then(|t| t.files()) else {
        return Vec::new();
    };
    (0..list.length()).filter_map(|i| list.get(i)).collect()
}

/// The tray of what is attached, drawn above what is being typed.
///
/// Horizontal and scrollable rather than wrapping: the composer must not grow taller as pictures
/// are added, or attaching four of them pushes the transcript you are replying to off the screen.
#[component]
pub fn AttachTray(attach: Attaching) -> impl IntoView {
    let files = attach.files;
    view! {
        {move || {
            let attached = files.get();
            if attached.is_empty() {
                return None;
            }
            Some(view! {
                <div class="flex gap-2 overflow-x-auto px-1 pb-2" role="list">
                    {attached
                        .into_iter()
                        .map(|item| view! { <Thumb item=item files=files/> })
                        .collect::<Vec<_>>()}
                </div>
            })
        }}
    }
}

/// One attachment: its picture (or its name), its state, and the ✕ that takes it back.
#[component]
fn Thumb(item: Attached, files: RwSignal<Vec<Attached>>) -> impl IntoView {
    let key = item.key.clone();
    let is_file = item.kind == AttachKind::File;
    let failed = match &item.state {
        AttachState::Failed(why) => Some(why.clone()),
        _ => None,
    };
    let uploading = item.state == AttachState::Uploading;
    let title = match &failed {
        Some(why) => format!("{} — {why}", item.name),
        None => item.name.clone(),
    };
    let frame = if failed.is_some() {
        "border-err"
    } else {
        "border-line"
    };
    view! {
        <div
            class=format!(
                "group relative size-16 shrink-0 overflow-hidden rounded-md border {frame} bg-raise",
            )
            role="listitem"
            title=title
        >
            {(!is_file && !item.preview.is_empty())
                .then(|| view! {
                    <img
                        class=move || {
                            // Dimmed while it is still going: the picture is there, the send is not.
                            if uploading {
                                "size-full object-cover opacity-50"
                            } else {
                                "size-full object-cover"
                            }
                        }
                        src=item.preview.clone()
                        alt=item.name.clone()
                    />
                })}
            {is_file
                .then(|| view! {
                    // A file has no thumbnail, so the frame carries what a person actually needs to
                    // tell one attachment from another: its name, wrapped and cut to the tile.
                    <div class=move || {
                        // Dimmed while it is still going, exactly as a thumbnail is — the row is
                        // there, the send is not.
                        let dim = if uploading { "opacity-50" } else { "" };
                        format!(
                            "flex size-full flex-col items-center justify-center gap-1 px-1 \
                             text-center text-ink-2 {dim}",
                        )
                    }>
                        <Icon icon=Lucide::Paperclip size=IconSize::Sm label="File"/>
                        <span class="line-clamp-2 break-all text-[10px] leading-tight">
                            {item.name.clone()}
                        </span>
                    </div>
                })}
            {uploading
                .then(|| view! {
                    <div class="absolute inset-0 grid place-items-center text-mini text-ink-3">
                        "…"
                    </div>
                })}
            {failed
                .map(|_| view! {
                    <div class="absolute inset-0 grid place-items-center bg-raise/80 text-mini \
                                text-err">
                        "failed"
                    </div>
                })}
            <button
                class="absolute right-0.5 top-0.5 grid size-5 cursor-pointer place-items-center \
                       rounded-md bg-raise/90 text-ink-2 hover:text-ink"
                type="button"
                title="Remove this attachment"
                on:click=move |_| {
                    let key = key.clone();
                    files.update(|list| list.retain(|a| a.key != key));
                }
            >
                <Icon icon=Lucide::X size=IconSize::Sm label=format!("Remove {}", item.name)/>
            </button>
        </div>
    }
}

/// The paperclip, and the hidden file input it opens.
///
/// A real `<input type=file>` rather than anything cleverer, because the file picker is one of the
/// few things a page cannot open by itself: it opens only from a genuine user gesture on an input,
/// and every substitute for that is a picker that silently does not appear.
#[component]
pub fn AttachButton(attach: Attaching) -> impl IntoView {
    let input = NodeRef::<html::Input>::new();
    let on_files = attach.on_files;
    view! {
        <input
            class="hidden"
            // No `accept`: a person attaching a PDF, a CSV or a log is doing the same thing as one
            // attaching a screenshot, and a filter here would grey out the file they came to send.
            // What may actually be attached is decided by the caller, where the refusal can say why.
            type="file"
            multiple
            node_ref=input
            on:change=move |ev| {
                let Some(target) = ev.target() else { return };
                let Ok(element) = target.dyn_into::<web_sys::HtmlInputElement>() else {
                    return;
                };
                let Some(list) = element.files() else { return };
                let picked: Vec<web_sys::File> = (0..list.length())
                    .filter_map(|i| list.get(i))
                    .collect();
                if !picked.is_empty() {
                    on_files.run(picked);
                }
                // Cleared so that picking the *same* file twice in a row fires `change` the second
                // time — the browser only reports a change of value, and the value is the path.
                element.set_value("");
            }
        />
        <button
            class="grid size-8 shrink-0 cursor-pointer place-items-center rounded-md text-ink-2 \
                   transition-colors duration-100 hover:bg-hover hover:text-ink"
            type="button"
            title="Attach a file — or paste one, or drop it here"
            on:click=move |_| {
                if let Some(el) = input.get_untracked() {
                    el.click();
                }
            }
        >
            <Icon icon=Lucide::Paperclip size=IconSize::Md label="Attach a file"/>
        </button>
    }
}

/// What the composer says when the conversation cannot take images at all.
#[component]
pub fn AttachRefusal(
    #[prop(into)] reason: Signal<String>,
    #[prop(optional)] class: String,
) -> impl IntoView {
    view! {
        <div class=merge("text-mini text-ink-3", class)>{move || reason.get()}</div>
    }
}

/// Whether an event's files are worth handing on: a paste of plain text carries none, and a drop
/// from another window can carry anything at all.
#[must_use]
pub(crate) fn dropped(ev: &ev::DragEvent) -> Vec<web_sys::File> {
    files_of(ev.data_transfer())
}

/// The files on a paste — a screenshot from the system clipboard, or a picture copied from another
/// page. `clipboardData.files` is the one that carries both; `items` also holds the same picture as
/// a string of HTML, which is not what anybody meant by pasting it.
#[must_use]
pub(crate) fn pasted(ev: &ev::ClipboardEvent) -> Vec<web_sys::File> {
    files_of(ev.clipboard_data())
}
