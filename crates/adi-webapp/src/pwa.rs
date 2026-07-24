//! Installing the control panel as an app.
//!
//! The browser plumbing lives in [`index.html`](../index.html): it registers `sw.js` and
//! parks the `beforeinstallprompt` event on `window.__adiPwa`. That has to happen in
//! JavaScript, not here — the event fires once, early, before wasm is running, and the saved
//! event object is the only handle that can open the install dialog afterwards.
//!
//! This module is the Rust half of that bridge: [`installable`] says whether there's an
//! install to offer, [`install`] opens the dialog. Both no-op when the bridge is missing,
//! which is what happens outside a secure context — over plain `http://app.adi` the browser
//! withholds `serviceWorker` and never fires the install event, so no button appears. Open
//! the panel on `http://localhost:8000` (a loopback origin *is* trusted) to install it.

use js_sys::{Function, Reflect};
use leptos::prelude::*;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};

/// The `window.__adiPwa` bridge object, or `None` when the bootstrap script didn't install
/// one (no secure context, so the browser offers no install path).
fn bridge() -> Option<JsValue> {
    let window = JsValue::from(web_sys::window()?);
    let pwa = Reflect::get(&window, &JsValue::from_str("__adiPwa")).ok()?;
    (!pwa.is_undefined() && !pwa.is_null()).then_some(pwa)
}

/// Read one of the bridge's boolean flags, defaulting to `false`.
fn flag(pwa: &JsValue, name: &str) -> bool {
    Reflect::get(pwa, &JsValue::from_str(name))
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// True while the browser has an install dialog for us to open and we aren't already running
/// as an installed app.
///
/// Registers `__adiPwa.onchange`, so the button appears the moment the browser decides the
/// app qualifies — usually a beat after load — and goes away once the install is accepted.
/// There is one `onchange` slot, so call this once per mounted app (the three entry points in
/// [`main`](../main.rs) are mutually exclusive, so that holds).
pub(crate) fn installable() -> RwSignal<bool> {
    let can_install = RwSignal::new(false);
    let Some(pwa) = bridge() else {
        return can_install;
    };

    let refresh = {
        let pwa = pwa.clone();
        move || can_install.set(flag(&pwa, "canInstall") && !flag(&pwa, "standalone"))
    };
    refresh(); // in case the event already fired while wasm was still booting

    let on_change = Closure::<dyn Fn()>::new(refresh);
    let _ = Reflect::set(
        &pwa,
        &JsValue::from_str("onchange"),
        on_change.as_ref().unchecked_ref(),
    );
    // The bridge now holds the only reference, and it lives as long as the page.
    on_change.forget();

    can_install
}

/// Open the browser's install dialog. A no-op when nothing offered one.
pub(crate) fn install() {
    let Some(pwa) = bridge() else {
        return;
    };
    if let Ok(f) = Reflect::get(&pwa, &JsValue::from_str("install"))
        && let Some(f) = f.dyn_ref::<Function>()
    {
        let _ = f.call0(&pwa);
    }
}
