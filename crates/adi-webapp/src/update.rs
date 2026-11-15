//! The version pill in the top bar, the ⌘K menu's version row, and the About screen's version
//! line: three doors into the same thing — [`dialog`] — so a click answers the same way
//! wherever it was made.
//!
//! The chip is identity: it is always there and it says the installed version. The button
//! beside it, while there is one, is a louder version of the same door. **Neither checks nor
//! installs by itself any more** — clicking either only opens the dialog, which is where a
//! check, a result and an install all happen in view, including the install's own progress
//! and its restart outcome. A click that silently asked the manifest and then said nothing
//! (the state this module used to leave a quiet check in) is the bug this file now refuses to
//! reproduce: every door opens *something*, every time.
//!
//! Nothing here knows a DMG from a tarball. The server hands back the same
//! [`UpdateState`] on every platform, so a Mac, a Linux node and a
//! Windows node all get this bar — and a release that publishes no artifact for the host
//! reads as "no update for this machine", not as an offer that would fail to download.
//!
//! **The install restarts the app.** `POST /api/update/run` answers as soon as the updater
//! is running, and a minute later that updater kickstarts the service this page is talking
//! to. So a failed poll must not clear what we know: the pill keeps saying "updating…"
//! through the gap, the dialog keeps showing the installing state, and both settle on the new
//! version once the backend answers again.

use gloo_timers::callback::Interval;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use adi_ui::{Button, ButtonSize, ButtonVariant, Icon, IconSize, Lucide, Markdown, Modal};
use adi_webapp_api::types::UpdateState;

use crate::fetch;
use crate::icons;
use crate::launcher::Action;

/// The base tick. `/api/update` reads two small files, so this is cheap — but it is still a
/// request per tab, and the answer changes about twice a day.
const IDLE_TICK_MS: u32 = 3_000;

/// Ticks skipped between polls while nothing is happening: 3s × 20 = once a minute. While an
/// install *is* running every tick polls, because that is the minute in which the answer
/// changes and the page has just told the user to wait.
const IDLE_EVERY: u32 = 20;

/// The pill's state: the machine's, plus whether a call this page made is in flight.
#[derive(Clone, Copy)]
pub(crate) struct UpdateWatch {
    /// The last answer from `/api/update`. `None` only before the first one lands — after
    /// that it is never cleared, so a backend that went away mid-update keeps showing what
    /// it last said instead of blanking the bar.
    state: RwSignal<Option<UpdateState>>,
    /// A check or an install this pill started hasn't answered yet. Distinct from
    /// [`UpdateState::installing`], which is the *machine's* business and outlives the page.
    busy: RwSignal<bool>,
    /// Whether [`dialog`] is open. Here rather than in [`version_pill`] because the bar it
    /// renders into is rebuilt whenever the page's own state moves, and a dialog that closes
    /// itself because something behind it changed is a dialog nobody can finish reading.
    open: RwSignal<bool>,
}

/// Start watching for updates: read the state, ask the manifest if the record has gone
/// stale, and keep both fresh.
///
/// Call once per mounted app — it owns a timer, the way [`crate::pwa::installable`] owns an
/// event handler. The four entry points in [`main`](../main.rs) and [`crate::new_ui`] are
/// mutually exclusive, so that holds; each one also mounts [`dialog`] exactly once beside it,
/// so the version row it carries always has somewhere to open into.
pub(crate) fn watch() -> UpdateWatch {
    let watch = UpdateWatch {
        state: RwSignal::new(None),
        busy: RwSignal::new(false),
        open: RwSignal::new(false),
    };

    // The first read, and — only if the server says its record is older than the configured
    // interval — one trip to the release manifest. That second call is what makes the pill
    // right on a machine where the periodic background updater is switched off; the staleness
    // gate is what stops every tab from making it on every load.
    spawn_local(async move {
        if let Ok(u) = fetch::update_state().await {
            let stale = u.stale;
            watch.state.set(Some(u));
            if stale && let Ok(fresh) = fetch::check_update().await {
                watch.state.set(Some(fresh));
            }
        }
    });

    let ticks = RwSignal::new(0u32);
    Interval::new(IDLE_TICK_MS, move || {
        // Only an install earns every tick. Staleness deliberately does *not*: `/api/update`
        // never clears it (only a check does), so polling on it would spin at 3s forever on a
        // machine that is offline — which is precisely the machine that stays stale.
        let installing = watch.state.get_untracked().is_some_and(|u| u.installing);
        let n = ticks.get_untracked().wrapping_add(1);
        ticks.set(n);
        if !installing && !n.is_multiple_of(IDLE_EVERY) {
            return;
        }
        spawn_local(async move {
            // A failed read is left on the floor on purpose: mid-update the server is gone by
            // design, and blanking the pill would replace "updating…" with nothing at exactly
            // the moment the user is watching it.
            if let Ok(u) = fetch::update_state().await {
                watch.state.set(Some(u));
            }
        });
    })
    .forget();

    watch
}

