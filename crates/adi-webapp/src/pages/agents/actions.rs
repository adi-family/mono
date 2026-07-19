//! The Agents page run, stop, and live-view actions.

use adi_webapp_api::types::{AgentDto, AgentsState};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::routing::scroll_top;
use crate::state::{AgentsWatch, Flash, State};
use crate::ui::apply_mutation;

use super::send_bar;

/// The Run / View / Stop action buttons for one agent row. Process runs are deliberately
/// non-interactive, so only tmux runs expose View; both kinds can be stopped.
pub(crate) fn agent_actions(state: State, watch: AgentsWatch, a: &AgentDto) -> AnyView {
    let run_name = a.name.clone();
    let show_run = a.runnable && !a.running;
    let running = a.running;
    let interactive = a.executor == "tmux";
    let stop_title = if interactive {
        "kill the tmux session"
    } else {
        "stop the background process"
    };
    view! {
        {running.then(|| {
            let watch_name = run_name.clone();
            let stop_name = run_name.clone();
            view! {
                {interactive.then(|| view! {
                    <button class="adi-btn adi-btn--link" title="watch the live tmux session"
                        on:click=move |_| open_watch(watch, watch_name.clone())>"● View"</button>
                    " "
                })}
                <button class="adi-btn adi-btn--link" title=stop_title
                    on:click=move |_| stop_agent(state, watch, stop_name.clone())>"■ Stop"</button>
                " "
            }
        })}
        {show_run.then(|| { let run_name = run_name.clone(); view! {
            <button class="adi-btn adi-btn--link"
                on:click=move |_| run_agent(state, run_name.clone())>"▶ Run"</button>
            " "
        }})}
    }
    .into_any()
}

/// Run an agents mutation: set the returned list and a success flash, or an error flash; toggles
/// `busy` around the request when a form is driving it.
pub(crate) fn apply_agents<F>(state: State, busy: Option<RwSignal<bool>>, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<AgentsState, String>> + 'static,
{
    apply_mutation(state, busy, ok_msg, |s, a| s.agents.set(Some(a)), fut);
}

/// Launch an agent (the ▶ Run action). The server supplies an executor-specific success message.
fn run_agent(state: State, name: String) {
    spawn_local(async move {
        match fetch::run_agent(name).await {
            Ok(res) => {
                state.agents.set(Some(res.state));
                state.flash.set(Some(Flash::ok(res.message)));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Stop a running agent, refresh the list, and close its live view if one is open.
fn stop_agent(state: State, watch: AgentsWatch, name: String) {
    if watch.name.get_untracked().as_deref() == Some(name.as_str()) {
        watch.close();
    }
    apply_agents(
        state,
        None,
        format!("Stopped {name}."),
        fetch::stop_agent(name),
    );
}

/// Open the live view on an agent (the ● View action): show the panel, fetch the first snapshot
/// immediately (the 1s poll takes over from there), and scroll up to where the panel renders.
fn open_watch(watch: AgentsWatch, name: String) {
    watch.peek.set(None);
    watch.name.set(Some(name));
    poll_watch(watch);
    scroll_top();
}

/// Fetch a fresh pane snapshot for the watched agent, if any. The shell calls this every second;
/// it no-ops while the live view is closed. A response landing after the view moved to another
/// agent (or closed) is dropped instead of flashing the wrong pane.
pub(crate) fn poll_watch(watch: AgentsWatch) {
    let Some(name) = watch.name.get_untracked() else {
        return;
    };
    spawn_local(async move {
        if let Ok(peek) = fetch::peek_agent(name).await
            && watch.name.get_untracked().as_deref() == Some(peek.name.as_str())
        {
            watch.peek.set(Some(peek));
        }
    });
}

/// The live-view panel: a 1s-refreshed capture of the watched agent's tmux pane, with a send
/// bar to type into the session. Renders nothing while no agent is being watched. Shared with
/// a project's Agents panel.
pub(crate) fn live_view(state: State, watch: AgentsWatch) -> Option<AnyView> {
    let name = watch.name.get()?;
    let peek = watch.peek.get();
    let attach = peek.as_ref().map(|p| p.attach.clone()).unwrap_or_default();
    let running = peek.as_ref().is_some_and(|p| p.running);
    let body = match peek {
        None => view! { <div class="adi-empty">"Connecting…"</div> }.into_any(),
        Some(p) if !p.running => view! {
            <div class="adi-empty">"The session has ended — run the agent again to restart it."</div>
        }
        .into_any(),
        Some(p) => view! { <pre class="adi-term">{p.output}</pre> }.into_any(),
    };
    Some(
        view! {
            <section class="adi-panel">
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">{format!("Live view — {name}")}</h2>
                    <span class="adi-spacer"></span>
                    {(!attach.is_empty()).then(|| view! {
                        <code class="adi-mono adi-muted" style="font-size:var(--text-sm)">{attach}</code>
                    })}
                    <button class="adi-btn adi-btn--link" on:click=move |_| watch.close()>"Close"</button>
                </div>
                {body}
                {running.then(|| send_bar(state, watch))}
            </section>
        }
        .into_any(),
    )
}
