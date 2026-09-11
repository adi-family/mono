//! The Shared assets settings page: which of the three [`SharedAssetsMode`]s decides whether the
//! browser's copy of the webapp bundle comes from the shared-assets CDN or this instance's own
//! `dist/`. The setting itself (`GET`/`POST /api/settings/shared-assets`) is
//! `adi_webapp_api::handlers::shared_assets`; the actual rewriting is `adi-app`'s `shared_assets`
//! module, which this page only ever describes.

use adi_webapp_api::types::SharedAssetsMode;
use leptos::prelude::*;

use crate::fetch;
use crate::state::{State, read_error};
use crate::ui::apply_mutation;

/// The read this page is: the setting it shows, and the key a failure to load it is filed under.
const ENDPOINT: &str = "/api/settings/shared-assets";

/// The Shared assets settings page: the mode picker, and what each mode buys and costs, plus the
/// base URL and version prefix this instance would ask the CDN for — so an operator can tell at a
/// glance whether their build has actually been published there.
pub(crate) fn shared_assets_view(state: State) -> AnyView {
    let shared = state.shared_assets;
    // The CDN version prefix is this build's own version — the same field `/api/health` answers
    // with (`adi-app`'s `VERSION`, threaded into both `handlers::health` and
    // `shared_assets::SharedAssets::active`), so the health poll every page already runs is
    // enough; the setting itself carries only `mode` and `base_url`.
    let version = move || state.health.get().map(|h| h.version).unwrap_or_default();

    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Shared assets"</h2>
            </div>
            {move || match shared.get() {
                // Why, wherever there is a why. A read that failed and a read that has not
                // answered yet are the same empty signal, so a page that only ever says
                // "Loading…" is telling an operator nothing about a setting that will never
                // arrive — which is exactly how this one read while the live channel was
                // refusing to watch it.
                None => view! {
                    <div class="adi-empty">{move || read_error(state, ENDPOINT)
                        .map_or_else(|| "Loading\u{2026}".to_string(),
                            |why| format!("Couldn't load this: {why}"))}</div>
                }.into_any(),
                Some(s) => view! {
                    <div class="adi-panel__body">
                        <div class="adi-field">
                            <label class="adi-field__label" for="shared-assets-mode">
                                "Serve the webapp bundle from"
                            </label>
                            <select class="adi-input" id="shared-assets-mode"
                                prop:value=move || mode_value(s.mode)
                                on:change=move |ev| {
                                    let mode = mode_from_value(&event_target_value(&ev));
                                    apply_mutation(state, None, mode_change_message(mode),
                                        |s, v| s.shared_assets.set(Some(v)),
                                        fetch::set_shared_assets(mode));
                                }>
                                <option value="local-always">{mode_label(SharedAssetsMode::LocalAlways)}</option>
                                <option value="cdn-when-remote">{mode_label(SharedAssetsMode::CdnWhenRemote)}</option>
                                <option value="cdn-always">{mode_label(SharedAssetsMode::CdnAlways)}</option>
                            </select>
                            <p class="adi-field__note">{mode_hint(s.mode)}</p>
                        </div>
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
                                " has been published under this prefix before switching away from "
                                {mode_label(SharedAssetsMode::LocalAlways)}
                                "."
                            </div>
                        </div>
                    </div>
                }.into_any(),
            }}
        </section>
    }
    .into_any()
}

/// The `<option>` value / `<select>` value each mode round-trips as. Not [`SharedAssetsMode`]'s
/// own wire spelling reused by accident — it happens to be the same kebab-case, but this is a
/// separate, small mapping so the `<select>` never breaks silently if the wire format changes.
fn mode_value(mode: SharedAssetsMode) -> &'static str {
    match mode {
        SharedAssetsMode::LocalAlways => "local-always",
        SharedAssetsMode::CdnWhenRemote => "cdn-when-remote",
        SharedAssetsMode::CdnAlways => "cdn-always",
    }
}

/// The inverse of [`mode_value`]. An unrecognized value (there shouldn't be one — the `<select>`
/// only ever offers the three above) reads as [`SharedAssetsMode::LocalAlways`], the safe default.
fn mode_from_value(value: &str) -> SharedAssetsMode {
    match value {
        "cdn-when-remote" => SharedAssetsMode::CdnWhenRemote,
        "cdn-always" => SharedAssetsMode::CdnAlways,
        _ => SharedAssetsMode::LocalAlways,
    }
}

/// The `<option>` label for each mode, and what the "switching away from" note below refers back
/// to — the same words, so the two clearly name the same setting.
fn mode_label(mode: SharedAssetsMode) -> &'static str {
    match mode {
        SharedAssetsMode::LocalAlways => "This machine, always",
        SharedAssetsMode::CdnWhenRemote => "The CDN, only when the browser isn't on this machine",
        SharedAssetsMode::CdnAlways => "The CDN, always",
    }
}

/// What the currently selected mode buys and costs, shown under the picker.
fn mode_hint(mode: SharedAssetsMode) -> &'static str {
    match mode {
        SharedAssetsMode::LocalAlways => {
            "Every request loads the bundle from this instance's own disk. Nothing about how \
             the panel is served changes, and no request for it ever leaves the machine."
        }
        SharedAssetsMode::CdnWhenRemote => {
            "A request that looks like it's on this same machine \u{2014} loopback, or this \
             instance's own name \u{2014} still loads locally. Anything else, a paired fleet \
             node reached over n.adi say, loads from the CDN instead: that uplink is the one \
             case the CDN actually helps. Falls back to this instance's own bundle automatically \
             if the CDN can't be reached, so this never breaks the page \u{2014} at worst it \
             costs the round trip the fallback takes."
        }
        SharedAssetsMode::CdnAlways => {
            "Every request loads the bundle from the CDN, even one already on this machine, so \
             every instance on the same version shares one browser cache instead of each visitor \
             downloading its own copy. Falls back to this instance's own bundle automatically if \
             the CDN can't be reached, so this never breaks the page \u{2014} at worst it costs \
             the round trip the fallback takes."
        }
    }
}

/// The flash message after a successful `POST` — what changed, in the operator's own terms.
fn mode_change_message(mode: SharedAssetsMode) -> String {
    match mode {
        SharedAssetsMode::LocalAlways => {
            "Now always serving the webapp bundle from this machine.".to_string()
        }
        SharedAssetsMode::CdnWhenRemote => {
            "Now serving the webapp bundle from the CDN, except when the browser looks local."
                .to_string()
        }
        SharedAssetsMode::CdnAlways => {
            "Now always serving the webapp bundle from the shared-assets CDN.".to_string()
        }
    }
}
