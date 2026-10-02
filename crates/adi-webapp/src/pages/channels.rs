//! The Channels page: a card per service (`docs/channels.md` §7) — Telegram and, since
//! ADI-MONO-123, Slack — each showing its connections, a router-connection status pill, and the
//! Connect dialog that registers a fresh one.
//!
//! Page-local, like [`super::embedding_backends_view`] beside it: nothing here rides the shell's
//! 4s poll (there is no live-channel subscription for `/api/channels*` — see this crate's own
//! note on `docs/fleet.md` §14's L3, which this page's every fetch follows), so the connection
//! list, the status pill and the agent picker's options are all fetched once when the page opens.

use adi_ui::{Icon, Lucide};
use adi_webapp_api::types::{AgentDto, ChannelAllowlistDto, ChannelConnectionDto, ChannelTargetDto};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;
use gloo_timers::future::TimeoutFuture;

use crate::fetch;
use crate::state::{ChannelsConsole, Flash, State};
use crate::ui::{confirm, copy_row, field_hint, flash_view};

/// The services this build knows about, in the order their cards show: id, label, and whether
/// this build can actually connect one. Both are, now that ADI-MONO-123 has shipped Slack's
/// adapter; the third element stays rather than collapsing to a plain id/label list, since a
/// future provider still being built gets a card that says "coming soon" the same way Slack's
/// used to.
const PROVIDERS: [(&str, &str, bool); 2] =
    [("telegram", "Telegram", true), ("slack", "Slack", true)];

/// How often the Connect dialog polls `GET /api/channels/<id>` while waiting for `linked`.
const LINK_POLL_MS: u32 = 2_000;
/// Give up after this many polls (~10 minutes) — the connection stays either way; this just stops
/// a forgotten tab from polling forever.
const LINK_POLL_TRIES: u32 = 300;

pub(crate) fn channels_view(state: State, console: ChannelsConsole) -> AnyView {
    Effect::new(move |loaded: Option<()>| {
        if loaded.is_none() {
            spawn_local(refresh(console));
        }
    });

    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Channels"</h2>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--ghost" type="button"
                    on:click=move |_| spawn_local(refresh(console))>
                    <Icon icon=Lucide::RefreshCw/>"Reload"
                </button>
            </div>
            <p class="adi-hint">
                "One ADI agent, reachable from Telegram or Slack. Connect a service, pick who may \
                 talk to it, and the run's answer posts back automatically."
            </p>
        </section>

        {move || error_view(console)}

        {PROVIDERS.iter().map(|&(id, label, buildable)| {
            provider_card(state, console, id, label, buildable)
        }).collect::<Vec<_>>()}
    }
    .into_any()
}

fn error_view(console: ChannelsConsole) -> AnyView {
    match console.error.get() {
        Some(e) => view! { <p class="adi-error" role="alert">{e}</p> }.into_any(),
        None => ().into_any(),
    }
}

// ------------------------------------------------------------------- provider card

/// One service: its connections (if any), a router-connection status pill, and either a Connect
/// button or (for a provider this build can't register yet) a "coming soon" note.
fn provider_card(
    state: State,
    console: ChannelsConsole,
    provider: &'static str,
    label: &'static str,
    buildable: bool,
) -> AnyView {
    let connections = move || {
        console
            .connections
            .get()
            .map(|s| s.connections)
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.provider == provider)
            .collect::<Vec<_>>()
    };

    view! {
        <div class="adi-channels-card">
            <div class="adi-channels-card__head">
                <h2 class="adi-panel__title">{label}</h2>
                {move || buildable.then(|| status_pill(console, provider))}
                <span class="adi-spacer"></span>
                {buildable.then(|| view! {
                    <button class="adi-btn adi-btn--primary" type="button"
                        on:click=move |_| console.open_connect(provider)>
                        "Connect"
                    </button>
                })}
            </div>
            <div class="adi-channels-card__body">
                {move || if buildable {
                    let rows = connections();
                    if rows.is_empty() {
                        view! {
                            <p class="adi-hint">"Not connected."</p>
                        }
                        .into_any()
                    } else {
                        rows.iter()
                            .map(|c| connection_row(state, console, c))
                            .collect::<Vec<_>>()
                            .into_any()
                    }
                } else {
                    view! {
                        <p class="adi-hint">
                            "Coming soon — "{label}" support ships in a later build."
                        </p>
                    }
                    .into_any()
                }}
            </div>
        </div>

        {buildable.then(|| connect_dialog(state, console, provider, label))}
    }
    .into_any()
}

