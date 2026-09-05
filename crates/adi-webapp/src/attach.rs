//! Attaching files to a message: picking them up, uploading them, and handing the composer the
//! ids a send names them by.
//!
//! # A picture and a PDF are the same act
//!
//! Both are uploaded here and stored the same way; what differs is what happens to them afterwards.
//! An image is shown to the model in the request body. Anything else — a PDF, a CSV, an export
//! somebody was sent — reaches the agent as a **path**, because that is what attaching a file to a
//! conversation is actually for: getting the bytes onto the machine the run happens on, where the
//! agent's own tools can open them.
//!
//! [`attaching`] builds the [`adi_ui::Attaching`] one composer needs, bound to one tray signal.
//! `adi-ui` draws the tray, the paperclip and the paste/drop handling; everything that touches the
//! network or the filesystem is here, for the same reason dictation's recorder is in `voice` and
//! not in the button.
//!
//! # The upload starts when the picture is chosen, not when Send is pressed
//!
//! A screenshot is pasted and then a sentence is typed about it. That sentence is several seconds
//! of the upload happening for free — so this fires the moment the file arrives, and the composer
//! holds the send until it lands. Doing it at send time would put the whole round trip *after* the
//! last keystroke, which is exactly where a person is watching for the message to go.
//!
//! # The thumbnail is local
//!
//! It is drawn from an object URL over the picked `File`, so it appears instantly and without a
//! round trip — and is swapped for the stored image's own URL when the upload lands, at which point
//! the object URL is revoked. Keeping the local one would leak a blob per attachment for as long as
//! the tab is open.

use adi_ui::{AttachKind, AttachState, Attached, Attaching};
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use crate::fetch;
use crate::state::State;

/// How many attachments one message may carry.
///
/// Not a limit the server minds — it is a limit the *model* minds, and the wallet: every image is
/// thousands of tokens on every round of the turn that answers it. Six is more than a message
/// realistically needs and well under the point where the reply is mostly pictures.
const MAX_ATTACHMENTS: usize = 6;

/// The largest image to accept, mirroring the store's own cap. Checked here so an oversized file is
/// refused where it was dropped, rather than after the seconds it takes to upload it.
const MAX_BYTES: f64 = 5.0 * 1024.0 * 1024.0;

/// The largest of anything else — the store's file cap, mirrored for the same reason.
///
/// Higher than an image's, because a file is never in a request body: it is written to disk and
/// named in the message, so the number is about what belongs in a chat rather than what a provider
/// will take.
const MAX_FILE_BYTES: f64 = 25.0 * 1024.0 * 1024.0;

/// What the browser calls a file it has no type for. Sent as-is: the store files it by the
/// extension of its name, and an empty `Content-Type` would be one more thing for the server to
/// have an opinion about.
const UNKNOWN_TYPE: &str = "application/octet-stream";

/// Bind one composer's tray to the uploader.
///
/// `can_attach` is the backend's answer, not a preference: an engine handed its message on a
/// command line has nowhere to put a picture, and the composer says so rather than accepting one it
/// would drop. `refusal` is what it says.
pub(crate) fn attaching(
    state: State,
    node: Option<String>,
    files: RwSignal<Vec<Attached>>,
    can_attach: Signal<bool>,
    refusal: Signal<String>,
) -> Attaching {
    Attaching {
        files,
        on_files: Callback::new(move |picked: Vec<web_sys::File>| {
            take(state, node.clone(), files, picked);
        }),
        can_attach,
        refusal,
    }
}

/// Where a stored attachment's bytes are served from.
///
/// The one agent read that does not go through `/api/node/<node>` while a node's sessions are on
/// screen (`docs/fleet.md` §13): the forwarder answers JSON and this answers a PNG. It goes to the
/// node's own origin instead, which needs nothing new — the front door routes `*.n.adi` to the
/// gateway, and the gateway attaches the password this machine already holds (§11). An `<img src>`
/// is a plain GET, so there is no preflight to fail on the way.
#[must_use]
pub(crate) fn url_of(node: Option<&str>, id: &str) -> String {
    let path = format!("/api/agents/attachment/{id}");
    match node {
        Some(node) => format!("http://{}{path}", adi_webapp_api::types::node_app_host(node)),
        None => path,
    }
}

/// The ids of everything in a tray that is ready to send, in the order it was attached.
///
/// Anything still uploading or failed is left out. The composer will not fire a send while
/// something is on its way, so in practice this is the whole tray minus what has already failed —
/// and a failed picture that quietly blocked the message would be worse than one left behind.
#[must_use]
pub(crate) fn ready_ids(files: RwSignal<Vec<Attached>>) -> Vec<String> {
    files
        .get_untracked()
        .iter()
        .filter_map(|a| a.id().map(str::to_string))
        .collect()
}

/// Empty a tray, releasing any object URLs still in it — what a sent message does to its
/// attachments.
pub(crate) fn clear(files: RwSignal<Vec<Attached>>) {
    for item in files.get_untracked() {
        revoke(&item.preview);
    }
    files.set(Vec::new());
}

