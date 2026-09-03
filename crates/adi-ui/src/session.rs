//! The session — one conversation, as a row in the left rail.
//!
//! Everything the row *sits in* — the rail, the bands, the card, the quiet second line —
//! lives in [`crate::rail`] and is shared with the apps on the other side. What is here is
//! only what a session is: five states, and what each of them does to a row.

use leptos::prelude::*;

use crate::badge::{Dot, DotTone};
use crate::kbd::Kbd;
use crate::rail::{RailCard, meta_line};

/// Where a session stands, which is the only thing that decides how its row looks.
///
/// A state is said with a 6px dot before the title and a word in the meta line — never a
/// fill, never motion (§8). Only `Waiting` asks for anything; the rest report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionState {
    /// Nothing is pending. The default, because a row you forgot to mark should sit quiet
    /// rather than claim the eye.
    #[default]
    Done,
    /// It stopped and cannot go on without you: a question, an approval, a choice.
    Waiting,
    /// It failed. Said in the dot and in the word; it wants reading, not answering.
    Error,
    /// An agent is working in it right now. The one orange dot a row may carry.
    Working,
    /// It stopped, but it is coming back on its own: it left a wake registered — an event
    /// to watch for, a deadline, a command that decides — and will pick the conversation up
    /// again when that fires.
    ///
    /// The state the other four cannot say. `Done` would call it finished when it is not,
    /// `Working` would claim a turn is in flight when none is, and `Waiting` would put it in
    /// the inbox of things that need a person — which is the one thing it does not need.
    Awaiting,
}

impl SessionState {
    /// The fill for a row in this state, open or not: `--bg-active` when open, a hover tone
    /// otherwise. The state itself is not in the fill — it is the dot.
    #[must_use]
    pub fn row_classes(self, selected: bool) -> &'static str {
        if selected {
            "bg-active"
        } else {
            "hover:bg-hover"
        }
    }

    /// Ink for the title. Every row's title is `--ink` (§6); the state is said beside it.
    #[must_use]
    pub fn title_classes(self) -> &'static str {
        "text-ink"
    }

    /// Ink for the row's one loud word.
    ///
    /// A question is amber, a registered wake is plain, and everything else worth saying
    /// twice is red: amber is the colour of *your turn*, and spending red on a run that merely
    /// wants an answer would put it in the same voice as one that broke.
    #[must_use]
    pub fn alert_classes(self) -> &'static str {
        match self {
            Self::Waiting => "font-medium text-warn",
            Self::Awaiting => "text-ink-2",
            Self::Done | Self::Error | Self::Working => "font-medium text-err",
        }
    }

    /// The dot before the title, when this state gets one.
    ///
    /// Orange for busy, amber for waiting on you, red for broken, grey for coming back — and
    /// nothing for `Done`, which has nothing to report. The orange is the only accent a rail
    /// carries, and it is 6px (§3).
    #[must_use]
    pub fn dot(self) -> Option<DotTone> {
        match self {
            Self::Working => Some(DotTone::Live),
            Self::Waiting => Some(DotTone::Warn),
            Self::Error => Some(DotTone::Err),
            Self::Awaiting => Some(DotTone::Idle),
            Self::Done => None,
        }
    }
}

/// One session.
///
/// Event handlers attach the ordinary Leptos way — `<SessionItem on:click=open/>` lands on
/// the underlying `<button>` — so, as with [`crate::Button`], there is no callback prop.
///
/// ```ignore
/// <SessionItem
///     title="Walk the linear board"
///     state=SessionState::Waiting
///     selected=Signal::derive(move || open.get() == id)
///     alert="agent question"
///     age="14m"
///     shortcut="\u{2303}1"
///     on:click=move |_| open.set(id)
/// />
/// ```
#[component]
pub fn SessionItem(
    #[prop(optional, into)] title: String,
    #[prop(optional)] state: SessionState,
    /// The one the screen is currently showing.
    #[prop(optional, into)]
    selected: Signal<bool>,
    /// Who ran it. Sans, `--ink-2` 500 — a name, not a machine string.
    #[prop(optional, into)]
    agent: String,
    /// What the row is waiting on, in the state's own colour: "agent question", "needs
    /// approval". It is the only coloured word in the row, so spending it on anything else
    /// costs the list the signal.
    ///
    /// Say what it wants, not how much of it there is. A count — "3 errors" — reads as
    /// news and carries none: it does not say what broke, it changes every run, and the
    /// row is already red.
    #[prop(optional, into)]
    alert: String,
    /// How long ago, already written the way you want it read: "14m", "2h", "4d".
    #[prop(optional, into)]
    age: String,
    /// The key that opens this row — `⌃1`. Shown at the right on hover and on the open row,
    /// hidden the rest of the time (§6).
    #[prop(optional, into)]
    shortcut: String,
    #[prop(optional, into)] class: String,
    /// A `<span>` after the title — an unread count, a badge.
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    let (open, shut) = (state.row_classes(true), state.row_classes(false));
    let fill = Signal::derive(move || if selected.get() { open } else { shut });
    let meta = meta_line(vec![
        ("font-medium text-ink-2", agent),
        (state.alert_classes(), alert),
        ("", age),
    ]);
    let has_key = !shortcut.is_empty();

    view! {
        <RailCard fill=fill current=selected class=class>
            <div class=if has_key { "flex items-center gap-2 pr-7" } else { "flex items-center gap-2" }>
                {state.dot().map(|tone| view! { <Dot tone=tone/> })}
                <span class=format!("truncate text-row {}", state.title_classes())>
                    {title}
                </span>
                {children.map(|c| c())}
            </div>
            {meta}
            {has_key.then(|| view! {
                // Hidden until the row is hovered or open; the class is reactive on the row's
                // own `selected`, so it lands on a wrapper rather than on the key itself.
                <span class=move || {
                    if selected.get() {
                        "absolute top-0.5 right-0 opacity-100"
                    } else {
                        "absolute top-0.5 right-0 opacity-0 group-hover:opacity-100"
                    }
                }>
                    <Kbd>{shortcut.clone()}</Kbd>
                </span>
            })}
        </RailCard>
    }
}
