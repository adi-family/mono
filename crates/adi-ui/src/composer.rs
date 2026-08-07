//! [`Composer`] — the box you type into, at the bottom of a chat.

use leptos::{ev, html, prelude::*};

use crate::merge;

/// The composer.
///
/// It is a [`crate::Textarea`]'s frame with the three behaviours a chat box needs and a bare
/// textarea does not have:
///
/// - **Enter sends, Shift+Enter breaks the line.** Every chat works this way and every
///   person expects it; a box where Enter inserts a newline makes you hunt for a button.
/// - **It grows with the message**, up to a ceiling, then scrolls. A composer that stays one
///   line hides what you wrote; one that grows without limit eats the transcript you are
///   replying to.
/// - **It says whether it can send.** Empty or busy, the button is out; both are states you
///   can be in for a while, and a control that looks live and does nothing is worse than one
///   that is visibly out.
///
/// Sending is the caller's: `on_send` gets the text, and clearing the box is the caller's
/// call too — a send that fails should not have thrown the message away.
///
/// ```ignore
/// <Composer value=draft busy=sending on_send=Callback::new(move |text| post(text))/>
/// ```
#[component]
pub fn Composer(
    /// What is typed, two-way. The caller owns it, so a draft survives a re-render and a
    /// failed send keeps its text.
    value: RwSignal<String>,
    /// Called with the message when Enter is pressed or the button is clicked. Never called
    /// with something blank.
    #[prop(into)]
    on_send: Callback<String>,
    /// A send is in flight. The box stays readable and stops accepting.
    #[prop(optional, into)]
    busy: Signal<bool>,
    #[prop(default = "Message the agent…")] placeholder: &'static str,
    /// The tallest it grows before it starts scrolling, in `rem`-free pixels — a number
    /// because the ceiling is about the screen, not about the type.
    #[prop(default = 200)]
    max_height: u32,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let area = NodeRef::<html::Textarea>::new();

    // Grow to fit: reset to nothing, measure what the content wants, take the smaller of
    // that and the ceiling. Reading `scrollHeight` without the reset only ever grows, so a
    // box that has been tall stays tall after the text is deleted.
    let fit = move || {
        if let Some(el) = area.get_untracked() {
            el.style(("height", "auto"));
            let wanted = el.scroll_height().max(0).unsigned_abs().min(max_height);
            el.style(("height", format!("{wanted}px")));
        }
    };

    let send = move || {
        let text = value.get_untracked();
        if text.trim().is_empty() || busy.get_untracked() {
            return;
        }
        on_send.run(text);
        // Not cleared here — see the component docs.
        fit();
    };

    let ready = Signal::derive(move || !value.get().trim().is_empty() && !busy.get());

    view! {
        <div class=merge(
            "island flex items-end gap-2 bg-card p-2 focus-within:border-accent \
             focus-within:shadow-[0_0_0_3px_color-mix(in_srgb,var(--accent)_16%,transparent)] \
             transition-[border-color,box-shadow] duration-100",
            class,
        )>
            <textarea
                class="min-h-8 w-full flex-1 resize-none self-center border-0 bg-transparent \
                       px-1 py-1 text-msg text-body outline-none \
                       placeholder:text-placeholder max-[620px]:text-[16px]"
                rows="1"
                placeholder=placeholder
                node_ref=area
                disabled=move || busy.get()
                prop:value=move || value.get()
                on:input=move |ev| {
                    value.set(event_target_value(&ev));
                    fit();
                }
                on:keydown=move |ev: ev::KeyboardEvent| {
                    // Shift+Enter is a newline; every other modifier is somebody else's
                    // shortcut and none of our business.
                    if ev.key() == "Enter" && !ev.shift_key() && !ev.alt_key() && !ev.ctrl_key()
                        && !ev.meta_key()
                    {
                        ev.prevent_default();
                        send();
                    }
                }
            ></textarea>
            <button
                class="grid size-8 shrink-0 cursor-pointer place-items-center rounded-sm \
                       bg-accent-fill text-on-accent transition-opacity duration-100 \
                       hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-40 \
                       focus-visible:outline-2 focus-visible:outline-offset-2 \
                       focus-visible:outline-accent"
                type="button"
                aria-label="Send"
                disabled=move || !ready.get()
                on:click=move |_| send()
            >
                <svg
                    class="size-4"
                    viewBox="0 0 16 16"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.6"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    aria-hidden="true"
                >
                    <path d="M8 13V3"></path>
                    <path d="M3.5 7.5 8 3l4.5 4.5"></path>
                </svg>
            </button>
        </div>
    }
}
