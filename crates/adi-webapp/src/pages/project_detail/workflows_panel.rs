//! The Workflows panel of the project detail page: the workflow files filed under this project
//! (`projects/<id>/workflows/*.ts`), sharing the global Workflows page's row/detail/run views and
//! its [`WorkflowsConsole`] — same state, filtered to this project client-side.

use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::pages::workflows::{project_rows_view, refresh};
use crate::state::{State, WorkflowsConsole};

/// The Workflows panel on a project's detail page: this project's own workflow files, each
/// expandable into its starters/shapes/enable toggle/run-now box and its own runs list, the same
/// way a row on the global page is.
pub(crate) fn workflows_panel(state: State, console: WorkflowsConsole) -> AnyView {
    // Load once per visit to this section — the same shared load the global page's own Effect
    // does, since there is no per-project list endpoint to fetch instead.
    Effect::new(move |loaded: Option<()>| {
        if loaded.is_none() {
            spawn_local(refresh(console));
        }
    });
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Workflows"</h2>
                <span class="adi-updated">"filed under this project"</span>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--ghost" type="button"
                    on:click=move |_| spawn_local(refresh(console))>
                    "Reload"
                </button>
            </div>
            <div class="adi-panel__body">
                {move || {
                    let project = state.current_project.get();
                    project_rows_view(state, console, &project)
                }}
            </div>
        </section>
    }
    .into_any()
}
