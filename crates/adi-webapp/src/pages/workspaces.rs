//! The Workspaces panel on a project's detail page: the project's working copies (created
//! by its hook scripts — the first by `init`, later ones by `workspace`) plus the hook files
//! themselves with Run/Log/Edit actions. Hooks are plain files at `.adi/hooks/<name>`; the
//! Edit action opens them in a dedicated hook editor panel rendered right above this one.

use adi_webapp_api::types::{NewProjectHook, NewWorkspace, WorkspacesState};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::routing::scroll_top;
use crate::state::{Flash, HookEditor, HookLogView, State, TermWatch};
use crate::ui::{TextField, data_table, fmt_date, placeholder_row};

/// A hook's path inside the project, as the file API sees it.
fn hook_rel_path(name: &str) -> String {
    format!(".adi/hooks/{name}")
}

/// The panel's workspace create form (name, an optional absolute path, and the "link local"
/// switch; the project is fixed to the open project). `Copy` so it threads into handlers.
#[derive(Clone, Copy)]
pub(crate) struct WorkspaceForm {
    pub(crate) name: RwSignal<String>,
    pub(crate) path: RwSignal<String>,
    pub(crate) local: RwSignal<bool>,
    pub(crate) busy: RwSignal<bool>,
}

/// The panel's hook create form (name + template). Editing an existing hook happens in the
/// Files editor, not here. `Copy` so it threads into handlers.
#[derive(Clone, Copy)]
pub(crate) struct NewHookForm {
    pub(crate) name: RwSignal<String>,
    pub(crate) template: RwSignal<String>,
    pub(crate) busy: RwSignal<bool>,
}

/// The open project's workspaces snapshot, but only when it actually belongs to the open
/// project (a stale snapshot from the previously viewed project renders as loading instead).
fn current_snapshot(state: State) -> Option<WorkspacesState> {
    let snapshot = state.workspaces.get()?;
    (snapshot.id == state.current_project.get()).then_some(snapshot)
}

/// The Workspaces panel: the registered working copies with live status, a create form, and
/// the project's hook files with Run/Log/Edit actions.
pub(crate) fn workspaces_panel(
    state: State,
    form: WorkspaceForm,
    hook_form: NewHookForm,
    log: HookLogView,
    editor: HookEditor,
    term: TermWatch,
) -> AnyView {
    let WorkspaceForm {
        name,
        path,
        local,
        busy,
    } = form;
    // The two-step unregister confirmation: `Some(name)` after the first click.
    let confirm_remove = RwSignal::new(None::<String>);
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Workspaces"</h2>
                <span class="adi-updated">"working copies created by the project's hooks"</span>
                <span class="adi-spacer"></span>
                {move || initialize_button(state, form)}
            </div>
            {data_table(&["Name", "Path", "Kind", "Status", "Created", ""], move || workspace_rows(state, confirm_remove, term))}
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                submit_workspace(state, form);
            }>
                <TextField id="ws-name" label="Name" placeholder="main" mono=true value=name />
                <TextField id="ws-path" label="Path" placeholder="(default: workspaces/<name>)" mono=true
                    wide=true field_class="adi-field--grow"
                    hint="absolute; empty = inside the project" value=path />
                <label class="adi-field adi-field--check">
                    <input type="checkbox"
                        prop:checked=move || local.get()
                        on:change=move |ev| local.set(event_target_checked(&ev)) />
                    <span class="adi-field__label">"Link existing dir (no hook)"</span>
                </label>
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    "Add workspace"
                </button>
            </form>
            <div class="adi-hint">
                {move || next_hook_hint(state)}
            </div>

            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Hooks"</h2>
                <span class="adi-updated">"plain files under " <code>".adi/hooks"</code></span>
            </div>
            {data_table(&["Hook", "Status", "Last run", ""], move || hook_rows(state, log, editor))}
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                submit_hook(state, hook_form);
            }>
                <TextField id="hook-name" label="Name" placeholder="init" mono=true
                    hint="init / workspace are the lifecycle hooks" value=hook_form.name />
                <div class="adi-field">
                    <label class="adi-field__label" for="hook-template">"Template"</label>
                    <select class="adi-input" id="hook-template"
                        prop:value=move || hook_form.template.get()
                        on:change=move |ev| hook_form.template.set(event_target_value(&ev))>
                        <option value="blank">"blank"</option>
                        <option value="init">"init (git clone)"</option>
                        <option value="workspace">"workspace (git worktree)"</option>
                    </select>
                </div>
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || hook_form.busy.get()>
                    "Add hook"
                </button>
            </form>
            <div class="adi-hint">
                "The first workspace runs the " <code>"init"</code> " hook (e.g. git clone); every "
                "further one runs " <code>"workspace"</code> " (e.g. git worktree add). Other hooks "
                "run manually. Edit opens the script right here, in the hook editor."
            </div>
        </section>
    }
    .into_any()
}

