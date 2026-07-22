//! The Tools page: user CLIs under `~/.adi/mono/tools/` — small sh/ts scripts an agent runs.
//!
//! A tool is either **owned** (its script lives in the store, created and edited here) or
//! **linked** (a manifest pointing at an existing file on disk). Every active tool is exposed as
//! a `tools/.bin/<name>` shim, so an agent with that `.bin` on its PATH runs a tool by name — the
//! panel surfaces both the shim name and the directory to add.
//!
//! The run panel and the script editor are shared with a project's Tools panel (they render the
//! same way there), so their view + action helpers are `pub(crate)`.

use adi_webapp_api::types::{LinkTool, NewTool, ToolDto, ToolsState};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::highlight::Lang;
use crate::routing::scroll_top;
use crate::state::{Flash, State, ToolEditor, ToolRunView, ToolsForm};
use crate::ui::{
    TextField, apply_mutation, code_editor, code_viewer, confirm, data_table, flash_view, menu_item,
    placeholder_row, row_actions, segmented, updated_text,
};

/// The columns of the global tools table (the project panel uses its own, project-free set).
const TOOL_COLS: &[&str] = &["Tool", "Runtime", "Invoke", "Project", ""];

/// The Tools page: the run panel and script editor (when open), the live tools table, the
/// create/link form, and a collapsed archive of removed tools at the foot.
pub(crate) fn tools_view(
    state: State,
    form: ToolsForm,
    editor: ToolEditor,
    run: ToolRunView,
) -> AnyView {
    let State {
        tools, secs_since, ..
    } = state;

    view! {
        {move || tool_run_view(state, run)}
        {move || tool_editor_view(state, editor)}

        <section class="adi-panel">
            <div class="adi-panel__head">
                <span class="adi-chip adi-mono" title="Active tools">
                    {move || tools.get().map_or_else(|| "\u{2014}".to_string(),
                        |t| t.tools.iter().filter(|x| !x.is_archived()).count().to_string())}
                </span>
                <span class="adi-updated">{move || updated_text(tools, secs_since)}</span>
            </div>
            {data_table(TOOL_COLS, move || rows_view(state, editor, run, false, None, true))}
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">{move || if form.linking.get() { "Link a tool" } else { "New tool" }}</h2>
                <span class="adi-spacer"></span>
                {segmented("Tool source", form.linking, "New file", "Link file")}
            </div>
            {tool_create_form(state, form, None)}
            {flash_view(state.flash)}
            <footer class="adi-footer">
                "Every active tool is a shim in "
                {move || view! { <code>{tools.get().map(|t| t.bin_dir).unwrap_or_else(|| "~/.adi/mono/tools/.bin".to_string())}</code> }}
                " — add that directory to an agent's " <code>"PATH"</code> " and it runs a tool by name. "
                "Or run one directly with " <code>"adi-mono tools run <id> [args…]"</code> "."
            </footer>
        </section>

        {archived_section(state, editor, run, form.show_archived)}
    }
    .into_any()
}

