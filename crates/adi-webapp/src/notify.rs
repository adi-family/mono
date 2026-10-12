//! "Notify me when this chat stops" — the bell above the composer (`adi-chat__bell`).
//!
//! The chat polls the open conversation's snapshot once a second
//! ([`crate::pages::agents::actions::poll_watch`]). When the bell is armed and that conversation
//! goes from running to idle — it finished, or it stopped to ask the person something — this plays
//! a short chime and raises a desktop notification, so somebody who has looked away learns the
//! agent is done without having to watch the tab.
//!
//! Being armed is a preference about how this person uses the page, not a fact about the stack, so
//! it lives in `localStorage` (the reasoning of [`crate::ui::advice_hidden`]) and never in the
//! store. Two browser gates stand between us and a ping, and both are opened by the one click that
//! arms the bell — a user gesture: a desktop notification needs its one-time permission, and an
//! `AudioContext` created outside a gesture starts suspended and stays silent. So [`set_armed`]
//! asks for the permission and unlocks the audio there and then, while the click is still on the
//! stack, and a run that settles minutes later can chime without a gesture of its own.

use std::cell::RefCell;

use adi_webapp_api::types::AgentPeek;
use leptos::prelude::WithUntracked;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::state::AgentsWatch;

/// Where the choice is remembered. Per browser; see the module note.
const KEY: &str = "adi-notify-on-stop";

/// The one app icon the desktop notification wears — the same `/assets` PWA icon the manifest uses.
const ICON: &str = "/assets/icon-192.png";

/// Whether this browser pings when a watched conversation stops.
pub(crate) fn armed() -> bool {
    crate::ui::storage()
        .and_then(|s| s.get_item(KEY).ok().flatten())
        .is_some_and(|v| v == "1")
}

/// Turn the bell on or off and remember it. Turning it on is the one user gesture we are handed, so
/// it is also where the two browser gates are opened: the notification permission is asked for, the
/// audio is unlocked, and one confirming chime plays — so the sound is known to work and to be
/// audible before the moment it actually matters.
pub(crate) fn set_armed(on: bool) {
    if let Some(s) = crate::ui::storage() {
        let _ = if on {
            s.set_item(KEY, "1")
        } else {
            s.remove_item(KEY)
        };
    }
    if on {
        request_permission();
        unlock_audio();
        chime();
    }
}

thread_local! {
    /// One audio context for the tab, created on the gesture that arms the bell and reused for
    /// every chime after. A fresh context per chime would start suspended (no gesture behind it)
    /// and play nothing; keeping this one, resumed once while the arming click is still on the
    /// stack, is what lets a later chime be heard.
    static AUDIO: RefCell<Option<web_sys::AudioContext>> = const { RefCell::new(None) };
}

/// Make sure the tab's audio context exists and is running. Called from the arm gesture, where
/// creating and resuming it is allowed.
fn unlock_audio() {
    AUDIO.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = web_sys::AudioContext::new().ok();
        }
        if let Some(ctx) = slot.as_ref() {
            let _ = ctx.resume();
        }
    });
}

/// A short two-note chime through the tab's audio context. At worst silent — if the context could
/// not be created, or is still suspended because the bell has never been armed with a click in this
/// tab — which is the right failure: a missing sound is better than a thrown error on a poll.
fn chime() {
    AUDIO.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = web_sys::AudioContext::new().ok();
        }
        let Some(ctx) = slot.as_ref() else { return };
        let _ = ctx.resume();
        let t0 = ctx.current_time();
        // Two rising notes, each a quick swell and decay — a notification, not an alarm.
        beep(ctx, 880.0, t0, 0.15);
        beep(ctx, 1174.0, t0 + 0.16, 0.24);
    });
}

