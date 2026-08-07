//! The sessions rail — the column of conversations down the left of a chat screen.
//!
//! Four pieces, meant to be composed: [`SessionList`] is the rail itself (title, filter box
//! and the scrolling column), [`SessionGroup`] is one labelled band inside it,
//! [`SessionItem`] is a session, and under it [`SessionCard`] is the box that session sits
//! in — which is also what a row `SessionItem` does not describe should be built from,
//! rather than a second copy of the padding and the focus ring.

use leptos::prelude::*;

use crate::{input::FRAME, merge};

/// Where a session stands, which is the only thing that decides how its row looks.
///
/// Only one of the four asks for anything: `Waiting` is *your turn*, and it is the only one
/// that moves. The other three report — finished, broken, busy — and a row that reports
/// should be readable at a glance and quiet the rest of the time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionState {
    /// Nothing is pending. The default, because a row you forgot to mark should sit quiet
    /// rather than claim the eye.
    #[default]
    Done,
    /// It stopped and cannot go on without you: a question, an approval, a choice.
    Waiting,
    /// It failed. Loud in the dot and in the words, but still — it wants reading, not
    /// answering, so it does not move.
    Error,
    /// An agent is working in it right now.
    Working,
}

impl SessionState {
    /// The fill and border for a row in this state, open or not.
    ///
    /// The open row is a card: a fill and a hairline **all the way round**, rather than a
    /// marker down one edge. A rail in the gutter costs every row in the list the same 2px
    /// whether it is using them or not, and it said a second time what the tint, the dot
    /// and the ink already say.
    ///
    /// The border is on the card in every state and only ever changes colour — see
    /// [`CARD`] — so the open row does not grow a pixel taller than the ones around it.
    ///
    /// `Waiting` breathes whether it is open or not. A session blocked on you has not
    /// stopped being blocked because you are looking at it, and its wash sits *over* the
    /// open row's fill rather than instead of it — see the `attention-pulse` utility.
    #[must_use]
    pub fn row_classes(self, selected: bool) -> &'static str {
        match (self, selected) {
            (Self::Done | Self::Error | Self::Working, false) => {
                "border-transparent hover:bg-card"
            }
            (Self::Done | Self::Error | Self::Working, true) => "border-edge bg-selected",
            (Self::Waiting, false) => "attention-pulse border-transparent hover:bg-queue-bg",
            (Self::Waiting, true) => "attention-pulse border-edge bg-selected",
        }
    }

    /// Ink for the title. Anything still standing open is ink; what is finished recedes to
    /// secondary, which is most of a long list and should read as background until it is
    /// searched.
    #[must_use]
    pub fn title_classes(self) -> &'static str {
        match self {
            Self::Waiting | Self::Error | Self::Working => "font-medium text-ink",
            Self::Done => "text-secondary",
        }
    }

    /// Ink for the row's one loud word.
    ///
    /// A question is amber and everything else that is worth saying twice is red: amber is
    /// the colour of *your turn*, and spending red on it would put a run that merely wants
    /// an answer in the same voice as one that broke.
    #[must_use]
    pub fn alert_classes(self) -> &'static str {
        match self {
            Self::Waiting => "font-medium text-attention",
            Self::Done | Self::Error | Self::Working => "font-medium text-err",
        }
    }

    /// The dot before the title, when this state gets one.
    ///
    /// Green for busy, red for broken, and nothing for the two that already say it
    /// elsewhere: `Waiting` has the wash under it and `Done` has nothing to report. A dot
    /// per state would be four marks where two carry the news.
    #[must_use]
    pub fn dot_classes(self) -> Option<&'static str> {
        match self {
            Self::Working => Some("bg-accent"),
            Self::Error => Some("bg-err"),
            Self::Waiting | Self::Done => None,
        }
    }
}

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
/// [`SessionItem`] is this box with a session's ink in it, and a screen that needs a row
/// `SessionItem` does not describe should reach for this rather than rebuild the box — a
/// second copy of the padding and the inset focus ring is how two rows in the same list
/// end up half a pixel apart.
///
/// `fill` is separate from `class` because it is the half that moves: which row is open
/// changes under the list, so the state's fill arrives as a signal while the call site's
/// own utilities stay a plain string.
///
/// ```ignore
/// <SessionCard fill=Signal::derive(move || if open.get() { "bg-selected" } else { "" })>
///     <span class="truncate text-row text-ink">"Walk the linear board"</span>
/// </SessionCard>
/// ```
#[component]
pub fn SessionCard(
    /// The state's fill, hover and any wash over it — see [`SessionState::row_classes`].
    #[prop(optional, into)]
    fill: Signal<&'static str>,
    /// Whether this is the row the screen is showing. Says so to a screen reader; the
    /// visible half of "open" is the fill.
    #[prop(optional, into)]
    current: Signal<bool>,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let card = move || merge(&format!("{CARD} {}", fill.get()), class.clone());
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
}

