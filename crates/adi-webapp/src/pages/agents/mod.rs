//! The Agents page: the list of agent definitions (docs/adi-agents.md §5), with delete and the
//! launch actions on each row. ▶ Run starts either an interactive pty session or a headless
//! background process; deeper orchestration is future work.
//!
//! Writing a definition — a backend (`executor:what`), a system prompt, a CLI command scope, and
//! the backend-specific params — happens in [`agent_detail_view`], on the agent's own page: New
//! agent and Edit navigate there rather than filling a form under the list. The form adapts its
//! params to the chosen backend, and for the `harness:adi` backend also to its chosen provider.

use std::collections::BTreeMap;

use adi_ui::{Row as TableRow, Table};
use adi_webapp_api::types::{AgentDto, SaveAgent, SecretDto, SecretRef, ToolDto};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::routing::{
    Route, agent_form_path, push_state, replace_state, scroll_top, spa_click, spa_nav,
};
use crate::state::{AgentsForm, AgentsWatch, Flash, Simulate, State};
use crate::ui::{
    Key, flash_view, menu_item, row_actions, rows_or_placeholder, sort_rows, updated_text,
};

/// The Agents page's columns; the trailing blank one holds Run / View / Stop and the kebab.
pub(crate) const COLS: &[&str] = &["Name", "Backend", "Model", "Project", "Tags", ""];

mod actions;
mod form;
mod simulate;

use actions::apply_agents;
pub(crate) use actions::{
    CHAT_COLS, CHAT_RUN_COLS, NEWEST_FIRST, RUN_COLS, adopt_run_settings, agent_actions,
    all_chats_view, chat_home_view, live_view, poll_watch, project_run_limit_view, reset_chat_home,
    run_limit_view,
};
// The onboarding wizard renders the same fields from the same schema, so its half of the form
// lives here rather than as a second copy of these renderers.
pub(crate) use form::{
    agent_argument_values, agent_environment_fields, agent_param_applies, agent_schema_fields,
    load_agent_into_form, parsed_env_vars, parsed_path_dirs, parsed_prelude, set_agent_field_value,
};
use form::{agent_form_fields, clear_agent_form};

/// The Agents page: every agent defined on this machine, with the live view above it and the
/// per-row launch/delete actions. Editing one, or writing a new one, opens [`agent_detail_view`]
/// on that agent's own URL — the definition form is not part of this page.
pub(crate) fn agents_view(
    state: State,
    form: AgentsForm,
    watch: AgentsWatch,
    sim: Simulate,
    route: RwSignal<Route>,
) -> AnyView {
    let agents = state.agents;
    let secs_since = state.secs_since;
    view! {
        // Above everything, when it is open: taking the model's seat is the whole screen, not a
        // panel beside the list of agents you could take it in.
        {move || simulate::simulate_view(state, sim)}

        {move || live_view(state, watch)}

        <section class="adi-panel">
            <div class="adi-panel__head">
                <span class="adi-chip adi-mono" title="Agents defined">
                    {move || agents.get().map_or_else(|| "\u{2014}".to_string(),
                        |a| a.agents.len().to_string())}
                </span>
                {run_limit_view(state)}
                <span class="adi-spacer"></span>
                <span class="adi-updated">{move || updated_text(agents, secs_since)}</span>
                <a class="adi-btn adi-btn--primary" href=agent_form_path("")
                    on:click=move |ev| if spa_nav(&ev) {
                        open_agent_editor(state, route, form, None);
                    }>
                    "New agent"
                </a>
            </div>

            <Table state=state.tables.agents>{move || agent_rows(state, form, watch, sim, route)}</Table>
            {flash_view(state.flash)}
            // What the row actions do. It sat under the create form before that moved to a page
            // of its own; it belongs with the buttons it explains, which are in the table above.
            <div class="adi-hint">
                "▶ Run launches pty backends in an interactive " <code>"adi-agent-<name>"</code>
                " session you type into. A "<code>"process"</code>" backend is a template: ▶ Run…
                 starts an independent one-shot run from a task you give it — one "<code>"--print"</code>
                " turn, never continued — and several can run at once. A "<code>"harness"</code>"
                 backend instead starts an answerable "<strong>"conversation"</strong>": ▶ Chat…
                 sends a first message, the agent answers, and you reply to continue the same thread.
                 Each run/conversation keeps its own log + transcript under "
                <code>"~/.adi/mono/sessions/{process,harness}/<agent>/"</code>
                ", browsable as history in ● View."
            </div>
        </section>
    }
    .into_any()
}

