//! The Dashboards page: every dashboard under `~/.adi/mono/dashboards/`, with its frontend and
//! backend liveness, and what agents have authored into it.
//!
//! A dashboard's UI is loose `.ts` files — `frontend/modules/*.ts` panels and
//! `backend/routes/*.ts` endpoints — so the module and route lists, not the two fixed entry
//! points, are what actually tell you what a dashboard does. That is what this page surfaces.
//!
//! Every link out of this page goes to a dashboard's **host**, not to a port — see [`open_url`].
//! A dashboard is one origin (`docs/fleet.md` §4), and only its host routes the `/api` prefix its
//! page calls.
//!
//! It is also where a dashboard leaves this machine: [`transfer_panel`] sends one to a paired node
//! and, in move mode, stands the local copy down. That the destinations are a `<select>` of the
//! fleet rather than anything typed is the point — "run this in the cloud" should be picking a
//! name off a list, not learning a deployment.

use adi_webapp_api::types::{
    Dashboard, DashboardsState, NewDashboard, TransferDashboard, TransferMode,
};
use leptos::prelude::*;
use adi_ui::{EmptyRow, Row as TableRow, Table};
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::{DashboardsForm, Flash, State, load};
use crate::ui::{
    Key, TableState, TextField, apply_mutation, confirm, field_hint,
    flash_view, menu_item, row_actions, segmented, sort_rows, updated_text,
};

/// The columns shared by the live table and the archived disclosure — both render one dashboard
/// per row through [`row_view`], with a trailing archive/restore action.
pub(crate) const COLS: &[&str] = &[
    "Dashboard",
    "Project",
    "Frontend",
    "Backend",
    "Modules",
    "Routes",
    "",
];

/// The Dashboards page: summary tiles, one row per dashboard, the create form, and a collapsed
/// archive of removed dashboards at the foot.
pub(crate) fn dashboards_view(state: State, form: DashboardsForm) -> AnyView {
    let State {
        dashboards,
        secs_since,
        ..
    } = state;

    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <span class="adi-chip adi-mono" title="Live dashboards">
                    {move || dashboards.get().map_or_else(|| "\u{2014}".to_string(),
                        |d| d.dashboards.iter().filter(|x| !x.is_archived()).count().to_string())}
                </span>
                <span class="adi-updated">{move || updated_text(dashboards, secs_since)}</span>
            </div>

            <Table state=state.tables.dashboards>{move || rows_view(state, form, false)}</Table>
        </section>

        {transfer_panel(state, form)}

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"New dashboard"</h2>
            </div>

            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let name = form.name.get().trim().to_string();
                if name.is_empty() {
                    return;
                }
                let description = form.description.get().trim().to_string();
                form.busy.set(true);
                spawn_local(async move {
                    let body = NewDashboard {
                        name: name.clone(),
                        description: (!description.is_empty()).then_some(description),
                        // New dashboards start unfiled; file them from the row's Project picker.
                        project: None,
                    };
                    match fetch::create_dashboard(body).await {
                        Ok(d) => {
                            form.name.set(String::new());
                            form.description.set(String::new());
                            // The supervisor leases ports and starts both servers on its own
                            // within a few seconds; say so rather than showing a dead row.
                            state.flash.set(Some(Flash::ok(format!(
                                "Created “{}” ({}). Starting — ports appear in a few seconds.",
                                d.name,
                                short_id(&d.id),
                            ))));
                            load(state).await;
                        }
                        Err(e) => state.flash.set(Some(Flash::err(e))),
                    }
                    form.busy.set(false);
                });
            }>
                <TextField id="dash-name" label="Name" placeholder="Metrics" value=form.name />
                <TextField id="dash-desc" label="Description" placeholder="What it is for"
                    value=form.description wide=true field_class="adi-field--grow" />
                <button class="adi-btn adi-btn--primary" type="submit"
                    prop:disabled=move || form.busy.get()>
                    "New dashboard"
                </button>
            </form>
            {flash_view(state.flash)}

        </section>

        {archived_section(state, form)}
    }
    .into_any()
}

