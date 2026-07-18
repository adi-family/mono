//! The Services panel of the project detail page.

use adi_webapp_api::types::{NewService, ProjectDetail, ProjectService};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::{Flash, State};
use crate::ui::{TextField, apply_mutation, dash, fmt_ports, placeholder_row};

use super::reload_project;

/// The quick service create form (name, run command, optional host and port; the project is
/// fixed to the open project). The service lands in the project's `.adi/hive.yaml`; editing or
/// removing one means editing that file in the Files panel. `Copy` so it threads into the
/// panel view and its submit handler.
#[derive(Clone, Copy)]
pub(crate) struct QuickServiceForm {
    pub(crate) name: RwSignal<String>,
    pub(crate) run: RwSignal<String>,
    pub(crate) host: RwSignal<String>,
    /// The explicit `http` port as typed, or empty for an auto-leased ports-manager port.
    pub(crate) port: RwSignal<String>,
    pub(crate) busy: RwSignal<bool>,
}

/// Rows for the services table: a message when there's no hive / no services, else one row per
/// service (host, ports as `key:port`, run command, restart policy, and a Start action for
/// services that declare a runner).
pub(crate) fn service_rows(
    state: State,
    project: String,
    services: Vec<ProjectService>,
    has_hive: bool,
) -> AnyView {
    if services.is_empty() {
        let msg = if has_hive {
            "This project's .adi/hive.yaml declares no services."
        } else {
            "No .adi/hive.yaml — this project has no runtime services yet."
        };
        return placeholder_row("6", msg);
    }
    services
        .into_iter()
        .map(|s| {
            let name = s.name.clone();
            let host = dash(s.host);
            let ports = fmt_ports(&s.ports);
            let has_runner = s.run.is_some();
            let running = s.running;
            let run = dash(s.run);
            let restart = dash(s.restart);
            let action = if !has_runner {
                view! { <span class="adi-muted">"—"</span> }.into_any()
            } else if running {
                let (p, n) = (project.clone(), name.clone());
                view! {
                    <span style="color:var(--ok,#3fb950);margin-right:.5rem" title="Primary port is listening">"● Running"</span>
                    <button class="adi-btn adi-btn--ghost" type="button" title="Stop this service"
                        on:click=move |_| stop_service(state, Some(p.clone()), n.clone())>
                        "Stop"
                    </button>
                }
                .into_any()
            } else {
                let (p, n) = (project.clone(), name.clone());
                view! {
                    <button class="adi-btn adi-btn--ghost" type="button"
                        title="Run this service's command with its ports-manager port"
                        on:click=move |_| start_service(state, Some(p.clone()), n.clone())>
                        "Start"
                    </button>
                }
                .into_any()
            };
            view! {
                <tr>
                    <td class="adi-mono">{name}</td>
                    <td class="adi-mono">{host}</td>
                    <td class="adi-mono adi-table__port">{ports}</td>
                    <td class="adi-mono adi-muted">{run}</td>
                    <td class="adi-muted">{restart}</td>
                    <td>{action}</td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// The quick service create form under the Services table: name + run command, an optional
/// proxied host, and an optional explicit port (empty → a ports-manager-leased one). Posts to
/// `/api/hive/create`, which writes the service into the project's `.adi/hive.yaml` and
/// returns the fresh detail.
pub(crate) fn service_create_form(state: State, form: QuickServiceForm) -> AnyView {
    let QuickServiceForm {
        name,
        run,
        host,
        port,
        busy,
    } = form;
    view! {
        <form class="adi-form" on:submit=move |ev| {
            ev.prevent_default();
            let id = state.current_project.get_untracked();
            if id.is_empty() {
                return;
            }
            let nm = name.get().trim().to_string();
            if nm.is_empty() {
                state.flash.set(Some(Flash::err("A service name is required.".to_string())));
                return;
            }
            let run_cmd = run.get().trim().to_string();
            if run_cmd.is_empty() {
                state.flash.set(Some(Flash::err("A run command is required.".to_string())));
                return;
            }
            let host_v = host.get().trim().to_string();
            let port_txt = port.get().trim().to_string();
            let port_v = if port_txt.is_empty() {
                None
            } else {
                match port_txt.parse::<u16>() {
                    Ok(p) => Some(p),
                    Err(_) => {
                        state.flash.set(Some(Flash::err(
                            "The port must be a number (1–65535), or empty for an auto-leased one.".to_string(),
                        )));
                        return;
                    }
                }
            };
            let body = NewService {
                project: id,
                name: nm.clone(),
                run: run_cmd,
                host: (!host_v.is_empty()).then_some(host_v),
                port: port_v,
                working_dir: None,
                restart: None,
            };
            name.set(String::new());
            run.set(String::new());
            host.set(String::new());
            port.set(String::new());
            apply_mutation(state, Some(busy), format!("Added service “{nm}”."),
                |s: State, d: ProjectDetail| s.project_detail.set(Some(d)), fetch::create_service(body));
        }>
            <TextField id="pservice-name" label="Name" placeholder="api" mono=true
                hint="the key under services:" value=name />
            <TextField id="pservice-run" label="Command" placeholder="bun run start" mono=true wide=true
                field_style="flex:1 1 260px; min-width:0"
                hint="runs as sh -c with PORT injected" value=run />
            <TextField id="pservice-host" label="Host" placeholder="myapp.adi" mono=true
                hint="optional — routed by the front door" value=host />
            <TextField id="pservice-port" label="Port" placeholder="auto" mono=true numeric=true
                hint="optional — auto-leased when empty" value=port />
            <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                "Add service"
            </button>
        </form>
    }
    .into_any()
}

/// Start a service's runner on the backend (its `run` command, with the ports-manager `PORT`
/// injected), then refresh the project page so its status can flip to running.
fn start_service(state: State, project: Option<String>, service: String) {
    spawn_local(async move {
        match fetch::start_service(project.clone(), service.clone()).await {
            Ok(r) => {
                let at = r.port.map_or(String::new(), |p| format!(" on :{p}"));
                state
                    .flash
                    .set(Some(Flash::ok(format!("Started {}{at}.", r.service))));
                if let Some(id) = project {
                    reload_project(state, id);
                }
            }
            Err(e) => state
                .flash
                .set(Some(Flash::err(format!("Couldn't start {service}: {e}")))),
        }
    });
}

/// Stop a running service on the backend (kill its port's listener), then refresh the project page.
fn stop_service(state: State, project: Option<String>, service: String) {
    spawn_local(async move {
        match fetch::stop_service(project.clone(), service.clone()).await {
            Ok(r) => {
                state
                    .flash
                    .set(Some(Flash::ok(format!("Stopped {}.", r.service))));
                if let Some(id) = project {
                    reload_project(state, id);
                }
            }
            Err(e) => state
                .flash
                .set(Some(Flash::err(format!("Couldn't stop {service}: {e}")))),
        }
    });
}
