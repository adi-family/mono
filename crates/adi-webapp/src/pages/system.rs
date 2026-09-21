//! The System page: every managed service's live status and its own actions, the platform-wide
//! power switch, a restart, updates, and a diagnostic report to send — the web twin of the mac
//! app's menu-bar window (`apps/macos/Sources/AppModel.swift`, `Maintenance.swift`).
//!
//! **Always about this machine.** `/api/system/*` never follows the panel-wide source picker
//! (`docs/fleet.md` §14's L3, alongside `/api/health` and `/api/update`) — "restart this
//! machine's own process" can only ever mean the box actually serving this page, whatever the
//! picker is pointed at. [`elsewhere_note`] is the page's own honesty about that, since a bare
//! `crate::ui::confirm` would otherwise (wrongly) name the picker's node in the confirm text.
//!
//! **The dangerous half never touches DNS.** Restart bounces every *other* running service
//! (`adi_core::Adi::restart`); the platform-wide power switch inherits whatever
//! `adi-mono enable`/`disable` already do, DNS included, and says so plainly in its own confirm
//! text rather than being filtered to match Restart's narrower promise.

use gloo_timers::callback::Interval;
use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use adi_ui::{Button, ButtonSize, ButtonVariant};
use adi_webapp_api::types::{DiagnosticReport, SystemAction, SystemService, SystemStatus};

use crate::fetch;
use crate::icons;
use crate::state::{Flash, State};
use crate::ui::confirm_local;
use crate::update::{self, UpdateWatch};

/// How often the page polls `/api/system` while nothing is mid-outage.
const POLL_MS: u32 = 4_000;

/// How long to keep polling `/api/health` after a restart or a power-on before giving up and
/// saying so — matches `adi-core`'s own `Update::HEALTH_TIMEOUT`, the budget a cold start (bind
/// ports, read the store, on a node wait for the mesh) is already given elsewhere.
const HEALTH_POLL_TRIES: u32 = 90;
const HEALTH_POLL_MS: u32 = 1_000;

/// What the page shows instead of the ordinary panels while the platform is between requests
/// nobody here can answer — because the very process serving them was just asked to go away.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outage {
    /// A restart or a power-on was accepted; waiting for `/api/health` to answer again.
    ComingBack,
    /// The wait above ran out — still not answering.
    TimedOut,
    /// The platform was turned off on purpose; nothing here will answer again until it isn't.
    Off,
}

/// The diagnostic report row's state — mirrors the mac app's `AppModel.ReportState`.
#[derive(Clone)]
enum ReportState {
    Idle,
    Collecting,
    Ready(DiagnosticReport),
    Failed,
}

/// The System page's own signals — self-contained rather than wired into the global [`State`]'s
/// poll (`crate::state::load`): nothing else on the panel needs this data, the way nothing else
/// needs the update pill's.
#[derive(Clone, Copy)]
struct SystemWatch {
    status: RwSignal<Option<SystemStatus>>,
    busy: RwSignal<bool>,
    outage: RwSignal<Option<Outage>>,
    report: RwSignal<ReportState>,
}

/// Start polling `/api/system`, pausing for as long as [`Outage`] holds.
fn watch(state: State) -> SystemWatch {
    let watch = SystemWatch {
        status: RwSignal::new(None),
        busy: RwSignal::new(false),
        outage: RwSignal::new(None),
        report: RwSignal::new(ReportState::Idle),
    };
    refresh(state, watch);
    Interval::new(POLL_MS, move || {
        if watch.outage.get_untracked().is_none() {
            refresh(state, watch);
        }
    })
    .forget();
    watch
}

