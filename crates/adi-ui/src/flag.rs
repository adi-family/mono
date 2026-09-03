//! [`FlagMark`] and [`FlagList`] — reading a prompt and marking what is wrong with it.
//!
//! This is what the simulator is *for*. Sitting in the model's seat is only worth the trouble
//! if what you notice while you are there can be written down against the exact words that
//! caused it: not "the tool section is confusing" filed a day later, but this sentence, this
//! run, this note.
//!
//! So the affordance is the one every reader already has. Select a passage and a **Flag this**
//! button appears at the selection; press it and the passage is quoted into a list with an
//! empty note under it. Nothing else is asked for — a form standing between noticing something
//! and recording it is a form that loses most of what gets noticed.
//!
//! The quote is a copy, deliberately. Flags outlive the run they were taken in and become
//! proposed edits to an agent's own prompt, and a flag holding an offset into a document that
//! has since been edited points at whatever moved into that position.

use leptos::{html, prelude::*, wasm_bindgen::JsCast, web_sys};

use crate::icon::{Icon, IconSize, Lucide};
use crate::{Empty, Textarea, merge};

/// A passage somebody marked, and what they had to say about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Flag {
    /// The passage, copied out of the prompt as it read at the time.
    pub quote: String,
    /// What is wrong with it. A signal, so the note can be typed after the flag is taken —
    /// which is the order it happens in — and read back by whoever is going to file it.
    pub note: RwSignal<String>,
}

impl Flag {
    /// A flag on a passage, with nothing said about it yet.
    #[must_use]
    pub fn new(quote: impl Into<String>) -> Self {
        Self {
            quote: quote.into(),
            note: RwSignal::new(String::new()),
        }
    }
}

/// Where the button sits, and what it would quote.
#[derive(Debug, Clone, PartialEq)]
struct Offer {
    /// Pixels from the host's left edge and top edge. Both rects come from
    /// `getBoundingClientRect`, so subtracting them cancels the page scroll *and* any scroll
    /// inside the host — which a prompt this long always has.
    x: f64,
    y: f64,
    quote: String,
}

/// What the user has selected inside `host`, if anything worth flagging.
fn selected_in(host: &web_sys::Element) -> Option<Offer> {
    let selection = window().get_selection().ok().flatten()?;
    if selection.is_collapsed() {
        return None;
    }
    let quote = selection.to_string().as_string()?;
    if quote.trim().is_empty() {
        return None;
    }
    let range = selection.get_range_at(0).ok()?;
    // The *common ancestor* rather than the anchor: a selection dragged from inside the prompt
    // out past its edge still has its anchor inside, and flagging text the reader also caught
    // from the page around it would quote words the model was never handed.
    let within = range.common_ancestor_container().ok()?;
    if !host.contains(Some(&within)) {
        return None;
    }
    let at = range.get_bounding_client_rect();
    let frame = host.get_bounding_client_rect();
    Some(Offer {
        x: at.left() - frame.left(),
        y: at.bottom() - frame.top(),
        quote,
    })
}

