//! The chat windows. Both are `<adi-chat>` — the chat as plain HTML and JavaScript
//! (`design/elements/chat.js`), which reads the agent API and polls on its own, so nothing of the
//! panel's chat state is shared with them.
//!
//! - [`Chat`], at `/chat`: pick an agent, then one of its conversations. Its list's right-click
//!   "Open in new window" asks for a [`Pinned`] one, which the desk opens.
//! - [`Pinned`], at `/chat/[<node>/]<agent>/<run>`: one conversation and nothing else, as many as
//!   are open — or a second [`Chat`], when the island's "New window" asked for one.

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

/// One conversation in a window of its own: no pickers, and no list to go back to — or, opened by
/// the island's "New window" with no conversation, a second chat window with pickers of its own.
#[component]
pub(super) fn Pinned(desk: Desk, id: u32) -> impl IntoView {
    view! {
        {move || desk.chat(id).map(|c| {
            if c.agent.is_empty() {
                view! { <Chat desk/> }.into_any()
            } else {
                view! {
                    <div class="adi-new-chat">
                        <adi-chat
                            class="adi-new-chat__body"
                            node=c.node.unwrap_or_default()
                            agent=c.agent
                            run=c.run
                        ></adi-chat>
                    </div>
                }
                .into_any()
            }
        })}
    }
}

/// The conversation an `open-window` event names: `{ node, agent, run, title }` in its detail —
/// `node` null for this machine, which `as_string` reads as absent.
fn chat_of(ev: &web_sys::Event) -> Option<ChatRef> {
    let detail = js_sys::Reflect::get(ev.unchecked_ref::<JsValue>(), &"detail".into()).ok()?;
    let field = |k: &str| {
        js_sys::Reflect::get(&detail, &k.into())
            .ok()
            .and_then(|v| v.as_string())
    };
    Some(ChatRef {
        node: field("node").filter(|n| !n.is_empty()),
        agent: field("agent")?,
        run: field("run")?,
        title: field("title").unwrap_or_default(),
    })
}
