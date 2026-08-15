//! [`MicButton`] — the control you dictate through.
//!
//! Presentational only: it owns no microphone and knows no transcription service. It renders one
//! of four states and reports presses. Whoever mounts it does the recording — in this tree that is
//! `adi-webapp`'s `voice` module, because capturing audio and choosing an engine are decisions
//! about the app, not about the button.

use leptos::{ev, prelude::*};

use crate::merge;

/// Where dictation has got to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MicState {
    /// Ready, and not listening.
    #[default]
    Idle,
    /// The microphone is open.
    Listening,
    /// Recording stopped; the words are being worked out.
    Working,
    /// Dictation cannot run at all, for the reason carried. Distinct from an error — this is a
    /// standing condition (no microphone, an insecure page, a browser without the API), so the
    /// button stays visible but out, and the reason is what the user needs in order to fix it.
    Blocked(String),
}

impl MicState {
    /// Whether a press should be accepted.
    #[must_use]
    pub fn pressable(&self) -> bool {
        matches!(self, Self::Idle | Self::Listening)
    }
}

/// The microphone button.
///
/// **A press toggles; it is not held down.** Push-to-talk by holding is the obvious reading of
/// "push to talk", and it is the wrong one for a control that dictates a *message*: a held button
/// cannot be reached from the keyboard, it fights every screen reader, and it means a hand
/// occupied for as long as the sentence takes. Click to open the microphone, click to close it.
///
/// The caller supplies `state` and never has it changed from in here — a button that decided by
/// itself that it was listening would be claiming something only the recorder can know.
#[component]
pub fn MicButton(
    /// Where dictation is, reactively.
    #[prop(into)]
    state: Signal<MicState>,
    /// A press, when [`MicState::pressable`]. Start or stop is the caller's to work out from the
    /// state it is already holding.
    #[prop(into)]
    on_press: Callback<()>,
    /// Said on hover, after the state's own word. The engine's name belongs here — which
    /// recogniser is about to run is exactly what a person wants to check before speaking.
    #[prop(optional, into)]
    hint: Signal<String>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let title = move || match state.get() {
        MicState::Blocked(why) => why,
        other => {
            let verb = match other {
                MicState::Listening => "Stop dictating",
                MicState::Working => "Working out what you said",
                _ => "Dictate a message",
            };
            let hint = hint.get();
            if hint.is_empty() {
                verb.to_string()
            } else {
                format!("{verb} · {hint}")
            }
        }
    };

    let tone = move || match state.get() {
        // Listening is the one state that must be unmistakable from across the room: the
        // microphone is open, and nothing about a message half-dictated says so otherwise.
        MicState::Listening => "border-err-edge-2 text-err bg-err-bg animate-pulse",
        MicState::Working => "border-dim text-meta",
        _ => "border-dim bg-card text-meta hover:border-accent hover:text-accent",
    };

    view! {
        <button
            class=move || merge(
                &format!(
                    "grid size-8 shrink-0 place-items-center rounded-sm border \
                     transition-colors duration-100 focus-visible:outline-2 \
                     focus-visible:outline-offset-2 focus-visible:outline-accent \
                     disabled:cursor-not-allowed disabled:opacity-40 {}",
                    tone(),
                ),
                class.clone(),
            )
            type="button"
            // The label says the state, not the glyph: to a screen reader "Dictate a message" and
            // "Stop dictating" are different controls, which is the truth of it.
            aria-label=title
            title=title
            // Reported so assistive tech hears the microphone open without the label being read
            // out again from scratch.
            aria-pressed=move || (state.get() == MicState::Listening).then_some("true")
            disabled=move || !state.get().pressable()
            on:click=move |_: ev::MouseEvent| {
                if state.get_untracked().pressable() {
                    on_press.run(());
                }
            }
        >
            {move || match state.get() {
                // A spinner, so that a slow provider looks like waiting rather than like nothing
                // having happened.
                MicState::Working => view! {
                    <svg class="size-4 animate-spin" viewBox="0 0 16 16" fill="none"
                         stroke="currentColor" stroke-width="1.8" aria-hidden="true">
                        <path d="M8 1.5a6.5 6.5 0 1 0 6.5 6.5" stroke-linecap="round"></path>
                    </svg>
                }.into_any(),
                _ => view! {
                    <svg class="size-4" viewBox="0 0 16 16" fill="none" stroke="currentColor"
                         stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"
                         aria-hidden="true">
                        <rect x="6" y="1.75" width="4" height="8" rx="2"></rect>
                        <path d="M3.5 7.5a4.5 4.5 0 0 0 9 0"></path>
                        <path d="M8 12v2.25"></path>
                    </svg>
                }.into_any(),
            }}
        </button>
    }
}
