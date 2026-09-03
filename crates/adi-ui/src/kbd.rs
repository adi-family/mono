//! [`Kbd`] — the key to press.

use leptos::prelude::*;

use crate::merge;

/// A keyboard shortcut.
///
/// This is a *hint*, never a control: it rides a row that already does the thing and only says
/// there is a faster way in. So it is as quiet as a thing can be and still be read — 12px
/// `--ink-3`, no fill, no frame — because a list of forty rows wearing forty key caps is a list
/// nobody scans any more. Where it sits at the right of a list item it is hidden until the row
/// is hovered or open (§6).
///
/// It renders exactly the text it is given and translates nothing. Write the shortcut the way
/// the platform writes it — `⌘1` on a Mac, `Ctrl+1` elsewhere — and decide that at the call
/// site, where the platform is known.
///
/// ```ignore
/// <Kbd>"\u{2318}1"</Kbd>
/// ```
#[component]
pub fn Kbd(#[prop(optional, into)] class: String, children: Children) -> impl IntoView {
    // `select-none` because the hint sits inside a row's text: dragging a selection across the
    // list should take the titles and leave the shortcuts out of it.
    let own = "inline-flex shrink-0 items-center font-sans text-mini whitespace-nowrap \
               text-ink-3 select-none";
    view! { <kbd class=merge(own, class)>{children()}</kbd> }
}
