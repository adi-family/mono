//! [`Modal`] — the one thing on the screen, over everything else that was.

use leptos::{ev, prelude::*};

use crate::icon::{Icon, Lucide};
use crate::merge;

/// A dialog over the page.
///
/// A genuinely detachable thing, so it is a card (§5): the large radius and a hairline, on
/// the page surface, over a scrim that dims what it interrupts. No blur, no shadow, no fade —
/// it is there or it is not.
///
/// Three ways out, because a reader who cannot find the way out of a dialog is stuck in
/// your app: the close button, the scrim, and `Escape`. The listener lives only while the
/// dialog is open and is torn down with it.
///
/// ```ignore
/// let open = RwSignal::new(false);
/// view! { <Modal open=open title="Questions"><Faq items=QNA/></Modal> }
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
                <div class="fixed inset-0 bg-scrim" on:click=move |_| open.set(false)></div>
                <div class=merge(
                    &format!(
                        "relative flex max-h-[80vh] w-full flex-col overflow-hidden rounded-lg \
                         border border-line bg-bg text-ink {width}"
                    ),
                    class.clone(),
                )>
                    <header class="flex min-h-10 shrink-0 items-center justify-between gap-2 \
                                   border-b border-line px-4 py-2">
                        <span class="truncate text-section font-semibold text-ink">
                            {title.clone()}
                        </span>
                        <button
                            class="grid size-7 shrink-0 cursor-pointer place-items-center \
                                   rounded-md text-ink-3 hover:bg-hover hover:text-ink \
                                   focus-visible:outline-[1.5px] \
                                   focus-visible:outline-offset-[-2px] \
                                   focus-visible:outline-focus"
                            type="button"
                            aria-label="Close"
                            on:click=move |_| open.set(false)
                        >
                            <Icon icon=Lucide::X/>
                        </button>
                    </header>
                    <div class="min-h-0 flex-1 overflow-y-auto p-4">{children()}</div>
                </div>
            </div>
        </Show>
    }
}
