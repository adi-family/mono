//! The Agents page employee-code editor panel.

use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::routing::scroll_top;
use crate::state::{AgentCodeEditor, Flash, State};

/// The employee-code editor panel (the `{ } Code` action on a wasm agent's row): a textarea
/// over the agent's `src` file with Save / Build / Reload / Close, plus the last build's
/// output. `None` while closed.
pub(crate) fn code_editor_view(state: State, code: AgentCodeEditor) -> Option<AnyView> {
    let name = code.open.get()?;
    let dirty = move || code.buffer.get() != code.original.get();
    let build_name = name.clone();
    let reload_name = name.clone();
    let save_name = name.clone();

    // An unreadable source gets the panel to itself: the reason, the Retry that re-runs the
    // fetch, and Close. There is nothing to edit, so no toolbar and no textarea.
    if let Some(err) = code.error.get() {
        let retry_name = name.clone();
        return view! {
            <section class="adi-panel">
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">{format!("Employee code — {name}")}</h2>
                    <span class="adi-spacer"></span>
                    <button class="adi-btn adi-btn--ghost" type="button"
                        prop:disabled=move || code.busy.get()
                        on:click=move |_| open_code_editor(state, code, retry_name.clone())>"Retry"</button>
                    <button class="adi-btn adi-btn--link" type="button"
                        on:click=move |_| code.close()>"Close"</button>
                </div>
                <div class="adi-panel__body">
                    <div class="adi-flash" data-kind="err">{err}</div>
                    <p class="adi-muted">
                        "The agent's " <code>"src"</code> " argument points at a file that isn't there. "
                        "Point it at an existing TypeScript source with Edit on the agent's row, or "
                        "create the file at that path."
                    </p>
                </div>
            </section>
        }
        .into_any()
        .into();
    }

    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">{format!("Employee code — {name}")}</h2>
                <span class="adi-updated">"TypeScript → esbuild → jco → WASM component"</span>
            </div>
            <div class="adi-form adi-form--toolbar">
                <span class="adi-chip adi-mono">{move || code.path.get()}</span>
                <span class="adi-muted" style="font-size:var(--text-md)">
                    {move || if dirty() { "unsaved changes".to_string() } else { "saved".to_string() }}
                </span>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--primary" type="button"
                    prop:disabled=move || code.busy.get() || !dirty()
                    on:click=move |_| save_code(state, code, save_name.clone())>"Save"</button>
                <button class="adi-btn adi-btn--primary" type="button"
                    title="save if needed, then compile the source to its component"
                    prop:disabled=move || code.busy.get()
                    on:click=move |_| build_code(state, code, build_name.clone())>"⚙ Build"</button>
                <button class="adi-btn adi-btn--ghost" type="button"
                    prop:disabled=move || code.busy.get()
                    on:click=move |_| open_code_editor(state, code, reload_name.clone())>"Reload"</button>
                <button class="adi-btn adi-btn--link" type="button"
                    on:click=move |_| code.close()>"Close"</button>
            </div>
            <div class="adi-panel__body">
                <textarea class="adi-textarea adi-mono" spellcheck="false" autocomplete="off"
                    prop:value=move || code.buffer.get()
                    on:input=move |ev| code.buffer.set(event_target_value(&ev))></textarea>
                {move || code.build.get().map(|(ok, output)| view! {
                    <div class="adi-muted" style="font-size:var(--text-md); padding:var(--space-2) 0 var(--space-1)">
                        {if ok { "build succeeded" } else { "build failed" }}
                    </div>
                    <pre class="adi-term">{output}</pre>
                })}
            </div>
        </section>
    }
    .into_any()
    .into()
}

/// Open (or reload) the employee-code editor on a wasm agent: fetch the `src` file through the
/// agent code API into the buffer, then scroll up to where the panel renders.
pub(crate) fn open_code_editor(state: State, code: AgentCodeEditor, name: String) {
    code.busy.set(true);
    scroll_top();
    spawn_local(async move {
        match fetch::agent_code(name.clone()).await {
            Ok(c) => {
                code.open.set(Some(c.name));
                code.path.set(c.path);
                code.original.set(c.code.clone());
                code.buffer.set(c.code);
                code.build.set(None);
                code.error.set(None);
            }
            // Open the panel on failure too. The click scrolled the page to where the panel
            // renders, so a flash at the foot of the page would leave the button looking dead.
            Err(e) => {
                code.open.set(Some(name));
                code.path.set(String::new());
                code.original.set(String::new());
                code.buffer.set(String::new());
                code.build.set(None);
                code.error.set(Some(e.clone()));
                state.flash.set(Some(Flash::err(e)));
            }
        }
        code.busy.set(false);
    });
}

/// Save the code editor's buffer back to the agent's `src` file (the Save action).
fn save_code(state: State, code: AgentCodeEditor, name: String) {
    let content = code.buffer.get_untracked();
    code.busy.set(true);
    spawn_local(async move {
        match fetch::save_agent_code(name, content).await {
            Ok(c) => {
                code.original.set(c.code);
                state.flash.set(Some(Flash::ok(format!("Saved {}.", c.path))));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        code.busy.set(false);
    });
}

/// Compile the source to its component (the ⚙ Build action): save the buffer first when dirty,
/// then run the server-side build and show its output under the editor. A successful first
/// build fills the agent's `wasm` argument, so the fresh state lands in the list too.
fn build_code(state: State, code: AgentCodeEditor, name: String) {
    let content = code.buffer.get_untracked();
    let dirty = content != code.original.get_untracked();
    code.busy.set(true);
    spawn_local(async move {
        if dirty {
            match fetch::save_agent_code(name.clone(), content).await {
                Ok(c) => code.original.set(c.code),
                Err(e) => {
                    state.flash.set(Some(Flash::err(e)));
                    code.busy.set(false);
                    return;
                }
            }
        }
        match fetch::build_agent(name).await {
            Ok(res) => {
                state.agents.set(Some(res.state));
                code.build.set(Some((res.ok, res.output)));
                state.flash.set(Some(if res.ok {
                    Flash::ok(format!("Built {}.", res.wasm))
                } else {
                    Flash::err("Build failed — see the output below.".to_string())
                }));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        code.busy.set(false);
    });
}
