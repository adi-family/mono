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
    /// Ink for the line. Semantic colour as text, never as a fill (§3).
    #[must_use]
    pub fn classes(self) -> &'static str {
        match self {
            Self::Neutral => "text-ink-3",
            Self::Ok => "text-ok",
            Self::Err => "text-err",
        }
    }
}

/// A line of feedback about the thing that just happened. 13px, in the kind's colour, and
/// nothing around it.
///
/// Inline (the default) it belongs directly under a [`crate::Form`]. It keeps a minimum
/// height, which is the whole point: the row is already there when a message arrives, so
/// nothing below it jumps down the page.
///
/// ```ignore
/// <Flash kind=FlashKind::Err>{move || error.get()}</Flash>
/// ```
#[component]
pub fn Flash(
    #[prop(optional)] kind: FlashKind,
    /// Stand alone above a page's content instead — a notice (§6): the same line with a
    /// hairline under it, for a message that is not answering a form right above it.
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
            "border-b border-line pb-3 text-small leading-normal {}",
            kind.classes()
        )
    } else {
        format!("min-h-4 pt-2 text-small leading-normal {}", kind.classes())
    };
    view! { <div class=merge(&own, class) role="status">{children()}</div> }
}

/// The quiet line a list shows when it has nothing in it — loading, empty, or failed. Its
/// padding is what stops an empty section from collapsing to a sliver.
#[component]
pub fn Empty(#[prop(optional, into)] class: String, children: Children) -> impl IntoView {
    view! {
        <div class=merge("py-6 text-small text-ink-3", class)>
            {children()}
        </div>
    }
}
