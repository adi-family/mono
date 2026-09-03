//! The version pill in the top bar: what this machine is on, and — when a newer release is
//! published *for this platform* — the one control that installs it.
//!
//! Two halves, because they are two different things. The chip is identity: it is always
//! there, it says the installed version, and clicking it asks the release manifest again.
//! The button beside it is an offer, and it exists only while there is something to accept.
//!
//! **The offer opens the changelog, it does not install.** Taking an update restarts the
//! whole stack, which is not something to do to somebody who meant to click the thing next to
//! it — and the release has notes precisely so there is an answer to "what for?". So the
//! button opens *What's new*: the section of `CHANGELOG.md` published with that release
//! (`docs/adi-update.md` §5), and the two ways out of it — Install, or not now.
//!
//! Nothing here knows a DMG from a tarball. The server hands back the same
//! [`UpdateState`] on every platform (`docs/adi-update.md`), so a Mac, a Linux node and a
//! Windows node all get this bar — and a release that publishes no artifact for the host
//! reads as "no update for this machine", not as an offer that would fail to download.
//!
//! **The install restarts the app.** `POST /api/update/run` answers as soon as the updater
//! is running, and a minute later that updater kickstarts the service this page is talking
//! to. So a failed poll must not clear what we know: the pill keeps saying "updating…"
//! through the gap and settles on the new version once the backend answers again.

use gloo_timers::callback::Interval;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use adi_ui::{Button, ButtonSize, ButtonVariant, Markdown, Modal};
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
    /// Whether *What's new* is open. Here rather than in [`version_pill`] because the bar it
    /// renders into is rebuilt whenever the page's own state moves, and a dialog that closes
    /// itself because something behind it changed is a dialog nobody can finish reading.
    offer: RwSignal<bool>,
}

/// Start watching for updates: read the state, ask the manifest if the record has gone
/// stale, and keep both fresh.
///
/// Call once per mounted app — it owns a timer, the way [`crate::pwa::installable`] owns an
/// event handler. The three entry points in [`main`](../main.rs) are mutually exclusive, so
/// that holds.
pub(crate) fn watch() -> UpdateWatch {
    let watch = UpdateWatch {
        state: RwSignal::new(None),
        busy: RwSignal::new(false),
        offer: RwSignal::new(false),
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
    /// Ask the release manifest now — what the chip does when clicked.
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

    /// Hand the install to the updater — what *What's new* ends in.
    fn install(self) {
        if self.busy.get_untracked() {
            return;
        }
        // Closed here rather than when the answer lands: the reply arrives moments before the
        // app restarts, and a dialog still up at that point sits over a page whose backend is
        // going away, with a button that would start a second install.
        self.offer.set(false);
        self.busy.set(true);
        spawn_local(async move {
            match fetch::run_update().await {
                Ok(u) => self.state.set(Some(u)),
                // Report it where the user is already looking rather than through the page's
                // flash: this control lives in a bar shared by three different screens, and
                // only one of them has a flash line to write to.
                Err(e) => self.state.update(|s| {
                    if let Some(s) = s {
                        s.outcome = Some("error".to_string());
                        s.error = Some(e);
                    }
                }),
            }
            self.busy.set(false);
        });
    }
}

/// The version, as a row in the root screens' menu.
///
/// The bar this replaced said the version *and*, when there was one to take, offered it in a
/// button beside the chip. A menu row is one thing, so the two are one row that changes what
/// it says: normally the installed version, clicking to re-ask the manifest; while a release
/// is actually available, the offer.
///
/// Nothing is short-circuited on the way to an install — the row opens *What's new* exactly
/// as the button did, and the notes are still the only door to it. See the module header for
/// why that matters.
pub(crate) fn action(watch: UpdateWatch) -> Action {
    let Some(u) = watch.state.get() else {
        // Before the first answer we do not know the version, so the row offers the one thing
        // that is true regardless: ask.
        return Action::new("Check for updates", "", icons::Icon::Box, move || {
            watch.check()
        });
    };
    if u.installing {
        return Action::new(
            "Updating\u{2026}",
            "The stack is restarting onto the new version",
            icons::Icon::Upgrade,
            move || watch.check(),
        );
    }
    match u.latest.clone().filter(|_| u.update_available) {
        Some(latest) => Action::new(
            format!("Update to {latest}"),
            "See what is in it, then install",
            icons::Icon::Upgrade,
            move || watch.offer.set(true),
        ),
        None => Action::new(
            format!("adi v{}", u.installed),
            "Check for a newer release",
            icons::Icon::Box,
            move || watch.check(),
        ),
    }
}

/// *What's new*, mounted on its own.
///
/// [`version_pill`] carries this dialog inside it; the root screens have no pill any more, so
/// they mount it beside the menu whose row opens it. Same dialog either way — the offer is
/// one place, however it was reached.
pub(crate) fn offer_dialog(watch: UpdateWatch) -> impl IntoView {
    move || {
        watch.state.get().and_then(|u| {
            (u.update_available && !u.installing)
                .then(|| whats_new(watch, u.latest.clone().unwrap_or_default(), u.notes.clone()))
        })
    }
}

/// The pill itself: the version chip, and the update button when there is one to offer.
///
/// Renders nothing until the first answer lands — an empty bar for half a second beats a
/// placeholder version that then changes into a real one.
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
            let dialog = offer.then(|| whats_new(watch, latest.clone(), u.notes.clone()));
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
                         px-2 text-small hover:bg-hover disabled:cursor-default {tone}"
                    )
                    title=tooltip(&u)
                    disabled=move || watch.busy.get()
                    on:click=move |_| watch.check()
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
                                disabled=watch.busy
                                attr:title=format!("See what is in {latest}, then install it")
                                on:click=move |_| watch.offer.set(true)
                            >
                                {format!("Update to {latest}")}
                            </Button>
                        }
                    })}
                {dialog}
            }
        })
    }
}

