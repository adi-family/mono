//! The app — one living thing, as a row in the right rail.
//!
//! *Living* is the word that matters. These are not documents you open; they are running
//! somewhere right now, on a machine that may or may not still be answering. An app belongs
//! to two things at once and the row says both: the **project** it is part of, which is the
//! band it sits in, and the **machine** it runs on, which is the line under its name.

use leptos::prelude::*;

use crate::{merge, rail::RailCard};

/// Whether an app is showing you anything right now.
///
/// The three are not degrees of the same thing: `Live` is working, `Offline` is *the
/// machine* being away rather than the app being wrong, and `ViewOnly` is working perfectly
/// on someone else's machine, where you are a guest. Only the middle one is a problem, and
/// only it gets red.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppState {
    /// Up, and its machine is answering.
    #[default]
    Live,
    /// The machine it lives on is not reachable. The app is fine; the machine is away.
    Offline,
    /// Live, on a machine you can watch but not touch.
    ViewOnly,
}

impl AppState {
    /// The dot on the icon's corner — **the only thing in the row that reports state**.
    ///
    /// It used to be a dot *and* a word ("machine offline", "view only") on every row. Two
    /// marks for one fact is one too many in a list you scan rather than read, and the
    /// words were the half that could be dropped: a colour says it at a glance, from the
    /// corner of the eye, without taking a column.
    #[must_use]
    pub fn dot_classes(self) -> &'static str {
        match self {
            // `live`, not `accent`. These three are a semantic triple — good, bad, inert —
            // and the accent is for what is interactive or selected. It was `bg-accent`
            // while the accent was mint, which happened to read as "running"; against an
            // orange accent the same rule painted every healthy app the colour of a warning.
            Self::Live => "bg-live",
            Self::Offline => "bg-err",
            Self::ViewOnly => "bg-faint",
        }
    }

    /// What the dot means, for whoever cannot see it. It is also the button's label when
    /// the dot is one.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Live => "Live",
            Self::Offline => "Machine offline",
            Self::ViewOnly => "View only",
        }
    }

    /// Ink for the name. Only a live one is at full strength; the other two are still there,
    /// still readable, and no longer the thing your eye should stop on.
    #[must_use]
    pub fn title_classes(self) -> &'static str {
        match self {
            Self::Live => "text-ink",
            Self::Offline | Self::ViewOnly => "text-secondary",
        }
    }
}

/// One app.
///
/// The icon is **two lines tall** and everything else is set against it: the name on the
/// first line, the machine on the second. An app is recognised by its mark long before its
/// name is read, so the mark is what the column is built on.
///
/// ```ignore
/// <AppItem title="IVR Call Funnel" favicon="/icons/ivr.png" machine="zomro-de1" on:click=open/>
/// ```
#[component]
pub fn AppItem(
    #[prop(optional, into)] title: String,
    #[prop(optional)] state: AppState,
    /// The app's own icon, by URL — the favicon its front door serves. Left off, the row
    /// draws a monogram of the name instead, so the column never loses its left edge.
    #[prop(optional, into)]
    favicon: String,
    /// The machine it runs on. **Left empty it reads "this machine"** — an app running
    /// right here is the common case, and naming the box you are sitting at tells you
    /// nothing you did not know.
    #[prop(optional, into)]
    machine: String,
    /// The one the screen is currently showing.
    #[prop(optional, into)]
    selected: Signal<bool>,
    /// What this row's state can offer, revealed by clicking the dot — "Connect machine"
    /// under an offline one. **Hidden until asked for**: a button that sits on every broken
    /// row is a button in the way of the five working ones.
    #[prop(optional, into)]
    action: Option<ViewFn>,
    /// Where the app opens. It makes the row a link — see [`RailCard`].
    #[prop(optional, into)]
    href: String,
    /// Open it in a new tab. An app runs on its own origin, so this is usually what you
    /// want.
    #[prop(optional)]
    blank: bool,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let fill = Signal::derive(move || {
        if selected.get() {
            "border-edge bg-selected"
        } else {
            "border-transparent hover:bg-card"
        }
    });
    // Whether the dot has been asked what it means. Local: nothing outside this row cares,
    // and a row that reveals its own control does not need the screen's permission.
    let revealed = RwSignal::new(false);
    let dot = state.dot_classes();
    let mark = title
        .chars()
        .next()
        .unwrap_or('·')
        .to_uppercase()
        .to_string();
    let machine = if machine.is_empty() {
        String::from("this machine")
    } else {
        machine
    };
    let interactive = action.is_some();

    view! {
        <div class=merge("relative", class)>
            <RailCard fill=fill current=selected href=href blank=blank>
                <div class="flex items-center gap-2.5">
                    // Two lines tall, and the row is built around it.
                    {if favicon.is_empty() {
                        view! {
                            <span class="grid size-8 shrink-0 place-items-center rounded-sm \
                                         bg-bubble font-mono text-row font-medium text-meta">
                                {mark}
                            </span>
                        }
                        .into_any()
                    } else {
                        view! {
                            <img
                                class="size-8 shrink-0 rounded-sm bg-bubble object-cover"
                                src=favicon
                                alt=""
                                loading="lazy"
                            />
                        }
                        .into_any()
                    }}
                    <span class="flex min-w-0 flex-col">
                        <span class=format!("truncate text-row {}", state.title_classes())>
                            {title}
                        </span>
                        <span class="truncate font-mono text-mini text-meta">{machine}</span>
                    </span>
                </div>
            </RailCard>
            // The dot rides the icon's corner, and it is laid *over* the card rather than
            // inside it: when it is a control, a button inside a button is not a thing a
            // browser will do. Its box is the icon's box — the card's own `px-2 py-1.5`
            // plus `size-8` — so the two cannot drift apart.
            {if interactive {
                view! {
                    <button
                        class="absolute top-1.5 left-2 size-8 cursor-pointer rounded-sm \
                               focus-visible:outline-2 focus-visible:outline-offset-2 \
                               focus-visible:outline-accent"
                        type="button"
                        title=state.label()
                        aria-label=state.label()
                        on:click=move |_| revealed.update(|r| *r = !*r)
                    >
                        <span
                            class=format!(
                                "absolute -right-0.5 -bottom-0.5 size-2 rounded-full ring-2 \
                                 ring-panel {dot}",
                            )
                            aria-hidden="true"
                        ></span>
                    </button>
                }
                .into_any()
            } else {
                view! {
                    <span
                        class="pointer-events-none absolute top-1.5 left-2 size-8"
                        title=state.label()
                    >
                        <span
                            class=format!(
                                "absolute -right-0.5 -bottom-0.5 size-2 rounded-full ring-2 \
                                 ring-panel {dot}",
                            )
                            aria-hidden="true"
                        ></span>
                    </span>
                }
                .into_any()
            }}
            {action.map(|a| view! {
                <Show when=move || revealed.get()>
                    <div class="mt-1 pl-2">{a.run()}</div>
                </Show>
            })}
        </div>
    }
}