/// The router-connection pill: whether this provider's socket to the router is actually open
/// right now (`GET /api/channels/status`) — not whether any chat has linked, which the
/// connection rows below say on their own.
fn status_pill(console: ChannelsConsole, provider: &str) -> AnyView {
    let connected = console.status.get().get(provider).copied().unwrap_or(false);
    if connected {
        view! {
            <span class="adi-status" data-state="online"
                title="This node's socket to the channel router is open.">
                <span class="adi-status__led"></span>
                <span>"router connected"</span>
            </span>
        }
        .into_any()
    } else {
        view! {
            <span class="adi-status" data-state="down"
                title="No live socket to the channel router right now.">
                <span class="adi-status__led"></span>
                <span>"router offline"</span>
            </span>
        }
        .into_any()
    }
}

// ------------------------------------------------------------------- connection row

/// One connected row: its state, who it targets, who may talk — and, inline, whichever of
/// "Change agent" / "Who may talk" is being edited, in place of the summary line it replaces.
fn connection_row(state: State, console: ChannelsConsole, c: &ChannelConnectionDto) -> AnyView {
    let id = c.id.clone();
    let editing_route = console.editing_route.get() == id;
    let editing_allow = console.editing_allow.get() == id;

    let state_label = match (c.linked, c.paused) {
        (false, _) => ("pending link", "idle"),
        (true, true) => ("paused", "idle"),
        (true, false) => ("live", "online"),
    };

    let change_agent_row = c.clone();
    let pause_id = id.clone();
    let paused = c.paused;
    let allow_row = c.clone();
    let disconnect_id = id.clone();
    let disconnect_label = c.routing_key.clone();

    view! {
        <div class="adi-channels-row">
            <div class="adi-channels-row__head">
                <span class="adi-status" data-state=state_label.1>
                    <span class="adi-status__led"></span>
                    <span>{state_label.0}</span>
                </span>
                <span class="adi-mono adi-muted">{id.clone()}</span>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--ghost" type="button"
                    on:click=move |_| console.start_edit_route(&change_agent_row)>
                    "Change agent"
                </button>
                <button class="adi-btn adi-btn--ghost" type="button"
                    on:click=move |_| apply_pause(state, console, pause_id.clone(), !paused)>
                    {if paused { "Resume" } else { "Pause" }}
                </button>
                <button class="adi-btn adi-btn--ghost" type="button"
                    on:click=move |_| console.start_edit_allow(&allow_row)>
                    "Who may talk"
                </button>
                <button class="adi-btn adi-btn--ghost" type="button"
                    on:click=move |_| {
                        let question = if disconnect_label.is_empty() {
                            "Disconnect this connection? The router drops it, and it stops reaching this agent.".to_string()
                        } else {
                            format!(
                                "Disconnect {disconnect_label}? The router drops it, and it stops reaching this agent."
                            )
                        };
                        if confirm(&question) {
                            apply_disconnect(state, console, disconnect_id.clone());
                        }
                    }>
                    "Disconnect"
                </button>
            </div>
            {(!editing_route && !editing_allow).then(|| view! {
                <div class="adi-channels-row__note">
                    "target: "{target_label(&c.target)}" · who may talk: "{allowlist_label(&c.allowlist)}
                    {(c.linked && !c.routing_key.is_empty()).then(|| format!(" · {}", c.routing_key))}
                </div>
            })}
        </div>

        {editing_route.then(|| route_editor(state, console, id.clone()))}
        {editing_allow.then(|| allow_editor(state, console, id.clone()))}
    }
    .into_any()
}

fn target_label(target: &ChannelTargetDto) -> String {
    match target {
        ChannelTargetDto::Agent { agent } => agent.clone(),
        ChannelTargetDto::Trigger { trigger } => format!("trigger:{trigger}"),
        ChannelTargetDto::AppRoute { app, route } => format!("app:{app}/{route}"),
    }
}