/// One agent's editor, on a page of its own (`/agents/new`, `/agents/<name>/edit`) rather than
/// under the list: the whole schema-driven definition form — backend params, tools, knowledge,
/// secrets, run environment — for the agent named by [`State::current_agent`], or a blank one.
///
/// The form's signals are the same [`AgentsForm`] the list page used to hold, so what fills it is
/// unchanged: Edit loads the agent before navigating, and [`main`] covers the arrive-directly case.
pub(crate) fn agent_detail_view(state: State, form: AgentsForm, route: RwSignal<Route>) -> AnyView {
    let agents = state.agents;
    let flash = state.flash;
    let AgentsForm {
        name,
        backend,
        project,
        tags,
        starred,
        arguments,
        argument_values,
        editing,
        busy,
        ..
    } = form;
    // A URL naming an agent the list doesn't have: a stale link, or one deleted from under this
    // tab. Say so rather than opening a create form prefilled with nothing, which would silently
    // turn "edit" into "make a second agent".
    let missing = move || {
        let open = state.current_agent.get();
        !open.is_empty()
            && agents
                .get()
                .is_some_and(|s| !s.agents.iter().any(|a| a.name == open))
    };
    view! {
        // Its own head, the way a project's detail page has one: what is being edited, and the
        // way back up to the list — laid out as `parent_link` lays out a sub-project's.
        <div class="adi-bar">
            <h1 class="adi-bar__title">
                {move || match editing.get() {
                    Some(n) => n,
                    None => "New agent".to_string(),
                }}
            </h1>
            <a class="adi-btn adi-btn--link" href=Route::Agents.path() title="back to every agent"
                on:click=move |ev| spa_click(&ev, route, Route::Agents)>"\u{2191} Agents"</a>
        </div>

        {move || missing().then(|| view! {
            <section class="adi-panel">
                <div class="adi-empty">
                    {format!("No agent named “{}” — it may have been deleted or renamed.",
                        state.current_agent.get())}
                </div>
            </section>
        })}

        // The form stays mounted either way: rebuilding it whenever the polled agents list lands
        // would drop focus mid-edit, and a form still holding a deleted agent is how you put it
        // back.
        <section class="adi-panel">
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let nm = name.get().trim().to_string();
                if nm.is_empty() {
                    flash.set(Some(Flash::err("An agent name is required.".to_string())));
                    return;
                }
                let be = backend.get();
                if be.trim().is_empty() {
                    flash.set(Some(Flash::err("Pick a backend.".to_string())));
                    return;
                }
                let spec = agents.get().map(|st| st.form);
                let prov = argument_values.get().get("provider").cloned().unwrap_or_default();
                // Whether each backend-conditional first-class param applies is driven by the
                // server schema (does a field of that name apply to this backend?), so rescoping a
                // field in the API also stops its value being sent for backends it no longer fits.
                let pm_applies = agent_param_applies(spec.as_ref(), &be, &prov, "permission_mode");
                let temp_applies = agent_param_applies(spec.as_ref(), &be, &prov, "temperature");
                let body = SaveAgent {
                    name: nm.clone(),
                    backend: be.clone(),
                    arguments: agent_argument_values(
                        spec.as_ref(),
                        &be,
                        arguments.get(),
                        argument_values.get(),
                        form,
                        pm_applies,
                        temp_applies,
                    ),
                    // This form owns the tags, the star, and the project, so it states all three
                    // even when they are empty — `Some(empty)` clears, where the `None` the other
                    // forms send means "leave as is". A blank project is how "global" is said.
                    tags: Some(tags.get().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()),
                    starred: Some(starred.get()),
                    project: Some(project.get()),
                    // The adi tools ticked on for this agent — its own `.bin` at launch. This is
                    // the form that owns the checkboxes, so it states the set even when empty.
                    bin_tools: Some(form.bin_tools.get().into_iter().collect()),
                    // The secrets ticked on for this agent — injected into its runs as env vars.
                    secrets: Some(form.secrets.get().into_iter()
                        .map(|(project, name)| SecretRef { project, name })
                        .collect()),
                    // The knowledge bases ticked on, and whether this agent keeps its own memory.
                    // This is the form that owns both checkboxes, so it states them even when
                    // empty — the other forms send `None`, which leaves them as they are.
                    knowledge: Some(form.knowledge.get().into_iter().collect()),
                    memory: Some(form.memory.get()),
                    // This is the one form that edits the run environment, so it always states it —
                    // `Some(empty)` clears, where the `None` other forms send means "leave as is".
                    prelude: Some(parsed_prelude(&form.prelude.get())),
                    path: Some(parsed_path_dirs(&form.path.get())),
                    env: Some(parsed_env_vars(&form.env.get())),
                    // …and the same for whether this agent may stop and wait for somebody.
                    unattended: Some(form.unattended.get()),
                    // Editing with the name field changed is a rename, not a second agent.
                    rename_from: editing.get(),
                };
                // Optimistic, like `editing` beside it: a create (or a rename) moves this page to
                // the saved agent's own URL, so a refresh lands back on what is in the form rather
                // than on `new` or on the name it used to have.
                editing.set(Some(nm.clone()));
                state.current_agent.set(nm.clone());
                replace_state(&agent_form_path(&nm));
                apply_agents(state, Some(busy), format!("Saved agent “{nm}”."), fetch::save_agent(body));
            }>
                {move || agent_form_fields(state, form)}
                {move || agent_tool_checkboxes(state, form)}
                {move || agent_knowledge_checkboxes(form)}
                {move || agent_secret_checkboxes(state, form)}
                {move || agent_environment_fields(form)}
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    {move || if editing.get().is_some() { "Update agent" } else { "Create agent" }}
                </button>
            </form>
            {flash_view(flash)}
        </section>
    }
    .into_any()
}