/// The create/link form, shared by the global page and a project's quick panel. `project` fixes
/// the tool's project (the project panel passes `Some(id)`); the global form lets the user type one.
pub(crate) fn tool_create_form(state: State, form: ToolsForm, project: Option<String>) -> AnyView {
    let scoped = project.clone();
    view! {
        <form class="adi-form" on:submit=move |ev| {
            ev.prevent_default();
            let linking = form.linking.get();
            let description = form.description.get().trim().to_string();
            let description = (!description.is_empty()).then_some(description);
            // A project-scoped panel fixes the project; the global form reads its own field.
            let project = match &scoped {
                Some(id) => Some(id.clone()),
                None => {
                    let p = form.project.get().trim().to_string();
                    (!p.is_empty()).then_some(p)
                }
            };
            form.busy.set(true);
            if linking {
                let path = form.path.get().trim().to_string();
                if path.is_empty() {
                    state.flash.set(Some(Flash::err("A file path is required.".to_string())));
                    form.busy.set(false);
                    return;
                }
                let name = form.name.get().trim().to_string();
                let body = LinkTool {
                    path,
                    name: (!name.is_empty()).then_some(name),
                    // Inferred from the extension server-side unless the user set a runtime.
                    runtime: None,
                    description,
                    project,
                };
                reset_form(form);
                apply_mutation(state, Some(form.busy), "Linked tool.".to_string(),
                    |s: State, t: ToolsState| s.tools.set(Some(t)), fetch::link_tool(body));
            } else {
                let name = form.name.get().trim().to_string();
                if name.is_empty() {
                    state.flash.set(Some(Flash::err("A tool name is required.".to_string())));
                    form.busy.set(false);
                    return;
                }
                let runtime = form.runtime.get();
                let runtime = if runtime.trim().is_empty() { "sh".to_string() } else { runtime };
                let body = NewTool { name: name.clone(), runtime, description, project, content: None };
                reset_form(form);
                apply_mutation(state, Some(form.busy), format!("Created tool “{name}”."),
                    |s: State, t: ToolsState| s.tools.set(Some(t)), fetch::create_tool(body));
            }
        }>
            {move || if form.linking.get() {
                view! {
                    <TextField id="tool-path" label="File path" mono=true wide=true
                        field_class="adi-field--grow"
                        placeholder="/Users/you/scripts/build.ts"
                        hint="an existing sh/ts file — linked in place, never copied" value=form.path />
                    <TextField id="tool-name" label="Name" mono=true
                        placeholder="(defaults to the file name)" value=form.name />
                }.into_any()
            } else {
                view! {
                    <TextField id="tool-name" label="Name" mono=true placeholder="deploy"
                        hint="also the .bin/<name> agents run it by" value=form.name />
                    <div class="adi-field">
                        <label class="adi-field__label" for="tool-runtime">"Runtime"</label>
                        <select class="adi-input" id="tool-runtime"
                            prop:value=move || form.runtime.get()
                            on:change=move |ev| form.runtime.set(event_target_value(&ev))>
                            <option value="sh">"sh — shell script"</option>
                            <option value="ts">"ts — TypeScript (bun)"</option>
                        </select>
                    </div>
                }.into_any()
            }}
            <TextField id="tool-desc" label="Description" wide=true field_class="adi-field--grow"
                placeholder="What it does" value=form.description />
            {(project.is_none()).then(|| view! {
                <TextField id="tool-project" label="Project" mono=true
                    placeholder="(global — a project id files it under one)" value=form.project />
            })}
            <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || form.busy.get()>
                {move || if form.linking.get() { "Link tool" } else { "New tool" }}
            </button>
        </form>
    }
    .into_any()
}

/// The archive: its own collapsed panel at the foot, revealing archived tools so they can be
/// restored or permanently deleted. Renders nothing when nothing is archived (mirrors Dashboards).
fn archived_section(
    state: State,
    editor: ToolEditor,
    run: ToolRunView,
    show: RwSignal<bool>,
) -> AnyView {
    view! {
        {move || {
            let n = state.tools.get().map_or(0,
                |t| t.tools.iter().filter(|x| x.is_archived()).count());
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
                        {open.then(|| data_table(TOOL_COLS, move || rows_view(state, editor, run, true, None, true)))}
                    </section>
                }
                .into_any()
            })
        }}
    }
    .into_any()
}

/// Render a tools table body: the loading/empty placeholder, or one row per matching tool.
/// `archived` picks live vs. archived; `project` (when `Some`) filters to that project;
/// `show_project` adds the Project column (off in a project's own panel).
pub(crate) fn rows_view(
    state: State,
    editor: ToolEditor,
    run: ToolRunView,
    archived: bool,
    project: Option<String>,
    show_project: bool,
) -> AnyView {
    let cols = if show_project { "5" } else { "4" };
    let Some(loaded) = state.tools.get() else {
        return placeholder_row(cols, "Loading…");
    };
    let rows: Vec<ToolDto> = loaded
        .tools
        .into_iter()
        .filter(|t| t.is_archived() == archived)
        .filter(|t| project.as_deref().is_none_or(|p| t.project.as_deref() == Some(p)))
        .collect();
    if rows.is_empty() {
        return placeholder_row(
            cols,
            if archived {
                "Nothing archived."
            } else {
                "No tools yet — create or link one below."
            },
        );
    }
    rows.into_iter()
        .map(|t| row_view(state, editor, run, t, show_project))
        .collect::<Vec<_>>()
        .into_any()
}

