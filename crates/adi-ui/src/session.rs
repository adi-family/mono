//! The sessions rail — the column of conversations down the left of a chat screen.
//!
//! Four pieces, meant to be composed: [`SessionList`] is the rail itself (title, filter box
//! and the scrolling column), [`SessionGroup`] is one labelled band inside it,
//! [`SessionItem`] is a session, and [`SessionRollup`] is what a run of near-identical
//! sessions collapses into.

use leptos::prelude::*;

use crate::{input::FRAME, merge};

/// Where a session stands, which is the only thing that decides how its row looks.
///
/// The list is sorted by this in practice — what is running, then what is stuck on you,
/// then everything already answered — so the three states are also the three bands a rail
/// is usually built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionState {
    /// An agent is working in it right now.
    Running,
    /// It stopped and cannot go on without you: a question, an approval, a failure.
    Waiting,
    /// Nothing is pending. The default, because a row you forgot to mark should sit quiet
    /// rather than claim the eye.
    #[default]
    Done,
}

impl SessionState {
    /// Rail colour and fill for a row in this state, open or not.
    ///
    /// The rail is a left border that is *always* there and only ever changes colour, so a
    /// row does not shift sideways as it gains and loses attention.
    ///
    /// `Waiting` carries its fill unselected too. A session blocked on you is the one thing
    /// in the list that has to read as raised without being clicked, and it keeps its red
    /// when it *is* the open one — the question has not gone away just because you are
    /// looking at it.
    #[must_use]
    pub fn row_classes(self, selected: bool) -> &'static str {
        match (self, selected) {
            (Self::Running | Self::Done, false) => "border-l-transparent hover:bg-card",
            (Self::Running, true) => "border-l-accent bg-selected",
            (Self::Done, true) => "border-l-dim bg-selected",
            (Self::Waiting, false) => "border-l-err-btn bg-err-bg-2/60 hover:bg-err-bg-2",
            (Self::Waiting, true) => "border-l-err-btn bg-err-bg-2",
        }
    }

    /// Ink for the title. Live work is ink; what is finished recedes to secondary, which is
    /// most of a long list and should read as background until it is searched.
    #[must_use]
    pub fn title_classes(self) -> &'static str {
        match self {
            Self::Running | Self::Waiting => "font-medium text-ink",
            Self::Done => "text-secondary",
        }
    }

    /// The dot before the title, when this state gets one.
    ///
    /// Only `Running` does. `Waiting` already says it in the rail and in the red note under
    /// the title; a third mark for the same fact is noise.
    #[must_use]
    pub fn dot_classes(self) -> Option<&'static str> {
        match self {
            Self::Running => Some("bg-accent"),
            Self::Waiting | Self::Done => None,
        }
    }
}

/// Shared by every row: the box, and a focus ring drawn *inside* it. The usual
/// `outline-offset-2` would land outside the row and be clipped by the scroll container the
/// rail always is.
const ROW: &str = "block w-full cursor-pointer rounded-sm border-l-2 px-2 py-1.5 text-left \
                   transition-colors duration-100 focus-visible:outline-2 \
                   focus-visible:outline-offset-[-2px] focus-visible:outline-accent";

/// One session.
///
/// Event handlers attach the ordinary Leptos way — `<SessionItem on:click=open/>` lands on
/// the underlying `<button>` — so, as with [`crate::Button`], there is no callback prop.
///
/// ```ignore
/// <SessionItem
///     title="Walk the linear board"
///     state=SessionState::Running
///     selected=Signal::derive(move || open.get() == id)
///     alert="3 errors"
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
    /// The one fact worth stopping for — "3 errors", "agent question". Red, and it is the
    /// only thing in the row that is, so spending it on anything else costs the list the
    /// signal.
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
    let row = move || {
        let own = format!("{ROW} {}", if selected.get() { open } else { shut });
        merge(&own, class.clone())
    };
    let meta = meta_line(vec![
        ("text-meta", agent),
        ("font-medium text-err", alert),
        ("text-meta", age),
    ]);

    view! {
        <button
            class=row
            type="button"
            aria-current=move || selected.get().then_some("true")
        >
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
        </button>
    }
}

/// A run of near-identical sessions folded into one line — the nine retries of the same
/// prompt that would otherwise be nine rows of the same words.
///
/// Dashed rather than filled: it is a placeholder for rows, not a row. Clicking it is how
/// they come back, so it takes `on:click` like a [`SessionItem`] does.
///
/// ```ignore
/// <SessionRollup title="Deep-analysis pass on target" note="9 repeats" age="4d"/>
/// ```
#[component]
pub fn SessionRollup(
    #[prop(optional, into)] title: String,
    /// What was folded away: "9 repeats", "3 failed runs".
    #[prop(optional, into)]
    note: String,
    #[prop(optional, into)] age: String,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let meta = meta_line(vec![("text-meta", note), ("text-meta", age)]);
    view! {
        <button
            class=merge(
                "block w-full cursor-pointer rounded-sm border border-dashed border-dim \
                 px-2 py-1.5 text-left transition-colors duration-100 hover:border-edge \
                 hover:bg-card focus-visible:outline-2 \
                 focus-visible:outline-offset-[-2px] focus-visible:outline-accent",
                class,
            )
            type="button"
        >
            <span class="block truncate text-row text-secondary">{title}</span>
            {meta}
        </button>
    }
}

/// One labelled band of the rail: a caps heading, optionally with how many rows are under
/// it, and the rows.
///
/// ```ignore
/// <SessionGroup label="Running now" count=2>
///     <SessionItem title="…" state=SessionState::Running/>
/// </SessionGroup>
/// ```
#[component]
pub fn SessionGroup(
    #[prop(optional, into)] label: String,
    /// How many rows are in the band, printed after the label. Left off,
    /// the heading is just the label — which is what a band holding everything else
    /// ("Done") wants, since a count nobody will read is a number that only ever changes.
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
/// It fills the height it is given and scrolls only its body, so the title and the filter
/// stay put however long the list gets. Give it a parent with a height — `h-full` against
/// an auto-height parent collapses to nothing.
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
            {has_head.then(|| view! {
                <header class="flex items-center justify-between gap-2 px-3.5 pt-3">
                    <h2 class="m-0 text-sub font-semibold text-ink">{title}</h2>
                    <div class="flex items-center gap-1">{actions.map(|a| a.run())}</div>
                </header>
            })}
            {search.map(|value| view! {
                <div class="px-3.5 pt-2.5">
                    <SearchBox value=value placeholder=search_placeholder/>
                </div>
            })}
            <div class="min-h-0 flex-1 overflow-y-auto px-1.5 pb-3">{children()}</div>
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
/// Each part arrives with its own ink, because "3 errors" is red and `nakityok-lead` is
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
