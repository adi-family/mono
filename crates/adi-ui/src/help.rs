//! [`HelpLink`] — the `?` beside a label, for the thing a tooltip cannot say in one line.

use leptos::prelude::*;

use crate::icon::{Icon, IconSize, Lucide};
use crate::merge;

/// A `?` that goes to the documentation for whatever it sits beside.
///
/// The sibling of [`Field`](crate::Field)'s hint, and the other half of the same idea: a hint
/// answers "what does this control do" in the one line it has room for, and this answers "what
/// *is* this" by handing over the page that explains it. Reach for the hint first — a link the
/// reader has to follow is a worse answer than a sentence they can read where they stand.
///
/// **A new tab, always.** It is opened from a menu, a panel header, a table head — places the
/// operator is in the middle of doing something — and taking the panel out from under them to
/// answer a question they asked in passing is how a half-finished action gets lost.
///
/// 14px and `ink-3`, quieter than the label it follows, brightening to `ink-2` under the
/// cursor: it is an offer, not part of the sentence.
///
/// ```ignore
/// <HelpLink href="https://…/docs/fleet.md#13-…" label="What sessions and sources are"/>
/// ```
#[component]
pub fn HelpLink(
    /// Where the explanation lives.
    #[prop(into)]
    href: String,
    /// What is being explained, for the tooltip and for a screen reader — "Star ratings",
    /// not "Help". The `?` on its own says nothing about which thing it belongs to.
    #[prop(optional, into)]
    label: String,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let title = if label.is_empty() {
        "What this is \u{2014} opens the docs in a new tab".to_string()
    } else {
        format!("{label} \u{2014} opens the docs in a new tab")
    };
    // The same words twice on purpose: the tooltip for a cursor, the `aria-label` for a reader,
    // and a link whose only content is an icon has nothing else to announce.
    let spoken = title.clone();
    view! {
        <a
            class=merge(
                "inline-grid shrink-0 cursor-pointer place-items-center text-ink-3 \
                 no-underline hover:text-ink-2",
                class,
            )
            href=href
            target="_blank"
            rel="noopener noreferrer"
            title=title
        >
            <Icon icon=Lucide::CircleHelp size=IconSize::Sm label=spoken/>
        </a>
    }
}