/// One tool row: identity (name, short id, source/path), runtime, its `.bin` invocation, the
/// project (when shown), and the Run / Edit / Archive-or-Restore/Delete actions.
fn row_view(
    state: State,
    editor: ToolEditor,
    run: ToolRunView,
    t: ToolDto,
    show_project: bool,
) -> AnyView {
    let source = if t.system {
        "system"
    } else if t.linked {
        "linked"
    } else {
        "owned"
    };
    let sub = match &t.path {
        Some(path) => format!("{source} · {path}"),
        None => format!("{source} · {}", short_id(&t.id)),
    };
    let project_cell = show_project.then(|| {
        view! { <td class="adi-mono adi-muted">{t.project.clone().unwrap_or_else(|| "—".to_string())}</td> }
    });
    view! {
        <tr>
            <td title=t.id.clone()>
                <div>{t.name.clone()}</div>
                <div class="adi-mono adi-muted" style="font-size:var(--text-sm)">{sub}</div>
            </td>
            <td class="adi-mono">{t.runtime.clone()}</td>
            <td class="adi-mono" title=format!("adi-mono tools run {}", t.id)>{format!(".bin/{}", t.bin_name)}</td>
            {project_cell}
            <td class="adi-table__actions">{tool_actions(state, editor, run, &t)}</td>
        </tr>
    }
    .into_any()
}

/// The trailing actions for a tool row — shared by the global table and a project's panel. Active:
/// **▶ Run** inline (opens the panel and runs with no args), with Edit (opens the script editor) and
/// Archive in the kebab. Archived: **Restore** inline, with Delete in the kebab (behind a confirm; a
/// linked target file is never touched — a system tool has no Delete, so no kebab).
pub(crate) fn tool_actions(
    state: State,
    editor: ToolEditor,
    run: ToolRunView,
    t: &ToolDto,
) -> AnyView {
    let id = t.id.clone();
    let key = format!("tool:{id}");
    if t.is_archived() {
        let restore_id = id.clone();
        let del_id = id.clone();
        let del_short = short_id(&id);
        let restore = view! {
            <button class="adi-btn adi-btn--link" on:click=move |_| {
                apply_tools(state, "Restored tool.".to_string(),
                    fetch::unarchive_tool(restore_id.clone()));
            }>"Restore"</button>
        };
        // A system tool is protected from hard delete (archive is how you disable it), so its row
        // has no overflow item — `row_actions` then drops the kebab entirely.
        let mut items = Vec::new();
        if !t.system {
            items.push(menu_item(state, "Delete", true, move || {
                if !confirm(&format!(
                    "Permanently delete tool {del_short}? This removes its manifest (and, for \
                     an owned tool, its script). A linked file is left alone.")) {
                    return;
                }
                apply_tools(state, "Deleted tool.".to_string(), fetch::remove_tool(del_id.clone()));
            }));
        }
        row_actions(state, key, restore, items)
    } else {
        let run_id = id.clone();
        let run_name = t.name.clone();
        let edit_id = id.clone();
        let edit_name = t.name.clone();
        let edit_runtime = t.runtime.clone();
        let arch_id = id.clone();
        let run_btn = view! {
            <button class="adi-btn adi-btn--link" title="Run with no arguments" on:click=move |_| {
                run_tool_now(state, run, run_id.clone(), run_name.clone(), Vec::new());
            }>"▶ Run"</button>
        };
        let items = vec![
            menu_item(state, "Edit", false, move || {
                open_tool_editor(state, editor, edit_id.clone(), edit_name.clone(), edit_runtime.clone());
            }),
            menu_item(state, "Archive", false, move || {
                apply_tools(state, "Archived tool.".to_string(), fetch::archive_tool(arch_id.clone()));
            }),
        ];
        row_actions(state, key, run_btn, items)
    }
}

