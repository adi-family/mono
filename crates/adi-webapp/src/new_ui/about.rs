//! "About adi", at `/about` — the new UI's About This Mac: the mark, the version, and a few lines
//! on the instance this page is talking to. Opened from the mark in the top bar.
//!
//! Everything here is read from endpoints the panel already serves — `/api/update`, `/api/health`,
//! `/api/system`, `/api/fleet` — once, as the window opens. An About window is looked at, not
//! watched. A read that fails leaves its lines out rather than drawing them empty.

use adi_ui::Mark;
use leptos::prelude::*;

use crate::{fetch, ui};

/// What the window shows, gathered from four reads; each part stays `None` until its read
/// answers, and for good if it fails.
#[derive(Clone, Default)]
struct Facts {
    /// `(installed, running)` — they differ on a development build.
    version: Option<(String, String)>,
    platform: Option<String>,
    update: Option<String>,
    uptime: Option<u64>,
    /// `(running, total)`.
    services: Option<(usize, usize)>,
    agents: Option<u32>,
    /// `(active, paired)`.
    machines: Option<(usize, usize)>,
}

#[component]
pub(super) fn About() -> impl IntoView {
    let facts = RwSignal::new(Facts::default());
    // Four independent reads, each filling its own part as it answers — side by side, not one
    // after another, and a slow or failed one holds up nothing else.
    leptos::task::spawn_local(async move {
        if let Ok(u) = fetch::update_state().await {
            facts.update(|f| {
                f.version = Some((u.installed.clone(), u.running.clone()));
                f.platform = Some(u.platform.clone());
                f.update = Some(match (u.update_available, u.latest) {
                    (true, Some(latest)) => format!("{latest} available"),
                    _ => "Up to date".into(),
                });
            });
        }
    });
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
                {move || match facts.get().version {
                    Some((installed, _)) => format!("Version {installed}"),
                    // Holds the line's height, so the rows below do not jump when it lands.
                    None => "\u{00a0}".into(),
                }}
            </p>
            {move || rows(facts.get(), host.clone())}
        </div>
    }
}

/// The key-value lines, in the order macOS's About window puts its own: what it is first, what
/// it is doing after.
fn rows(f: Facts, host: String) -> impl IntoView {
    let mut rows: Vec<(&'static str, String, bool)> = vec![("Address", host, true)];
    if let Some((installed, running)) = &f.version
        && installed != running
    {
        // A development build reports its crate version, not the release it came from — worth
        // saying, because it is why the two lines disagree.
        rows.push(("Running build", running.clone(), true));
    }
    if let Some(p) = f.platform {
        rows.push(("Platform", platform_name(&p), false));
    }
    if let Some(u) = f.update {
        rows.push(("Updates", u, false));
    }
    if let Some(s) = f.uptime {
        rows.push(("Up for", ui::fmt_uptime(s), false));
    }
    if let Some((running, total)) = f.services {
        rows.push(("Services", format!("{running} of {total} running"), false));
    }
    if let Some(n) = f.agents {
        let runs = match n {
            0 => "None running".to_string(),
            1 => "1 running".to_string(),
            n => format!("{n} running"),
        };
        rows.push(("Agents", runs, false));
    }
    if let Some((active, paired)) = f.machines {
        rows.push((
            "Machines",
            format!("{paired} paired, {active} active now"),
            false,
        ));
    }
    view! {
        <dl class="adi-new-about__facts">
            {rows
                .into_iter()
                .map(|(k, v, mono)| view! {
                    <dt>{k}</dt>
                    <dd class:mono=mono>{v}</dd>
                })
                .collect_view()}
        </dl>
    }
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
