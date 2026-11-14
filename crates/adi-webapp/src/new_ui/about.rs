//! "About adi", at `/about` — the new UI's About This Mac: the mark, the version, and a few lines
//! on the instance this page is talking to. Opened from the mark in the top bar.
//!
//! Everything here is read from endpoints the panel already serves — `/api/health`,
//! `/api/system`, `/api/fleet` — once, as the window opens; `update` is the one exception,
//! since its "Updates" row has to stay live and clickable while the window sits open
//!. A read that fails leaves its lines out rather than drawing them
//! empty.

use adi_ui::Mark;
use adi_webapp_api::types::UpdateState;
use leptos::prelude::*;

use crate::{fetch, ui, update};

/// What the window shows from its three one-shot reads; each part stays `None` until its read
/// answers, and for good if it fails. The version line has a fourth source, [`update::UpdateWatch`],
/// read live rather than stored here — see the module header.
#[derive(Clone, Default)]
struct Facts {
    /// `(installed, running)` — they differ on a development build. Filled from `/api/health`
    /// only, as a fallback for the instant before `updates` has answered at all.
    version: Option<(String, String)>,
    uptime: Option<u64>,
    /// `(running, total)`.
    services: Option<(usize, usize)>,
    agents: Option<u32>,
    /// `(active, paired)`.
    machines: Option<(usize, usize)>,
}

#[component]
pub(super) fn About(updates: update::UpdateWatch) -> impl IntoView {
    let facts = RwSignal::new(Facts::default());
    // Three independent reads, each filling its own part as it answers — side by side, not one
    // after another, and a slow or failed one holds up nothing else.
    leptos::task::spawn_local(async move {
        if let Ok(h) = fetch::health().await {
            facts.update(|f| {
                f.uptime = Some(h.uptime_secs);
                f.version
                    .get_or_insert_with(|| (h.version.clone(), h.version.clone()));
            });
        }
    });
    leptos::task::spawn_local(async move {
        if let Ok(s) = fetch::system_status().await {
            let running = s.services.iter().filter(|s| s.running).count();
            facts.update(|f| {
                f.services = Some((running, s.services.len()));
                f.agents = Some(s.live_runs);
            });
        }
    });
    leptos::task::spawn_local(async move {
        if let Ok(fl) = fetch::fleet().await {
            let active = fl.nodes.iter().filter(|n| n.active).count();
            facts.update(|f| f.machines = Some((active, fl.nodes.len())));
        }
    });

    let host = window().location().host().unwrap_or_default();

    view! {
        <div class="adi-new-about">
            <Mark class="adi-new-about__mark"/>
            <h2 class="adi-new-about__name">"adi"</h2>
            <p class="adi-new-about__version">
                {move || match updates.state().map(|u| u.installed).or_else(|| {
                    facts.get().version.map(|(installed, _)| installed)
                }) {
                    Some(installed) => format!("Version {installed}"),
                    // Holds the line's height, so the rows below do not jump when it lands.
                    None => "\u{00a0}".into(),
                }}
            </p>
            {move || rows(updates, facts.get(), host.clone())}
        </div>
    }
}

/// The key-value lines, in the order macOS's About window puts its own: what it is first, what
/// it is doing after. "Updates" is the one row that is also a button — see [`updates_row`].
fn rows(updates: update::UpdateWatch, f: Facts, host: String) -> impl IntoView {
    let mut rows: Vec<AnyView> = vec![fact_row("Address", host, true)];
    let u = updates.state();
    match (&u, &f.version) {
        (Some(u), _) if u.running != u.installed => {
            // A development build reports its crate version, not the release it came from —
            // worth saying, because it is why the two lines disagree.
            rows.push(fact_row("Running build", u.running.clone(), true));
        }
        (None, Some((installed, running))) if installed != running => {
            rows.push(fact_row("Running build", running.clone(), true));
        }
        _ => {}
    }
    if let Some(u) = &u {
        rows.push(fact_row("Platform", platform_name(&u.platform), false));
        rows.push(updates_row(updates, u));
    }
    if let Some(s) = f.uptime {
        rows.push(fact_row("Up for", ui::fmt_uptime(s), false));
    }
    if let Some((running, total)) = f.services {
        rows.push(fact_row("Services", format!("{running} of {total} running"), false));
    }
    if let Some(n) = f.agents {
        let runs = match n {
            0 => "None running".to_string(),
            1 => "1 running".to_string(),
            n => format!("{n} running"),
        };
        rows.push(fact_row("Agents", runs, false));
    }
    if let Some((active, paired)) = f.machines {
        rows.push(fact_row(
            "Machines",
            format!("{paired} paired, {active} active now"),
            false,
        ));
    }
    view! {
        <dl class="adi-new-about__facts">
            {rows.into_iter().collect_view()}
        </dl>
    }
}

/// One plain `dt`/`dd` pair.
fn fact_row(k: &'static str, v: String, mono: bool) -> AnyView {
    view! {
        <dt>{k}</dt>
        <dd class:mono=mono>{v}</dd>
    }
    .into_any()
}

/// "Updates": the one row that is also a door. The About window has no pill and no menu of
/// its own, so this is the only way into [`update::dialog`] on this screen — the same dialog
/// the pill and the ⌘K row open everywhere else, not a fourth copy of the flow.
fn updates_row(updates: update::UpdateWatch, u: &UpdateState) -> AnyView {
    let text = if u.installing {
        "Installing…".to_string()
    } else if u.update_available {
        format!("{} available", u.latest.clone().unwrap_or_default())
    } else if u.outcome.as_deref() == Some("error") {
        "Could not check".to_string()
    } else if u.outcome.as_deref() == Some("rolled-back") {
        "Last update rolled back".to_string()
    } else {
        "Up to date".to_string()
    };
    view! {
        <dt>"Updates"</dt>
        <dd>
            <button
                type="button"
                class="cursor-pointer underline decoration-ink-3 underline-offset-[3px] \
                       hover:text-ink"
                on:click=move |_| updates.open_dialog()
            >
                {text}
            </button>
        </dd>
    }
    .into_any()
}

/// `macos` → "macOS", and so on; anything unknown as it came.
fn platform_name(p: &str) -> String {
    match p {
        "macos" => "macOS".into(),
        "linux" => "Linux".into(),
        "windows" => "Windows".into(),
        other => other.into(),
    }
}
