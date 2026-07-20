//! The Tools panel of the project detail page: the tools filed under this project (from the
//! shared list at `/api/tools`), with the same Run / Edit / Archive actions as the global Tools
//! page, plus a create/link form pre-scoped to the open project.

use leptos::prelude::*;

use crate::pages::tools::{rows_view, tool_create_form};
use crate::state::{State, ToolEditor, ToolRunView, ToolsForm};
use crate::ui::data_table;

/// The Tools panel on a project's detail page. The run panel and script editor render above it
/// (in `project_detail_view`, as the triggers log does); this is the table + create form.
pub(crate) fn tools_panel(
    state: State,
    form: ToolsForm,
    editor: ToolEditor,
    run: ToolRunView,
) -> AnyView {
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Tools"</h2>
                <span class="adi-updated">"filed under this project"</span>
            </div>
            {data_table(&["Tool", "Runtime", "Invoke", ""], move || {
                let project = state.current_project.get();
                rows_view(state, editor, run, false, Some(project), false)
            })}
            {move || {
                // Fix the create/link form to the open project.
                let project = state.current_project.get();
                (!project.is_empty()).then(|| tool_create_form(state, form, Some(project)))
            }}
            <div class="adi-hint">
                "These appear in the global " <code>"Tools"</code> " list too, and each gets a "
                <code>".bin/<name>"</code> " shim agents run. A project-scoped tool runs in the "
                "project's directory."
            </div>
        </section>
    }
    .into_any()
}
