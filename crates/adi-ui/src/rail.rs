//! The rail — the column of rows down either side of a chat screen, and the furniture
//! every one of them is made of.
//!
//! [`Rail`] is the container (a title, an optional filter box, a scrolling body),
//! [`RailGroup`] is one labelled band inside it, and [`RailCard`] is the box a row sits in.
//!
//! None of it knows what a row *is*. The left rail fills it with [`crate::SessionItem`] and
//! the right one with [`crate::AppItem`]; a third kind would add a row type and nothing else.

use leptos::prelude::*;

use crate::{input::FRAME, merge};

/// The box every row in the rail is, and nothing else: one hit target, one radius, one
/// focus ring — drawn *inside* the row, because the usual `outline-offset-2` would land
/// outside it and be clipped by the scroll container a rail always is.
///
/// The 1px border is here rather than in the state that wants it, transparent until then.
/// A border a row only grows when it is opened is a row that grows 2px the moment you click
/// it, and a list that shuffles under the pointer.
const CARD: &str = "relative block w-full cursor-pointer rounded-sm border px-2 py-1.5 \
                    text-left transition-colors duration-100 focus-visible:outline-2 \
                    focus-visible:outline-offset-[-2px] focus-visible:outline-accent";

/// The card a row in the rail sits in.
///
/// [`crate::SessionItem`] is this box with a session's ink in it, and a screen that needs a row
/// `SessionItem` and `AppItem` do not describe should reach for this rather than rebuild the box — a
/// second copy of the padding and the inset focus ring is how two rows in the same list
/// end up half a pixel apart.
///
/// `fill` is separate from `class` because it is the half that moves: which row is open
/// changes under the list, so the state's fill arrives as a signal while the call site's
/// own utilities stay a plain string.
///
/// ```ignore
/// <RailCard fill=Signal::derive(move || if open.get() { "bg-selected" } else { "" })>
///     <span class="truncate text-row text-ink">"Walk the linear board"</span>
/// </RailCard>
/// ```
#[component]
pub fn RailCard(
    /// The state's fill, hover and any wash over it — see [`crate::SessionState::row_classes`].
    #[prop(optional, into)]
    fill: Signal<&'static str>,
    /// Whether this is the row the screen is showing. Says so to a screen reader; the
    /// visible half of "open" is the fill.
    #[prop(optional, into)]
    current: Signal<bool>,
    /// Where the row goes. **Given one, the card is an `<a>` rather than a `<button>`** —
    /// a row that opens something is a link, and a link is what a browser lets you
    /// middle-click, copy, and open in a background tab. A button that calls `window.open`
    /// takes all three away and looks identical while doing it.
    #[prop(optional, into)]
    href: String,
    /// Open it in a new tab. Only meaningful with `href`.
    #[prop(optional)]
    blank: bool,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let card = move || merge(&format!("{CARD} {}", fill.get()), class.clone());
    if !href.is_empty() {
        return view! {
            <a
                class=card
                href=href
                target=blank.then_some("_blank")
                rel=blank.then_some("noreferrer noopener")
                aria-current=move || current.get().then_some("true")
            >
                <span class="relative block">{children()}</span>
            </a>
        }
        .into_any();
    }
    view! {
        <button
            class=card
            type="button"
            aria-current=move || current.get().then_some("true")
        >
            // Positioned, so the content paints above an `attention-pulse` wash instead of
            // under it. A `::before` overlay outranks in-flow content, not a positioned box.
            <span class="relative block">{children()}</span>
        </button>
    }
    .into_any()
}

/// One labelled band of the rail: a caps heading, optionally with how many rows are under
/// it, and the rows.
///
/// ```ignore
/// <RailGroup label="Running now" count=2>
///     <crate::SessionItem title="…" state=SessionState::Working/>
/// </RailGroup>
/// ```
#[component]
pub fn RailGroup(
    #[prop(optional, into)] label: String,
    /// How many rows are in the band, printed after the label. Left off, the heading is
    /// just the label — which is what a band holding everything else ("Done") wants, since
    /// a count nobody will read is a number that only ever changes.
    #[prop(optional)]
    count: Option<usize>,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    view! {
        <section class=merge("flex flex-col", class)>
            <h3 class="caps m-0 flex items-baseline justify-between gap-2 px-2.5 pt-4 \
                       pb-1.5 text-faint">
                <span class="truncate">{label}</span>
                {count.map(|n| view! { <span class="shrink-0 text-fainter">{n}</span> })}
            </h3>
            <div class="flex flex-col gap-0.5">{children()}</div>
        </section>
    }
}

