//! [`Composer`] — the box you type into, at the bottom of a chat.

use leptos::{ev, html, prelude::*};

use crate::attach::{AttachButton, AttachRefusal, AttachTray, Attaching};
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
/// Given [`Attaching`] it also takes **images** — pasted, dropped onto it, or picked through the
/// paperclip — and shows what is attached above what you are typing. A message that is only a
/// picture sends; a message whose picture is still uploading does not, because that send would
/// arrive without it.
///
/// Sending is the caller's: `on_send` gets the text, and clearing the box is the caller's
/// call too — a send that fails should not have thrown the message away.
///
/// Interrupting is the caller's too, and optional: give it `on_stop` and it grows a Stop
/// *beside* the send button for as long as `stoppable` holds. Beside, not instead —
/// whether typing during a reply is refused, queued, or sent alongside is the caller's
/// question, and a box that swapped its send away would have answered it for them.
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
    /// Called when Stop is pressed. Without it the composer has no Stop at all — a box whose
    /// caller has nothing to interrupt should not carry a control that says otherwise.
    #[prop(optional, into)]
    on_stop: Option<Callback<()>>,
    /// Whether there is something to interrupt *right now*. Reactive, and the Stop appears and
    /// leaves with it: a stop button standing there with nothing running is a button that lies.
    #[prop(optional, into)]
    stoppable: Signal<bool>,
    /// Reactive, because what a composer asks for can change under it: the same box starts a
    /// conversation or takes a one-shot task depending on what the backend turns out to be,
    /// and that answer arrives after the box is already on screen.
    #[prop(default = "Message the agent…".into(), into)]
    placeholder: Signal<String>,
    /// The tallest it grows before it starts scrolling, in `rem`-free pixels — a number
    /// because the ceiling is about the screen, not about the type.
    #[prop(default = 200)]
    max_height: u32,
    /// A control to sit at the left of the button row — [`crate::MicButton`] in this tree.
    ///
    /// A slot rather than a `voice` flag because dictating needs a microphone, an engine and a
    /// network call, and a component library that reached for those would be deciding for every
    /// app that embeds it. The composer only lends the corner.
    #[prop(optional, into)]
    mic: Option<ViewFn>,
    /// Images this message may carry — the tray, the paperclip, and the paste/drop handling. Absent
    /// (the default) the composer takes text and nothing else, and pasting a picture into it does
    /// what it did before: nothing.
    ///
    /// The caller keeps the list and does the uploading, for the same reason [`mic`](Self) is a
    /// slot: where an image is stored is a question about the app.
    #[prop(optional)]
    attach: Option<Attaching>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let area = NodeRef::<html::Textarea>::new();

    // Grow to fit: reset to nothing, measure what the content wants, take the smaller of
    // that and the ceiling. Reading `scrollHeight` without the reset only ever grows, so a
    // box that has been tall stays tall after the text is deleted.
    let fit = move || {
        if let Some(el) = area.get_untracked() {
            el.style(("height", "auto"));
            // An empty box is then left at its `rows`, never measured: for an empty textarea
            // the browser reports the height of the *placeholder* in `scrollHeight`, and a
            // placeholder can be a sentence long — so a sent-and-cleared composer would settle
            // a row taller than an untouched one.
            if el.value().is_empty() {
                return;
            }
            let wanted = el.scroll_height().max(0).unsigned_abs().min(max_height);
            el.style(("height", format!("{wanted}px")));
        }
    };

    // Whether a send would carry anything. Words are the usual answer; an attached picture is the
    // other one, and a message that is only a screenshot is a whole message rather than an empty
    // one. Untracked because `send` is called from an event, not from a render.
    let has_content = move || {
        !value.get_untracked().trim().is_empty()
            || attach.is_some_and(|a| {
                a.files
                    .get_untracked()
                    .iter()
                    .any(crate::Attached::sendable)
            })
    };
    // A picture still on its way is the one thing that holds a send back without the box looking
    // busy: sending now would deliver the words without the image they are about.
    let waiting = move || attach.is_some_and(|a| Attaching::uploading(&a));

    let send = move || {
        let text = value.get_untracked();
        if !has_content() || busy.get_untracked() {
            return;
        }
        on_send.run(text);
        // Clearing is the caller's — see the component docs — but *emptying the element* is
        // not, because `fit` measures the element and the framework writes an emptied signal
        // back on its own clock. Measuring before that write lands would re-measure the
        // message that has already gone, leaving the box standing at its old height.
        if value.get_untracked().is_empty()
            && let Some(el) = area.get_untracked()
        {
            el.set_value("");
        }
        fit();
    };

    let ready = Signal::derive(move || {
        let typed = !value.get().trim().is_empty();
        let attached = attach.is_some_and(|a| a.has_sendable());
        (typed || attached) && !waiting() && !busy.get()
    });

    // The three doors a file comes in by are all wired to one handler. `dragover` has to be
    // cancelled or the browser navigates to the dropped file instead of handing it over — the one
    // default in this component whose absence loses the page you are typing on.
    let take = attach.map(|a| a.on_files);
    let can_attach = attach.map_or(Signal::derive(|| false), |a| a.can_attach);

    view! {
        <div
            class=merge(
                "island flex flex-col gap-1 bg-card p-2 focus-within:border-accent \
                 focus-within:shadow-[0_0_0_3px_color-mix(in_srgb,var(--accent)_16%,transparent)] \
                 transition-[border-color,box-shadow] duration-100",
                class,
            )
            on:dragover=move |ev: ev::DragEvent| {
                if take.is_some() && can_attach.get_untracked() {
                    ev.prevent_default();
                }
            }
            on:drop=move |ev: ev::DragEvent| {
                let Some(take) = take else { return };
                if !can_attach.get_untracked() {
                    return;
                }
                let files = crate::attach::dropped(&ev);
                if !files.is_empty() {
                    ev.prevent_default();
                    take.run(files);
                }
            }
        >
            {attach.map(|attach| view! { <AttachTray attach=attach/> })}
        <div class="flex items-end gap-2">
            <textarea
                class="min-h-8 w-full flex-1 resize-none self-center border-0 bg-transparent \
                       px-1 py-1 text-msg text-body outline-none \
                       placeholder:text-placeholder max-[620px]:text-[16px]"
                rows="1"
                placeholder=move || placeholder.get()
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
                    //
                    // `is_composing` is the one that is not cosmetic: while an IME is open,
                    // Enter accepts the candidate word being typed and must not be read as
                    // send — the same keystroke means two things, and the event alone does
                    // not show which.
                    if ev.key() == "Enter" && !ev.shift_key() && !ev.alt_key() && !ev.ctrl_key()
                        && !ev.meta_key() && !ev.is_composing()
                    {
                        ev.prevent_default();
                        send();
                    }
                }
                on:paste=move |ev: ev::ClipboardEvent| {
                    // Cmd-V with a screenshot on the clipboard. Text pastes carry no files and fall
                    // straight through to the browser's own handling, which is what puts the words
                    // in the box.
                    let Some(take) = take else { return };
                    if !can_attach.get_untracked() {
                        return;
                    }
                    let files = crate::attach::pasted(&ev);
                    if !files.is_empty() {
                        ev.prevent_default();
                        take.run(files);
                    }
                }
            ></textarea>
            // Furthest from send, because it is the control that is pressed *before* a message
            // exists rather than after: putting it next to send would sit an "open the
            // microphone" under the thumb that just went for "send it".
            // Reactive, and it has to be: whether the conversation takes images is an answer that
            // arrives *after* this box is on screen — the chat is drawn as soon as it is opened and
            // its snapshot lands a poll later. Read once at render, every freshly opened chat would
            // sit there refusing images the engine can plainly take.
            {move || {
                attach
                    .filter(|a| a.can_attach.get())
                    .map(|attach| view! { <AttachButton attach=attach/> })
            }}
            {mic.map(|mic| mic.run())}
            // Stop sits to the left of send, so send keeps the corner the hand already goes to
            // and the row does not shuffle under it when a turn starts.
            {move || on_stop.filter(|_| stoppable.get()).map(|on_stop| view! {
                <button
                    class="grid size-8 shrink-0 cursor-pointer place-items-center rounded-sm \
                           border border-dim bg-card text-meta transition-colors duration-100 \
                           hover:border-err-edge-2 hover:text-err \
                           focus-visible:outline-2 focus-visible:outline-offset-2 \
                           focus-visible:outline-accent"
                    type="button"
                    aria-label="Stop"
                    title="Stop"
                    on:click=move |_| on_stop.run(())
                >
                    <svg class="size-3" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
                        <rect x="3" y="3" width="10" height="10" rx="1.5"></rect>
                    </svg>
                </button>
            })}
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
            // Said before anything is pasted, not after: a picture the send would have dropped is
            // one you have already decided to send. Reactive for the reason the paperclip is.
            {move || {
                attach
                    .filter(|a| !a.can_attach.get())
                    .map(|a| view! { <AttachRefusal reason=a.refusal/> })
            }}
            {move || waiting().then(|| view! {
                <div class="px-1 text-mini text-meta">"attaching…"</div>
            })}
        </div>
    }
}