// MARK: transferring a dashboard to a node (`docs/fleet.md` §10)

/// The transfer panel: pick a node, say copy or move, type the node's password, send.
///
/// It appears only once a row's **Transfer** has named a dashboard, and it names that dashboard in
/// its own title. A panel rather than a chain of prompts because a transfer has three answers to
/// give at once, and two of them — move-or-copy, and whether to delete afterwards — are the kind a
/// person wants to see beside each other before committing.
///
/// The password field is deliberately empty every time. This machine keeps a *verifier* for each
/// node's credential and not the credential (`docs/fleet.md` §8); nothing here is stored, so
/// nothing here can be pre-filled.
fn transfer_panel(state: State, form: DashboardsForm) -> AnyView {
    view! {
        {move || {
            // Only `transfer_id` is read out here, so the panel is built when it opens and torn
            // down when it closes — and never in between. The listing polls every few seconds; a
            // panel rebuilt on that tick would drop whatever was half-typed into the password
            // field, which is the one input on this page that cannot be recovered from anywhere.
            if form.transfer_id.get().is_empty() {
                return ().into_any();
            }
            view! {
                <section class="adi-panel">
                    <div class="adi-panel__head">
                        <h2 class="adi-panel__title">{move || transfer_title(state, form)}</h2>
                        <button class="adi-btn adi-btn--link" type="button"
                            on:click=move |_| close_transfer(form)>"Cancel"</button>
                    </div>
                    <form class="adi-form" on:submit=move |ev| {
                        ev.prevent_default();
                        submit_transfer(state, form);
                    }>
                        {transfer_node_picker(state, form)}
                        <div class="adi-field">
                            <label class="adi-field__label" for="dash-transfer-mode">"Mode"</label>
                            <div id="dash-transfer-mode">
                                {segmented("Transfer mode", form.transfer_move, "Copy", "Move")}
                            </div>
                            {field_hint(move || if form.transfer_move.get() {
                                "the local one is archived once the node has it"
                            } else {
                                "both machines run it afterwards"
                            })}
                        </div>
                        {transfer_password_field(form)}
                        {move || form.transfer_move.get().then(|| view! {
                            <label class="adi-field adi-field--check">
                                <input type="checkbox"
                                    prop:checked=move || form.transfer_delete.get()
                                    on:change=move |ev| form.transfer_delete
                                        .set(event_target_checked(&ev)) />
                                <span class="adi-field__label">"Delete the local copy"</span>
                            </label>
                        })}
                        <button class="adi-btn adi-btn--primary" type="submit"
                            prop:disabled=move || form.transfer_busy.get()>
                            {move || if form.transfer_busy.get() { "Transferring\u{2026}" }
                                else { "Transfer" }}
                        </button>
                    </form>
                    {transfer_warning(form)}
                </section>
            }
            .into_any()
        }}
    }
    .into_any()
}

/// The panel's title, in its own reactive scope so the listing's poll can rename the heading
/// without rebuilding the form under it.
///
/// The row may have been archived, deleted or renamed since Transfer was clicked, so the listing
/// is the truth; the short id is the fallback when it no longer names anything.
fn transfer_title(state: State, form: DashboardsForm) -> String {
    let id = form.transfer_id.get();
    let name = state
        .dashboards
        .get()
        .and_then(|d| d.dashboards.into_iter().find(|d| d.id == id).map(|d| d.name))
        .unwrap_or_else(|| short_id(&id));
    format!("Transfer \u{201c}{name}\u{201d}")
}