/// The run panel: the last run's output (a tailing viewer), its exit code, an args input to
/// re-run, and Close. `None` while no run is open. Shared with a project's Tools panel.
pub(crate) fn tool_run_view(state: State, run: ToolRunView) -> Option<AnyView> {
    let id = run.id.get()?;
    let name = run.name.get();
    let rerun_id = id.clone();
    let rerun_name = name.clone();
    let status = move || match run.code.get() {
        _ if run.busy.get() => ("running".to_string(), "unknown"),
        Some(0) => ("exit 0".to_string(), "online"),
        Some(c) => (format!("exit {c}"), "down"),
        None => ("no exit code".to_string(), "unknown"),
    };
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">{format!("Run — {name}")}</h2>
                <span class="adi-status" data-state=move || status().1>
                    <span class="adi-status__led"></span>
                    <span class="adi-mono">{move || status().0}</span>
                </span>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--link" type="button" on:click=move |_| run.close()>"Close"</button>
            </div>
            <div class="adi-form adi-form--toolbar">
                <TextField id="tool-run-args" label="Arguments" mono=true wide=true
                    field_class="adi-field--grow"
                    placeholder="passed to the tool verbatim (space-separated)" value=run.args />
                <button class="adi-btn adi-btn--primary" type="button"
                    prop:disabled=move || run.busy.get()
                    on:click=move |_| {
                        let args = split_args(&run.args.get());
                        run_tool_now(state, run, rerun_id.clone(), rerun_name.clone(), args);
                    }>"▶ Run again"</button>
            </div>
            <div class="adi-panel__body">
                {code_viewer(|| Lang::Sh, run.output, "adi-code--form", "tool-run-output")}
            </div>
        </section>
    }
    .into_any()
    .into()
}

/// The script editor panel: a highlighted editor over the tool's script with Save / Reload /
/// Close. `None` while closed. An unreadable script gets the panel to itself (the reason + Close),
/// mirroring the agent code editor. Shared with a project's Tools panel.
pub(crate) fn tool_editor_view(state: State, editor: ToolEditor) -> Option<AnyView> {
    let id = editor.open.get()?;
    let name = editor.name.get();
    let dirty = move || editor.buffer.get() != editor.original.get();

    if let Some(err) = editor.error.get() {
        let retry_id = id.clone();
        let retry_name = name.clone();
        let retry_runtime = editor.runtime.get();
        return view! {
            <section class="adi-panel">
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">{format!("Script — {name}")}</h2>
                    <span class="adi-spacer"></span>
                    <button class="adi-btn adi-btn--ghost" type="button"
                        prop:disabled=move || editor.busy.get()
                        on:click=move |_| open_tool_editor(state, editor, retry_id.clone(), retry_name.clone(), retry_runtime.clone())>"Retry"</button>
                    <button class="adi-btn adi-btn--link" type="button"
                        on:click=move |_| editor.close()>"Close"</button>
                </div>
                <div class="adi-panel__body">
                    <div class="adi-flash" data-kind="err">{err}</div>
                    <p class="adi-muted">
                        "The tool's script isn't readable. For a linked tool, the target file may have "
                        "moved; re-link it, or restore the file at that path."
                    </p>
                </div>
            </section>
        }
        .into_any()
        .into();
    }

    let save_id = id.clone();
    let reload_id = id.clone();
    let reload_name = name.clone();
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">{format!("Script — {name}")}</h2>
                <span class="adi-updated">{move || editor.runtime.get()}</span>
            </div>
            <div class="adi-form adi-form--toolbar">
                <span class="adi-chip adi-mono">{move || editor.path.get()}</span>
                <span class="adi-muted" style="font-size:var(--text-md)">
                    {move || if dirty() { "unsaved changes".to_string() } else { "saved".to_string() }}
                </span>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--primary" type="button"
                    prop:disabled=move || editor.busy.get() || !dirty()
                    on:click=move |_| save_tool_script(state, editor, save_id.clone())>"Save"</button>
                <button class="adi-btn adi-btn--ghost" type="button"
                    prop:disabled=move || editor.busy.get()
                    on:click=move |_| open_tool_editor(state, editor, reload_id.clone(), reload_name.clone(), editor.runtime.get())>"Reload"</button>
                <button class="adi-btn adi-btn--link" type="button"
                    on:click=move |_| editor.close()>"Close"</button>
            </div>
            <div class="adi-panel__body">
                {code_editor(move || Lang::from_path(&format!("s.{}", editor.runtime.get())),
                    editor.buffer, "adi-code--form", "tool-script-edit")}
            </div>
        </section>
    }
    .into_any()
    .into()
}

