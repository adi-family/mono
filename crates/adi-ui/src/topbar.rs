//! [`TopBar`] — the bar across the top of a screen: who this is on the left, where you are
//! in the middle, what you can do on the right.

use leptos::prelude::*;

use crate::mark::Mark;
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
/// Written left to right from the mark — `adi / Settings / Fleet` — so the bar reads as one
/// sentence rather than as two clumps with a void between them. Sans, 13px, `--ink-3`, with
/// the segment you are on in `--ink` 500: a location is not a machine string (§2.3).
///
/// The last segment is where you are and is never a link, however it was built: a link to
/// the page you are on is a control that does nothing, and the reader has to click it to
/// find that out.
///
/// ```ignore
/// <Crumbs items=vec![Crumb::new("Settings").href("/settings"), Crumb::new("Fleet")]/>
/// ```
#[component]
pub fn Crumbs(
    #[prop(into)] items: Signal<Vec<Crumb>>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    view! {
        <nav
            class=merge("flex min-w-0 items-center gap-1 text-small text-ink-3", class)
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
                            <span class="shrink-0" aria-hidden="true">"/"</span>
                            {match crumb.href {
                                Some(href) if !here => view! {
                                    <a class="truncate no-underline hover:text-ink-2 \
                                              hover:no-underline" href=href>
                                        {crumb.label}
                                    </a>
                                }
                                .into_any(),
                                _ => view! {
                                    <span
                                        class=if here { "truncate font-medium text-ink" } else { "truncate" }
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

/// The bar across the very top of a screen: 48px, on `--bg-side`, a hairline under it (§5).
///
/// It is `sticky`, so it stays while a document scrolls under it, and harmless in an app
/// shell where it is a flex row that never scrolls in the first place.
///
/// Three slots, left to right: the mark and wordmark, whatever says where you are, and the
/// controls. The middle one takes the free space, which is what puts the actions hard
/// against the right edge whether it is filled or empty. The one filled orange a bar may hold
/// is the update button, and only while an update exists.
///
/// ```ignore
/// <TopBar
///     logo="adi"
///     actions=|| view! { <Button size=ButtonSize::Small icon=Lucide::Settings2/> }.into_any()
/// >
///     <Crumbs/>
/// </TopBar>
/// ```
#[component]
pub fn TopBar(
    /// The word beside the mark — `adi`, 15px/600 (§10). Left off, the bar starts with
    /// whatever the children are: the two travel together, so there is no mark without it.
    #[prop(optional, into)]
    logo: String,
    /// Where the mark goes when you click it: the way home, and the one navigation every
    /// screen owes the reader. Left off, the mark is not a link and not focusable, which is
    /// correct for a screen that *is* home *and* has nothing to reset — otherwise see
    /// `on_home`.
    #[prop(optional, into)]
    home: String,
    /// What the mark does on a screen that is already home: put it back the way it opened.
    ///
    /// A single-page screen keeps "where you are" in state rather than in the URL, so the way
    /// home there is not a href — it is closing whatever got opened. The mark is still the
    /// thing every reader clicks to get out of a corner, so it takes that job too, as a
    /// button. Takes precedence over `home`, since a screen with both is home already.
    ///
    /// It must stay a *reset*, not an action: clicking the wordmark may undo a selection, but
    /// it may never start, send, or destroy anything.
    #[prop(optional, into)]
    on_home: Option<Callback<()>>,
    /// Controls pinned to the right: the version, an install button, the update button.
    /// Small [`crate::Button`]s.
    #[prop(optional, into)]
    actions: Option<ViewFn>,
    #[prop(optional, into)] class: String,
    /// Between the two: breadcrumbs, a document title, a status. Read left to right from the
    /// mark, which is the natural reading order and keeps the bar from being two clumps with
    /// a void between them.
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    view! {
        <header class=merge(
            "sticky top-0 z-40 flex h-12 shrink-0 items-center gap-3 border-b border-line \
             bg-side px-4 text-small text-ink-3",
            class,
        )>
            {(!logo.is_empty()).then(|| {
                // The same mark all three ways, so it does not move or change weight when a
                // screen gains a way home. The link only adds what a link is: a target, a
                // focus ring, and no underline — the mark is a mark, not a sentence.
                let mark = view! {
                    <Mark class="size-[18px]"/>
                    <span>{logo}</span>
                };
                let cls = "flex shrink-0 items-center gap-2 text-[15px] font-semibold text-ink \
                           no-underline hover:text-ink hover:no-underline";
                match (on_home, home.is_empty()) {
                    // Home already: the mark reopens this screen rather than going to it. A
                    // button and not a link, because nothing is being navigated to.
                    (Some(reset), _) => view! {
                        <button
                            type="button"
                            class=format!(
                                "{cls} cursor-pointer bg-transparent p-0 \
                                 focus-visible:outline-[1.5px] focus-visible:outline-offset-2 \
                                 focus-visible:outline-focus"
                            )
                            title="Back to the start"
                            on:click=move |_| reset.run(())
                        >
                            {mark}
                        </button>
                    }
                    .into_any(),
                    (None, false) => view! { <a class=cls href=home title="Home">{mark}</a> }
                        .into_any(),
                    (None, true) => view! { <span class=cls>{mark}</span> }.into_any(),
                }
            })}
            <div class="flex min-w-0 flex-1 items-center gap-2">{children.map(|c| c())}</div>
            <div class="flex shrink-0 items-center gap-2">{actions.map(|a| a.run())}</div>
        </header>
    }
}