fn allowlist_label(allowlist: &ChannelAllowlistDto) -> String {
    match allowlist {
        ChannelAllowlistDto::OwnerOnly => "only me".to_string(),
        ChannelAllowlistDto::Open => "anyone in this chat".to_string(),
        ChannelAllowlistDto::List { sender_ids } => format!("list: {}", sender_ids.join(", ")),
    }
}

/// "Change agent": a picker from this node's own agent list, Save/Cancel.
fn route_editor(state: State, console: ChannelsConsole, id: String) -> AnyView {
    let agents = console
        .agents
        .get()
        .map(|a| a.agents)
        .unwrap_or_default();
    view! {
        <div class="adi-form">
            {agent_picker(console.route_agent, &agents)}
            <button class="adi-btn adi-btn--primary" type="button"
                prop:disabled=move || console.route_agent.get().is_empty()
                on:click=move |_| apply_route(state, console, id.clone())>
                "Save"
            </button>
            <button class="adi-btn adi-btn--ghost" type="button"
                on:click=move |_| console.cancel_edit_route()>
                "Cancel"
            </button>
        </div>
    }
    .into_any()
}

/// "Who may talk": owner-only / open / a named list, Save/Cancel.
fn allow_editor(state: State, console: ChannelsConsole, id: String) -> AnyView {
    view! {
        <div class="adi-form">
            <select class="adi-input"
                prop:value=move || console.allow_mode.get()
                on:change=move |ev| console.allow_mode.set(event_target_value(&ev))>
                <option value="owner_only">"Only me"</option>
                <option value="open">"Anyone in this chat"</option>
                <option value="list">"Only these senders"</option>
            </select>
            {move || (console.allow_mode.get() == "list").then(|| view! {
                <input class="adi-input adi-input--wide" placeholder="sender id, sender id, …"
                    prop:value=move || console.allow_senders.get()
                    on:input=move |ev| console.allow_senders.set(event_target_value(&ev)) />
            })}
            <button class="adi-btn adi-btn--primary" type="button"
                on:click=move |_| apply_allow(state, console, id.clone())>
                "Save"
            </button>
            <button class="adi-btn adi-btn--ghost" type="button"
                on:click=move |_| console.cancel_edit_allow()>
                "Cancel"
            </button>
        </div>
    }
    .into_any()
}

fn agent_picker(value: RwSignal<String>, agents: &[AgentDto]) -> AnyView {
    let options = agents.to_vec();
    view! {
        <select class="adi-input adi-input--wide"
            prop:value=move || value.get()
            on:change=move |ev| value.set(event_target_value(&ev))>
            <option value="" selected=move || value.get().is_empty()>"Pick an agent"</option>
            {options.iter().map(|a| {
                let name = a.name.clone();
                view! { <option value=name.clone() selected=move || value.get() == name>{a.name.clone()}</option> }
            }).collect::<Vec<_>>()}
        </select>
    }
    .into_any()
}

// ------------------------------------------------------------------- connect dialog

/// The Connect dialog: an agent picker and who-may-talk, then — once `connect` has registered
/// the connection — the install/link URL and a wait on `linked` (`docs/channels.md` §7).
///
/// One per provider card, each with its own `open` signal kept in step with
/// [`ChannelsConsole::connect_provider`] — the same two-`Effect` pattern
/// `pages::marketplace::install_dialog` uses for the same reason: a dialog dismissed by its own
/// ×/Escape/scrim has to put that signal back, or the Connect button that opens it stops working.
fn connect_dialog(state: State, console: ChannelsConsole, provider: &'static str, label: &'static str) -> AnyView {
    let open = RwSignal::new(false);
    Effect::new(move |_| {
        let wanted = console.connect_provider.get() == provider;
        if open.get_untracked() != wanted {
            open.set(wanted);
        }
    });
    Effect::new(move |_| {
        if !open.get() && console.connect_provider.get_untracked() == provider {
            console.close_connect();
        }
    });

    view! {
        <adi_ui::Modal open=open title=format!("Connect {label}") width="max-w-lg">
            {move || match console.connect_result.get() {
                None => connect_form(state, console, provider).into_any(),
                Some(result) => connect_waiting(console, label, result.install_url, result.connection.id).into_any(),
            }}
        </adi_ui::Modal>
    }
    .into_any()
}