impl UpdateWatch {
    /// The installed version and this host's platform key, once the first answer has landed —
    /// for anything that wants to name this build without polling `/api/update` a second time
    /// (the System page's issue draft).
    pub(crate) fn identity(self) -> Option<(String, String)> {
        self.state
            .get_untracked()
            .map(|u| (u.installed, u.platform))
    }

    /// The watched state, read reactively — for the one reader that renders off it directly
    /// rather than through [`dialog`] or [`version_pill`]: the About screen's version line and
    /// its own "Updates" row, which needs the live value to stay right while the window sits
    /// open.
    pub(crate) fn state(self) -> Option<UpdateState> {
        self.state.get()
    }

    /// Open [`dialog`] from outside this module — the About screen's "Updates" row has no
    /// pill or menu row of its own to borrow the click from.
    pub(crate) fn open_dialog(self) {
        self.open.set(true);
    }

    /// Ask the release manifest now — what the dialog's Check button does.
    fn check(self) {
        if self.busy.get_untracked() {
            return;
        }
        self.busy.set(true);
        spawn_local(async move {
            if let Ok(u) = fetch::check_update().await {
                self.state.set(Some(u));
            }
            self.busy.set(false);
        });
    }

    /// Hand the install to the updater — what the dialog's Install button ends in.
    fn install(self) {
        if self.busy.get_untracked() {
            return;
        }
        self.busy.set(true);
        spawn_local(async move {
            match fetch::run_update().await {
                // Left open on purpose, unlike the old pill-and-button pair this replaced: the
                // dialog is now where install progress and the restart outcome are watched, so
                // closing it the moment the install starts would hide the one thing it was just
                // opened to show. The server's own reply already carries `installing: true`
                // (see the handler), so the next render shows the progress state immediately.
                Ok(u) => self.state.set(Some(u)),
                // Report it where the user is already looking rather than through the page's
                // flash: this control lives in a bar shared by three different screens, and
                // only one of them has a flash line to write to.
                Err(e) => self.state.update(|s| {
                    if let Some(s) = s {
                        s.outcome = Some("error".to_string());
                        s.error = Some(e);
                        s.error_kind = None;
                    }
                }),
            }
            self.busy.set(false);
        });
    }
}

/// The version, as a row in the ⌘K menu.
///
/// Every state the row can be in opens the same [`dialog`] — the row only ever changes what
/// it *says*, never what pressing it does. That is the whole fix for the bug this module was
/// rewritten over: a click used to sometimes ask the manifest in silence and sometimes open an
/// offer, so a reader who got the silent branch learned that the version row does nothing.
pub(crate) fn action(watch: UpdateWatch) -> Action {
    let Some(u) = watch.state.get() else {
        // Before the first answer we do not know the version, so the row offers the one thing
        // that is true regardless: open the dialog, which will say so itself.
        return Action::new("Check for updates", "", icons::Icon::Box, move || {
            watch.open.set(true);
        });
    };
    if u.installing {
        return Action::new(
            "Updating\u{2026}",
            "The stack is restarting onto the new version",
            icons::Icon::Upgrade,
            move || watch.open.set(true),
        );
    }
    match u.latest.clone().filter(|_| u.update_available) {
        Some(latest) => Action::new(
            format!("Update to {latest}"),
            "See what is in it, then install",
            icons::Icon::Upgrade,
            move || watch.open.set(true),
        ),
        None => Action::new(
            format!("adi v{}", u.installed),
            "Check for a newer release",
            icons::Icon::Box,
            move || watch.open.set(true),
        ),
    }
}

