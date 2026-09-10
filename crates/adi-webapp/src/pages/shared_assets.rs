//! The Shared assets settings page: one switch for pointing the browser's copy of the webapp
//! bundle at the shared-assets CDN instead of this instance's own `dist/`. The setting itself
//! (`GET`/`POST /api/settings/shared-assets`) is `adi_webapp_api::handlers::shared_assets`; the
//! actual rewriting is `adi-app`'s `shared_assets` module, which this page only ever describes.

use leptos::prelude::*;

use crate::fetch;
use crate::state::State;
use crate::ui::{apply_mutation, field_hint};

/// The Shared assets settings page: one switch, and what it buys and costs, plus the base URL and
/// version prefix this instance would ask the CDN for — so an operator can tell at a glance
/// whether their build has actually been published there.
pub(crate) fn shared_assets_view(state: State) -> AnyView {
    let shared = state.shared_assets;
    // The CDN version prefix is this build's own version — the same field `/api/health` answers
    // with (`adi-app`'s `VERSION`, threaded into both `handlers::health` and
    // `shared_assets::SharedAssets::active`), so the health poll every page already runs is
    // enough; the setting itself carries only `enabled` and `base_url`.
    let version = move || state.health.get().map(|h| h.version).unwrap_or_default();

    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Shared assets"</h2>
            </div>
            {move || match shared.get() {
                None => view! { <div class="adi-empty">"Loading…"</div> }.into_any(),
                Some(s) => view! {
                    <div class="adi-panel__body">
                        <label class="adi-field adi-field--check">
                            <input type="checkbox"
                                prop:checked=s.enabled
                                on:change=move |ev| {
                                    let enabled = event_target_checked(&ev);
                                    let msg = if enabled {
                                        "Now serving the webapp bundle from the shared-assets CDN.".to_string()
                                    } else {
                                        "Now serving the webapp bundle from this machine.".to_string()
                                    };
                                    apply_mutation(state, None, msg, |s, v| s.shared_assets.set(Some(v)),
                                        fetch::set_shared_assets(enabled));
                                } />
                            <span class="adi-field__label">"Serve the webapp bundle from the shared-assets CDN"</span>
                            {field_hint("Assets (the wasm bundle, its stylesheets and fonts) load from the CDN instead of this machine, so every instance on the same version shares one browser cache instead of each visitor downloading its own copy. Falls back to this instance's own bundle automatically if the CDN can't be reached, so turning this on never breaks the page \u{2014} at worst it costs the round trip the fallback takes.")}
                        </label>
                        <div class="adi-field">
                            <label class="adi-field__label">"CDN base URL"</label>
                            <span class="adi-mono adi-muted">{s.base_url}</span>
                        </div>
                        <div class="adi-field">
                            <label class="adi-field__label">"Version prefix"</label>
                            <span class="adi-mono adi-muted">{version}</span>
                            <div class="adi-field__note">
                                "What this instance asks the CDN for \u{2014} confirm "
                                <code>"monoapp/<version>/"</code>
                                " has been published under this prefix before turning the switch on."
                            </div>
                        </div>
                    </div>
                }.into_any(),
            }}
        </section>
    }
    .into_any()
}
