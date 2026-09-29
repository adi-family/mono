//! The chat windows — `<adi-chat>`, the chat as plain HTML and JavaScript
//! (`design/elements/chat.js`), which reads the agent API and polls on its own.
//!
//! Every chat window is the same thing at its own address, like a browser tab: `/chat`,
//! `/chat/<agent>`, `/chat/<agent>/<run>`, with `@<node>/` first for a paired machine
//! ([`ChatRef::link`]). Where a window is lives on the desk under that window alone, so a reload
//! puts each back where it was and no window ever opens where another one is. The first is
//! [`Win::Chat`], the island's own; the rest are [`Win::Talk`]s.

use leptos::prelude::*;
use wasm_bindgen::{JsCast, JsValue};

use super::windows::{ChatRef, Desk, Win};

#[component]
pub(super) fn Chat(desk: Desk, win: Win) -> impl IntoView {
    let place = Memo::new(move |_| desk.chat_place(win));
    view! {
        <div
            class="adi-new-chat"
            // Both events are `<adi-chat>`'s, composed, so they reach this wrapper from inside the
            // element's shadow root. `place`: this window moved itself. `open-window`: one of its
            // conversations was asked for in a window of its own.
            on:place=move |ev: web_sys::Event| {
                if let Some(to) = chat_of(&ev) {
                    desk.chat_moved(win, to);
                }
            }
            on:open-window=move |ev: web_sys::Event| {
                if let Some(to) = chat_of(&ev) {
                    desk.open_chat(to);
                }
            }
        >
            <adi-chat
                class="adi-new-chat__body"
                picker=""
                windows=""
                node=move || place.with(|p| p.node.clone().unwrap_or_default())
                agent=move || place.with(|p| p.agent.clone())
                run=move || place.with(|p| p.run.clone())
            ></adi-chat>
        </div>
    }
}

/// The place an event names: `{ node, agent, run, title }` in its detail — `node` null for this
/// machine and `run` null for the list, which `as_string` reads as absent.
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
        run: field("run").unwrap_or_default(),
        title: field("title").unwrap_or_default(),
    })
}
