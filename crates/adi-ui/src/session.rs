//! The session — one conversation, as a row in the left rail.
//!
//! Everything the row *sits in* — the rail, the bands, the card, the quiet second line —
//! lives in [`crate::rail`] and is shared with the dashboards on the other side. What is
//! here is only what a session is: five states, and what each of them does to a row.

use leptos::prelude::*;

use crate::rail::{RailCard, meta_line};

/// Where a session stands, which is the only thing that decides how its row looks.
///
/// Only one of the five asks for anything: `Waiting` is *your turn*, and it is the only one
/// that moves. The other four report — finished, broken, busy, coming back — and a row that
/// reports should be readable at a glance and quiet the rest of the time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionState {
    /// Nothing is pending. The default, because a row you forgot to mark should sit quiet
    /// rather than claim the eye.
    #[default]
    Done,
    /// It stopped and cannot go on without you: a question, an approval, a choice.
    Waiting,
    /// It failed. Loud in the dot and in the words, but still — it wants reading, not
    /// answering, so it does not move.
    Error,
    /// An agent is working in it right now.
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
    /// The fill and border for a row in this state, open or not.
    ///
    /// The open row is a card: a fill and a hairline **all the way round**, rather than a
    /// marker down one edge. A rail in the gutter costs every row in the list the same 2px
    /// whether it is using them or not, and it said a second time what the tint, the dot
    /// and the ink already say.
    ///
    /// The border is on the card in every state and only ever changes colour — see
    /// [`crate::rail::RailCard`] — so the open row does not grow a pixel taller than the ones around it.
    ///
    /// `Waiting` breathes whether it is open or not. A session blocked on you has not
    /// stopped being blocked because you are looking at it, and its wash sits *over* the
    /// open row's fill rather than instead of it — see the `attention-pulse` utility.
    ///
    /// `Awaiting` does not breathe, and that is the difference between the two of them said
    /// in motion: movement is how a row asks, and this one is not asking. It takes the blue
    /// only under the cursor, where hovering the row is already a question about it.
    #[must_use]
    pub fn row_classes(self, selected: bool) -> &'static str {
        match (self, selected) {
            (Self::Done | Self::Error | Self::Working, false) => "border-transparent hover:bg-card",
            (Self::Done | Self::Error | Self::Working, true) => "border-edge bg-selected",
            (Self::Awaiting, false) => "border-transparent hover:bg-await-bg",
            (Self::Awaiting, true) => "border-await-edge bg-selected",
            (Self::Waiting, false) => "attention-pulse border-transparent hover:bg-queue-bg",
            (Self::Waiting, true) => "attention-pulse border-edge bg-selected",
        }
    }

    /// Ink for the title. Anything still standing open is ink; what is finished recedes to
    /// secondary, which is most of a long list and should read as background until it is
    /// searched.
    #[must_use]
    pub fn title_classes(self) -> &'static str {
        match self {
            Self::Waiting | Self::Error | Self::Working | Self::Awaiting => "font-medium text-ink",
            Self::Done => "text-secondary",
        }
    }

    /// Ink for the row's one loud word.
    ///
    /// A question is amber, a registered wake is blue, and everything else that is worth
    /// saying twice is red: amber is the colour of *your turn*, blue is the colour of
    /// nobody's, and spending red on either would put a run that merely wants an answer in
    /// the same voice as one that broke.
    #[must_use]
    pub fn alert_classes(self) -> &'static str {
        match self {
            Self::Waiting => "font-medium text-attention",
            Self::Awaiting => "font-medium text-await",
            Self::Done | Self::Error | Self::Working => "font-medium text-err",
        }
    }

    /// The dot before the title, when this state gets one.
    ///
    /// Green for busy, blue for coming back, red for broken, and nothing for the two that
    /// already say it elsewhere: `Waiting` has the wash under it and `Done` has nothing to
    /// report. A dot per state would be five marks where three carry the news.
    ///
    /// `Working` and `Awaiting` get the same mark in two colours on purpose — they are the
    /// two rows a conversation is still alive in, and a rail is scanned for exactly that.
    /// What separates them is only whether anything is happening in it this second.
    #[must_use]
    pub fn dot_classes(self) -> Option<&'static str> {
        match self {
            Self::Working => Some("bg-accent"),
            Self::Awaiting => Some("bg-await"),
            Self::Error => Some("bg-err"),
            Self::Waiting | Self::Done => None,
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
    /// Who ran it. Monospace, because it is a name the machine chose.
    #[prop(optional, into)]
    agent: String,
    /// What the row is waiting on, in the state's own colour: "agent question", "needs
    /// approval". It is the only coloured thing in the row, so spending it on anything else
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
    #[prop(optional, into)] class: String,
    /// A `<span>` after the title — an unread count, a badge.
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    let (open, shut) = (state.row_classes(true), state.row_classes(false));
    let fill = Signal::derive(move || if selected.get() { open } else { shut });
    let meta = meta_line(vec![
        ("text-meta", agent),
        (state.alert_classes(), alert),
        ("text-meta", age),
    ]);

    view! {
        <RailCard fill=fill current=selected class=class>
            <div class="flex items-center gap-1.5">
                {state.dot_classes().map(|dot| view! {
                    <span
                        class=format!("size-1.5 shrink-0 rounded-full {dot}")
                        aria-hidden="true"
                    ></span>
                })}
                <span class=format!("truncate text-row {}", state.title_classes())>
                    {title}
                </span>
                {children.map(|c| c())}
            </div>
            {meta}
        </RailCard>
    }
}