/// Open an agent's editor on its own page: load `agent` into the form — or clear it, for a
/// definition that doesn't exist yet — then navigate to `/agents/<name>/edit` (or `/agents/new`).
///
/// Loading before navigating is what makes the editor render filled on the first paint. A project's
/// Agents panel lands here too, so Edit means the same thing wherever it is clicked.
pub(crate) fn open_agent_editor(
    state: State,
    route: RwSignal<Route>,
    form: AgentsForm,
    agent: Option<&AgentDto>,
) {
    match agent {
        Some(a) => load_agent_into_form(form, a),
        None => clear_agent_form(form),
    }
    let name = agent.map(|a| a.name.clone()).unwrap_or_default();
    state.current_agent.set(name.clone());
    push_state(&agent_form_path(&name));
    route.set(Route::AgentDetail);
    scroll_top();
}

/// The per-agent tool checkboxes: one toggle per registered (active) tool — system or user — that
/// adds/removes its id from the agent's enabled set (`bin_tools`). The ticked tools become shims in
/// the agent's own `.bin`, on its PATH at launch. This is the "functionality enabled for this agent"
/// control: nothing is auto-added, including the system tools — an agent gets a tool only when it's
/// ticked on here.
fn agent_tool_checkboxes(state: State, form: AgentsForm) -> AnyView {
    let Some(st) = state.tools.get() else {
        return view! {
            <div class="adi-field adi-field--grow">
                <label class="adi-field__label">"Tools"</label>
                <div class="adi-muted">"Loading tools…"</div>
            </div>
        }
        .into_any();
    };
    let mut tools: Vec<ToolDto> = st.tools.into_iter().filter(|t| !t.is_archived()).collect();
    // System tools first, then by name — the platform CLIs group together.
    tools.sort_by(|a, b| b.system.cmp(&a.system).then_with(|| a.name.cmp(&b.name)));
    if tools.is_empty() {
        return view! {
            <div class="adi-field adi-field--grow">
                <label class="adi-field__label">"Tools"</label>
                <div class="adi-hint">
                    "No tools yet — create one on the Tools page. Whatever you enable here lands "
                    "in this agent's " <code>".bin"</code> "."
                </div>
            </div>
        }
        .into_any();
    }
    let boxes = tools
        .into_iter()
        .map(|t| {
            let id = t.id.clone();
            let id_checked = id.clone();
            let id_toggle = id.clone();
            let checked = move || form.bin_tools.get().contains(&id_checked);
            let label = if t.system {
                format!("{} (system)", t.name)
            } else if let Some(p) = t.project.as_deref().filter(|p| !p.trim().is_empty()) {
                format!("{} · {p}", t.name)
            } else {
                t.name.clone()
            };
            let title = t.description.clone().unwrap_or_default();
            view! {
                <label class="adi-check" title=title
                    style="display:inline-flex; align-items:center; gap:var(--space-1); margin:0 var(--space-3) var(--space-1) 0">
                    <input type="checkbox" prop:checked=checked
                        on:change=move |ev| {
                            let on = event_target_checked(&ev);
                            form.bin_tools.update(|set| {
                                if on { set.insert(id_toggle.clone()); } else { set.remove(&id_toggle); }
                            });
                        } />
                    <span class="adi-mono">{label}</span>
                </label>
            }
        })
        .collect::<Vec<_>>();
    view! {
        <div class="adi-field adi-field--grow">
            <label class="adi-field__label">"Tools (this agent's .bin)"</label>
            <div style="display:flex; flex-wrap:wrap; align-items:center; padding:var(--space-1) 0">
                {boxes}
            </div>
            <div class="adi-hint">
                "Each one is asked " <code>"llm help"</code> " (then " <code>"help"</code> ", then "
                <code>"--help"</code> ") at launch, and what it answers is appended to this agent's "
                "system prompt — so it knows what the commands are without being told twice."
            </div>
        </div>
    }
    .into_any()
}