fn refresh(state: State, watch: SystemWatch) {
    spawn_local(async move {
        match fetch::system_status().await {
            Ok(s) => watch.status.set(Some(s)),
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Poll `/api/health` until it answers or the budget runs out, then reload the page — or, on a
/// timeout, say so instead of spinning forever. `/api/health` is always local
/// ([`fetch::health`]), which is what makes this correct even though `watch.status` itself never
/// refreshes during the outage: the page is not reading its own state back, it is asking whether
/// anything is home yet.
fn wait_then_reload(watch: SystemWatch) {
    spawn_local(async move {
        for _ in 0..HEALTH_POLL_TRIES {
            TimeoutFuture::new(HEALTH_POLL_MS).await;
            if fetch::health().await.is_ok() {
                if let Some(w) = web_sys::window() {
                    let _ = w.location().reload();
                }
                return;
            }
        }
        watch.outage.set(Some(Outage::TimedOut));
    });
}

/// A live agent-run count, worded for a confirm dialog: the number a person about to press a
/// destructive button should see in front of them, not have to go check on another page first.
fn runs_clause(live_runs: u32) -> String {
    match live_runs {
        0 => "No agent runs are live right now.".to_string(),
        1 => "1 agent run is live right now and will end with it.".to_string(),
        n => format!("{n} agent runs are live right now and will end with it."),
    }
}

/// The System page. The ordinary panels are dropped entirely for as long as [`Outage`] holds —
/// not just left showing stale, pre-outage numbers beside a banner saying not to trust them: a
/// "Turn off" button that stayed on screen (and enabled) after the platform already turned off
/// would be it lying about what is about to happen a second time.
pub(crate) fn system_view(state: State, updates: UpdateWatch) -> AnyView {
    let watch = watch(state);
    view! {
        <div class="flex flex-col gap-6">
            {move || state.panel_source.get().map(elsewhere_note)}
            {move || outage_banner(watch)}
            {move || watch.outage.get().is_none().then(|| panels(state, watch, updates))}
        </div>
    }
    .into_any()
}

fn panels(state: State, watch: SystemWatch, updates: UpdateWatch) -> AnyView {
    view! {
        <div class="flex flex-col gap-6">
            <section class="adi-panel">
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">"Services"</h2>
                    <span class="adi-updated">
                        {move || watch.status.get().map(|s| {
                            let running = s.services.iter().filter(|r| r.running).count();
                            format!("{} declared \u{b7} {running} running", s.services.len())
                        })}
                    </span>
                </div>
                {move || services_view(watch)}
            </section>

            <section class="adi-panel">
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">"Power"</h2>
                </div>
                {move || power_view(state, watch)}
            </section>

            <section class="adi-panel">
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">"Restart"</h2>
                </div>
                {move || restart_view(state, watch)}
            </section>

            <section class="adi-panel">
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">"Updates"</h2>
                </div>
                <div class="flex items-center gap-3">
                    {update::version_pill(updates)}
                </div>
            </section>

            <section class="adi-panel">
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">"Report a problem"</h2>
                </div>
                {move || report_view(state, watch, updates)}
            </section>
        </div>
    }
    .into_any()
}

/// The panel-wide picker is pointed somewhere else — said loudly, because `crate::ui::confirm`
/// would otherwise name that node in a confirm dialog for a control this page never actually
/// routes to it (`/api/system/*` is always local).
fn elsewhere_note(node: String) -> AnyView {
    view! {
        <div class="adi-flash" data-kind="note">
            "System always shows and controls this machine \u{2014} not "
            <span class="adi-mono">{node}</span>
            ", which the picker above is pointed at."
        </div>
    }
    .into_any()
}

fn outage_banner(watch: SystemWatch) -> Option<AnyView> {
    let (kind, text) = match watch.outage.get()? {
        Outage::ComingBack => ("note", "Coming back \u{2014} waiting for this machine to answer again\u{2026}"),
        Outage::TimedOut => ("err", "Still not answering. Check the logs on this machine, or reload this page yourself once it is."),
        Outage::Off => ("note", "adi is off on this machine. Turn it back on above to reach the rest of this panel."),
    };
    Some(view! { <div class="adi-flash" data-kind=kind>{text}</div> }.into_any())
}

fn services_view(watch: SystemWatch) -> AnyView {
    let Some(status) = watch.status.get() else {
        return view! { <p class="adi-muted">"Loading\u{2026}"</p> }.into_any();
    };
    if status.services.is_empty() {
        return view! { <p class="adi-muted">"No managed services."</p> }.into_any();
    }
    status
        .services
        .into_iter()
        .map(|svc| service_row(watch, svc))
        .collect::<Vec<_>>()
        .into_any()
}

/// One service's row: its live state and, except for the `app` service (this very process — see
/// the module header), its own actions as buttons. `app`'s only actions are the page's dedicated
/// Power/Restart controls below, which is why nothing here is rendered for it beyond the status.
fn service_row(watch: SystemWatch, svc: SystemService) -> AnyView {
    let state_attr = if svc.running {
        "online"
    } else if svc.enabled {
        ""
    } else {
        "down"
    };
    let is_app = svc.id == "app";
    // The updater's own "check for updates now" action installs in place — the same hazard the
    // page's own Restart/Power controls exist to handle safely — and is redundant with the
    // dedicated Updates panel above; every other action a service offers is safe to run in place.
    let actions: Vec<SystemAction> = svc
        .actions
        .into_iter()
        .filter(|a| !is_app && a.args != ["update".to_string(), "run".to_string()])
        .collect();
    view! {
        <div class="flex items-center gap-3 border-b border-line py-2 last:border-b-0">
            <span class="adi-status" data-state=state_attr>
                <span class="adi-status__led"></span>
            </span>
            <div class="min-w-0 flex-1">
                <div class="text-ui text-ink">{svc.name}</div>
                <div class="text-small text-ink-3">{svc.detail}</div>
            </div>
            <div class="flex shrink-0 items-center gap-2">
                {actions.into_iter().map(|a| action_button(watch, a)).collect::<Vec<_>>()}
            </div>
        </div>
    }
    .into_any()
}

fn action_button(watch: SystemWatch, action: SystemAction) -> AnyView {
    let title = action.title.clone();
    let args = action.args;
    view! {
        <button class="adi-btn adi-btn--ghost" type="button"
            prop:disabled=move || watch.busy.get()
            on:click=move |_| run_action(watch, args.clone())>
            {title}
        </button>
    }
    .into_any()
}

fn run_action(watch: SystemWatch, args: Vec<String>) {
    if watch.busy.get_untracked() {
        return;
    }
    watch.busy.set(true);
    spawn_local(async move {
        match fetch::run_system_action(args).await {
            Ok(s) => watch.status.set(Some(s)),
            Err(_) => {
                // The service's own row still shows what actually happened on the next poll;
                // nothing here has a flash line of its own to write a one-off failure into.
                refresh_after_busy(watch).await;
            }
        }
        watch.busy.set(false);
    });
}

async fn refresh_after_busy(watch: SystemWatch) {
    if let Ok(s) = fetch::system_status().await {
        watch.status.set(Some(s));
    }
}

/// On == at least one service is enabled — the same reading `apps/macos`'s `AppModel.isOn` uses.
fn is_on(status: &SystemStatus) -> bool {
    status.services.iter().any(|s| s.enabled)
}

fn power_view(state: State, watch: SystemWatch) -> AnyView {
    let Some(status) = watch.status.get() else {
        return view! { <p class="adi-muted">"Loading\u{2026}"</p> }.into_any();
    };
    let on = is_on(&status);
    let live_runs = status.live_runs;
    view! {
        <div class="flex items-start justify-between gap-4">
            <p class="text-small text-ink-3 max-w-prose">
                "The same switch as `adi-mono enable` / `adi-mono disable` \u{2014} every managed \
                service, DNS included. Turning it off stops `.adi` name resolution on this \
                machine until it is turned back on."
            </p>
            <Button
                size=ButtonSize::Small
                variant=if on { ButtonVariant::Danger } else { ButtonVariant::Primary }
                icon=icons::Icon::Power.lucide()
                disabled=watch.busy
                on:click=move |_| set_power(state, watch, !on, live_runs)
            >
                {if on { "Turn off" } else { "Turn on" }}
            </Button>
        </div>
    }
    .into_any()
}

fn set_power(state: State, watch: SystemWatch, on: bool, live_runs: u32) {
    if watch.busy.get_untracked() {
        return;
    }
    if !on {
        let msg = format!(
            "Turn adi off on this machine?\n\n{} This also stops the .adi DNS resolver and \
             front door here \u{2014} .adi names will stop resolving on this machine until you \
             turn it back on.",
            runs_clause(live_runs)
        );
        if !confirm_local(&msg) {
            return;
        }
    }
    watch.busy.set(true);
    spawn_local(async move {
        match fetch::set_system_power(on).await {
            Ok(_) => {
                watch.outage.set(Some(if on {
                    Outage::ComingBack
                } else {
                    Outage::Off
                }));
                if on {
                    wait_then_reload(watch);
                }
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        watch.busy.set(false);
    });
}

fn restart_view(state: State, watch: SystemWatch) -> AnyView {
    let Some(status) = watch.status.get() else {
        return view! { <p class="adi-muted">"Loading\u{2026}"</p> }.into_any();
    };
    let live_runs = status.live_runs;
    view! {
        <div class="flex items-start justify-between gap-4">
            <p class="text-small text-ink-3 max-w-prose">
                "Bounces every running service except DNS, so each one picks itself back up from \
                whatever is on disk right now \u{2014} the same fix as toggling services off and \
                on, and what actually swaps a running process after an update. `.adi` name \
                resolution is never touched by this."
            </p>
            <Button
                size=ButtonSize::Small
                variant=ButtonVariant::Danger
                icon=icons::Icon::Restart.lucide()
                disabled=watch.busy
                on:click=move |_| restart(state, watch, live_runs)
            >
                "Restart"
            </Button>
        </div>
    }
    .into_any()
}

fn restart(state: State, watch: SystemWatch, live_runs: u32) {
    if watch.busy.get_untracked() {
        return;
    }
    let msg = format!(
        "Restart adi on this machine?\n\n{} DNS is not touched \u{2014} .adi names keep \
         resolving throughout.",
        runs_clause(live_runs)
    );
    if !confirm_local(&msg) {
        return;
    }
    watch.busy.set(true);
    spawn_local(async move {
        match fetch::restart_system().await {
            Ok(_) => {
                watch.outage.set(Some(Outage::ComingBack));
                wait_then_reload(watch);
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        watch.busy.set(false);
    });
}

fn report_view(state: State, watch: SystemWatch, updates: UpdateWatch) -> AnyView {
    let report = watch.report.get();
    let (note, button_label, collecting) = match &report {
        ReportState::Idle => (
            "Collect the logs, routes and every service's state into one file, then open an \
             issue with it.".to_string(),
            "Create report",
            false,
        ),
        ReportState::Collecting => ("Reading logs, services and routes\u{2026}".to_string(), "Create report", true),
        ReportState::Ready(r) => {
            let note = if r.findings.is_empty() {
                "Download it and drag it into the issue you open.".to_string()
            } else {
                format!(
                    "Download it and drag it into the issue you open. {} thing{} already look \
                     wrong \u{2014} summary.txt says which.",
                    r.findings.len(),
                    if r.findings.len() == 1 { "" } else { "s" }
                )
            };
            (note, "Create another report", false)
        }
        ReportState::Failed => ("It could not be written. Try again.".to_string(), "Create report", false),
    };
    let body = issue_body(
        watch.status.get_untracked().as_ref(),
        report_bundle(&report),
        updates.identity(),
    );
    view! {
        <div class="flex flex-col gap-3">
            <p class="text-small text-ink-3">{note}</p>
            <div class="flex flex-wrap items-center gap-2">
                <Button
                    size=ButtonSize::Small
                    icon=icons::Icon::Bug.lucide()
                    disabled=collecting
                    on:click=move |_| create_report(state, watch)
                >
                    {button_label}
                </Button>
                {report_download_link(&report)}
                <Button
                    size=ButtonSize::Small
                    variant=ButtonVariant::Ghost
                    icon=icons::Icon::ExternalLink.lucide()
                    attr:title="Open a pre-filled GitHub issue, then drag the report into it"
                    on:click=move |_| open_issue(&body)
                >
                    "Open an issue"
                </Button>
            </div>
        </div>
    }
    .into_any()
}

fn report_bundle(report: &ReportState) -> Option<&DiagnosticReport> {
    match report {
        ReportState::Ready(r) => Some(r),
        _ => None,
    }
}

fn report_download_link(report: &ReportState) -> Option<AnyView> {
    let ReportState::Ready(r) = report else {
        return None;
    };
    Some(
        view! {
            <a class="adi-btn" href=r.download_url.clone() download=r.file_name.clone()>
                "Download " {r.file_name.clone()}
            </a>
        }
        .into_any(),
    )
}

fn create_report(state: State, watch: SystemWatch) {
    if matches!(watch.report.get_untracked(), ReportState::Collecting) {
        return;
    }
    watch.report.set(ReportState::Collecting);
    spawn_local(async move {
        match fetch::diagnose_system().await {
            Ok(r) => watch.report.set(ReportState::Ready(r)),
            Err(e) => {
                watch.report.set(ReportState::Failed);
                state.flash.set(Some(Flash::err(e)));
            }
        }
    });
}

/// Where a bug gets filed — mirrors `apps/macos/Sources/Core.swift`'s `issuesURL` default; the
/// web build carries no per-flavour override of its own.
const ISSUES_URL: &str = "https://github.com/adi-family/mono/issues/new";

/// How much of the draft to put in the URL — GitHub, and the browser, both refuse a request past
/// some length; a few hundred characters of draft never comes close except on a machine with an
/// implausible number of findings.
const ISSUE_BODY_LIMIT: usize = 6_000;

/// The draft: a space to write in, then the state of this install — mirrors `AppModel.issueBody`.
/// The one thing it cannot carry is the archive itself: GitHub takes an attachment only from a
/// drop onto the form, so the draft ends by saying so.
fn issue_body(
    status: Option<&SystemStatus>,
    report: Option<&DiagnosticReport>,
    identity: Option<(String, String)>,
) -> String {
    let mut lines = vec![
        "<!-- What happened, and what you expected instead. -->".to_string(),
        String::new(),
        String::new(),
        "---".to_string(),
    ];
    if let Some((installed, platform)) = identity {
        lines.push(String::new());
        lines.push(format!("**adi** {installed} \u{b7} {platform}"));
    }
    if let Some(status) = status
        && !status.services.is_empty()
    {
        let lines_of_state: Vec<String> = status
            .services
            .iter()
            .map(|s| {
                let word = if s.running {
                    "running"
                } else if s.enabled {
                    "enabled, not running"
                } else {
                    "off"
                };
                format!("{}: {word}", s.name)
            })
            .collect();
        lines.push(String::new());
        lines.push(format!("**Services** \u{2014} {}", lines_of_state.join(" \u{b7} ")));
    }
    if let Some(bundle) = report {
        if !bundle.findings.is_empty() {
            lines.push(String::new());
            lines.push("**The report already flags**".to_string());
            for finding in bundle.findings.iter().take(10) {
                lines.push(format!("- {finding}"));
            }
        }
        lines.push(String::new());
        lines.push(format!(
            "Attached: `{}` \u{2014} download it above and drag it into this box; it has the logs.",
            bundle.file_name
        ));
    } else {
        lines.push(String::new());
        lines.push(
            "Press **Create report** above and drag the archive it makes into this box \u{2014} \
             it carries the logs, the routes and every service's state."
                .to_string(),
        );
    }
    lines.join("\n")
}

/// Open a new GitHub issue with what we already know filled in.
fn open_issue(body: &str) {
    let truncated: String = body.chars().take(ISSUE_BODY_LIMIT).collect();
    let Some(encoded) = js_sys::encode_uri_component(&truncated).as_string() else {
        return;
    };
    let url = format!("{ISSUES_URL}?body={encoded}");
    if let Some(window) = web_sys::window() {
        let _ = window.open_with_url_and_target(&url, "_blank");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_clause_is_singular_only_at_one() {
        assert_eq!(runs_clause(0), "No agent runs are live right now.");
        assert_eq!(runs_clause(1), "1 agent run is live right now and will end with it.");
        assert_eq!(
            runs_clause(2),
            "2 agent runs are live right now and will end with it."
        );
    }

    #[test]
    fn the_issue_body_never_claims_a_report_when_there_is_none() {
        let body = issue_body(None, None, None);
        assert!(body.contains("Press **Create report**"));
        assert!(!body.contains("Attached:"));
    }

    #[test]
    fn the_issue_body_names_the_archive_once_one_exists() {
        let bundle = DiagnosticReport {
            download_url: "/api/system/diagnose/download/x.zip".to_string(),
            file_name: "x.zip".to_string(),
            bytes: 10,
            files: vec![],
            findings: vec!["dns route missing".to_string()],
        };
        let body = issue_body(None, Some(&bundle), None);
        assert!(body.contains("Attached: `x.zip`"));
        assert!(body.contains("dns route missing"));
        assert!(!body.contains("Press **Create report**"));
    }
}