/// Wraps something readable and offers to flag whatever is selected in it.
///
/// It renders its children and nothing else — no border, no padding, no opinion about what is
/// inside. A [`PromptText`](crate::PromptText) is the intended child, but anything that is
/// text works, and that is the point: the component knows about selections, not about prompts.
///
/// ```ignore
/// <FlagMark on_flag=Callback::new(move |quote| flags.update(|f| f.push(Flag::new(quote))))>
///     <PromptText tokens=prompt/>
/// </FlagMark>
/// ```
#[component]
pub fn FlagMark(
    /// Called with the selected passage, verbatim.
    #[prop(into)]
    on_flag: Callback<String>,
    /// The button's words. "Flag this" by default, which is what it does.
    #[prop(default = "Flag this".into(), into)]
    label: String,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let host = NodeRef::<html::Div>::new();
    let offer = RwSignal::new(None::<Offer>);

    // `mouseup` for the drag, `keyup` for Shift+arrows: both finish a selection, and a reader
    // who works from the keyboard is exactly the reader who reads the whole prompt. Two
    // closures rather than one, because the two events carry different types.
    let settle = move || {
        let found = host
            .get_untracked()
            .and_then(|el| el.dyn_into::<web_sys::Element>().ok())
            .and_then(|el| selected_in(&el));
        offer.set(found);
    };

    view! {
        // `relative`, because the button is positioned against this box — see [`Offer`].
        <div
            class=merge("relative", class)
            node_ref=host
            on:mouseup=move |_| settle()
            on:keyup=move |_| settle()
        >
            {children()}
            {move || offer.get().map(|Offer { x, y, quote }| view! {
                <button
                    // Above everything in the prompt, and out of the way of the words it is
                    // pointing at: the selection's *bottom* edge, so it never covers what you
                    // just read to decide to flag it. An ink fill — the strong button — so it
                    // stands off the raised surface without a shadow.
                    class="absolute z-20 cursor-pointer rounded-md bg-ink px-2.5 py-1 \
                           text-small font-medium text-bg hover:bg-white \
                           focus-visible:outline-[1.5px] focus-visible:outline-offset-1 \
                           focus-visible:outline-focus"
                    style=format!("left:{}px;top:{}px", x.max(0.0), y + 6.0)
                    type="button"
                    // The selection is gone by the time `click` runs — the browser drops it on
                    // mousedown anywhere else. The quote was taken when the offer was made, so
                    // this reads from the offer and not from the document.
                    on:click=move |_| {
                        on_flag.run(quote.clone());
                        offer.set(None);
                    }
                >
                    {label.clone()}
                </button>
            })}
        </div>
    }
}

/// The flags taken so far, each quoting its passage with a note under it.
///
/// The quote is set in mono against a rule down its left, so it reads as something lifted out
/// of a document rather than something somebody wrote here — which is the distinction the
/// whole list turns on when it is read back later.
///
/// ```ignore
/// <FlagList flags=Signal::derive(move || flags.get()) on_drop=drop_flag/>
/// ```
#[component]
pub fn FlagList(
    /// The flags, in the order they were taken.
    #[prop(into)]
    flags: Signal<Vec<Flag>>,
    /// Take one back.
    #[prop(optional, into)]
    on_drop: Option<Callback<usize>>,
    /// What the empty list says. It gets one, because an empty flag list is the normal state
    /// and it should explain the affordance rather than report a void.
    #[prop(default = "Select any passage above to flag it.".into(), into)]
    empty: String,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let rows = move || {
        let flags = flags.get();
        if flags.is_empty() {
            let empty = empty.clone();
            return view! { <Empty>{empty}</Empty> }.into_any();
        }
        flags
            .into_iter()
            .enumerate()
            .map(|(i, flag)| {
                view! {
                    <div class="flex flex-col gap-1.5">
                        <div class="flex items-start gap-2">
                            <blockquote class="mono m-0 min-w-0 flex-1 border-l-2 \
                                               border-line-strong pl-3 leading-[1.6] \
                                               whitespace-pre-wrap [word-break:break-word]">
                                {flag.quote}
                            </blockquote>
                            {on_drop.map(|cb| view! {
                                <button
                                    class="grid size-6 shrink-0 cursor-pointer place-items-center \
                                           rounded-md text-ink-3 hover:bg-hover hover:text-ink \
                                           focus-visible:outline-[1.5px] \
                                           focus-visible:outline-offset-1 \
                                           focus-visible:outline-focus"
                                    type="button"
                                    title="Unflag this"
                                    aria-label="Unflag this passage"
                                    on:click=move |_| cb.run(i)
                                >
                                    <Icon icon=Lucide::X size=IconSize::Sm/>
                                </button>
                            })}
                        </div>
                        <Textarea
                            value=flag.note
                            rows=2
                            prose=true
                            placeholder="What is wrong with it?"
                        />
                    </div>
                }
            })
            .collect::<Vec<_>>()
            .into_any()
    };

    view! { <div class=merge("flex flex-col gap-3", class)>{rows}</div> }
}