/// The per-agent secret checkboxes: one toggle per registered secret (across every scope) that
/// adds/removes its `(scope, name)` reference from the agent's attachment set. Only the ticked
/// secrets are decrypted and injected into the agent's runs as environment variables — an explicit
/// allowlist, so nothing is inherited from a scope for merely existing. Populated from the shared
/// secrets list (`state.secrets`), which carries metadata only (never a value).
/// The agent's knowledge: whether it keeps a memory of its own, and which existing bases it
/// works with.
///
/// The base list is **honest about reach**. `knowledge` on an agent is a wish list, not a grant —
/// the three isolation levels decide what it actually reads, so a base outside this agent's scope
/// (another project's) is shown disabled with the reason, rather than offered as a checkbox that
/// would be silently dropped at read time. Another agent's memory *is* offered: every agent may
/// read every other's, which is the point of that level.
///
/// The list is fetched once, here, rather than polled into the shell state: a base's counts cost
/// a status pass over its storage.
fn agent_knowledge_checkboxes(form: AgentsForm) -> AnyView {
    // One fetch when the fieldset first renders. `Effect` rather than a fetch per render, and
    // guarded on `None` so re-opening the form doesn't re-ask.
    Effect::new(move |_| {
        if form.knowledge_bases.get_untracked().is_none() {
            spawn_local(async move {
                if let Ok(state) = fetch::knowledge().await {
                    form.knowledge_bases.set(Some(state.bases));
                }
            });
        }
    });

    let memory_toggle = view! {
        <label class="adi-check" style="display:inline-flex; align-items:center; gap:var(--space-1)">
            <input type="checkbox" prop:checked=move || form.memory.get()
                on:change=move |ev| form.memory.set(event_target_checked(&ev)) />
            <span>"Give this agent a memory of its own"</span>
        </label>
        <div class="adi-hint">
            {move || {
                let name = form.name.get();
                let base = if name.trim().is_empty() {
                    "agent:<name>/memory".to_string()
                } else {
                    format!("agent:{}/memory", name.trim())
                };
                format!("{base} — it alone writes there, and every other agent may read it. \
                         Off by default: an agent that records what it learns is a different \
                         thing from one that does not.")
            }}
        </div>
    };

    view! {
        <div class="adi-field adi-field--grow">
            <label class="adi-field__label">"Knowledge"</label>
            {memory_toggle}
            <div style="padding-top:var(--space-2)">{move || base_checkboxes(form)}</div>
        </div>
    }
    .into_any()
}

