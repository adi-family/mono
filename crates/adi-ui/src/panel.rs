//! The panel — a section of a page.

use leptos::prelude::*;

use crate::merge;

/// A titled section: the section title (16px/600), its actions on the right, a hairline
/// under them, and the content.
///
/// Not a card. Grouping is done with tone and hairlines, never boxes (`design/DESIGN.md`
/// §2.5): the section has no border, no fill and no radius, and its content sits flush with
/// its title — a table inside it draws its own rules against the same left edge.
///
/// The header is dropped entirely when there is no `title` and no `actions`, which makes
/// `<Panel>` on its own a plain block to put anything in.
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
    /// An anchor for the section, so a long page can be linked and jumped into by chapter.
    /// Pair it with a `scroll-mt-*` in `class`, or the sticky bar lands on the title.
    #[prop(optional, into)]
    id: String,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let has_head = !title.is_empty() || actions.is_some();
    // `None` rather than `""`: an empty id is a valid attribute and a useless one.
    let id = (!id.is_empty()).then_some(id);

    view! {
        <section id=id class=merge("flex flex-col text-ink", class)>
            {has_head.then(|| view! {
                <header class="mb-3 flex min-h-8 items-center justify-between gap-3 border-b \
                               border-line pb-2.5">
                    <h2 class="m-0 text-section font-semibold text-ink">{title}</h2>
                    <div class="flex items-center gap-2">
                        {actions.map(|a| a.run())}
                    </div>
                </header>
            })}
            <div class="min-w-0">{children()}</div>
        </section>
    }
}