/// The dialog, mounted on its own.
///
/// [`version_pill`] stands beside this rather than carrying it, so the panel, the root screen,
/// the marketplace door and the About window all open the exact same thing — one modal per
/// mounted [`UpdateWatch`], never rebuilt while it is open: only [`body`] re-renders as the
/// state underneath it moves, which is what lets the install's progress tick by without the
/// header, the scroll position or the close button ever flickering.
pub(crate) fn dialog(watch: UpdateWatch) -> impl IntoView {
    view! {
        <Modal open=watch.open title="Software update">
            {move || body(watch)}
        </Modal>
    }
}

/// The pill itself: the version chip, and — while there is one to take — a louder button
/// beside it. Both only open [`dialog`]; see the module header for why that is the whole
/// point of this rewrite.
pub(crate) fn version_pill(watch: UpdateWatch) -> impl IntoView {
    move || {
        watch.state.get().map(|u| {
            let offer = u.update_available && !u.installing;
            let latest = u.latest.clone().unwrap_or_default();
            let label = if u.installing {
                "updating…".to_string()
            } else {
                format!("v{}", u.installed)
            };
            // A rollback is the one state that has to catch the eye: the machine is running
            // the version it started on because the new one failed its health check, and
            // nothing else on screen says so.
            let tone = if u.outcome.as_deref() == Some("rolled-back") {
                "text-err hover:text-err"
            } else {
                "text-ink-3 hover:text-ink-2"
            };
            view! {
                <button
                    type="button"
                    class=format!(
                        "inline-flex h-7 shrink-0 cursor-pointer items-center rounded-md \
                         px-2 text-small hover:bg-hover {tone}"
                    )
                    title=tooltip(&u)
                    on:click=move |_| watch.open.set(true)
                >
                    {label}
                </button>
                {offer
                    .then(|| {
                        view! {
                            <Button
                                size=ButtonSize::Small
                                variant=ButtonVariant::Primary
                                icon=icons::Icon::Upgrade.lucide()
                                attr:title=format!("See what is in {latest}, then install it")
                                on:click=move |_| watch.open.set(true)
                            >
                                {format!("Update to {latest}")}
                            </Button>
                        }
                    })}
            }
        })
    }
}

/// The dialog's whole body, rebuilt whenever the watched state or an in-flight call changes —
/// the one reactive seam [`dialog`] has, so everything around it (the title, the close button,
/// the scrim) stays put.
fn body(watch: UpdateWatch) -> AnyView {
    let Some(u) = watch.state.get() else {
        return view! { <p class="text-ui text-ink-2">"Reading the installed version…"</p> }
            .into_any();
    };
    let info = facts(&u).into_any();
    view! {
        <div class="flex flex-col gap-4">
            {info}
            {status(watch, &u)}
        </div>
    }
    .into_any()
}

/// What this machine is on and what it last heard — the part of the dialog that never goes
/// away, whatever [`status`] below it is showing.
fn facts(u: &UpdateState) -> impl IntoView {
    let mut lines = vec![format!("Running {}", u.installed)];
    if u.running != u.installed {
        // Worth saying out loud rather than hiding: the panel you are looking at was built
        // from a checkout, so the number beside it is the bundle's, not this binary's.
        lines.push(format!(
            "This panel is running {} from a checkout",
            u.running
        ));
    }
    match &u.latest {
        Some(latest) => lines.push(format!("Latest published: {latest}")),
        None => lines.push("No release checked yet".to_string()),
    }
    if let Some(secs) = u.checked_secs_ago {
        lines.push(format!("Checked {}", ago(secs)));
    }
    view! {
        <div class="flex flex-col gap-1 text-small text-ink-3">
            {lines.into_iter().map(|l| view! { <p>{l}</p> }).collect_view()}
        </div>
    }
}