/// The rail itself: a title, the filter box, and the scrolling column of
/// [`RailGroup`]s under them.
///
/// **The title scrolls away and the filter box does not.** Everything is inside one scroll
/// port, and the filter is `sticky` at its top: scroll the list and the title goes with the
/// rows, leaving the one control you reach for while scrolling pinned over them. Which is
/// why it carries the panel's own fill — rows pass underneath it.
///
/// It is an island: it carries its own radius and edge rather than waiting for a wrapper to
/// draw them, because a rail is a thing on the screen and not a region of one. It fills the
/// height it is given, so give it a parent with a height — `h-full` against an auto-height
/// parent collapses to nothing.
///
/// Filtering is the caller's: the box binds to a signal and nothing else happens, because
/// only the caller knows whether a query should match a title, an agent, or the transcript.
///
/// ```ignore
/// <Rail
///     title="Sessions"
///     search=query
///     actions=|| view! { <Button variant=ButtonVariant::Link>"+ New"</Button> }.into_any()
/// >
///     <RailGroup label="Running now" count=2>…</RailGroup>
/// </Rail>
/// ```
#[component]
pub fn Rail(
    #[prop(optional, into)] title: String,
    /// Controls pinned to the right of the title — usually the one button that starts a
    /// session.
    #[prop(optional, into)]
    actions: Option<ViewFn>,
    /// Two-way binding for the filter box. Omit it and no filter box is drawn.
    #[prop(optional)]
    search: Option<RwSignal<String>>,
    #[prop(default = String::from("Search"), into)] search_placeholder: String,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let has_head = !title.is_empty() || actions.is_some();

    view! {
        <aside class=merge(
            "island flex h-full min-h-0 flex-col overflow-hidden bg-panel",
            class,
        )>
            <div class="min-h-0 flex-1 overflow-y-auto">
                {has_head.then(|| view! {
                    <header class="flex items-center justify-between gap-2 px-3.5 pt-3">
                        <h2 class="m-0 text-sub font-semibold text-ink">{title}</h2>
                        <div class="flex items-center gap-1">{actions.map(|a| a.run())}</div>
                    </header>
                })}
                {search.map(|value| view! {
                    <div class="sticky top-0 z-10 bg-panel px-3.5 pt-2.5 pb-2">
                        <SearchBox value=value placeholder=search_placeholder/>
                    </div>
                })}
                <div class="px-1.5 pb-3">{children()}</div>
            </div>
        </aside>
    }
}

/// The filter box. Wears the same frame as every other control in the crate — it only adds
/// the room the glyph needs, and takes the sans body type instead of the mono an
/// [`crate::Input`] sets, because what you type here is words rather than a value.
#[component]
fn SearchBox(value: RwSignal<String>, placeholder: String) -> impl IntoView {
    view! {
        <div class="relative">
            <svg
                class="pointer-events-none absolute top-1/2 left-2.5 size-3.5 \
                       -translate-y-1/2 text-faint"
                viewBox="0 0 16 16"
                fill="none"
                stroke="currentColor"
                stroke-width="1.4"
                aria-hidden="true"
            >
                <circle cx="7" cy="7" r="4.6"></circle>
                <path d="M10.6 10.6 14 14" stroke-linecap="round"></path>
            </svg>
            <input
                class=format!("{FRAME} w-full pl-8")
                type="search"
                placeholder=placeholder
                prop:value=move || value.get()
                on:input=move |ev| value.set(event_target_value(&ev))
            />
        </div>
    }
}

/// The quiet second line of a row: monospace, an interpunct between parts.
///
/// Each part arrives with its own ink, because "agent question" is amber and `nakityok-lead` is
/// not, and only the caller knows which is which. Empty parts drop out, so a row can offer
/// everything it might have and render only what it does.
pub(crate) fn meta_line(parts: Vec<(&'static str, String)>) -> impl IntoView {
    let parts: Vec<_> = parts
        .into_iter()
        .filter(|(_, text)| !text.is_empty())
        .collect();

    (!parts.is_empty()).then(|| {
        view! {
            <div class="mt-1 flex items-center gap-1.5 overflow-hidden font-mono text-mini">
                {parts
                    .into_iter()
                    .enumerate()
                    .map(|(i, (ink, text))| view! {
                        {(i > 0).then(|| view! {
                            <span class="shrink-0 text-fainter" aria-hidden="true">"·"</span>
                        })}
                        <span class=format!("shrink-0 whitespace-nowrap {ink}")>{text}</span>
                    })
                    .collect::<Vec<_>>()}
            </div>
        }
    })
}