/// One session.
///
/// Event handlers attach the ordinary Leptos way — `<SessionItem on:click=open/>` lands on
/// the underlying `<button>` — so, as with [`crate::Button`], there is no callback prop.
///
/// ```ignore
/// <SessionItem
///     title="Walk the linear board"
///     state=SessionState::Waiting
///     selected=Signal::derive(move || open.get() == id)
///     alert="agent question"
///     age="14m"
///     on:click=move |_| open.set(id)
/// />
/// ```
#[component]
pub fn SessionItem(
    #[prop(optional, into)] title: String,
    #[prop(optional)] state: SessionState,
    /// The one the screen is currently showing.
    #[prop(optional, into)]
    selected: Signal<bool>,
    /// Who ran it. Monospace, because it is a name the machine chose.
    #[prop(optional, into)]
    agent: String,
    /// What the row is waiting on, in the state's own colour: "agent question", "needs
    /// approval". It is the only coloured thing in the row, so spending it on anything else
    /// costs the list the signal.
    ///
    /// Say what it wants, not how much of it there is. A count — "3 errors" — reads as
    /// news and carries none: it does not say what broke, it changes every run, and the
    /// row is already red.
    #[prop(optional, into)]
    alert: String,
    /// How long ago, already written the way you want it read: "14m", "2h", "4d".
    #[prop(optional, into)]
    age: String,
    #[prop(optional, into)] class: String,
    /// A `<span>` after the title — an unread count, a badge.
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    let (open, shut) = (state.row_classes(true), state.row_classes(false));
    let fill = Signal::derive(move || if selected.get() { open } else { shut });
    let meta = meta_line(vec![
        ("text-meta", agent),
        (state.alert_classes(), alert),
        ("text-meta", age),
    ]);

    view! {
        <SessionCard fill=fill current=selected class=class>
            <div class="flex items-center gap-1.5">
                {state.dot_classes().map(|dot| view! {
                    <span
                        class=format!("size-1.5 shrink-0 rounded-full {dot}")
                        aria-hidden="true"
                    ></span>
                })}
                <span class=format!("truncate text-row {}", state.title_classes())>
                    {title}
                </span>
                {children.map(|c| c())}
            </div>
            {meta}
        </SessionCard>
    }
}

/// One labelled band of the rail: a caps heading, optionally with how many rows are under
/// it, and the rows.
///
/// ```ignore
/// <SessionGroup label="Running now" count=2>
///     <SessionItem title="…" state=SessionState::Working/>
/// </SessionGroup>
/// ```
#[component]
pub fn SessionGroup(
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
            <h3 class="caps m-0 px-2.5 pt-4 pb-1.5 text-faint">
                {label}
                {count.map(|n| view! { <span class="text-fainter">" · "{n}</span> })}
            </h3>
            <div class="flex flex-col gap-0.5">{children()}</div>
        </section>
    }
}

/// The rail itself: a title, the filter box, and the scrolling column of
/// [`SessionGroup`]s under them.
///
/// **The title scrolls away and the filter box does not.** Everything is inside one scroll
/// port, and the filter is `sticky` at its top: scroll the list and the title goes with the
/// rows, leaving the one control you reach for while scrolling pinned over them. Which is
/// why it carries the panel's own fill — rows pass underneath it.
///
/// It fills the height it is given, so give it a parent with a height — `h-full` against an
/// auto-height parent collapses to nothing.
///
/// Filtering is the caller's: the box binds to a signal and nothing else happens, because
/// only the caller knows whether a query should match a title, an agent, or the transcript.
///
/// ```ignore
/// <SessionList
///     title="Sessions"
///     search=query
///     actions=|| view! { <Button variant=ButtonVariant::Link>"+ New"</Button> }.into_any()
/// >
///     <SessionGroup label="Running now" count=2>…</SessionGroup>
/// </SessionList>
/// ```
#[component]
pub fn SessionList(
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
        <aside class=merge("flex h-full min-h-0 flex-col bg-panel", class)>
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
fn meta_line(parts: Vec<(&'static str, String)>) -> impl IntoView {
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
