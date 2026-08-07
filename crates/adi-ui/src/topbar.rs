//! [`TopBar`] — the lid of the window: who this is on the left, where you are in the
//! middle, what you can do on the right.

use leptos::prelude::*;

use crate::merge;

/// One segment of the path in the bar.
///
/// A segment with an `href` is somewhere you can go back to; the one without is where you
/// are. In practice that means every crumb but the last carries one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crumb {
    pub label: String,
    pub href: Option<String>,
}

impl Crumb {
    /// A segment that is just a name — where you are, or a scope with nothing behind it.
    #[must_use]
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            href: None,
        }
    }

    /// Make it somewhere you can go back to.
    #[must_use]
    pub fn href(mut self, href: impl Into<String>) -> Self {
        self.href = Some(href.into());
        self
    }
}

/// The path to what is open, for [`TopBar`]'s middle slot.
///
/// Written left to right from the mark — `adi. / projects / adi-ui` — so the bar reads as
/// one sentence rather than as two clumps with a void between them. Monospace, because a
/// path is a path.
///
/// The last segment is where you are and is never a link, however it was built: a link to
/// the page you are on is a control that does nothing, and the reader has to click it to
/// find that out.
///
/// ```ignore
/// <Crumbs items=vec![Crumb::new("projects").href("/projects"), Crumb::new("adi-ui")]/>
/// ```
#[component]
pub fn Crumbs(
    #[prop(into)] items: Signal<Vec<Crumb>>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    view! {
        <nav
            class=merge("flex min-w-0 items-center gap-1.5 font-mono text-mini", class)
            aria-label="Breadcrumb"
        >
            {move || {
                let items = items.get();
                let last = items.len().saturating_sub(1);
                items
                    .into_iter()
                    .enumerate()
                    .map(|(i, crumb)| {
                        let here = i == last;
                        view! {
                            <span class="shrink-0 text-fainter" aria-hidden="true">"/"</span>
                            {match crumb.href {
                                Some(href) if !here => view! {
                                    <a class="truncate text-meta no-underline hover:text-accent \
                                              hover:no-underline" href=href>
                                        {crumb.label}
                                    </a>
                                }
                                .into_any(),
                                _ => view! {
                                    <span
                                        class="truncate text-secondary"
                                        aria-current=here.then_some("page")
                                    >
                                        {crumb.label}
                                    </span>
                                }
                                .into_any(),
                            }}
                        }
                    })
                    .collect::<Vec<_>>()
            }}
        </nav>
    }
}

/// The bar across the very top of a screen.
///
/// **It is the one thing in this crate that is not an island.** Everything else floats on
/// the canvas with a gap around it, because everything else is an object on the screen; the
/// top bar is the screen's own edge, and an edge that floats reads as a card that got
/// stuck to the ceiling. So it goes wall to wall, on the bar surface, closed by a hairline
/// — the same shape the control panel's titlebar has always had.
///
/// It is `sticky`, so it stays while a document scrolls under it, and harmless in an app
/// shell where it is a flex row that never scrolls in the first place.
///
/// Three slots, left to right: the wordmark, whatever says where you are, and the controls.
/// The middle one takes the free space, which is what puts the actions hard against the
/// right edge whether it is filled or empty.
///
/// ```ignore
/// <TopBar
///     logo="adi"
///     actions=|| view! { <Button size=ButtonSize::Small icon=GEAR/> }.into_any()
/// >
///     <Crumbs/>
/// </TopBar>
/// ```
#[component]
pub fn TopBar(
    /// The wordmark, set in mono and closed with an accent dot — `logo="adi"` reads `adi.`,
    /// which is the mark. Left off, the bar starts with whatever the children are.
    #[prop(optional, into)]
    logo: String,
    /// Where the mark goes when you click it: the way home, and the one navigation every
    /// screen owes the reader. Left off, the mark is not a link and not focusable, which is
    /// correct for a screen that *is* home.
    #[prop(optional, into)]
    home: String,
    /// Controls pinned to the right: a theme toggle, an install button, an account menu.
    /// Small [`crate::Button`]s — the bar is 38px.
    #[prop(optional, into)]
    actions: Option<ViewFn>,
    #[prop(optional, into)] class: String,
    /// Between the two: breadcrumbs, a document title, a status pill. Read left to right
    /// from the mark, which is the natural reading order and keeps the bar from being two
    /// clumps with a void between them.
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    view! {
        <header class=merge(
            "sticky top-0 z-40 flex h-9.5 shrink-0 items-center gap-3 border-b \
             border-divider bg-bar px-3",
            class,
        )>
            {(!logo.is_empty()).then(|| {
                // The same mark either way, so it does not move or change weight when a
                // screen gains a way home. The link only adds what a link is: a target, a
                // focus ring, and no underline — the mark is a mark, not a sentence.
                let mark = view! {
                    {logo}<span class="text-accent">"."</span>
                };
                let cls = "shrink-0 font-mono text-sub font-semibold tracking-[-0.03em] \
                           text-ink no-underline hover:text-ink hover:no-underline";
                if home.is_empty() {
                    view! { <span class=cls>{mark}</span> }.into_any()
                } else {
                    view! { <a class=cls href=home title="Home">{mark}</a> }.into_any()
                }
            })}
            <div class="flex min-w-0 flex-1 items-center gap-2">{children.map(|c| c())}</div>
            <div class="flex shrink-0 items-center gap-1">{actions.map(|a| a.run())}</div>
        </header>
    }
}
