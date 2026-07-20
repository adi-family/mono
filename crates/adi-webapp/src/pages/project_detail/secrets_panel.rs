//! The Secrets panel of the project detail page: the secrets filed under this project (from the
//! shared list at `/api/secrets`), with the same Reveal / Remove actions as the global Secrets
//! page, plus a create form pre-scoped to the open project.

use leptos::prelude::*;

use crate::pages::secrets::{rows_view, secret_create_form};
use crate::state::{SecretsForm, State};
use crate::ui::data_table;

/// The Secrets panel on a project's detail page: the project's secrets table + a create form
/// fixed to this project.
pub(crate) fn secrets_panel(state: State, form: SecretsForm) -> AnyView {
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Secrets"</h2>
                <span class="adi-updated">"filed under this project"</span>
            </div>
            {data_table(&["Name", "Value", "Description", ""], move || {
                let project = state.current_project.get();
                rows_view(state, form, Some(project), false)
            })}
            {move || {
                // Fix the create form to the open project.
                let project = state.current_project.get();
                (!project.is_empty()).then(|| secret_create_form(state, form, Some(project)))
            }}
            <div class="adi-hint">
                "These appear in the global " <code>"Secrets"</code> " list too. A trigger or agent "
                "filed under this project inherits them (overriding a global of the same name) as "
                "environment variables."
            </div>
        </section>
    }
    .into_any()
}