/// The dialog's result: checking, installing, a rollback, an error, an offer, or plain
/// up-to-date — exactly one of these at a time, in the order a reader should hear them in.
fn status(watch: UpdateWatch, u: &UpdateState) -> AnyView {
    if u.installing {
        return installing_view().into_any();
    }
    if watch.busy.get() {
        return checking_view().into_any();
    }
    if u.outcome.as_deref() == Some("rolled-back") {
        return rolled_back_view(watch, u).into_any();
    }
    if u.outcome.as_deref() == Some("error") {
        return error_view(watch, u).into_any();
    }
    match u.latest.clone().filter(|_| u.update_available) {
        Some(latest) => offer_view(watch, &latest, u.notes.clone()).into_any(),
        None => up_to_date_view(watch, u).into_any(),
    }
}

fn checking_view() -> impl IntoView {
    view! {
        <div class="flex items-center gap-2 text-ui text-ink-2">
            <Icon icon=Lucide::LoaderCircle size=IconSize::Md class="animate-spin" label="Checking"/>
            "Checking for updates…"
        </div>
    }
}

fn installing_view() -> impl IntoView {
    view! {
        <div class="flex flex-col gap-2">
            <div class="flex items-center gap-2 text-ui text-ink-2">
                <Icon
                    icon=Lucide::LoaderCircle
                    size=IconSize::Md
                    class="animate-spin"
                    label="Installing"
                />
                "Installing the update."
            </div>
            <p class="text-small text-ink-3">
                "The stack is restarting onto the new version. This page reconnects on its own, "
                "and the update rolls itself back automatically if the services do not come up."
            </p>
        </div>
    }
}

fn rolled_back_view(watch: UpdateWatch, u: &UpdateState) -> impl IntoView {
    let detail = u.error.clone();
    let installed = u.installed.clone();
    view! {
        <div class="flex flex-col gap-3">
            <p class="text-ui text-err">
                {format!("The last update was rolled back — this machine is back on {installed}.")}
            </p>
            {detail.map(|d| view! { <p class="text-small text-ink-3">{d}</p> })}
            <div class="flex justify-end">
                <Button disabled=watch.busy on:click=move |_| watch.check()>
                    "Check again"
                </Button>
            </div>
        </div>
    }
}

fn error_view(watch: UpdateWatch, u: &UpdateState) -> impl IntoView {
    let human = human_error(u);
    let detail = u.error.clone();
    view! {
        <div class="flex flex-col gap-3">
            <p class="text-ui text-ink">{human}</p>
            {detail.map(|d| view! { <p class="text-small text-ink-3">{d}</p> })}
            <div class="flex justify-end">
                <Button disabled=watch.busy on:click=move |_| watch.check()>
                    "Try again"
                </Button>
            </div>
        </div>
    }
}

/// `error`'s plain-English reading. [`UpdateState::error_kind`] is a hint off the raw message
/// (`handlers/update.rs`'s `classify_error`, matched here by the same string), not a promise —
/// an unrecognised shape falls back to the generic line rather than guessing at one.
fn human_error(u: &UpdateState) -> String {
    match u.error_kind.as_deref() {
        Some("private-channel") => {
            "This machine's update channel is private and has no access token configured, so \
             it cannot see a new release until one is added."
                .to_string()
        }
        Some("licence-unauthenticated") => {
            "This machine isn't licensed. Open Settings → Licence to add one.".to_string()
        }
        Some("licence-expired") => {
            "This machine's licence has expired. Renew it to keep receiving updates."
                .to_string()
        }
        // `licence-unknown` reads the same as a revoked licence: from here a vanished code
        // and a revoked one look identical, and both end the same way — someone has to sort
        // it out on the licence side, not by retrying the fetch.
        Some("licence-revoked" | "licence-unknown") => {
            "This machine's licence was revoked. Contact support@withadi.dev.".to_string()
        }
        Some("licence-seat-conflict") => {
            "This licence's seats are all in use by other machines. Free one up, or contact              support@withadi.dev."
                .to_string()
        }
        Some("offline") => {
            "Could not reach the update server. Check this machine's network connection and \
             try again."
                .to_string()
        }
        _ => "The last attempt to talk to the updater failed.".to_string(),
    }
}