/// One checkbox per existing base, minus this agent's own memory (which is the toggle above).
fn base_checkboxes(form: AgentsForm) -> AnyView {
    let Some(bases) = form.knowledge_bases.get() else {
        return view! { <div class="adi-muted">"Loading knowledge bases…"</div> }.into_any();
    };
    let agent = form.name.get().trim().to_string();
    let project = form.project.get().trim().to_string();
    let offered: Vec<_> = bases
        .into_iter()
        // The agent's own memory is the toggle, not a checkbox — offering both would be two
        // controls for one fact.
        .filter(|b| !(b.memory && b.owner.as_deref() == Some(agent.as_str())))
        .collect();
    if offered.is_empty() {
        return view! {
            <div class="adi-hint">
                "No knowledge bases yet — make one on the Knowledge page. Whatever you tick here "
                "is what this agent searches."
            </div>
        }
        .into_any();
    }
    let boxes = offered
        .into_iter()
        .map(|b| {
            let id = b.id.clone();
            let (id_checked, id_toggle) = (id.clone(), id.clone());
            let checked = move || form.knowledge.get().contains(&id_checked);
            // Why this agent could not read it, if it could not. A project base belongs to one
            // project; an agent filed elsewhere (or nowhere) never sees it.
            let out_of_reach = match (b.level.as_str(), b.owner.as_deref()) {
                ("project", Some(owner)) if owner != project => Some(if project.is_empty() {
                    format!("only agents filed under project {owner} can read this")
                } else {
                    format!("this agent is filed under {project}, not {owner}")
                }),
                _ => None,
            };
            let disabled = out_of_reach.is_some();
            let title = out_of_reach.clone().unwrap_or_else(|| match b.level.as_str() {
                "agent" => "another agent's memory — readable, never writable".to_string(),
                "project" => "this project's knowledge".to_string(),
                _ => "shared by everything on this machine".to_string(),
            });
            let label = b.id.clone();
            view! {
                <label class="adi-check" title=title
                    style="display:inline-flex; align-items:center; gap:var(--space-1); margin:0 var(--space-3) var(--space-1) 0"
                    class:adi-muted=disabled>
                    <input type="checkbox" prop:checked=checked prop:disabled=disabled
                        on:change=move |ev| {
                            let on = event_target_checked(&ev);
                            form.knowledge.update(|set| {
                                if on { set.insert(id_toggle.clone()); } else { set.remove(&id_toggle); }
                            });
                        } />
                    <span class="adi-mono">{label}</span>
                    {out_of_reach.map(|_| view! { <span class="adi-chip">"out of scope"</span> })}
                </label>
            }
        })
        .collect::<Vec<_>>();
    view! {
        <div style="display:flex; flex-wrap:wrap; align-items:center">{boxes}</div>
    }
    .into_any()
}