/// Phase 1: pick an agent and who may talk, then submit.
fn connect_form(state: State, console: ChannelsConsole, provider: &'static str) -> AnyView {
    let agents = console
        .agents
        .get()
        .map(|a| a.agents)
        .unwrap_or_default();
    view! {
        <form class="adi-panel__body" on:submit=move |ev| {
            ev.prevent_default();
            submit_connect(state, console, provider);
        }>
            <div class="adi-field adi-field--grow">
                <label class="adi-field__label">"Agent"</label>
                {field_hint("Which agent every message on this connection runs against.")}
                {agent_picker(console.connect_agent, &agents)}
            </div>
            <div class="adi-field adi-field--grow">
                <label class="adi-field__label">"Who may talk"</label>
                {field_hint(
                    "Only whoever completes the link below, or anyone in the chat/workspace once \
                     it's linked. Narrow this further any time with \u{201c}Who may talk\u{201d} \
                     on the connected row."
                )}
                <select class="adi-input"
                    prop:value=move || if console.connect_open_allowlist.get() { "open" } else { "owner_only" }
                    on:change=move |ev| console.connect_open_allowlist.set(event_target_value(&ev) == "open")>
                    <option value="owner_only">"Only me (default)"</option>
                    <option value="open">"Anyone in this chat/workspace"</option>
                </select>
            </div>
            <div class="adi-form">
                <button class="adi-btn adi-btn--primary" type="submit"
                    prop:disabled=move || console.connect_busy.get() || console.connect_agent.get().is_empty()>
                    {move || if console.connect_busy.get() { "Connecting\u{2026}" } else { "Connect" }}
                </button>
            </div>
            {flash_view(state.flash)}
        </form>
    }
    .into_any()
}

/// Phase 2: the install/link URL, and a wait on `linked`.
fn connect_waiting(console: ChannelsConsole, label: &'static str, install_url: String, id: String) -> AnyView {
    let field = NodeRef::new();
    let url_for_link = install_url.clone();
    view! {
        <div class="adi-panel__body">
            <p>"Connection "<code class="adi-mono">{id}</code>" created."</p>
            {if install_url.is_empty() {
                view! {
                    <p class="adi-hint">
                        "This build has no install/link URL for "{label}" yet \u{2014} complete the \
                         link however that service's own install flow works."
                    </p>
                }
                .into_any()
            } else {
                view! {
                    <div class="adi-field">
                        <span class="adi-field__label">"Complete the link at"</span>
                        {copy_row(field, move || install_url.clone())}
                        <div class="adi-field__note">
                            <a href=url_for_link.clone() target="_blank" rel="noreferrer">{url_for_link.clone()}</a>
                        </div>
                    </div>
                }
                .into_any()
            }}
            {move || if console.connect_linked.get() {
                view! {
                    <span class="adi-status" data-state="online">
                        <span class="adi-status__led"></span><span>"Linked"</span>
                    </span>
                }
                .into_any()
            } else {
                view! { <p class="adi-hint">"Waiting for the link to complete\u{2026}"</p> }.into_any()
            }}
        </div>
    }
    .into_any()
}

// ------------------------------------------------------------------- actions

async fn refresh(console: ChannelsConsole) {
    match fetch::channels().await {
        Ok(s) => {
            console.connections.set(Some(s));
            console.error.set(None);
        }
        Err(e) => console.error.set(Some(e)),
    }
    if let Ok(status) = fetch::channel_status().await {
        console.status.set(status.connected);
    }
    if console.agents.get_untracked().is_none()
        && let Ok(agents) = fetch::agents().await
    {
        console.agents.set(Some(agents));
    }
}

