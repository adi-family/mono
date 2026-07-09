//! The Overview page: system liveness at a glance.

use leptos::prelude::*;

use crate::state::State;
use crate::ui::tile;

/// The Overview page: system liveness at a glance.
pub(crate) fn overview_view(state: State) -> AnyView {
    let State { health, .. } = state;
    view! {
        <section class="adi-tiles">
            {tile("Uptime",
                move || health.get().map_or_else(|| "—".to_string(), |h| fmt_uptime(h.uptime_secs)),
                move || health.get().map_or_else(|| "adi-app".to_string(),
                    |h| format!("{} v{}", h.service, h.version)))}
        </section>
    }
    .into_any()
}

/// Format an uptime in seconds as `Ns` / `Nm Ss` / `Nh Mm`.
fn fmt_uptime(s: u64) -> String {
    if s < 60 {
        format!("{s}s")
    } else if s < 3_600 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}h {}m", s / 3_600, (s % 3_600) / 60)
    }
}
