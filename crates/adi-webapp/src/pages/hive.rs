//! The Hive settings page: every service declared across all projects' `.adi/hive.yaml` plus
//! the global front-door hive, each with a live running/stopped indicator.

use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::routing::{Route, open_project};
use crate::state::{Flash, State};
use crate::ui::{dash, data_table, fmt_ports, placeholder_row, tile};

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
        <section class="adi-tiles">
            {tile("Services",
                move || hive.get().map_or_else(|| "—".to_string(), |h| h.services.len().to_string()),
                "across all projects + front-door")}
            {tile("Running",
                move || hive.get().map_or_else(|| "—".to_string(),
                    |h| h.services.iter().filter(|s| s.running).count().to_string()),
                move || hive.get().map_or_else(|| "primary port listening".to_string(),
                    |h| format!("{} stopped", h.services.iter().filter(|s| !s.running).count())))}
            {tile("Projects",
                move || hive.get().map_or_else(|| "—".to_string(), |h| {
                    let mut ids: Vec<&String> = h.services.iter().filter_map(|s| s.project.as_ref()).collect();
                    ids.sort_unstable();
                    ids.dedup();
                    ids.len().to_string()
                }),
                "contributing services (+ front-door)")}
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Hive services"</h2>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--ghost" type="button"
                    title="Re-read every project's .adi/hive.yaml and the global hive from disk"
                    on:click=move |_| reload_hive(state)>"Reload config"</button>
                <span class="adi-updated">
                    {move || hive.get().map_or(String::new(), |h| format!("{} services", h.services.len()))}
                </span>
            </div>
            {data_table(&["Source", "Service", "Host", "Ports", "Command", "Restart", "Status"],
                move || hive_rows(state, route))}
            <footer class="adi-footer">
                "Read from each project's " <code>".adi/hive.yaml"</code> " and the global "
                <code>"~/.adi/mono/hive/hive.yaml"</code> ". Status = the service's primary port is listening."
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
    // Global (project == None) sorts first (None < Some), then by project id, then service name.
    services.sort_by(|a, b| a.project.cmp(&b.project).then_with(|| a.name.cmp(&b.name)));
    services
        .into_iter()
        .map(|s| {
            let source = match &s.project {
                None => view! { <span class="adi-chip">"front-door"</span> }.into_any(),
                Some(id) => {
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