/// Which node the dashboard is going to. A `<select>` of the fleet, because every valid answer is
/// already known here and a typed petname would only ever be a 404 to decode.
///
/// With nothing paired there is nothing to pick, so the picker says how a node joins instead —
/// that is the missing step, not a missing selection.
fn transfer_node_picker(state: State, form: DashboardsForm) -> AnyView {
    view! {
        <div class="adi-field">
            <label class="adi-field__label" for="dash-transfer-node">"Node"</label>
            <select class="adi-input" id="dash-transfer-node"
                on:change=move |ev| form.transfer_node.set(event_target_value(&ev))>
                <option value="" selected=move || form.transfer_node.get().is_empty()>
                    "\u{2014} pick a node \u{2014}"
                </option>
                {move || {
                    let current = form.transfer_node.get();
                    state.fleet.get().map(|f| f.nodes.into_iter().map(|n| {
                        let (petname, selected) = (n.petname.clone(), n.petname == current);
                        view! { <option value=petname selected=selected>{n.petname}</option> }
                    }).collect::<Vec<_>>())
                }}
            </select>
            {move || state.fleet.get()
                .is_some_and(|f| f.nodes.is_empty())
                .then(|| field_hint("no nodes paired yet \u{2014} run `adi-mono mesh invite` here, \
                                     then `mesh join` on the machine you want to deploy to"))}
        </div>
    }
    .into_any()
}

/// The node's Basic-auth password. Hand-written rather than a [`TextField`] because it is the one
/// input on this page that must not be a readable `type="text"`, and it must never be offered back
/// by the browser's autofill — it is not this machine's secret to remember.
fn transfer_password_field(form: DashboardsForm) -> AnyView {
    view! {
        <div class="adi-field">
            <label class="adi-field__label" for="dash-transfer-pw">"Node password"</label>
            <input class="adi-input" id="dash-transfer-pw" type="password"
                autocomplete="new-password" placeholder="printed once, on the node"
                prop:value=move || form.transfer_password.get()
                on:input=move |ev| form.transfer_password.set(event_target_value(&ev)) />
            {field_hint("used for this transfer only \u{2014} it is stored nowhere")}
        </div>
    }
    .into_any()
}

/// The one consequence worth spelling out before the button is pressed: deleting the local copy
/// leaves the node's as the only one. Shown only when both boxes that make it true are set.
fn transfer_warning(form: DashboardsForm) -> AnyView {
    view! {
        {move || (form.transfer_move.get() && form.transfer_delete.get()).then(|| view! {
            <p class="adi-muted">
                "The local directory is removed once the node confirms it has the files. \
                 The node's copy is then the only one."
            </p>
        })}
    }
    .into_any()
}

/// Open the transfer panel for one dashboard, starting from a clean form: the previous transfer's
/// node and password must never carry over into this one.
fn start_transfer(form: DashboardsForm, id: &str) {
    form.transfer_id.set(id.to_string());
    form.transfer_node.set(String::new());
    form.transfer_move.set(false);
    form.transfer_delete.set(false);
    form.transfer_password.set(String::new());
}

/// Close the panel and drop what was typed into it — in particular the password, which has no
/// reason to outlive the form it was typed into.
fn close_transfer(form: DashboardsForm) {
    form.transfer_id.set(String::new());
    form.transfer_password.set(String::new());
}