/// The release's own case for being taken: the changelog section published with it, and the
/// two ways out.
///
/// The notes are markdown from `CHANGELOG.md`, rendered by the same component that draws a
/// chat message. They scroll in their own box rather than the dialog's, so a long release
/// never pushes Install off the bottom of the screen.
fn offer_view(watch: UpdateWatch, latest: &str, notes: Option<String>) -> impl IntoView {
    // A release may be cut without notes — by hand, or off another channel. Say so, rather
    // than opening an empty box that reads as a page which failed to load.
    let unnoted = format!("Release {latest} was published without release notes.");
    view! {
        <div class="flex flex-col gap-4">
            <p class="text-ui font-semibold text-ink">{format!("{latest} is available")}</p>
            <div class="max-h-[40vh] overflow-y-auto pr-1">
                {match notes {
                    Some(md) => view! { <Markdown source=md/> }.into_any(),
                    None => view! { <p class="text-ui text-ink-2">{unnoted}</p> }.into_any(),
                }}
            </div>
            <div class="flex flex-wrap items-center justify-between gap-3 border-t \
                        border-line pt-3">
                // The consequence, beside the button rather than discovered after it.
                <p class="min-w-0 flex-1 text-small text-ink-3">
                    "Installing restarts the stack onto the new version. This page "
                    "reconnects on its own, and the update rolls itself back if the "
                    "services do not come up."
                </p>
                <div class="flex shrink-0 items-center gap-2">
                    <Button
                        variant=ButtonVariant::Ghost
                        on:click=move |_| watch.open.set(false)
                    >
                        "Not now"
                    </Button>
                    <Button
                        variant=ButtonVariant::Primary
                        icon=icons::Icon::Upgrade.lucide()
                        disabled=watch.busy
                        on:click=move |_| watch.install()
                    >
                        {format!("Install {latest}")}
                    </Button>
                </div>
            </div>
        </div>
    }
}

fn up_to_date_view(watch: UpdateWatch, u: &UpdateState) -> impl IntoView {
    // A newer version exists but was never offered, because it ships no build for this
    // platform — worth saying, since otherwise this reads identically to "nothing published".
    let no_artifact = match (&u.latest, u.has_artifact) {
        (Some(latest), Some(false)) if latest != &u.installed => Some(format!(
            "{latest} is published but includes no {} build, so it is not offered to this \
             machine.",
            u.platform
        )),
        _ => None,
    };
    view! {
        <div class="flex flex-col gap-3">
            <div class="flex items-center justify-between gap-3">
                <p class="text-ui text-ink-2">"You're up to date."</p>
                <Button disabled=watch.busy on:click=move |_| watch.check()>
                    "Check for updates"
                </Button>
            </div>
            {no_artifact.map(|n| view! { <p class="text-small text-ink-3">{n}</p> })}
        </div>
    }
}

/// Everything the chip can't fit, as the hover text: the versions, the platform when it is
/// the reason there is no update, how long ago we asked, and what went wrong if anything did.
fn tooltip(u: &UpdateState) -> String {
    let mut lines = vec![format!("Installed {}", u.installed)];
    if u.running != u.installed {
        lines.push(format!(
            "This panel is running {} from a checkout",
            u.running
        ));
    }
    match (&u.latest, u.has_artifact) {
        (Some(latest), Some(false)) => lines.push(format!(
            "Release {latest} publishes no {} build, so it is not an update for this machine",
            u.platform
        )),
        (Some(latest), _) if u.update_available => lines.push(format!("{latest} is published")),
        (Some(latest), _) => lines.push(format!("Latest published is {latest}")),
        (None, _) => lines.push("No release checked yet".to_string()),
    }
    if let Some(secs) = u.checked_secs_ago {
        lines.push(format!("Checked {}", ago(secs)));
    }
    if let Some(secs) = u.installed_secs_ago {
        lines.push(format!("Last updated {}", ago(secs)));
    }
    if u.outcome.as_deref() == Some("rolled-back") {
        lines.push("The last update was rolled back".to_string());
    }
    if let Some(err) = &u.error {
        lines.push(err.clone());
    }
    lines.push("Click for details".to_string());
    lines.join("\n")
}

/// How long ago, in the coarsest unit that still says something. Only ever read by a human
/// deciding whether the number above it is worth trusting, so a minute's precision is plenty.
fn ago(secs: u64) -> String {
    match secs {
        0..=59 => "just now".to_string(),
        60..=3_599 => format!("{}m ago", secs / 60),
        3_600..=86_399 => format!("{}h ago", secs / 3_600),
        _ => format!("{}d ago", secs / 86_400),
    }
}
