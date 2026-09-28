//! The screen's last answers, kept in `localStorage` so a reload draws them at once and the reads
//! that follow only update them — stale-while-revalidate, done by the page rather than by
//! `sw.js`.
//!
//! Not the service worker, for two reasons. The panel is served over plain `http://*.adi`, and a
//! browser withholds `navigator.serviceWorker` from an insecure origin, so a worker never runs
//! where the operator uses this screen. And `sw.js` belongs to both screens and keeps `/api` out
//! of its cache on purpose: the old screens would rather fail than show a stale port table.
//! Here a stale home screen for the second a reload takes is the point.
//!
//! Every entry is only ever a head start: whoever reads one still asks the API and overwrites
//! it, and one that no longer parses — a type that changed shape between builds — is ignored as
//! if absent.

use serde::{Serialize, de::DeserializeOwned};

use crate::ui;

/// Bump to drop every entry this module ever wrote, when a change would make old ones misleading
/// rather than merely unparseable.
const PREFIX: &str = "adi-new-cache-v1:";

/// The entry under `key`, if there is one this build can read.
pub(super) fn load<T: DeserializeOwned>(key: &str) -> Option<T> {
    let raw = ui::storage()?.get_item(&format!("{PREFIX}{key}")).ok()??;
    serde_json::from_str(&raw).ok()
}

/// Keep `value` under `key` for the next load. A full or refused storage is not an error worth
/// showing: the screen then simply starts empty next time, as it did before this existed.
pub(super) fn save<T: Serialize + ?Sized>(key: &str, value: &T) {
    if let (Some(s), Ok(raw)) = (ui::storage(), serde_json::to_string(value)) {
        let _ = s.set_item(&format!("{PREFIX}{key}"), &raw);
    }
}
