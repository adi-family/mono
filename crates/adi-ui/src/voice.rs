//! [`MicButton`] — the control you dictate through.
//!
//! Presentational only: it owns no microphone and knows no transcription service. It renders one
//! of four states and reports presses. Whoever mounts it does the recording — in this tree that is
//! `adi-webapp`'s `voice` module, because capturing audio and choosing an engine are decisions
//! about the app, not about the button.

use leptos::{ev, prelude::*};

use crate::icon::{Icon, IconSize, Lucide};
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

/// The microphone button: one of the composer's 32px quiet icon buttons.
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
        // microphone is open, and nothing about a message half-dictated says so otherwise. Red
        // ink on the red tint — a pill's treatment (§3), on the one button that is a live state.
        MicState::Listening => "bg-err-soft text-err",
        MicState::Working => "text-ink-3",
        _ => "text-ink-2 hover:bg-hover hover:text-ink",
    };

    view! {
        <button
            class=move || merge(
                &format!(
                    "grid size-8 shrink-0 place-items-center rounded-md transition-colors \
                     duration-100 disabled:cursor-not-allowed disabled:opacity-40 {}",
                    tone(),
                ),
                class.clone(),
            )
            type="button"
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
                // Turning because the person pressed it and is waiting on the answer — the one
                // motion here, and it was asked for.
                MicState::Working => view! {
                    <Icon
                        icon=Lucide::LoaderCircle
                        size=IconSize::Md
                        class="animate-spin"
                        label="Working out what you said"
                    />
                }.into_any(),
                MicState::Listening => view! {
                    <Icon icon=Lucide::Mic size=IconSize::Md label="Stop dictating"/>
                }.into_any(),
                _ => view! {
                    <Icon icon=Lucide::Mic size=IconSize::Md label="Dictate a message"/>
                }.into_any(),
            }}
        </button>
    }
}
