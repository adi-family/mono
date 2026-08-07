//! The badge — a small, tinted label for a status or a count.

use leptos::prelude::*;

use crate::merge;

/// What the badge is saying. The tones map to the design system's status tokens, so
/// "running" here is the same green as "running" everywhere else in adi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BadgeTone {
    /// No judgement — a count, a tag, an id.
    #[default]
    Neutral,
    /// Healthy, up, done.
    Online,
    /// Degraded, pending, deprecated.
    Warn,
    /// Failed, down, blocked.
    Down,
    /// Selected or active, matching the accent used for interaction.
    Accent,
}

impl BadgeTone {
    /// Fill, text and border for this tone. The fill is the status colour at 12%, so the
    /// badge tints its surface instead of becoming a solid block of colour.
    #[must_use]
    pub fn classes(self) -> &'static str {
        match self {
            Self::Neutral => "border-dim bg-card text-meta",
            Self::Online => "border-accent-soft-edge bg-accent-soft text-accent",
            Self::Warn => "border-queue-edge bg-queue-bg text-queue",
            Self::Down => "border-err-edge bg-err-bg-2 text-err",
            Self::Accent => "border-tip-edge bg-tip text-accent",
        }
    }
}

/// A small tinted label.
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
    let font = if mono {
        "font-mono tabular-nums"
    } else {
        "font-medium"
    };
    let own = format!(
        "inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-caps \
         leading-none whitespace-nowrap {font} {}",
        tone.classes(),
    );
    view! { <span class=merge(&own, class)>{children()}</span> }
}
