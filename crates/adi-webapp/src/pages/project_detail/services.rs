//! The Services panel of the project detail page.

use adi_webapp_api::types::{NewService, NewServiceDocker, ProjectDetail, ProjectService};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::{Flash, State};
use crate::ui::{TextField, apply_mutation, dash, fmt_ports, placeholder_row};

use super::reload_project;

/// The quick service create form. A service runs one of two runner kinds — a **script** (a shell
/// command) or a **docker** container — chosen by `kind`; the fields for the other kind are simply
/// ignored on submit. The service lands in the project's `.adi/hive.yaml`; editing or removing one
/// means editing that file in the Files panel. `Copy` so it threads into the panel view and its
/// submit handler.
#[derive(Clone, Copy)]
pub(crate) struct QuickServiceForm {
    pub(crate) name: RwSignal<String>,
    /// Which runner kind the form is building: `"script"` (default) or `"docker"`.
    pub(crate) kind: RwSignal<String>,
    pub(crate) run: RwSignal<String>,
    pub(crate) host: RwSignal<String>,
    /// The explicit `http` **host** port as typed, or empty for an auto-leased ports-manager port.
    pub(crate) port: RwSignal<String>,
    // ---- docker-runner fields (used only when `kind == "docker"`) ----
    /// The container image, e.g. `nginx:1.27`.
    pub(crate) image: RwSignal<String>,
    /// The container port the host `http` port maps to.
    pub(crate) container_port: RwSignal<String>,
    /// Bind mounts, one `host:container[:mode]` per line.
    pub(crate) volumes: RwSignal<String>,
    /// Container environment, one `KEY=VALUE` per line.
    pub(crate) env: RwSignal<String>,
    /// Image pull policy (`""` | `always` | `missing` | `never`).
    pub(crate) pull: RwSignal<String>,
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
                    <span style="color:var(--online); margin-right:var(--space-2)" title="Primary port is listening">"● Running"</span>
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

/// The quick service create form under the Services table. A **Kind** toggle picks the runner: a
/// `script` (a shell command) or a `docker` container (image + a container port the leased host
/// port maps to, plus optional volumes/env/pull). Common fields — name, proxied host, and the
/// host port (empty → ports-manager-leased) — apply to both. Posts to `/api/hive/create`, which
/// writes the service into the project's `.adi/hive.yaml` and returns the fresh detail.
pub(crate) fn service_create_form(state: State, form: QuickServiceForm) -> AnyView {
    let QuickServiceForm {
        name,
        kind,
        run,
        host,
        port,
        image,
        container_port,
        volumes,
        env,
        pull,
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
            let host_v = host.get().trim().to_string();
            let host_opt = (!host_v.is_empty()).then_some(host_v);
            let port_v = match parse_opt_port(&port.get()) {
                Ok(p) => p,
                Err(msg) => { state.flash.set(Some(Flash::err(msg))); return; }
            };
            let is_docker = kind.get_untracked() == "docker";
            let (run_field, docker) = if is_docker {
                let img = image.get().trim().to_string();
                if img.is_empty() {
                    state.flash.set(Some(Flash::err("A docker image is required.".to_string())));
                    return;
                }
                let cport = match parse_opt_port(&container_port.get()) {
                    Ok(p) => p,
                    Err(_) => {
                        state.flash.set(Some(Flash::err(
                            "The container port must be a number (1–65535).".to_string())));
                        return;
                    }
                };
                let pull_v = pull.get().trim().to_string();
                (String::new(), Some(NewServiceDocker {
                    image: img,
                    container_port: cport,
                    volumes: lines(&volumes.get()),
                    environment: env_map(&env.get()),
                    pull: (!pull_v.is_empty()).then_some(pull_v),
                    args: Vec::new(),
                    command: Vec::new(),
                }))
            } else {
                let run_cmd = run.get().trim().to_string();
                if run_cmd.is_empty() {
                    state.flash.set(Some(Flash::err("A run command is required.".to_string())));
                    return;
                }
                (run_cmd, None)
            };
            let body = NewService {
                project: id,
                name: nm.clone(),
                run: run_field,
                host: host_opt,
                port: port_v,
                working_dir: None,
                restart: None,
                docker,
            };
            name.set(String::new());
            run.set(String::new());
            host.set(String::new());
            port.set(String::new());
            image.set(String::new());
            container_port.set(String::new());
            volumes.set(String::new());
            env.set(String::new());
            apply_mutation(state, Some(busy), format!("Added service “{nm}”."),
                |s: State, d: ProjectDetail| s.project_detail.set(Some(d)), fetch::create_service(body));
        }>
            <TextField id="pservice-name" label="Name" placeholder="api" mono=true
                hint="the key under services:" value=name />
            <div class="adi-field">
                <label class="adi-field__label" for="pservice-kind">"Kind"</label>
                <select class="adi-input" id="pservice-kind"
                    prop:value=move || kind.get()
                    on:change=move |ev| kind.set(event_target_value(&ev))>
                    <option value="script">"Script"</option>
                    <option value="docker">"Docker"</option>
                </select>
            </div>
            {move || if kind.get() == "docker" {
                docker_fields(image, container_port, pull, volumes, env)
            } else {
                view! {
                    <TextField id="pservice-run" label="Command" placeholder="bun run start" mono=true wide=true
                        field_class="adi-field--grow"
                        hint="runs as sh -c with PORT injected" value=run />
                }.into_any()
            }}
            <TextField id="pservice-host" label="Host" placeholder="myapp.adi" mono=true
                hint="optional — routed by the front door" value=host />
            <TextField id="pservice-port" label="Host port" placeholder="auto" mono=true numeric=true
                hint="optional — auto-leased when empty" value=port />
            <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                "Add service"
            </button>
        </form>
    }
    .into_any()
}