/// One note: an oscillator through a gain envelope, timed on the context clock. The oscillator
/// starts immediately and the envelope does the timing — it holds the note near-silent until
/// `start`, so a scheduled-start API (and the extra web-sys feature it needs) is not required.
fn beep(ctx: &web_sys::AudioContext, freq: f32, start: f64, dur: f64) {
    let (Ok(osc), Ok(gain)) = (ctx.create_oscillator(), ctx.create_gain()) else {
        return;
    };
    osc.set_type(web_sys::OscillatorType::Sine);
    osc.frequency().set_value(freq);
    let g = gain.gain();
    // Silent, then a fast swell, then an exponential decay. The ramps never target exactly zero —
    // an exponential ramp to zero is undefined — so the floor is a hair above it.
    let _ = g.set_value_at_time(0.0001, start);
    let _ = g.exponential_ramp_to_value_at_time(0.3, start + 0.02);
    let _ = g.exponential_ramp_to_value_at_time(0.0001, start + dur);
    let _ = osc.connect_with_audio_node(&gain);
    let _ = gain.connect_with_audio_node(&ctx.destination());
    let _ = osc.start();
    let _ = osc.stop_with_when(start + dur);
}

/// Ask for the desktop-notification permission, unless the browser has already answered. The
/// returned promise is dropped — the answer is read fresh from [`web_sys::Notification::permission`]
/// at the moment a notification is actually raised.
fn request_permission() {
    if web_sys::Notification::permission() == web_sys::NotificationPermission::Default {
        let _ = web_sys::Notification::request_permission();
    }
}

/// Raise a desktop notification, if this browser has granted the permission; a no-op without it (the
/// chime still played, and that half needs no gate). Clicking the notification brings the tab
/// forward, the way a person expects of a "come back" ping.
fn notify(title: &str, body: &str) {
    if web_sys::Notification::permission() != web_sys::NotificationPermission::Granted {
        return;
    }
    let opts = web_sys::NotificationOptions::new();
    opts.set_body(body);
    opts.set_icon(ICON);
    // One tag for the whole feature, so a second stop replaces the first rather than stacking: an
    // agent that finishes a run and then a follow-up turn leaves one card, not a column of them.
    opts.set_tag("adi-chat-stopped");
    let Ok(note) = web_sys::Notification::new_with_options(title, &opts) else {
        return;
    };
    let on_click = Closure::<dyn FnMut()>::new(move || {
        if let Some(w) = web_sys::window() {
            let _ = w.focus();
        }
    });
    note.set_onclick(Some(on_click.as_ref().unchecked_ref()));
    // The closure has to outlive this call to answer a click that comes later, so it is handed to
    // the GC rather than dropped here.
    on_click.forget();
}

/// Called by the poll with each fresh snapshot, just before it replaces the one on screen. Fires
/// once on the edge where the open conversation goes from running to not — and only there. A
/// snapshot of a *different* conversation (the view moved) carries a different id and is not that
/// edge; a run already idle when it was opened never ran under our eye to have stopped.
pub(crate) fn on_peek(watch: AgentsWatch, next: &AgentPeek) {
    if !armed() {
        return;
    }
    let stopped = watch.peek.with_untracked(|prev| {
        prev.as_ref()
            .is_some_and(|p| p.running && !next.running && id_of(p) == id_of(next))
    });
    if stopped {
        fire(next);
    }
}

/// What makes two snapshots the same conversation: the run id for a headless run, the agent name
/// for an interactive session (which carries no run id). Enough to tell "this chat finished" apart
/// from "the view switched to a chat that was already idle".
fn id_of(p: &AgentPeek) -> String {
    if p.run_id.is_empty() {
        p.name.clone()
    } else {
        p.run_id.clone()
    }
}

/// Chime and notify, with a line that names which of the kinds of stop this was: a question waiting
/// on the person, a pause on a wake, or a plain finish.
fn fire(peek: &AgentPeek) {
    let who = if peek.name.is_empty() {
        "The agent"
    } else {
        peek.name.as_str()
    };
    let (title, body) = if let Some(ask) = &peek.pending_question {
        let line = if ask.headline.is_empty() {
            "It has a question for you.".to_string()
        } else {
            ask.headline.clone()
        };
        (format!("{who} needs your answer"), line)
    } else if !peek.awaits.is_empty() {
        (
            format!("{who} is waiting"),
            "Paused until a wake fires.".to_string(),
        )
    } else {
        (format!("{who} stopped"), "The run has finished.".to_string())
    };
    chime();
    notify(&title, &body);
}
