//! The Usage events settings page: the on/off switch (ADI-MONO-157) and this install's id.
//! `GET`/`POST /api/usage-events` is `adi_webapp_api::handlers::usage`; the actual bus consumer
//! and flush to the collector is `adi-usage`, which this page only ever describes.
//!
//! Named apart from [`crate::routing::Route::Analytics`] (this instance's own run history, a
//! different and unrelated page) — see `UsageEventsState`'s own doc comment.

use leptos::prelude::*;

use crate::fetch;
use crate::state::{State, read_error_local};
use crate::ui::apply_mutation_local;

/// The read this page is: the setting it shows, and the key a failure to load it is filed under.
const ENDPOINT: &str = "/api/usage-events";

/// The Usage events settings page: the toggle, why it may be overridden, the install id once
/// there is one, and a way to forget it.
pub(crate) fn usage_events_view(state: State) -> AnyView {
    let usage = state.usage_events;
    let forgetting = RwSignal::new(false);

    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Usage events"</h2>
            </div>
            {move || match usage.get() {
                None => view! {
                    <div class="adi-empty">{move || read_error_local(state, ENDPOINT)
                        .map_or_else(|| "Loading\u{2026}".to_string(),
                            |why| format!("Couldn't load this: {why}"))}</div>
                }.into_any(),
                Some(u) => {
                let has_env_override = u.env_override.is_some();
                view! {
                    <div class="adi-panel__body">
                        <label class="adi-field adi-field--check">
                            <input type="checkbox" class="adi-check"
                                prop:checked=move || u.persisted
                                prop:disabled=move || has_env_override
                                on:change=move |ev| {
                                    let enabled = event_target_checked(&ev);
                                    apply_mutation_local(state, None,
                                        |s, v| s.usage_events.set(Some(v)),
                                        fetch::set_usage_events(enabled));
                                } />
                            <span class="adi-field__label">"Send usage events"</span>
                        </label>
                        <p class="adi-field__note">
                            "A fixed, short list of events — the first start, the setup wizard's \
                             Save, agent runs in the first week, once a day while it runs, and a \
                             few more — each carrying a random install id, the ADI version, the \
                             operating system and its architecture. Never a prompt, an answer, a \
                             file, a path, a project or agent name, or a secret."
                        </p>
                        {u.env_override.clone().map(|name| view! {
                            <p class="adi-field__note">
                                <code>{name}</code>
                                " is set in the environment, which overrides this toggle: usage \
                                 events are currently "
                                {if u.enabled { "on" } else { "off" }}
                                " regardless of what it shows."
                            </p>
                        })}
                        <div class="adi-field">
                            <label class="adi-field__label">"Install id"</label>
                            {match u.install_id.clone() {
                                Some(id) => view! {
                                    <span class="adi-mono adi-muted">{id}</span>
                                }.into_any(),
                                None => view! {
                                    <span class="adi-muted">
                                        "None yet \u{2014} minted the first time usage events \
                                         run while on."
                                    </span>
                                }.into_any(),
                            }}
                        </div>
                        {u.install_id.is_some().then(|| view! {
                            <div class="adi-field">
                                <button class="adi-btn adi-btn--ghost" type="button"
                                    prop:disabled=move || forgetting.get()
                                    on:click=move |_| {
                                        forgetting.set(true);
                                        apply_mutation_local(state, Some(forgetting),
                                            |s, v| s.usage_events.set(Some(v)),
                                            fetch::forget_usage_events());
                                    }>
                                    {move || if forgetting.get() { "Forgetting\u{2026}" } else { "Forget install id" }}
                                </button>
                                <p class="adi-field__note">
                                    "Clears the id on this machine. A fresh one is minted only \
                                     the next time usage events run while on \u{2014} switching \
                                     them off does not forget it by itself."
                                </p>
                            </div>
                        })}
                    </div>
                }.into_any()
                },
            }}
        </section>
    }
    .into_any()
}