/// *What's new* — the release's own case for being taken: the changelog section published
/// with it, and the two ways out.
///
/// The notes are markdown from `CHANGELOG.md`, rendered by the same component that draws a
/// chat message. They scroll in their own box rather than the dialog's, so a long release
/// never pushes Install off the bottom of the screen.
fn whats_new(watch: UpdateWatch, latest: String, notes: Option<String>) -> impl IntoView {
    // A release may be cut without notes — by hand, or off another channel. Say so, rather
    // than opening an empty dialog that reads as a page which failed to load.
    let unnoted = format!("Release {latest} was published without release notes.");
    view! {
        <Modal open=watch.offer title=format!("What's new in {latest}")>
            <div class="flex flex-col gap-4">
                // The notes scroll in their own box and not the dialog's, so a long release
                // never pushes Install off the bottom of the screen.
                <div class="max-h-[50vh] overflow-y-auto pr-1">
                    // Cloned per render: `Modal` takes a `ChildrenFn`, so this body is a
                    // `Fn` and cannot hand its captures away.
                    {match notes.clone() {
                        Some(md) => view! { <Markdown source=md/> }.into_any(),
                        None => {
                            view! { <p class="text-ui text-ink-2">{unnoted.clone()}</p> }
                                .into_any()
                        }
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
                            on:click=move |_| watch.offer.set(false)
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
        </Modal>
    }
}

/// Everything the chip can't fit, as the hover text: the versions, the platform when it is
/// the reason there is no update, how long ago we asked, and what went wrong if anything did.
fn tooltip(u: &UpdateState) -> String {
    let mut lines = vec![format!("Installed {}", u.installed)];
    if u.running != u.installed {
        // Worth saying out loud rather than hiding: the panel you are looking at was built
        // from a checkout, so the number beside it is the bundle's, not this binary's.
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
    lines.push("Click to check for updates".to_string());
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