/// Validate the form, post the transfer, and report where the dashboard ended up.
///
/// Success folds the returned local listing into the page — a move archives the row, so the table
/// has to change with it — and flashes the mesh URL the copy now answers on. A transfer that
/// worked but could not grant this machine access says so rather than handing over a link that
/// will refuse.
fn submit_transfer(state: State, form: DashboardsForm) {
    let (id, node) = (form.transfer_id.get(), form.transfer_node.get());
    if id.is_empty() {
        return;
    }
    if node.is_empty() {
        state.flash.set(Some(Flash::err("Pick a node to transfer to.".to_string())));
        return;
    }
    let password = form.transfer_password.get();
    if password.is_empty() {
        state.flash.set(Some(Flash::err(format!(
            "{node} needs its password before it will accept a dashboard."
        ))));
        return;
    }
    let moving = form.transfer_move.get();
    let delete_local = moving && form.transfer_delete.get();
    if delete_local
        && !confirm(&format!(
            "Move this dashboard to “{node}” and delete the local copy? Once {node} has it, the \
             local directory and all of its files are removed."
        ))
    {
        return;
    }

    let body = TransferDashboard {
        id,
        node: node.clone(),
        mode: if moving { TransferMode::Move } else { TransferMode::Copy },
        delete_local,
        // The user pairing mints; the server fills it in when absent.
        username: None,
        password,
    };
    form.transfer_busy.set(true);
    spawn_local(async move {
        match fetch::transfer_dashboard(body).await {
            Ok(done) => {
                let message = transferred_message(&done, moving);
                state.dashboards.set(Some(done.dashboards));
                close_transfer(form);
                state.flash.set(Some(Flash::ok(message)));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        form.transfer_busy.set(false);
    });
}

/// What to say once a transfer has landed: where it went, where to open it, and — when the node
/// would not grant this machine access — why that link does not work yet.
fn transferred_message(done: &adi_webapp_api::types::DashboardTransferred, moving: bool) -> String {
    let verb = if moving { "Moved" } else { "Copied" };
    let node = &done.node;
    let Some(url) = done.url.as_deref() else {
        return format!(
            "{verb} \u{201c}{}\u{201d} to {node}. It has no routable name there yet.",
            done.dashboard.name,
        );
    };
    if done.granted {
        format!(
            "{verb} \u{201c}{}\u{201d} to {node} \u{2014} it is at {url} (starting; give the \
             node's supervisor a few seconds).",
            done.dashboard.name,
        )
    } else {
        format!(
            "{verb} \u{201c}{}\u{201d} to {node}. {url} will refuse until {node} grants this \
             machine http:{} \u{2014} add it from {node}'s own Fleet page.",
            done.dashboard.name,
            // The grant names the host minus its local zone, so it matches what the node parses.
            done.dashboard
                .host
                .as_deref()
                .and_then(|host| host.trim_end_matches('.').strip_suffix(".adi"))
                .unwrap_or("<service>"),
        )
    }
}

/// The archive: its own collapsed panel at the foot of the page, with a caret header and a count.
/// Expanding reveals archived dashboards so they can be restored. Renders nothing at all when
/// nothing is archived. Mirrors the Projects page's archive.
fn archived_section(state: State, form: DashboardsForm) -> AnyView {
    let show = form.show_archived;
    view! {
        {move || {
            let n = state.dashboards.get().map_or(0,
                |d| d.dashboards.iter().filter(|x| x.is_archived()).count());
            (n > 0).then(|| {
                let open = show.get();
                view! {
                    <section class="adi-panel">
                        <div class="adi-panel__head">
                            <button class="adi-btn adi-btn--link" type="button"
                                aria-expanded=open.to_string()
                                on:click=move |_| show.update(|v| *v = !*v)>
                                {if open { "\u{25be}" } else { "\u{25b8}" }}" Archived"
                            </button>
                            <span class="adi-chip adi-mono">{n.to_string()}</span>
                        </div>
                        {open.then(|| view! { <Table state=state.tables.dashboards_archived>{move || rows_view(state, form, true)}</Table> }.into_any())}
                    </section>
                }
                .into_any()
            })
        }}
    }
    .into_any()
}

/// Render a table body — the live dashboards (`archived = false`) or the archived ones — as a
/// loading/empty placeholder or one row per matching dashboard.
fn rows_view(state: State, form: DashboardsForm, archived: bool) -> AnyView {
    let table: TableState = if archived {
        state.tables.dashboards_archived
    } else {
        state.tables.dashboards
    };
    let Some(loaded) = state.dashboards.get() else {
        return view! { <EmptyRow state=table>"Loading…"</EmptyRow> }.into_any();
    };
    let mut rows: Vec<Dashboard> = loaded
        .dashboards
        .into_iter()
        .filter(|d| d.is_archived() == archived)
        .collect();
    if rows.is_empty() {
        return view! { <EmptyRow state=table>{if archived {
                "Nothing archived."
            } else {
                "No dashboards yet — create one under ~/.adi/mono/dashboards/."
            }}</EmptyRow> }.into_any();
    }
    // A dead service sorts below every live one at the same port, so a descending Frontend sort
    // answers "what is actually up?" rather than interleaving the stopped ones.
    let service_key = |port: Option<u16>, running: bool| {
        Key::Int(i64::from(port.unwrap_or(0)) + if running { 1 << 20 } else { 0 })
    };
    // The Project cell shows a name, so the column orders by the name, not the id behind it.
    let names: std::collections::HashMap<String, String> = state
        .projects
        .get()
        .map(|p| p.projects.into_iter().map(|p| (p.id, p.name)).collect())
        .unwrap_or_default();
    sort_rows(
        &mut rows,
        table.sort.get(),
        |d, col| match col {
            "Project" => Key::maybe(d.project.as_deref().and_then(|id| names.get(id)).map(String::as_str)),
            "Frontend" => service_key(d.frontend_port, d.frontend_running),
            "Backend" => service_key(d.backend_port, d.backend_running),
            "Modules" => Key::count(d.modules.len()),
            "Routes" => Key::count(d.routes.len()),
            _ => Key::text(&d.name),
        },
        |d| Key::text(&d.name),
    );
    rows.into_iter()
        .map(|d| {
            let action = row_action(state, form, &d.id, d.is_archived());
            view! {
                <TableRow
                    state=table
                    cell=move |col| cell(col, &d, state)
                    actions=action
                />
            }
            .into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// One dashboard's cell under `col`: identity, both services' state, and what it serves. Matching
/// the header text — the same key the sort uses — is what lets the user hide and reorder columns
/// without the row builder knowing about it.
///
/// The name is the primary way in: while the dashboard is up it is a real link to the running
/// page, so the row can be verified by clicking rather than by reading a port number.
fn cell(col: &str, d: &Dashboard, state: State) -> AnyView {
    match col {
        "Project" => view! { <span>{project_cell(state, d)}</span> }.into_any(),
        // Only the frontend is a link — the backend serves JSON to the page, not the reader.
        "Frontend" => {
            view! { <span>{service_cell(d.frontend_port, d.frontend_running, open_url(d))}</span> }
                .into_any()
        }
        "Backend" => {
            view! { <span>{service_cell(d.backend_port, d.backend_running, None)}</span> }.into_any()
        }
        "Modules" => view! { <span class="font-mono">{summarize(&d.modules)}</span> }.into_any(),
        "Routes" => view! { <span class="font-mono">{summarize(&d.routes)}</span> }.into_any(),
        // "Dashboard", and anything the layout offers that this match doesn't name.
        _ => {
            let name = match open_url(d) {
                Some(href) => view! {
                    <a href=href.clone() target="_blank" rel="noreferrer" title=href>
                        {d.name.clone()}
                    </a>
                }
                .into_any(),
                // Nothing is listening, so a link would only fail — show the name plainly.
                None => view! { <span>{d.name.clone()}</span> }.into_any(),
            };
            view! {
                <span>
                    <div>{name}</div>
                    <div class="adi-mono adi-muted" title=d.id.clone()>{short_id(&d.id)}</div>
                    {moved_marker(d)}
                    {no_host_hint(d)}
                </span>
            }
            .into_any()
        }
    }
}

/// Where to actually open a dashboard, or `None` when nothing is up to open.
///
/// **Its host, whenever it has one** (`docs/fleet.md` §4). A dashboard is one origin: the page its
/// frontend serves calls the backend at `/api` on that same host, and only the front door routes
/// that prefix. So `http://127.0.0.1:<frontend port>` yields a page that loads and then 404s every
/// request it makes — it is the fallback for a dashboard with no routable name, never the default.
///
/// That host is put through [`origin::service_url`](crate::origin::service_url), because it is the
/// name *the node* routes: this panel is the same page whether it is read at `app.adi` or at
/// `app.<node>.n.adi`, and the second reader is on a different machine, where a bare `nosh.adi`
/// means their own front door. Which also disqualifies the loopback fallback there — over the mesh
/// `127.0.0.1` is the viewer's machine, so a dashboard with no routable name has no address at all
/// from where they are sitting.
pub(crate) fn open_url(d: &Dashboard) -> Option<String> {
    if !d.frontend_running {
        return None;
    }
    if let Some(host) = routable_host(d) {
        return crate::origin::service_url(host);
    }
    if crate::origin::viewing_node().is_some() {
        return None;
    }
    d.frontend_port.map(|port| format!("http://127.0.0.1:{port}"))
}

/// The dashboard's hostname, with a blank one read as absent: a hand-edited hive file can carry a
/// `host:` with nothing after it, and `http:///` is worse than no link at all.
fn routable_host(d: &Dashboard) -> Option<&str> {
    d.host.as_deref().map(str::trim).filter(|h| !h.is_empty())
}

/// The note under a dashboard that was moved to a node: where it went, and a link straight to it
/// there. Without this the row is simply an archived dashboard, and "why did this stop?" is a
/// question the page can answer but doesn't.
///
/// The link is the same `<service>.<node>.n.adi` name the transfer reported. It only resolves while
/// the node is reachable and has granted this machine the dashboard — the transfer asks for that
/// grant, and says so when it could not get it. It is also a name only *this* machine holds, so
/// read through a node there is no link at all and the row says only where the dashboard went.
fn moved_marker(d: &Dashboard) -> AnyView {
    let Some(node) = d.moved_to.clone() else {
        return ().into_any();
    };
    // The host as the node routes it, minus its local zone: `app.nosh.adi` moves as `app.nosh`,
    // not as `app`, which over there would be a different service or none at all.
    let there = routable_host(d)
        .and_then(|host| host.trim_end_matches('.').strip_suffix(".adi"))
        .filter(|service| !service.is_empty())
        .and_then(|service| crate::origin::service_url(&format!("{service}.{node}.n.adi")));
    view! {
        <div class="adi-muted">
            "moved to "
            {match there {
                Some(href) => view! {
                    <a href=href.clone() target="_blank" rel="noreferrer" title=href>
                        {node.clone()}
                    </a>
                }
                .into_any(),
                None => view! { <span class="adi-mono">{node.clone()}</span> }.into_any(),
            }}
        </div>
    }
    .into_any()
}

/// The note under a dashboard that declares no `proxy.host`: it is reachable only on loopback, so
/// the link above it bypasses the front door and the page's `/api` calls will not route. Said out
/// loud rather than left as a link that half-works — the page renders, and only then falls over.
fn no_host_hint(d: &Dashboard) -> AnyView {
    if routable_host(d).is_some() {
        return ().into_any();
    }
    view! {
        <div class="adi-muted"
            title="Give both of its services the same proxy.host in .adi/hive.yaml, and its \
                   /api calls route through the front door.">
            "no host yet — /api will not route"
        </div>
    }
    .into_any()
}

/// The Project cell: a compact picker filing this dashboard under a project (or none). Choosing an
/// option posts the change and folds the fresh listing back in, so the grouping updates at once.
fn project_cell(state: State, d: &Dashboard) -> AnyView {
    let id = d.id.clone();
    let current = d.project.clone().unwrap_or_default();
    let cur_none = current.is_empty();
    let options = state
        .projects
        .get()
        .map(|p| p.projects)
        .unwrap_or_default()
        .into_iter()
        .filter(|p| !p.is_archived())
        .map(|p| {
            let selected = p.id == current;
            view! { <option value=p.id.clone() selected=selected>{p.name}</option> }
        })
        .collect::<Vec<_>>();
    view! {
        <select class="adi-input adi-dash__proj"
            on:change=move |ev| {
                let val = event_target_value(&ev);
                apply_dashboards(state, "Filed dashboard.".to_string(),
                    fetch::set_dashboard_project(id.clone(), (!val.is_empty()).then_some(val)));
            }>
            <option value="" selected=cur_none>"\u{2014} none \u{2014}"</option>
            {options}
        </select>
    }
    .into_any()
}

/// The trailing actions for a dashboard row.
///
/// While live: **Transfer** and **Archive**, both inline. Transfer is one click from the row it
/// acts on — the whole point of the feature is that running a dashboard somewhere else is a
/// choice you make in passing, not a procedure — and it only opens the panel below, so nothing
/// leaves this machine until a node and a password have been given.
///
/// While archived: **Restore** inline, with Transfer and Delete in the kebab. An archived
/// dashboard still has all its files, so sending it to a node is a real thing to want — it is how
/// one that was moved away comes back up on a *different* node.
fn row_action(state: State, form: DashboardsForm, id: &str, archived: bool) -> AnyView {
    let id = id.to_string();
    let short = short_id(&id);
    let key = format!("dashboard:{id}");
    let transfer_id = id.clone();
    if archived {
        let del_id = id.clone();
        let del_short = short.clone();
        let restore = view! {
            <button class="adi-btn adi-btn--link" on:click=move |_| {
                apply_dashboards(state, format!("Restored {short}."),
                    fetch::unarchive_dashboard(id.clone()));
            }>"Restore"</button>
        };
        let transfer = menu_item(state, "Transfer to a node\u{2026}", false, move || {
            start_transfer(form, &transfer_id);
        });
        let delete = menu_item(state, "Delete", true, move || {
            if !confirm(&format!(
                "Permanently delete dashboard {del_short}? This removes all of its files \
                 and cannot be undone.")) {
                return;
            }
            apply_dashboards(state, format!("Deleted {del_short}."),
                fetch::delete_dashboard(del_id.clone()));
        });
        row_actions(state, key, restore, vec![transfer, delete])
    } else {
        let inline = view! {
            <button class="adi-btn adi-btn--link" type="button"
                title="Run this dashboard on a paired node \u{2014} a copy, or a move"
                on:click=move |_| start_transfer(form, &transfer_id)>"Transfer"</button>
            <button class="adi-btn adi-btn--link" on:click=move |_| {
                apply_dashboards(state, format!("Archived {short}."),
                    fetch::archive_dashboard(id.clone()));
            }>"Archive"</button>
        };
        row_actions(state, key, inline, Vec::new())
    }
}

/// Run a dashboards mutation: fold the returned state into the page and flash success, or flash
/// the error. A thin typed wrapper over [`apply_mutation`], as `apply_projects` is for projects.
fn apply_dashboards<F>(state: State, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<DashboardsState, String>> + 'static,
{
    apply_mutation(state, None, ok_msg, |s, d| s.dashboards.set(Some(d)), fut);
}

/// A service cell: the running led plus the loopback port the ports manager leased, or a note when
/// nothing is leased yet. The port is a readout of what the supervisor allocated, no longer the
/// address — `href` (from [`open_url`], so the dashboard's host when it has one) is what the cell
/// opens, and only the frontend is ever given one.
fn service_cell(port: Option<u16>, running: bool, href: Option<String>) -> AnyView {
    let Some(port) = port else {
        return view! { <span class="adi-muted">"not allocated"</span> }.into_any();
    };
    let (state_attr, label) = if running {
        ("online", format!(":{port}"))
    } else {
        ("down", format!(":{port} down"))
    };
    let body = if let Some(href) = href {
        view! {
            <a class="adi-mono" href=href.clone() target="_blank"
                rel="noreferrer" title=href>{label}</a>
        }
        .into_any()
    } else {
        view! { <span class="adi-mono">{label}</span> }.into_any()
    };
    view! {
        <span class="adi-status" data-state=state_attr>
            <span class="adi-status__led"></span>{body}
        </span>
    }
    .into_any()
}

/// The leading segment of a uuid — enough to recognize a dashboard without filling the column.
fn short_id(id: &str) -> String {
    id.split('-').next().unwrap_or(id).to_string()
}

/// Name the entries when there are few, else just count them — a dashboard an agent has been
/// working on for a while can have far more than fit in a cell.
fn summarize(items: &[String]) -> String {
    match items.len() {
        0 => "—".to_string(),
        1..=3 => items.join(", "),
        n => format!("{}, +{}", items[..2].join(", "), n - 2),
    }
}
