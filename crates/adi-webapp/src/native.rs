//! Whether this page is running inside the ADI Mac app's own window rather than in a browser.
//!
//! The app puts the panel in a `WKWebView` under a bar of its own (`apps/macos/Sources/
//! PanelWindow.swift`), and that bar carries the mark, the wordmark and `⌘K` — so the page must
//! not carry them too. Two of the same control, eight pixels apart, one of which scrolls away with
//! the rail it is docked in, is worse than either alone.
//!
//! Nothing else changes. This is not an "embed mode" and it is not a second skin: the page is the
//! same page, minus the one control its host has taken over. Anything that needs a *layout* for a
//! native shell should say so on its own terms rather than growing this into a mode.
//!
//! The flag is `window.__adiNative`, set by a `WKUserScript` at document start. A user script and
//! not a User-Agent suffix, because the UA goes to every host the window opens and to every
//! request those pages make, while this concerns exactly one page and one window.

use wasm_bindgen::JsValue;

/// True when the ADI app is the window around this page.
///
/// Read every time rather than cached: it costs one property read, and a `static` here would be a
/// second place for the answer to live. It cannot change within a document's life — the script
/// runs before anything else does.
pub(crate) fn in_app() -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    js_sys::Reflect::get(&window, &JsValue::from_str("__adiNative"))
        .is_ok_and(|flag| flag.is_truthy())
}