/// The docker-only fields of the create form: image, the container port the host port maps to,
/// pull policy, and multi-line volumes / environment. Command overrides and raw `docker run`
/// flags are left to editing the `.adi/hive.yaml` directly (Files panel) — the quick form keeps
/// to the common case.
fn docker_fields(
    image: RwSignal<String>,
    container_port: RwSignal<String>,
    pull: RwSignal<String>,
    volumes: RwSignal<String>,
    env: RwSignal<String>,
) -> AnyView {
    view! {
        <TextField id="pservice-image" label="Image" placeholder="nginx:1.27" mono=true
            hint="the container image to run" value=image />
        <TextField id="pservice-cport" label="Container port" placeholder="8080" mono=true numeric=true
            hint="the port inside the container the host port maps to" value=container_port />
        <div class="adi-field">
            <label class="adi-field__label" for="pservice-pull">"Pull"</label>
            <select class="adi-input" id="pservice-pull"
                prop:value=move || pull.get()
                on:change=move |ev| pull.set(event_target_value(&ev))>
                <option value="">"— default —"</option>
                <option value="missing">"missing"</option>
                <option value="always">"always"</option>
                <option value="never">"never"</option>
            </select>
        </div>
        <div class="adi-field adi-field--grow">
            <label class="adi-field__label" for="pservice-volumes">"Volumes"</label>
            <textarea class="adi-input adi-input--wide adi-mono" id="pservice-volumes" rows="2"
                placeholder="./data:/data
cache:/var/cache" autocomplete="off"
                prop:value=move || volumes.get()
                on:input=move |ev| volumes.set(event_target_value(&ev))></textarea>
        </div>
        <div class="adi-field adi-field--grow">
            <label class="adi-field__label" for="pservice-env">"Environment"</label>
            <textarea class="adi-input adi-input--wide adi-mono" id="pservice-env" rows="2"
                placeholder="LOG_LEVEL=debug
CACHE=1" autocomplete="off"
                prop:value=move || env.get()
                on:input=move |ev| env.set(event_target_value(&ev))></textarea>
        </div>
    }
    .into_any()
}

/// Parse an optional port field: empty → `None`, a valid `u16` → `Some`, anything else → an
/// error message for the flash.
fn parse_opt_port(raw: &str) -> Result<Option<u16>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    raw.parse::<u16>().map(Some).map_err(|_| {
        "The port must be a number (1–65535), or empty for an auto-leased one.".to_string()
    })
}

/// Split a textarea into trimmed, non-empty lines (volumes / raw args).
fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Parse a `KEY=VALUE`-per-line textarea into an environment map; lines without a `=` or with a
/// blank key are skipped. The value keeps its own leading/trailing spaces only trimmed at the ends.
fn env_map(text: &str) -> std::collections::BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            let key = key.trim();
            (!key.is_empty()).then(|| (key.to_string(), value.trim().to_string()))
        })
        .collect()
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
