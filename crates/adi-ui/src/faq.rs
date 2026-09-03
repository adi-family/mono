//! [`Faq`] — questions with the answers folded up under them.

use leptos::prelude::*;

use crate::icon::{Icon, IconSize, Lucide};
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

/// A list of questions, each folded up until it is asked for, separated by hairlines.
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
        <div class=merge("flex flex-col", class)>
            {move || items
                .get()
                .into_iter()
                .map(|qna| view! {
                    // `group` so the chevron can turn on the parent's own `open` state —
                    // no signal, no handler, and it stays right if the browser opens the
                    // details itself (find-in-page does).
                    <details class="group border-b border-line last:border-b-0">
                        <summary class="flex cursor-pointer list-none items-center gap-2 py-2.5 \
                                        text-row font-medium text-ink select-none \
                                        focus-visible:outline-[1.5px] \
                                        focus-visible:outline-offset-[-2px] \
                                        focus-visible:outline-focus \
                                        [&::-webkit-details-marker]:hidden">
                            <Icon
                                icon=Lucide::ChevronRight
                                size=IconSize::Sm
                                class="text-ink-3 transition-transform duration-100 \
                                       group-open:rotate-90"
                            />
                            <span class="min-w-0">{qna.question}</span>
                        </summary>
                        <div class="pb-3 pl-6">
                            <Markdown source=qna.answer/>
                        </div>
                    </details>
                })
                .collect::<Vec<_>>()}
        </div>
    }
}
