//! The badge — a pill for a status or a category — and the dot that says a state in 6px.

use leptos::prelude::*;

use crate::merge;

/// What the badge is saying. The tones are the design system's semantic colours, drawn the
/// one way §3 allows them: as text on a 12% tint, never as a fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BadgeTone {
    /// No judgement — a tag, a count, an id. The translucent chip.
    #[default]
    Neutral,
    /// Healthy, up, done, set.
    Online,
    /// Degraded, pending, waiting on somebody.
    Warn,
    /// Failed, down, blocked.
    Down,
    /// The chip, one step louder — a kind worth telling apart from the ordinary case without
    /// making it a state. Not the accent: orange is never a label (§3).
    Accent,
}

impl BadgeTone {
    /// Fill and text for this tone.
    #[must_use]
    pub fn classes(self) -> &'static str {
        match self {
            Self::Neutral => "bg-chip text-ink-2",
            Self::Online => "bg-ok-soft text-ok",
            Self::Warn => "bg-warn-soft text-warn",
            Self::Down => "bg-err-soft text-err",
            Self::Accent => "bg-chip text-ink",
        }
    }
}

/// A small tinted pill.
///
/// ```ignore
/// <Badge tone=BadgeTone::Online>"running"</Badge>
/// ```
#[component]
pub fn Badge(
    #[prop(optional)] tone: BadgeTone,
    /// Tabular monospace, for ids, ports, hashes and counts — anything that should not
    /// reflow as its digits change.
    #[prop(optional)]
    mono: bool,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let font = if mono { "font-mono tabular-nums" } else { "" };
    let own = format!(
        "inline-flex items-center gap-1.5 rounded-full px-2 py-0.5 text-mini leading-[1.4] \
         whitespace-nowrap {font} {}",
        tone.classes(),
    );
    view! { <span class=merge(&own, class)>{children()}</span> }
}

/// What a [`Dot`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DotTone {
    /// Online, set, done.
    #[default]
    Ok,
    /// Running, live — the one orange a row may carry, because it is 6px.
    Live,
    /// Waiting on somebody.
    Warn,
    /// Failed.
    Err,
    /// Nothing to report; a placeholder that keeps the column.
    Idle,
}

impl DotTone {
    #[must_use]
    pub fn classes(self) -> &'static str {
        match self {
            Self::Ok => "bg-ok",
            Self::Live => "bg-accent",
            Self::Warn => "bg-warn",
            Self::Err => "bg-err",
            Self::Idle => "bg-ink-3",
        }
    }
}

/// A 6px dot before a word — how a state is said in this system (§6). Never a filled badge.
///
/// ```ignore
/// <span class="flex items-center gap-2"><Dot tone=DotTone::Ok/>"online"</span>
/// ```
#[component]
pub fn Dot(
    #[prop(optional)] tone: DotTone,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let own = format!(
        "inline-block size-1.5 shrink-0 rounded-full {}",
        tone.classes()
    );
    view! { <span class=merge(&own, class) aria-hidden="true"></span> }
}