/// Open (or reload) the script editor on a tool: fetch the script into the buffer, then scroll up
/// to where the panel renders. On failure the panel still opens, carrying the error.
pub(crate) fn open_tool_editor(
    state: State,
    editor: ToolEditor,
    id: String,
    name: String,
    runtime: String,
) {
    editor.busy.set(true);
    editor.open.set(Some(id.clone()));
    editor.name.set(name.clone());
    editor.runtime.set(runtime);
    scroll_top();
    spawn_local(async move {
        match fetch::read_tool_script(id).await {
            Ok(s) => {
                editor.path.set(s.path);
                editor.runtime.set(s.runtime);
                editor.original.set(s.content.clone());
                editor.buffer.set(s.content);
                editor.error.set(None);
            }
            Err(e) => {
                editor.path.set(String::new());
                editor.original.set(String::new());
                editor.buffer.set(String::new());
                editor.error.set(Some(e.clone()));
                state.flash.set(Some(Flash::err(e)));
            }
        }
        editor.busy.set(false);
    });
}

/// Save the editor buffer back to the tool's script (owned file in the store, or linked target).
fn save_tool_script(state: State, editor: ToolEditor, id: String) {
    let content = editor.buffer.get_untracked();
    editor.busy.set(true);
    spawn_local(async move {
        match fetch::write_tool_script(id, content).await {
            Ok(s) => {
                editor.original.set(s.content);
                state.flash.set(Some(Flash::ok(format!("Saved {}.", s.path))));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        editor.busy.set(false);
    });
}

/// Run a tool now: open the run panel, spawn the run, and fold the captured output + fresh tools
/// state back into the page. Shared by the row's ▶ Run and the panel's ▶ Run again.
pub(crate) fn run_tool_now(
    state: State,
    run: ToolRunView,
    id: String,
    name: String,
    args: Vec<String>,
) {
    run.id.set(Some(id.clone()));
    run.name.set(name);
    run.busy.set(true);
    run.output.set(String::new());
    scroll_top();
    spawn_local(async move {
        match fetch::run_tool(id, args).await {
            Ok(res) => {
                run.output.set(if res.output.is_empty() {
                    "(no output)".to_string()
                } else {
                    res.output
                });
                run.code.set(res.exit_code);
                run.ok.set(res.ok);
                state.tools.set(Some(res.state));
                state.flash.set(Some(if res.ok {
                    Flash::ok("Tool exited 0.".to_string())
                } else {
                    Flash::err(format!(
                        "Tool exited {}.",
                        res.exit_code.map_or_else(|| "with no code".to_string(), |c| c.to_string())
                    ))
                }));
            }
            Err(e) => {
                run.output.set(e.clone());
                run.code.set(None);
                run.ok.set(false);
                state.flash.set(Some(Flash::err(e)));
            }
        }
        run.busy.set(false);
    });
}

/// Fold a tools mutation's fresh [`ToolsState`] into the page and flash success or the error — a
/// thin typed wrapper over [`apply_mutation`], as `apply_dashboards` is for dashboards.
fn apply_tools<F>(state: State, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<ToolsState, String>> + 'static,
{
    apply_mutation(state, None, ok_msg, |s, t| s.tools.set(Some(t)), fut);
}

/// Clear the create/link form's inputs after a submit (keeping the mode + project field).
fn reset_form(form: ToolsForm) {
    form.name.set(String::new());
    form.description.set(String::new());
    form.path.set(String::new());
}

/// Split an args string into words on whitespace (the panel's simple, quote-free tokenizer).
fn split_args(raw: &str) -> Vec<String> {
    raw.split_whitespace().map(str::to_string).collect()
}

/// The leading segment of a uuid — enough to recognize a tool without its full id.
fn short_id(id: &str) -> String {
    id.split('-').next().unwrap_or(id).to_string()
}
