//! The chat window, at `/chat`: a conversation with one of this machine's agents — the same
//! transcript, composer and conversation list the old screens show (`pages::live_view`),
//! mounted here as they are, with a picker for which agent above them.
//!
//! It keeps a chat's state of its own ([`State`], [`AgentsWatch`]), as the embedded chat in
//! `main.rs` does, so nothing here reaches into — or is disturbed by — the rest of the screen.
//! The home screen's chat widget ([`super::widgets`]) is a second one, beside the window's.

use adi_webapp_api::types::AgentDto;
use leptos::prelude::*;

use crate::pages::{live_view, poll_watch, reset_chat_home};
use crate::state::{self, AgentsWatch, ROOT_AGENT, State};
use super::watching;
use crate::{fetch, live};

/// How often the chat is read while the live channel is down. With the channel up, it pushes.
const POLL_MS: u64 = 1_000;

/// `widget` is the home screen's copy rather than the window's: only its element ids differ, so
/// the two can be on screen at once.
#[component]
pub(super) fn Chat(#[prop(optional)] widget: bool) -> impl IntoView {
    let picker_id = if widget {
        "adi-new-chat-agent-widget"
    } else {
        "adi-new-chat-agent"
    };
    let state = State::fresh();
    let watch = AgentsWatch::new();

    // The agents to choose from; the root agent is the one a chat opens on, as on the old screen.
    leptos::task::spawn_local(async move {
        if let Ok(a) = fetch::agents().await {
            let root = a.agents.iter().find(|d| d.name == ROOT_AGENT).cloned();
            state.agents.set(Some(a));
            if let Some(root) = root {
                choose(state, watch, &root);
            }
        }
    });

    let part = watching::join(move || state::chat_subscriptions(watch));
    Effect::new(move |_| watching::send());
    let tick = set_interval_with_handle(
        move || {
            if !live::connected() {
                poll_watch(watch);
            }
        },
        std::time::Duration::from_millis(POLL_MS),
    );
    on_cleanup(move || {
        if let Ok(t) = tick {
            t.clear();
        }
        watching::leave(part);
    });

    let agents = move || {
        let mut list: Vec<AgentDto> = state
            .agents
            .with(|a| a.as_ref().map(|a| a.agents.clone()))
            .unwrap_or_default();
        list.sort_by(|a, b| (a.name != ROOT_AGENT, &a.name).cmp(&(b.name != ROOT_AGENT, &b.name)));
        list
    };

    view! {
        <div class="adi-new-chat">
            <div class="adi-new-chat__bar">
                <label class="adi-new-chat__label" for=picker_id>"Agent"</label>
                <select
                    id=picker_id
                    class="adi-input adi-new-chat__agent"
                    prop:value=move || watch.name.get().unwrap_or_default()
                    on:change=move |ev| {
                        let name = event_target_value(&ev);
                        let picked = state
                            .agents
                            .with_untracked(|a| {
                                a.as_ref()?.agents.iter().find(|d| d.name == name).cloned()
                            });
                        if let Some(agent) = picked {
                            choose(state, watch, &agent);
                        }
                    }
                >
                    {move || {
                        agents()
                            .into_iter()
                            .map(|a| {
                                let label = a.name.clone();
                                view! { <option value=a.name>{label}</option> }
                            })
                            .collect_view()
                    }}
                </select>
            </div>
            {move || state.flash.get().map(|f| view! {
                <p class="adi-new-chat__error" role="alert">{f.msg}</p>
            })}
            <div class="adi-new-chat__body">
                {move || match live_view(state, watch) {
                    Some(v) => v,
                    // Before the agents arrive — or after the chat's own Close, which lets go of the agent.
                    None if state.agents.with(Option::is_none) => {
                        view! { <p class="adi-new-win__note">"Loading…"</p> }.into_any()
                    }
                    None => view! {
                        <p class="adi-new-win__note">"Pick an agent above to talk to it."</p>
                    }
                    .into_any(),
                }}
            </div>
        </div>
    }
}

/// Point the chat at one agent: close whatever conversation was open, then watch this one — its
/// live pane if it runs in a terminal, its conversations if not.
fn choose(state: State, watch: AgentsWatch, agent: &AgentDto) {
    reset_chat_home(state, watch);
    state.flash.set(None);
    watch.runs.set(Vec::new());
    watch.peek.set(None);
    watch.interactive.set(agent.executor == "pty");
    watch.name.set(Some(agent.name.clone()));
    poll_watch(watch);
}
