//! The panel — the surface almost everything else sits on.

use leptos::prelude::*;

use crate::merge;

/// A titled surface: a hairline-bordered card with a header strip and a body.
///
/// Depth is a border, not a shadow — the design system separates surfaces by line, so a
/// page of panels reads as one plane rather than a pile of cards.
///
/// The header is dropped entirely when there is no `title` and no `actions`, which makes
/// `<Panel>` on its own a plain surface to put anything on.
///
/// ```ignore
/// <Panel title="Ports" actions=|| view! { <Button>"Refresh"</Button> }.into_any()>
///     <PortTable/>
/// </Panel>
/// ```
#[component]
pub fn Panel(
    #[prop(optional, into)] title: String,
    /// Controls pinned to the right of the header — usually a [`crate::Button`] or two.
    #[prop(optional, into)]
    actions: Option<ViewFn>,
    /// Drop the body padding, for a panel whose child manages its own edges (a table, a
    /// terminal, a code view).
    #[prop(optional)]
    flush: bool,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let has_head = !title.is_empty() || actions.is_some();
    let body = if flush { "" } else { "p-4" };

    view! {
        <section class=merge("island bg-card text-body", class)>
            {has_head.then(|| view! {
                <header class="flex min-h-9 items-center justify-between gap-2 \
                               border-b border-divider px-4 py-2">
                    <h2 class="caps m-0 text-faint">
                        {title}
                    </h2>
                    <div class="flex items-center gap-2">
                        {actions.map(|a| a.run())}
                    </div>
                </header>
            })}
            <div class=body>{children()}</div>
        </section>
    }
}
