//! [`Modal`] — the one thing on the screen, over everything else that was.

use leptos::{ev, prelude::*};

use crate::merge;

/// A dialog over the page.
///
/// It is an island like everything else, laid over a scrim that dims what it interrupts —
/// which is the point of a modal and also its cost, so it is worth reserving for the times
/// the reader has to finish something before going on.
///
/// Three ways out, because a reader who cannot find the way out of a dialog is stuck in
/// your app: the close button, the scrim, and `Escape`. The listener lives only while the
/// dialog is open and is torn down with it.
///
/// ```ignore
/// let open = RwSignal::new(false);
/// view! { <Modal open=open title="FAQ"><Faq items=QNA/></Modal> }
/// ```
#[component]
pub fn Modal(
    /// Two-way: the dialog closes itself through this, so the caller only ever has to open
    /// it.
    open: RwSignal<bool>,
    #[prop(optional, into)] title: String,
    /// How wide it is allowed to get. The default suits prose; a form wants less.
    #[prop(default = "max-w-2xl")]
    width: &'static str,
    #[prop(optional, into)] class: String,
    children: ChildrenFn,
) -> impl IntoView {
    let handle = window_event_listener(ev::keydown, move |ev| {
        if ev.key() == "Escape" && open.get_untracked() {
            open.set(false);
        }
    });
    on_cleanup(move || handle.remove());

    view! {
        <Show when=move || open.get()>
            <div
                class="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto p-4 \
                       pt-[10vh]"
                role="dialog"
                aria-modal="true"
            >
                // The scrim: a click anywhere off the card is a way out, so it has to be
                // its own element under the card rather than a background on the wrapper.
                <div
                    class="fixed inset-0 bg-canvas/70 backdrop-blur-[2px]"
                    on:click=move |_| open.set(false)
                ></div>
                <div class=merge(
                    &format!(
                        "island relative flex max-h-[80vh] w-full flex-col overflow-hidden \
                         bg-card {width}"
                    ),
                    class.clone(),
                )>
                    <header class="flex min-h-9 shrink-0 items-center justify-between gap-2 \
                                   border-b border-divider bg-bar px-3 py-1.5">
                        <span class="truncate text-row font-medium text-ink">{title.clone()}</span>
                        <button
                            class="grid size-6 shrink-0 cursor-pointer place-items-center \
                                   rounded-sm text-meta hover:bg-card hover:text-ink \
                                   focus-visible:outline-2 focus-visible:outline-offset-[-2px] \
                                   focus-visible:outline-accent"
                            type="button"
                            aria-label="Close"
                            on:click=move |_| open.set(false)
                        >
                            <svg
                                class="size-3.5"
                                viewBox="0 0 16 16"
                                fill="none"
                                stroke="currentColor"
                                stroke-width="1.5"
                                stroke-linecap="round"
                                aria-hidden="true"
                            >
                                <path d="M4 4l8 8M12 4l-8 8"></path>
                            </svg>
                        </button>
                    </header>
                    <div class="min-h-0 flex-1 overflow-y-auto p-4">{children()}</div>
                </div>
            </div>
        </Show>
    }
}