fn agent_secret_checkboxes(state: State, form: AgentsForm) -> AnyView {
    let Some(st) = state.secrets.get() else {
        return view! {
            <div class="adi-field adi-field--grow">
                <label class="adi-field__label">"Secrets"</label>
                <div class="adi-muted">"Loading secrets…"</div>
            </div>
        }
        .into_any();
    };
    let mut secrets: Vec<SecretDto> = st.secrets;
    // Global secrets first, then by project, then by name — the shared baseline groups together.
    secrets.sort_by(|a, b| {
        a.project
            .is_some()
            .cmp(&b.project.is_some())
            .then_with(|| a.project.cmp(&b.project))
            .then_with(|| a.name.cmp(&b.name))
    });
    if secrets.is_empty() {
        return view! {
            <div class="adi-field adi-field--grow">
                <label class="adi-field__label">"Secrets"</label>
                <div class="adi-hint">
                    "No secrets yet — add one on the Secrets page. Whatever you tick here is "
                    "injected into this agent's runs as an environment variable."
                </div>
            </div>
        }
        .into_any();
    }
    let boxes = secrets
        .into_iter()
        .map(|s| {
            let key = (s.project.clone(), s.name.clone());
            let key_checked = key.clone();
            let key_toggle = key.clone();
            let checked = move || form.secrets.get().contains(&key_checked);
            let label = match s.project.as_deref().filter(|p| !p.trim().is_empty()) {
                Some(p) => format!("{} · {p}", s.name),
                None => s.name.clone(),
            };
            // The value is never sent here; the tooltip carries the description / OAuth provider.
            let title = match (&s.description, &s.oauth) {
                (Some(d), _) if !d.trim().is_empty() => d.clone(),
                (_, Some(o)) => format!("OAuth · {}", o.provider),
                _ => String::new(),
            };
            view! {
                <label class="adi-check" title=title
                    style="display:inline-flex; align-items:center; gap:var(--space-1); margin:0 var(--space-3) var(--space-1) 0">
                    <input type="checkbox" prop:checked=checked
                        on:change=move |ev| {
                            let on = event_target_checked(&ev);
                            form.secrets.update(|set| {
                                if on { set.insert(key_toggle.clone()); } else { set.remove(&key_toggle); }
                            });
                        } />
                    <span class="adi-mono">{label}</span>
                </label>
            }
        })
        .collect::<Vec<_>>();
    view! {
        <div class="adi-field adi-field--grow">
            <label class="adi-field__label">"Secrets (injected as env vars)"</label>
            <div style="display:flex; flex-wrap:wrap; align-items:center; padding:var(--space-1) 0">
                {boxes}
            </div>
        </div>
    }
    .into_any()
}

