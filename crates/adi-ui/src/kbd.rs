//! [`Kbd`] — the key to press, drawn as a key.

use leptos::prelude::*;

use crate::merge;

/// A keyboard shortcut, printed as a key cap.
///
/// This is a *hint*, never a control: it rides a row that already does the thing and only says
/// there is a faster way in. So it is drawn as quiet as a thing can be and still be read — the
/// faint ink, a hairline, no fill of its own — because a list of forty rows wearing forty badges
/// is a list nobody scans any more.
///
/// It renders exactly the text it is given and translates nothing. Write the shortcut the way the
/// platform writes it — `⌘1` on a Mac, `Ctrl+1` elsewhere — and decide that at the call site,
/// where the platform is known.
///
/// ```ignore
/// <Kbd>"\u{2318}1"</Kbd>
/// ```
#[component]
pub fn Kbd(#[prop(optional, into)] class: String, children: Children) -> impl IntoView {
    // `select-none` because the cap sits inside a row's text: dragging a selection across the list
    // should take the titles and leave the shortcuts out of it.
    let own = "inline-flex shrink-0 items-center rounded-sm border border-dim px-1 py-px \
               font-mono text-caps whitespace-nowrap text-faint select-none";
    view! { <kbd class=merge(own, class)>{children()}</kbd> }
}