/// Submit the Connect form: register the connection, then (if "anyone in this chat" was picked)
/// widen the allowlist in a second call — `POST /api/channels/connect` always starts owner-only
/// (`docs/channels.md` §7: who-may-talk is a separate step).
fn submit_connect(state: State, console: ChannelsConsole, provider: &'static str) {
    let agent = console.connect_agent.get().trim().to_string();
    if agent.is_empty() {
        state.flash.set(Some(Flash::err("Pick an agent.".to_string())));
        return;
    }
    let open_allowlist = console.connect_open_allowlist.get();
    console.connect_busy.set(true);
    spawn_local(async move {
        let result = fetch::connect_channel(
            provider.to_string(),
            ChannelTargetDto::Agent { agent },
        )
        .await;
        console.connect_busy.set(false);
        match result {
            Ok(connected) => {
                let id = connected.connection.id.clone();
                console.connect_result.set(Some(connected));
                state.flash.set(None);
                if open_allowlist {
                    let _ = fetch::allow_channel(id.clone(), ChannelAllowlistDto::Open).await;
                }
                spawn_local(refresh(console));
                poll_for_link(console, id);
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Poll `GET /api/channels/<id>` until `linked`, the dialog is closed, or the budget runs out.
fn poll_for_link(console: ChannelsConsole, id: String) {
    spawn_local(async move {
        for _ in 0..LINK_POLL_TRIES {
            TimeoutFuture::new(LINK_POLL_MS).await;
            if console.connect_result.get_untracked().is_none() {
                return; // the dialog moved on (closed, or a different connect started)
            }
            if let Ok(c) = fetch::channel(&id).await
                && c.linked
            {
                console.connect_linked.set(true);
                spawn_local(refresh(console));
                return;
            }
        }
    });
}

fn apply_route(state: State, console: ChannelsConsole, id: String) {
    let agent = console.route_agent.get().trim().to_string();
    if agent.is_empty() {
        return;
    }
    spawn_local(async move {
        match fetch::route_channel(id, ChannelTargetDto::Agent { agent }).await {
            Ok(fresh) => {
                console.connections.set(Some(fresh));
                console.cancel_edit_route();
                state.flash.set(None);
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

fn apply_pause(state: State, console: ChannelsConsole, id: String, paused: bool) {
    spawn_local(async move {
        match fetch::pause_channel(id, paused).await {
            Ok(fresh) => {
                console.connections.set(Some(fresh));
                state.flash.set(None);
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

fn apply_allow(state: State, console: ChannelsConsole, id: String) {
    let allowlist = match console.allow_mode.get().as_str() {
        "open" => ChannelAllowlistDto::Open,
        "list" => ChannelAllowlistDto::List {
            sender_ids: console
                .allow_senders
                .get()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        },
        _ => ChannelAllowlistDto::OwnerOnly,
    };
    spawn_local(async move {
        match fetch::allow_channel(id, allowlist).await {
            Ok(fresh) => {
                console.connections.set(Some(fresh));
                console.cancel_edit_allow();
                state.flash.set(None);
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

fn apply_disconnect(state: State, console: ChannelsConsole, id: String) {
    spawn_local(async move {
        match fetch::disconnect_channel(id).await {
            Ok(fresh) => {
                console.connections.set(Some(fresh));
                state.flash.set(None);
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_label_reads_as_one_short_word_per_kind() {
        assert_eq!(
            target_label(&ChannelTargetDto::Agent { agent: "adi-agent".into() }),
            "adi-agent"
        );
        assert_eq!(
            target_label(&ChannelTargetDto::Trigger { trigger: "t1".into() }),
            "trigger:t1"
        );
        assert_eq!(
            target_label(&ChannelTargetDto::AppRoute { app: "crm".into(), route: "/webhook".into() }),
            "app:crm//webhook"
        );
    }

    #[test]
    fn allowlist_label_reads_as_one_short_phrase_per_kind() {
        assert_eq!(allowlist_label(&ChannelAllowlistDto::OwnerOnly), "only me");
        assert_eq!(allowlist_label(&ChannelAllowlistDto::Open), "anyone in this chat");
        assert_eq!(
            allowlist_label(&ChannelAllowlistDto::List {
                sender_ids: vec!["a".into(), "b".into()],
            }),
            "list: a, b"
        );
    }

    /// Every provider named here is used for its own card and (when buildable) its own Connect
    /// dialog — two things in this file keyed on the same id, so a typo in one would silently
    /// draw a card whose dialog never opens.
    #[test]
    fn every_provider_has_a_non_empty_id_and_label() {
        for (id, label, _) in PROVIDERS {
            assert!(!id.is_empty());
            assert!(!label.is_empty());
        }
    }
}