/// Rows for the workspaces table, with Terminal and a two-step Unregister action.
fn workspace_rows(
    state: State,
    confirm_remove: RwSignal<Option<String>>,
    term: TermWatch,
) -> AnyView {
    let Some(snapshot) = current_snapshot(state) else {
        return placeholder_row("6", "Loading…");
    };
    if snapshot.workspaces.is_empty() {
        let hint = if snapshot.has_init_hook {
            "Not initialized yet — press ⚡ Initialize to run the init hook (or add a workspace with a custom name below)."
        } else {
            "No workspaces yet — create an init hook below first, then press ⚡ Initialize."
        };
        return placeholder_row("6", hint);
    }
    snapshot
        .workspaces
        .into_iter()
        .map(|w| {
            let status_data = match w.status.as_str() {
                "ready" => "ready",
                "failed" => "blocked",
                _ => "",
            };
            let status_label = match w.status.as_str() {
                "creating" => "Creating…".to_string(),
                s => {
                    let mut label = s.to_string();
                    if let Some(first) = label.get_mut(0..1) {
                        first.make_ascii_uppercase();
                    }
                    label
                }
            };
            let created = if w.created_at > 0 {
                fmt_date(w.created_at)
            } else {
                "—".to_string()
            };
            let label_name = w.name.clone();
            let click_name = w.name.clone();
            let term_name = w.name.clone();
            // A terminal needs the directory on disk: ready always has it, local links do
            // by definition; a creating/failed workspace has nothing to open a shell in.
            let can_term = matches!(w.status.as_str(), "ready" | "local");
            view! {
                <tr>
                    <td>
                        <span class="adi-mono">{w.name.clone()}</span>
                        {w.primary.then(|| view! {
                            <span class="adi-muted" style="font-size:var(--text-sm); display:block">"★ primary"</span>
                        })}
                    </td>
                    <td class="adi-mono adi-muted" style="font-size:var(--text-sm); word-break:break-all">{w.path.clone()}</td>
                    <td class="adi-mono">{w.kind.clone()}</td>
                    <td>
                        <span class="adi-tstatus" data-status=status_data
                            title=w.hook.map(|h| format!("created by the {h} hook")).unwrap_or_default()>
                            {status_label}
                        </span>
                    </td>
                    <td class="adi-mono adi-muted">{created}</td>
                    <td class="adi-table__actions">
                        {can_term.then(|| view! {
                            <button class="adi-btn adi-btn--link" title="open a tmux terminal in this directory"
                                on:click=move |_| open_terminal(state, term, term_name.clone())>"⌨ Terminal"</button>
                            " "
                        })}
                        <button class="adi-btn adi-btn--link"
                            title="unregister only — files stay on disk"
                            on:click=move |_| {
                                if confirm_remove.get_untracked().as_deref() == Some(click_name.as_str()) {
                                    confirm_remove.set(None);
                                    remove_workspace(state, click_name.clone());
                                } else {
                                    confirm_remove.set(Some(click_name.clone()));
                                }
                            }>
                            {move || if confirm_remove.get().as_deref() == Some(label_name.as_str()) {
                                "Confirm unregister?"
                            } else {
                                "Unregister"
                            }}
                        </button>
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// Rows for the hooks table: each hook file with Run / Log / Edit actions.
fn hook_rows(state: State, log: HookLogView, editor: HookEditor) -> AnyView {
    let Some(snapshot) = current_snapshot(state) else {
        return placeholder_row("4", "Loading…");
    };
    if snapshot.hooks.is_empty() {
        return placeholder_row("4", "No hooks yet — add one below.");
    }
    snapshot
        .hooks
        .into_iter()
        .map(|h| {
            let status_data = match h.status.as_str() {
                "ok" => "ready",
                "failed" => "blocked",
                _ => "",
            };
            let status_label = match (h.status.as_str(), h.exit_code) {
                ("never", _) => "Never ran".to_string(),
                ("running", _) => "Running…".to_string(),
                ("ok", _) => "Ok".to_string(),
                (_, Some(code)) => format!("Failed ({code})"),
                (_, None) => "Failed".to_string(),
            };
            let ran = h.last_run_at.map_or_else(|| "—".to_string(), fmt_date);
            let run_name = h.name.clone();
            let log_name = h.name.clone();
            let edit_name = h.name.clone();
            // init/workspace only make sense with the ADI_WORKSPACE_* env a workspace
            // create provides — no manual Run for them (the API refuses it anyway).
            let lifecycle = h.name == "init" || h.name == "workspace";
            view! {
                <tr>
                    <td>
                        <span class="adi-mono">{h.name.clone()}</span>
                        <span class="adi-muted adi-mono" style="font-size:var(--text-sm); display:block">
                            {format!(".adi/hooks/{}", h.name)}
                        </span>
                    </td>
                    <td><span class="adi-tstatus" data-status=status_data>{status_label}</span></td>
                    <td class="adi-mono adi-muted">{ran}</td>
                    <td class="adi-table__actions">
                        {if lifecycle {
                            view! {
                                <span class="adi-muted" style="font-size:var(--text-sm)"
                                    title="lifecycle hooks run when a workspace is created — use Add workspace">
                                    "via Add workspace"
                                </span>
                            }.into_any()
                        } else {
                            view! {
                                <button class="adi-btn adi-btn--link" title="run the hook now, detached"
                                    on:click=move |_| run_hook(state, log, run_name.clone())>"▶ Run"</button>
                            }.into_any()
                        }}
                        " "
                        <button class="adi-btn adi-btn--link" title="show the last run's output"
                            on:click=move |_| open_hook_log(state, log, log_name.clone())>"Log"</button>
                        " "
                        <button class="adi-btn adi-btn--link" title="open the script in the hook editor"
                            on:click=move |_| open_hook_editor(state, editor, edit_name.clone())>"Edit"</button>
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// The hint under the workspace create form: which lifecycle hook the next create would run,
/// and whether its file exists yet.
fn next_hook_hint(state: State) -> AnyView {
    let Some(snapshot) = current_snapshot(state) else {
        return view! { <span></span> }.into_any();
    };
    let have = match snapshot.next_hook.as_str() {
        "init" => snapshot.has_init_hook,
        _ => snapshot.has_workspace_hook,
    };
    let hook = snapshot.next_hook.clone();
    if have {
        view! {
            <span>"The next workspace create runs the " <code>{hook}</code> " hook, detached — its status shows as creating until the hook finishes."</span>
        }
        .into_any()
    } else {
        view! {
            <span>"The next workspace create needs the " <code>{hook}</code> " hook — add it below first."</span>
        }
        .into_any()
    }
}

/// The one-click ⚡ Initialize button: creates the first workspace (named `main`) with the
/// init hook. Rendered only while the project is uninitialized (no hook-created workspace
/// yet) and an init hook file exists.
fn initialize_button(state: State, form: WorkspaceForm) -> Option<AnyView> {
    let snapshot = current_snapshot(state)?;
    (snapshot.next_hook == "init" && snapshot.has_init_hook).then(|| {
        view! {
            <button class="adi-btn adi-btn--primary" type="button"
                title="run the init hook — creates the first workspace, “main”"
                prop:disabled=move || form.busy.get()
                on:click=move |_| initialize_project(state, form)>"⚡ Initialize"</button>
        }
        .into_any()
    })
}

/// Initialize the project: create the first workspace under the default name `main`, which
/// runs the init hook (e.g. git clone).
fn initialize_project(state: State, form: WorkspaceForm) {
    let id = state.current_project.get_untracked();
    if id.is_empty() {
        return;
    }
    send_create_workspace(
        state,
        form,
        NewWorkspace {
            id,
            name: "main".to_string(),
            path: None,
            local: false,
        },
    );
}

/// Submit the workspace create form.
fn submit_workspace(state: State, form: WorkspaceForm) {
    let id = state.current_project.get_untracked();
    if id.is_empty() {
        return;
    }
    let name = form.name.get_untracked().trim().to_string();
    if name.is_empty() {
        state
            .flash
            .set(Some(Flash::err("A workspace name is required.".to_string())));
        return;
    }
    let path = form.path.get_untracked().trim().to_string();
    send_create_workspace(
        state,
        form,
        NewWorkspace {
            id,
            name,
            path: (!path.is_empty()).then_some(path),
            local: form.local.get_untracked(),
        },
    );
}

/// Fire a workspace-create request and fan the result into the signals (shared by the
/// ⚡ Initialize button and the create form).
fn send_create_workspace(state: State, form: WorkspaceForm, body: NewWorkspace) {
    form.busy.set(true);
    spawn_local(async move {
        match fetch::create_workspace(body).await {
            Ok(res) => {
                state.workspaces.set(Some(res.state));
                state.flash.set(Some(Flash::ok(res.message)));
                form.name.set(String::new());
                form.path.set(String::new());
                form.local.set(false);
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        form.busy.set(false);
    });
}

/// Submit the hook create form (materialize a template file).
fn submit_hook(state: State, form: NewHookForm) {
    let id = state.current_project.get_untracked();
    if id.is_empty() {
        return;
    }
    let name = form.name.get_untracked().trim().to_string();
    if name.is_empty() {
        state
            .flash
            .set(Some(Flash::err("A hook name is required.".to_string())));
        return;
    }
    let template = form.template.get_untracked();
    let body = NewProjectHook {
        id,
        name: name.clone(),
        template: (!template.is_empty()).then_some(template),
    };
    form.busy.set(true);
    spawn_local(async move {
        match fetch::create_project_hook(body).await {
            Ok(state_dto) => {
                state.workspaces.set(Some(state_dto));
                state.flash.set(Some(Flash::ok(format!(
                    "Created hook “{name}” — edit it under .adi/hooks in Files."
                ))));
                form.name.set(String::new());
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        form.busy.set(false);
    });
}

/// Unregister a workspace (the confirmed second click). The registry entry goes; files stay.
fn remove_workspace(state: State, name: String) {
    let id = state.current_project.get_untracked();
    if id.is_empty() {
        return;
    }
    spawn_local(async move {
        match fetch::remove_workspace(id, name.clone()).await {
            Ok(snapshot) => {
                state.workspaces.set(Some(snapshot));
                state.flash.set(Some(Flash::ok(format!(
                    "Unregistered “{name}” (files left on disk)."
                ))));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Run a hook by hand (the ▶ Run action), then open its log so the output is visible live.
fn run_hook(state: State, log: HookLogView, name: String) {
    let id = state.current_project.get_untracked();
    if id.is_empty() {
        return;
    }
    spawn_local(async move {
        match fetch::run_project_hook(id.clone(), name.clone()).await {
            Ok(res) => {
                state.workspaces.set(Some(res.state));
                state.flash.set(Some(Flash::ok(res.message)));
                open_hook_log(state, log, name);
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Open the log view on a hook (the Log action): show the panel, fetch the first snapshot
/// immediately (the 1s poll takes over), and scroll up to where the panel renders.
fn open_hook_log(state: State, log: HookLogView, name: String) {
    let id = state.current_project.get_untracked();
    if id.is_empty() {
        return;
    }
    log.log.set(None);
    log.watched.set(Some((id, name)));
    poll_hook_log(log);
    scroll_top();
}

/// Open a hook script in the hook editor (the Edit action): load `.adi/hooks/<name>`
/// through the project file API into the buffer, then scroll up to where the editor panel
/// renders.
fn open_hook_editor(state: State, editor: HookEditor, name: String) {
    let id = state.current_project.get_untracked();
    if id.is_empty() {
        return;
    }
    editor.busy.set(true);
    spawn_local(async move {
        match fetch::read_file(&id, &hook_rel_path(&name)).await {
            Ok(fc) => {
                editor.open.set(Some((id, name)));
                editor.original.set(fc.content.clone());
                editor.buffer.set(fc.content);
                scroll_top();
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        editor.busy.set(false);
    });
}

/// Save the hook editor's buffer back to the script, then refresh the workspaces snapshot
/// so the hook's size/modified update in the table.
fn save_hook(state: State, editor: HookEditor) {
    let Some((id, name)) = editor.open.get_untracked() else {
        return;
    };
    let content = editor.buffer.get_untracked();
    editor.busy.set(true);
    spawn_local(async move {
        match fetch::write_file(&id, &hook_rel_path(&name), &content).await {
            Ok(fc) => {
                editor.original.set(fc.content.clone());
                editor.buffer.set(fc.content);
                state
                    .flash
                    .set(Some(Flash::ok(format!("Saved hook “{name}”."))));
                if let Ok(snapshot) = fetch::workspaces(&id).await {
                    state.workspaces.set(Some(snapshot));
                }
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        editor.busy.set(false);
    });
}

/// Reload the open hook's script from disk, dropping any unsaved edits.
fn reload_hook(state: State, editor: HookEditor) {
    let Some((id, name)) = editor.open.get_untracked() else {
        return;
    };
    editor.busy.set(true);
    spawn_local(async move {
        match fetch::read_file(&id, &hook_rel_path(&name)).await {
            Ok(fc) => {
                editor.original.set(fc.content.clone());
                editor.buffer.set(fc.content);
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        editor.busy.set(false);
    });
}

/// The hook editor panel: a toolbar (script path, dirty state, Save/Reload/Close) and a
/// monospace textarea bound to the buffer. Renders nothing while no hook is open.
pub(crate) fn hook_editor_view(state: State, editor: HookEditor) -> Option<AnyView> {
    let (_, name) = editor.open.get()?;
    let dirty = move || editor.buffer.get() != editor.original.get();
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">{format!("Edit hook — {name}")}</h2>
                <span class="adi-updated">"runs as sh -c, detached"</span>
            </div>
            <div class="adi-form adi-form--toolbar">
                <span class="adi-chip adi-mono">{hook_rel_path(&name)}</span>
                <span class="adi-muted" style="font-size:var(--text-md)">
                    {move || if dirty() { "unsaved changes".to_string() } else { "saved".to_string() }}
                </span>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--primary" type="button"
                    prop:disabled=move || editor.busy.get() || !dirty()
                    on:click=move |_| save_hook(state, editor)>"Save"</button>
                <button class="adi-btn adi-btn--ghost" type="button"
                    prop:disabled=move || editor.busy.get()
                    on:click=move |_| reload_hook(state, editor)>"Reload"</button>
                <button class="adi-btn adi-btn--link" type="button"
                    on:click=move |_| editor.close()>"Close"</button>
            </div>
            <div class="adi-panel__body">
                <textarea class="adi-textarea adi-mono" spellcheck="false" autocomplete="off"
                    prop:value=move || editor.buffer.get()
                    on:input=move |ev| editor.buffer.set(event_target_value(&ev))></textarea>
            </div>
        </section>
    }
    .into_any()
    .into()
}

/// Open the terminal view on a workspace (the ⌨ Terminal action): ensure the tmux session
/// exists (started in the workspace directory), show the panel with the first snapshot, and
/// scroll up to where it renders. Reopening an open terminal just reattaches the view.
fn open_terminal(state: State, term: TermWatch, name: String) {
    let id = state.current_project.get_untracked();
    if id.is_empty() {
        return;
    }
    term.peek.set(None);
    term.watched.set(Some((id.clone(), name.clone())));
    scroll_top();
    spawn_local(async move {
        match fetch::open_workspace_terminal(id, name).await {
            Ok(peek) => {
                if term
                    .watched
                    .get_untracked()
                    .is_some_and(|(wid, wname)| wid == peek.id && wname == peek.name)
                {
                    term.peek.set(Some(peek));
                }
            }
            Err(e) => {
                term.close();
                state.flash.set(Some(Flash::err(e)));
            }
        }
    });
}

/// Fetch a fresh pane snapshot for the watched terminal, if any. The shell calls this every
/// second; it no-ops while the view is closed. A response landing after the view moved to
/// another workspace (or closed) is dropped instead of flashing the wrong pane.
pub(crate) fn poll_term(term: TermWatch) {
    let Some((id, name)) = term.watched.get_untracked() else {
        return;
    };
    spawn_local(async move {
        if let Ok(peek) = fetch::peek_workspace_terminal(id, name).await
            && term
                .watched
                .get_untracked()
                .is_some_and(|(wid, wname)| wid == peek.id && wname == peek.name)
        {
            term.peek.set(Some(peek));
        }
    });
}

/// The terminal panel: a 1s-refreshed capture of the workspace's tmux pane, with a send bar
/// to type into the session. Renders nothing while no terminal is being watched. Close only
/// hides the view (the session lives on); Kill ends the session itself.
pub(crate) fn term_view(state: State, term: TermWatch) -> Option<AnyView> {
    let (_, name) = term.watched.get()?;
    let peek = term.peek.get();
    let attach = peek.as_ref().map(|p| p.attach.clone()).unwrap_or_default();
    let running = peek.as_ref().is_some_and(|p| p.running);
    let body = match peek {
        None => view! { <div class="adi-empty">"Connecting…"</div> }.into_any(),
        Some(p) if !p.running => view! {
            <div class="adi-empty">"The session has ended — open the terminal again to restart it."</div>
        }
        .into_any(),
        Some(p) => view! { <pre class="adi-term">{p.output}</pre> }.into_any(),
    };
    Some(
        view! {
            <section class="adi-panel">
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">{format!("Terminal — {name}")}</h2>
                    <span class="adi-spacer"></span>
                    {(!attach.is_empty()).then(|| view! {
                        <code class="adi-mono adi-muted" style="font-size:var(--text-sm)">{attach}</code>
                    })}
                    {running.then(|| view! {
                        <button class="adi-btn adi-btn--link" title="kill the tmux session"
                            on:click=move |_| kill_terminal(state, term)>"Kill"</button>
                    })}
                    <button class="adi-btn adi-btn--link" title="hide the view — the session keeps running"
                        on:click=move |_| term.close()>"Close"</button>
                </div>
                {body}
                {running.then(|| term_send_bar(state, term))}
            </section>
        }
        .into_any(),
    )
}

/// The terminal's input row: a text field (submit types it into the session, without a
/// trailing Enter — the ⏎ quick key sends that) plus the special keys interactive programs
/// need.
fn term_send_bar(state: State, term: TermWatch) -> impl IntoView {
    view! {
        <form class="adi-form"
            on:submit=move |ev| {
                ev.prevent_default();
                let text = term.input.get();
                term.input.set(String::new());
                send_to_terminal(state, term, text, "");
            }>
            <input class="adi-input adi-input--wide adi-mono" autocomplete="off"
                placeholder="type into the terminal…"
                prop:value=move || term.input.get()
                on:input=move |ev| term.input.set(event_target_value(&ev)) />
            <button class="adi-btn adi-btn--primary" type="submit">"Send"</button>
            {term_quick_key(state, term, "⏎", "Enter")}
            {term_quick_key(state, term, "↑", "Up")}
            {term_quick_key(state, term, "↓", "Down")}
            {term_quick_key(state, term, "Tab", "Tab")}
            {term_quick_key(state, term, "Esc", "Escape")}
            {term_quick_key(state, term, "^C", "C-c")}
        </form>
    }
}

/// One special-key button in the terminal send bar, pressing a single tmux key.
fn term_quick_key(
    state: State,
    term: TermWatch,
    label: &'static str,
    key: &'static str,
) -> impl IntoView {
    view! {
        <button class="adi-btn adi-btn--ghost adi-mono" type="button"
            title=format!("send {key}")
            on:click=move |_| send_to_terminal(state, term, String::new(), key)>{label}</button>
    }
}

/// Type into the watched terminal: send `text` literally, then press `key`. The reply is a
/// fresh pane snapshot, applied immediately (unless the view moved on meanwhile) so the
/// keystrokes show without waiting for the next poll; errors go to the flash line.
fn send_to_terminal(state: State, term: TermWatch, text: String, key: &'static str) {
    let Some((id, name)) = term.watched.get_untracked() else {
        return;
    };
    if text.is_empty() && key.is_empty() {
        return;
    }
    let key = key.to_string();
    spawn_local(async move {
        match fetch::send_workspace_terminal(id, name, text, key).await {
            Ok(peek) => {
                if term
                    .watched
                    .get_untracked()
                    .is_some_and(|(wid, wname)| wid == peek.id && wname == peek.name)
                {
                    term.peek.set(Some(peek));
                }
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Kill the watched terminal's tmux session (the Kill action). The view stays open showing
/// the not-running snapshot, so the kill is visibly confirmed.
fn kill_terminal(state: State, term: TermWatch) {
    let Some((id, name)) = term.watched.get_untracked() else {
        return;
    };
    spawn_local(async move {
        match fetch::kill_workspace_terminal(id, name).await {
            Ok(peek) => {
                if term
                    .watched
                    .get_untracked()
                    .is_some_and(|(wid, wname)| wid == peek.id && wname == peek.name)
                {
                    term.peek.set(Some(peek));
                }
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Fetch a fresh log snapshot for the watched hook, if any. The shell calls this every
/// second; it no-ops while the log view is closed. A response landing after the view moved
/// to another hook (or closed) is dropped instead of flashing the wrong log.
pub(crate) fn poll_hook_log(log: HookLogView) {
    let Some((id, name)) = log.watched.get_untracked() else {
        return;
    };
    spawn_local(async move {
        if let Ok(snapshot) = fetch::project_hook_log(id, name).await
            && log
                .watched
                .get_untracked()
                .is_some_and(|(wid, wname)| wid == snapshot.id && wname == snapshot.name)
        {
            log.log.set(Some(snapshot));
        }
    });
}

/// The hook-log panel: the watched hook's last run output, refreshed each second. Renders
/// nothing while no hook is being watched.
pub(crate) fn hook_log_view(log: HookLogView) -> Option<AnyView> {
    let (_, name) = log.watched.get()?;
    let snapshot = log.log.get();
    let status_line = snapshot
        .as_ref()
        .map(|s| match (s.status.as_str(), s.exit_code, s.ran_at) {
            ("running", _, _) => "still running…".to_string(),
            (_, Some(code), Some(at)) => format!("exit {code} · last run {}", fmt_date(at)),
            (_, Some(code), None) => format!("exit {code}"),
            _ => String::new(),
        })
        .unwrap_or_default();
    let body = match snapshot {
        None => view! { <div class="adi-empty">"Loading…"</div> }.into_any(),
        Some(s) if !s.ran => view! {
            <div class="adi-empty">"This hook has never run — its log is empty."</div>
        }
        .into_any(),
        Some(s) => view! { <pre class="adi-term">{s.output}</pre> }.into_any(),
    };
    Some(
        view! {
            <section class="adi-panel">
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">{format!("Hook log — {name}")}</h2>
                    <span class="adi-spacer"></span>
                    {(!status_line.is_empty()).then(|| view! {
                        <span class="adi-muted" style="font-size:var(--text-sm)">{status_line}</span>
                    })}
                    <button class="adi-btn adi-btn--link" on:click=move |_| log.close()>"Close"</button>
                </div>
                {body}
            </section>
        }
        .into_any(),
    )
}