/// Accept what was pasted, dropped or picked: refuse what cannot be sent, show the rest at once,
/// and upload each in the background.
fn take(
    state: State,
    node: Option<String>,
    files: RwSignal<Vec<Attached>>,
    picked: Vec<web_sys::File>,
) {
    for file in picked {
        let room = MAX_ATTACHMENTS.saturating_sub(files.get_untracked().len());
        if room == 0 {
            state.flash.set(Some(crate::state::Flash::err(format!(
                "A message can carry {MAX_ATTACHMENTS} attachments; the rest were left out."
            ))));
            return;
        }
        // Whatever the browser says it is, or nothing at all for a type it does not know — a
        // `.heic` from a phone, an export with a private extension. Neither is refused: only an
        // image is *shown* to a model, and everything else is a file the agent opens by path.
        let media_type = match file.type_() {
            t if t.trim().is_empty() => UNKNOWN_TYPE.to_string(),
            t => t,
        };
        let image = supported(&media_type);
        let cap = if image { MAX_BYTES } else { MAX_FILE_BYTES };
        if file.size() > cap {
            let limit = if image { "5 MB" } else { "25 MB" };
            state.flash.set(Some(crate::state::Flash::err(format!(
                "“{}” is larger than {limit} — it was left out.",
                file.name()
            ))));
            continue;
        }
        start(state, node.clone(), files, file, media_type);
    }
}

/// Put one file in the tray as uploading, then upload it and settle its row.
fn start(
    state: State,
    node: Option<String>,
    files: RwSignal<Vec<Attached>>,
    file: web_sys::File,
    media_type: String,
) {
    // The tray's own identity for this row, minted before there is a server id — the ✕ has to work
    // during the upload, and `key` is what it removes by. Monotonic within the page, which is all
    // it has to be: it never leaves the browser.
    let key = format!(
        "attach-{}",
        js_sys::Date::now() as u64 + files.get_untracked().len() as u64
    );
    let kind = if supported(&media_type) {
        AttachKind::Image
    } else {
        AttachKind::File
    };
    let name = {
        let given = file.name();
        if given.trim().is_empty() {
            match kind {
                AttachKind::Image => "pasted image".to_string(),
                AttachKind::File => "pasted file".to_string(),
            }
        } else {
            given
        }
    };
    // Only a picture has anything to preview. A blob URL over a PDF would be a thumbnail the
    // browser cannot draw, and one more object URL to revoke for nothing.
    let preview = match kind {
        AttachKind::Image => object_url(&file),
        AttachKind::File => String::new(),
    };
    files.update(|list| {
        list.push(Attached {
            key: key.clone(),
            name: name.clone(),
            preview: preview.clone(),
            kind,
            state: AttachState::Uploading,
        });
    });

    spawn_local(async move {
        let bytes = match read_bytes(&file).await {
            Some(bytes) => bytes,
            None => {
                settle(files, &key, AttachState::Failed("unreadable".to_string()));
                return;
            }
        };
        match fetch::upload_attachment(node.as_deref(), &name, &media_type, &bytes).await {
            Ok(stored) => {
                // The stored image replaces the local blob as the thumbnail's source, and the blob
                // is released: one object URL per attachment held for the life of the tab is a leak
                // that nothing later frees.
                files.update(|list| {
                    if let Some(row) = list.iter_mut().find(|a| a.key == key) {
                        revoke(&row.preview);
                        // A file keeps its empty preview: there is nothing to draw, and pointing the
                        // tray at bytes it cannot render would be a broken image where a name is.
                        row.preview = match row.kind {
                            AttachKind::Image => url_of(node.as_deref(), &stored.id),
                            AttachKind::File => String::new(),
                        };
                        row.state = AttachState::Ready(stored.id.clone());
                    }
                });
            }
            Err(e) => {
                // The row stays, marked failed, so it can be removed deliberately — and the flash
                // says why, because a thumbnail with a red corner does not.
                settle(files, &key, AttachState::Failed(e.clone()));
                state
                    .flash
                    .set(Some(crate::state::Flash::err(format!("{name}: {e}"))));
            }
        }
    });
}

/// Move one row to its final state, if it is still there — it may have been removed while the
/// upload was in flight, and putting it back would undo a deliberate ✕.
fn settle(files: RwSignal<Vec<Attached>>, key: &str, state: AttachState) {
    files.update(|list| {
        if let Some(row) = list.iter_mut().find(|a| a.key == key) {
            row.state = state;
        }
    });
}

/// Whether this is one of the four types every provider in the loop accepts — the line between an
/// attachment a model is *shown* and one it is told the path of. Not a line between what may be
/// attached and what may not: both may.
fn supported(media_type: &str) -> bool {
    matches!(
        media_type,
        "image/png" | "image/jpeg" | "image/webp" | "image/gif"
    )
}

/// How the transcript should draw one stored attachment, from the type the store recorded.
///
/// The same question [`supported`] answers for the composer, asked of a turn that was sent long ago
/// — a chat re-rendered on a poll has only the media type to go on.
#[must_use]
pub(crate) fn kind_of(media_type: &str) -> adi_ui::AttachmentKind {
    if supported(media_type) {
        adi_ui::AttachmentKind::Picture
    } else {
        adi_ui::AttachmentKind::File
    }
}

/// One file's bytes. `None` when the browser refused to read it — a file that was moved or a
/// permission that lapsed between the drop and the read.
async fn read_bytes(file: &web_sys::File) -> Option<Vec<u8>> {
    let buffer = JsFuture::from(file.array_buffer()).await.ok()?;
    let buffer: js_sys::ArrayBuffer = buffer.dyn_into().ok()?;
    let array = js_sys::Uint8Array::new(&buffer);
    Some(array.to_vec())
}

/// A `blob:` URL over this file, for the thumbnail shown before the upload lands. Empty when the
/// browser will not make one, which draws an empty frame rather than a broken image.
fn object_url(file: &web_sys::File) -> String {
    web_sys::Url::create_object_url_with_blob(file.as_ref()).unwrap_or_default()
}

/// Release an object URL. A no-op for anything that is not one, so callers need not check which
/// kind of preview a row is holding.
fn revoke(preview: &str) {
    if preview.starts_with("blob:") {
        let _ = web_sys::Url::revoke_object_url(preview);
    }
}
