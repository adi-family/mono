//! [`Faq`] — questions with the answers folded up under them.

use leptos::prelude::*;

use crate::{Markdown, merge};

/// One question and its answer.
///
/// The answer is Markdown, so it can hold a command, a path or a link without the caller
/// building a view for it — which is what makes a long FAQ a list of strings rather than a
/// page of markup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Qna {
    pub question: String,
    pub answer: String,
}

impl Qna {
    #[must_use]
    pub fn new(question: impl Into<String>, answer: impl Into<String>) -> Self {
        Self {
            question: question.into(),
            answer: answer.into(),
        }
    }
}

/// A list of questions, each folded up until it is asked for.
///
/// Built on `<details>`, which is the browser's own disclosure: it opens on click *and* on
/// Enter, it is findable by the page's own find-in-page in browsers that search closed
/// details, and it keeps working before wasm has loaded. Nothing here is a click handler.
///
/// Closed by default, all of them. A FAQ where the first answer is already open reads as an
/// article with a strange title, and the point of the list is that you can see every
/// question at once.
///
/// ```ignore
/// <Faq items=vec![Qna::new("Where does it live?", "Under `~/.adi`.")]/>
/// ```
#[component]
pub fn Faq(
    #[prop(into)] items: Signal<Vec<Qna>>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    view! {
        <div class=merge("flex flex-col gap-2", class)>
            {move || items
                .get()
                .into_iter()
                .map(|qna| view! {
                    // `group` so the chevron can turn on the parent's own `open` state —
                    // no signal, no handler, and it stays right if the browser opens the
                    // details itself (find-in-page does).
                    <details class="group island overflow-hidden bg-card">
                        <summary class="flex cursor-pointer list-none items-center gap-2 px-3 \
                                        py-2 text-row font-medium text-ink select-none \
                                        hover:bg-bubble \
                                        focus-visible:outline-2 \
                                        focus-visible:outline-offset-[-2px] \
                                        focus-visible:outline-accent \
                                        [&::-webkit-details-marker]:hidden">
                            <svg
                                class="size-3 shrink-0 text-meta transition-transform \
                                       duration-100 group-open:rotate-90"
                                viewBox="0 0 12 12"
                                fill="none"
                                stroke="currentColor"
                                stroke-width="1.6"
                                stroke-linecap="round"
                                stroke-linejoin="round"
                                aria-hidden="true"
                            >
                                <path d="M4.5 2.5 8 6l-3.5 3.5"></path>
                            </svg>
                            <span class="min-w-0">{qna.question}</span>
                        </summary>
                        <div class="border-t border-divider px-3 py-2.5">
                            <Markdown source=qna.answer/>
                        </div>
                    </details>
                })
                .collect::<Vec<_>>()}
        </div>
    }
}
