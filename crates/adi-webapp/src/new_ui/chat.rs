//! The chat window, at `/chat`: a conversation with one of this machine's agents.
//!
//! The whole window is `<adi-chat picker>` — the chat as plain HTML and JavaScript
//! (`design/elements/chat.js`), which picks the agent and the conversation, reads the agent API and
//! polls on its own. Nothing of the panel's chat state is shared with it, so nothing here reaches
//! into — or is disturbed by — the rest of the screen.

use leptos::prelude::*;

#[component]
pub(super) fn Chat() -> impl IntoView {
    view! {
        <div class="adi-new-chat">
            <adi-chat class="adi-new-chat__body" picker=""></adi-chat>
        </div>
    }
}
