//! The rail — the column of rows down either side of a chat screen, and the furniture
//! every one of them is made of.
//!
//! [`Rail`] is the container (a title, an optional filter box, a scrolling body),
//! [`RailGroup`] is one labelled band inside it, and [`RailCard`] is the row's hit target.
//!
//! None of it knows what a row *is*. The left rail fills it with [`crate::SessionItem`] and
//! the right one with [`crate::AppItem`]; a third kind would add a row type and nothing else.

use leptos::prelude::*;

use crate::icon::{Icon, IconSize, Lucide};
use crate::{input::FRAME, merge};

/// The row every item in the rail is: `padding 7px 8px; radius 6` (§6), one hit target, one
/// focus ring — drawn *inside* the row, because the usual `outline-offset-2` would land
/// outside it and be clipped by the scroll container a rail always is.
///
/// `group`, so what a row shows only on hover (a shortcut) can key off it.
const CARD: &str = "group relative block w-full cursor-pointer rounded-md px-2 py-[7px] \
                    text-left transition-colors duration-100 focus-visible:outline-[1.5px] \
                    focus-visible:outline-offset-[-2px] focus-visible:outline-focus";

/// The row a rail item sits in.
///
/// [`crate::SessionItem`] is this box with a session's ink in it, and a screen that needs a
/// row `SessionItem` and `AppItem` do not describe should reach for this rather than rebuild
/// the box — a second copy of the padding and the inset focus ring is how two rows in the
/// same list end up half a pixel apart.
///
/// `fill` is separate from `class` because it is the half that moves: which row is open
/// changes under the list, so the state's fill arrives as a signal while the call site's
/// own utilities stay a plain string. Active is `bg-active`, hover `bg-hover`.
///
/// ```ignore
/// <RailCard fill=Signal::derive(move || if open.get() { "bg-active" } else { "hover:bg-hover" })>
///     <span class="truncate text-row text-ink">"Walk the linear board"</span>
/// </RailCard>
/// ```
#[component]
pub fn RailCard(
    /// The state's fill — see [`crate::SessionState::row_classes`].
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
            <span class="relative block">{children()}</span>
        </button>
    }
    .into_any()
}

/// One labelled band of the rail: a 12px sentence-case heading in `--ink-3`, optionally with
/// how many rows are under it, and the rows.
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
            <h3 class="m-0 flex items-baseline justify-between gap-2 px-2 pt-2.5 pb-1 \
                       text-label font-normal text-ink-3">
                <span class="truncate">{label}</span>
                {count.map(|n| view! { <span class="shrink-0 tabular-nums">{n}</span> })}
            </h3>
            <div class="flex flex-col gap-px">{children()}</div>
        </section>
    }
}

/// The rail itself: a title, the filter box, and the scrolling column of [`RailGroup`]s
/// under them.
///
/// A panel, not a card: flush, with no surface, no border and no radius of its own. The pane
/// it sits in draws the `--bg-side` ground and the hairline where it meets the transcript
/// (§2.5); the rail only lays its rows out. It fills the height it is given, so give it a
/// parent with a height — and a surface.
///
/// **The title scrolls away and the filter box does not.** Everything is inside one scroll
/// port, and the filter is `sticky` at its top: scroll the list and the title goes with the
/// rows, leaving the one control you reach for while scrolling pinned over them. Which is
/// why it carries the rail's own fill — rows pass underneath it.
///
/// Filtering is the caller's: the box binds to a signal and nothing else happens, because
/// only the caller knows whether a query should match a title, an agent, or the transcript.
///
/// ```ignore
/// <Rail
///     title="Sessions"
///     search=query
///     actions=|| view! { <Button size=ButtonSize::Small>"New"</Button> }.into_any()
/// >
///     <RailGroup label="Running now" count=2>…</RailGroup>
/// </Rail>
/// ```
#[component]
pub fn Rail(
    #[prop(optional, into)] title: String,
    /// Controls pinned to the right of the title — icon buttons and the one button that
    /// starts a session.
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
        <aside class=merge("flex h-full min-h-0 flex-col text-ink", class)>
            <div class="min-h-0 flex-1 overflow-y-auto">
                {has_head.then(|| view! {
                    <header class="flex items-center gap-1.5 px-3 pt-3.5 pb-1.5">
                        <h2 class="m-0 mr-auto text-[15px] font-semibold text-ink">{title}</h2>
                        <div class="flex items-center gap-1">{actions.map(|a| a.run())}</div>
                    </header>
                })}
                {search.map(|value| view! {
                    <div class="sticky top-0 z-10 bg-side px-3 pt-1.5 pb-2">
                        <SearchBox value=value placeholder=search_placeholder/>
                    </div>
                })}
                <div class="px-2 pb-3">{children()}</div>
            </div>
        </aside>
    }
}

/// The filter box. Wears the same frame as every other control in the crate — it only adds
/// the room the glyph needs.
#[component]
fn SearchBox(value: RwSignal<String>, placeholder: String) -> impl IntoView {
    view! {
        <div class="relative">
            <Icon
                icon=Lucide::Search
                size=IconSize::Sm
                class="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-ink-3"
            />
            <input
                class=format!("{FRAME} w-full py-1.5 pl-8 text-row")
                type="search"
                placeholder=placeholder
                prop:value=move || value.get()
                on:input=move |ev| value.set(event_target_value(&ev))
            />
        </div>
    }
}

/// The quiet second line of a row: 12px `--ink-3`, an interpunct between parts. Sans — an
/// agent's name in a meta line is a name, not a machine string (§2.3).
///
/// Each part arrives with its own ink, because "agent question" is amber and `nakityok-lead`
/// is not, and only the caller knows which is which. Empty parts drop out, so a row can offer
/// everything it might have and render only what it does.
pub(crate) fn meta_line(parts: Vec<(&'static str, String)>) -> impl IntoView {
    let parts: Vec<_> = parts
        .into_iter()
        .filter(|(_, text)| !text.is_empty())
        .collect();

    (!parts.is_empty()).then(|| {
        view! {
            <div class="mt-px flex items-center gap-1.5 overflow-hidden text-mini text-ink-3">
                {parts
                    .into_iter()
                    .enumerate()
                    .map(|(i, (ink, text))| view! {
                        {(i > 0).then(|| view! {
                            <span class="shrink-0" aria-hidden="true">"·"</span>
                        })}
                        <span class=format!("shrink-0 truncate whitespace-nowrap {ink}")>{text}</span>
                    })
                    .collect::<Vec<_>>()}
            </div>
        }
    })
}
