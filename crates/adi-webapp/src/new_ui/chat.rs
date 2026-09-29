//! The chat windows. Both are `<adi-chat>` — the chat as plain HTML and JavaScript
//! (`design/elements/chat.js`), which reads the agent API and polls on its own, so nothing of the
//! panel's chat state is shared with them.
//!
//! - [`Chat`], at `/chat`: pick an agent, then one of its conversations. Its list's right-click
//!   "Open in new window" asks for a [`Pinned`] one, which the desk opens.
//! - [`Pinned`], at `/chat/<agent>/<run>`: one conversation and nothing else, as many as are open.

use leptos::prelude::*;
use wasm_bindgen::{JsCast, JsValue};

use super::windows::{ChatRef, Desk};

#[component]
pub(super) fn Chat(desk: Desk) -> impl IntoView {
    view! {
        <div
            class="adi-new-chat"
            // `open-window` is `<adi-chat>`'s — composed, so it reaches this wrapper from inside
            // the element's shadow root; its detail says which conversation.
            on:open-window=move |ev: web_sys::Event| {
                if let Some(chat) = chat_of(&ev) {
                    desk.open_chat(chat);
                }
            }
        >
            <adi-chat class="adi-new-chat__body" picker="" windows=""></adi-chat>
        </div>
    }
}

/// One conversation in a window of its own: no pickers, and no list to go back to.
#[component]
pub(super) fn Pinned(desk: Desk, id: u32) -> impl IntoView {
    view! {
        <div class="adi-new-chat">
            {move || desk.chat(id).map(|c| view! {
                <adi-chat class="adi-new-chat__body" agent=c.agent run=c.run></adi-chat>
            })}
        </div>
    }
}

/// The conversation an `open-window` event names: `{ agent, run, title }` in its detail.
fn chat_of(ev: &web_sys::Event) -> Option<ChatRef> {
    let detail = js_sys::Reflect::get(ev.unchecked_ref::<JsValue>(), &"detail".into()).ok()?;
    let field = |k: &str| {
        js_sys::Reflect::get(&detail, &k.into())
            .ok()
            .and_then(|v| v.as_string())
    };
    Some(ChatRef {
        agent: field("agent")?,
        run: field("run")?,
        title: field("title").unwrap_or_default(),
    })
}
