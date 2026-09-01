//! What a view says when it has news ([`Flash`]) or nothing to show ([`Empty`]).

use leptos::prelude::*;

use crate::merge;

/// How a [`Flash`] landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlashKind {
    /// Progress, or a plain statement of fact.
    #[default]
    Neutral,
    /// It worked.
    Ok,
    /// It did not.
    Err,
}

impl FlashKind {
    /// Ink for the inline form.
    #[must_use]
    pub fn classes(self) -> &'static str {
        match self {
            Self::Neutral => "text-meta",
            Self::Ok => "text-accent",
            Self::Err => "text-err",
        }
    }

    /// Border and fill for the card form. The tint is the status colour mixed into the
    /// surface rather than laid over it, so a card stays legible in both themes instead of
    /// glowing in the dark one.
    #[must_use]
    pub fn card_classes(self) -> &'static str {
        match self {
            Self::Neutral => "border-edge bg-card text-body",
            Self::Ok => "border-accent-soft-edge bg-accent-soft text-accent",
            Self::Err => "border-err-edge bg-err-bg text-err",
        }
    }
}

/// A line of feedback about the thing that just happened.
///
/// Inline (the default) it belongs directly under a [`crate::Form`], carrying the same tint
/// so the message reads as part of the strip that produced it. It keeps a minimum height,
/// which is the whole point: the row is already there when a message arrives, so nothing
/// below it jumps down the page.
///
/// ```ignore
/// <Flash kind=FlashKind::Err>{move || error.get()}</Flash>
/// ```
#[component]
pub fn Flash(
    #[prop(optional)] kind: FlashKind,
    /// Stand alone as a bordered card instead — for a message that is not answering a form
    /// right above it.
    #[prop(optional)]
    card: bool,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let own = if card {
        // No outer margin. A component that carries its own `mb-*` double-spaces inside any
        // `gap` layout and cannot be un-spaced from the call site; the container owns the
        // gaps, and `class` is there for the exception.
        format!(
            "rounded-sm border px-3 py-2 text-row font-medium {}",
            kind.card_classes()
        )
    } else {
        format!("min-h-4 bg-bar px-3.5 pb-2.5 text-mini {}", kind.classes())
    };
    view! { <div class=merge(&own, class) role="status">{children()}</div> }
}

/// The centred, quiet line a list shows when it has nothing in it — loading, empty, or
/// failed. Its padding is what stops an empty panel from collapsing to a sliver.
#[component]
pub fn Empty(#[prop(optional, into)] class: String, children: Children) -> impl IntoView {
    view! {
        <div class=merge("px-3.5 py-6 text-center text-mini text-meta", class)>
            {children()}
        </div>
    }
}