/// Render the agents table body: a loading/empty placeholder, or one row per agent with Run or
/// View (live session), Edit (opens the agent's own editor page), and Delete actions.
fn agent_rows(
    state: State,
    form: AgentsForm,
    watch: AgentsWatch,
    sim: Simulate,
    route: RwSignal<Route>,
) -> AnyView {
    let table = state.tables.agents;
    let mut agents = match rows_or_placeholder(
        table,
        state.agents.get().map(|v| v.agents),
        "No agents yet — “New agent” starts one.",
    ) {
        Ok(rows) => rows,
        Err(placeholder) => return placeholder,
    };
    sort_rows(&mut agents, table.sort.get(), agent_key, |a| {
        Key::text(&a.name)
    });
    agents
        .into_iter()
        .map(|a| {
            let del_name = a.name.clone();
            let a_edit = a.clone();
            let sim_name = a.name.clone();
            // Run/View/Stop stay inline (the live controls); Edit and the destructive Delete
            // move into the kebab.
            let items = vec![
                menu_item(state, "Edit", false, move || {
                    open_agent_editor(state, route, form, Some(&a_edit));
                }),
                // Take the model's seat in a run of this agent. In the kebab rather than inline
                // because it is not a way of *running* the agent — it is a way of reading what the
                // agent is told, and it opens a screen rather than starting work.
                menu_item(state, "Simulate", false, move || {
                    simulate::start_simulation(
                        state,
                        sim,
                        sim_name.clone(),
                        "Simulated run — read the prompt and take the model's seat.".to_string(),
                    );
                }),
                menu_item(state, "Delete", true, move || {
                    apply_agents(
                        state,
                        None,
                        format!("Deleted {del_name}."),
                        fetch::delete_agent(del_name.clone()),
                    );
                }),
            ];
            let actions = row_actions(
                state,
                format!("agent:{}", a.name),
                agent_actions(state, watch, &a),
                items,
            );
            view! {
                <TableRow state=table cell=move |col| agent_cell(col, &a) actions=actions/>
            }
            .into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// An agent's sort key under `col`. Shared with a project's Agents panel, which shows a subset of
/// the same columns over the same rows.
pub(crate) fn agent_key(a: &AgentDto, col: &str) -> Key {
    match col {
        "Backend" => Key::text(&a.backend),
        "Model" => Key::text(argument_text(&a.arguments, "model")),
        "Project" => Key::maybe(a.project.as_deref()),
        "Tags" => Key::text(a.tags.join(", ")),
        // "Name", and any header without a key of its own. Starred agents sort with their name,
        // not ahead of it — the star is a marker on the row, not a separate rank.
        _ => Key::text(&a.name),
    }
}

/// One agent's cell under `col`. Matching the header text — the same key the sort uses — is what
/// lets the user hide and reorder columns without the row builder knowing about it. The Status
/// column belongs to a project's panel, which builds it there from the live watch.
pub(crate) fn agent_cell(col: &str, a: &AgentDto) -> AnyView {
    match col {
        "Backend" => view! { <span class="font-mono">{a.backend.clone()}</span> }.into_any(),
        "Model" => view! {
            <span class="font-mono text-meta">{argument_text(&a.arguments, "model")}</span>
        }
        .into_any(),
        "Project" => match &a.project {
            Some(p) if !p.trim().is_empty() => {
                view! { <span><span class="adi-chip adi-mono">{p.clone()}</span></span> }.into_any()
            }
            _ => view! { <span><span class="adi-muted">"—"</span></span> }.into_any(),
        },
        "Tags" => view! { <span class="text-meta">{a.tags.join(", ")}</span> }.into_any(),
        // "Name", and anything the layout offers that this match doesn't name.
        _ => {
            let name = if a.starred {
                format!("★ {}", a.name)
            } else {
                a.name.clone()
            };
            view! { <span>{name}</span> }.into_any()
        }
    }
}

/// The live view's input row, wired to the watched agent's pty session.
fn send_bar(state: State, watch: AgentsWatch) -> impl IntoView {
    crate::ui::send_bar(watch.input, "type to the agent…", move |text, key| {
        send_to_agent(state, watch, text, key);
    })
}

/// Type into the watched agent's session: send `text` literally, then press `key`. The reply is
/// a fresh pane snapshot, applied immediately (unless the view moved on meanwhile) so the
/// keystrokes show without waiting for the next poll; errors go to the flash line.
fn send_to_agent(state: State, watch: AgentsWatch, text: String, key: &'static str) {
    let Some(name) = watch.name.get_untracked() else {
        return;
    };
    if text.is_empty() && key.is_empty() {
        return;
    }
    let key = key.to_string();
    spawn_local(async move {
        match fetch::send_agent_keys(name, text, key).await {
            Ok(peek) => {
                if watch.name.get_untracked().as_deref() == Some(peek.name.as_str()) {
                    watch.peek.set(Some(peek));
                }
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Trim a form string into an optional, dropping it when blank.
fn opt_str(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn argument_text(arguments: &BTreeMap<String, serde_json::Value>, name: &str) -> String {
    arguments
        .get(name)
        .and_then(scalar_argument_text)
        .unwrap_or_default()
}

fn scalar_argument_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Bool(value) => Some(value.to_string()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}
