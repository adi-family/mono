//! The Hive settings page: every service declared across all projects' and dashboards'
//! `.adi/hive.yaml` plus the global front-door hive, each with a live running/stopped indicator.
//! This view is meant to be the one place every hive service is visible, whichever adi-hive
//! instance actually supervises it.

use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::routing::{Route, open_project, push_state};
use crate::state::{Flash, State};
use crate::ui::{dash, data_table, fmt_ports, placeholder_row};

/// Re-fetch `/api/hive` — which re-reads every project's `.adi/hive.yaml` and the global hive
/// from disk (re-running any `bash`…`` port commands) — and refresh the Services view.
fn reload_hive(state: State) {
    spawn_local(async move {
        match fetch::hive().await {
            Ok(h) => {
                state.hive.set(Some(h));
                state
                    .flash
                    .set(Some(Flash::ok("Reloaded hive config.".to_string())));
            }
            Err(e) => state.flash.set(Some(Flash::err(format!(
                "Couldn't reload hive config: {e}"
            )))),
        }
    });
}

pub(crate) fn hive_view(state: State, route: RwSignal<Route>) -> AnyView {
    let State { hive, .. } = state;
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <span class="adi-chip adi-mono" title="Declared services">
                    {move || hive.get().map_or_else(|| "\u{2014}".to_string(),
                        |h| h.services.len().to_string())}
                </span>
                <span class="adi-updated">
                    {move || hive.get().map_or(String::new(),
                        |h| format!("{} running", h.services.iter().filter(|s| s.running).count()))}
                </span>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--ghost" type="button"
                    title="Re-read every project's .adi/hive.yaml and the global hive from disk"
                    on:click=move |_| reload_hive(state)>"Reload config"</button>
            </div>
            {data_table(&["Source", "Service", "Host", "Ports", "Command", "Restart", "Status"],
                move || hive_rows(state, route))}
            <footer class="adi-footer">
                "Read from each project's and dashboard's " <code>".adi/hive.yaml"</code> " and the global "
                <code>"~/.adi/mono/hive/hive.yaml"</code> ". Dashboard services are supervised by the "
                "per-user dashboards hive. Status = the service's primary port is listening."
            </footer>
        </section>
    }
    .into_any()
}

/// Rows for the aggregated hive table: global (front-door) services first, then per project;
/// the source cell links into the owning project's detail page.
fn hive_rows(state: State, route: RwSignal<Route>) -> AnyView {
    let Some(h) = state.hive.get() else {
        return placeholder_row("7", "Loading…");
    };
    if h.services.is_empty() {
        return placeholder_row(
            "7",
            "No hive services declared in any project or the global hive.",
        );
    }
    let mut services = h.services;
    // Front-door first (both ids None), then projects by id, then dashboards — each group's
    // services by name.
    services.sort_by(|a, b| {
        a.project
            .cmp(&b.project)
            .then_with(|| a.dashboard.cmp(&b.dashboard))
            .then_with(|| a.name.cmp(&b.name))
    });
    services
        .into_iter()
        .map(|s| {
            let source = match (&s.project, &s.dashboard) {
                // Supervised by the per-user dashboards hive, not the front door.
                (_, Some(id)) => {
                    let short = id.split('-').next().unwrap_or(id).to_string();
                    view! {
                        <a class="adi-btn adi-btn--link adi-mono" href="/dashboards"
                            title=format!("dashboard {id}")
                            on:click=move |ev: web_sys::MouseEvent| {
                                if ev.meta_key() || ev.ctrl_key() || ev.shift_key() || ev.button() != 0 { return; }
                                ev.prevent_default();
                                push_state(Route::Dashboards.path());
                                route.set(Route::Dashboards);
                            }>{short}</a>
                    }.into_any()
                }
                (None, None) => view! { <span class="adi-chip">"front-door"</span> }.into_any(),
                (Some(id), None) => {
                    let open_id = id.clone();
                    let href = format!("/projects/{id}");
                    view! {
                        <a class="adi-btn adi-btn--link adi-mono" href=href
                            on:click=move |ev: web_sys::MouseEvent| {
                                if ev.meta_key() || ev.ctrl_key() || ev.shift_key() || ev.button() != 0 { return; }
                                ev.prevent_default();
                                open_project(state, route, open_id.clone());
                            }>{id.clone()}</a>
                    }.into_any()
                }
            };
            let host = dash(s.host);
            let ports = fmt_ports(&s.ports);
            let run = dash(s.run);
            let restart = dash(s.restart);
            let (state_attr, label) = if s.running { ("online", "Running") } else { ("down", "Stopped") };
            view! {
                <tr>
                    <td>{source}</td>
                    <td class="adi-mono">{s.name}</td>
                    <td class="adi-mono">{host}</td>
                    <td class="adi-mono adi-table__port">{ports}</td>
                    <td class="adi-mono adi-muted">{run}</td>
                    <td class="adi-muted">{restart}</td>
                    <td>
                        <span class="adi-status" data-state=state_attr>
                            <span class="adi-status__led"></span><span>{label}</span>
                        </span>
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}
