//! [`Ask`] — the block a run puts up when it needs a person to decide something.
//!
//! # Why it is a block and not a message
//!
//! A run that stops to ask could simply say so in prose, and before this it did. The trouble is
//! that prose is not a *state*: the transcript reads the same whether the question was answered
//! an hour ago or is holding the work up right now, and there is nothing for a rail, an inbox or a
//! notification to point at. The block is the visible half of a stored question — while it is on
//! screen the conversation is blocked, and when it goes the answer is a turn like any other.
//!
//! It is drawn the way `design/DESIGN.md` §6 draws an ask: a 2px rule down its left edge and
//! nothing else around it. It is part of the transcript, not a card floating over it.
//!
//! # One tap where one tap will do
//!
//! An answer somebody taps is an answer you get; one they have to compose is one you wait for. So
//! the options are buttons, and the single commonest shape — one question, one choice — sends on
//! the click rather than making somebody confirm what they just said. Every question still keeps a
//! free-text box under its buttons, because the right answer is regularly "neither, do this
//! instead", and a block that cannot say that is a block people route around.
//!
//! # It never traps you
//!
//! Send lights up as soon as *any* question has an answer, not when all of them do. A person who
//! knows two of three answers should be able to say so — the run hears "(no answer)" for the third
//! and can ask again if it really cannot proceed. The alternative is a block that holds the whole
//! conversation hostage to the one question nobody can settle.

use std::collections::BTreeSet;

use leptos::prelude::*;

use crate::{Button, ButtonSize, Markdown, merge};

/// One thing the run wants decided.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AskQuestion {
    /// Two or three words naming the decision, shown as a tag. Optional.
    pub header: String,
    pub question: String,
    /// The answers offered as buttons. Empty means free text only.
    pub options: Vec<AskOption>,
    /// Whether more than one option may be chosen.
    pub multi_select: bool,
}

/// One offered answer: what the button says, and what choosing it implies.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AskOption {
    pub label: String,
    /// The trade-off, under the button. This is what turns picking a word into making a decision,
    /// so it is worth writing even when the label seems self-evident.
    pub description: String,
}

/// What one question has been answered with so far.
#[derive(Clone, Copy)]
struct Picked {
    /// Which options are chosen, by index. A set even for a single-select question, so the two
    /// cases differ only in what a click does rather than in what is stored.
    chosen: RwSignal<BTreeSet<usize>>,
    /// Anything typed under the buttons.
    typed: RwSignal<String>,
}

impl Picked {
    fn new() -> Self {
        Self {
            chosen: RwSignal::new(BTreeSet::new()),
            typed: RwSignal::new(String::new()),
        }
    }

    /// The one reply this question sends.
    ///
    /// Chosen labels first, then anything typed, joined by an em dash — so "Postgres — but read
    /// analytics off the replica" arrives as one sentence rather than as a choice with a footnote
    /// the model has to work out how to attach.
    fn reply(&self, question: &AskQuestion) -> String {
        let chosen = self.chosen.get();
        let labels: Vec<&str> = question
            .options
            .iter()
            .enumerate()
            .filter(|(index, _)| chosen.contains(index))
            .map(|(_, option)| option.label.as_str())
            .collect();
        let typed = self.typed.get();
        let typed = typed.trim();
        match (labels.is_empty(), typed.is_empty()) {
            (true, true) => String::new(),
            (true, false) => typed.to_string(),
            (false, true) => labels.join(", "),
            (false, false) => format!("{} — {typed}", labels.join(", ")),
        }
    }
}

/// The question block.
///
/// ```ignore
/// <Ask
///     note=ask.note
///     questions=questions
///     deadline_note="taking its own default in 4m".to_string()
///     on_answer=Callback::new(move |replies: Vec<String>| answer(replies))
/// />
/// ```
#[component]
pub fn Ask(
    /// What the run was doing when it stopped to ask. Markdown, like anything else it says.
    #[prop(optional, into)]
    note: String,
    questions: Vec<AskQuestion>,
    /// When the run will give up and take its own assumption, in words — the caller owns the
    /// clock, so this is a sentence rather than a timestamp. Empty when it waits indefinitely.
    #[prop(optional, into)]
    deadline_note: Signal<String>,
    /// One reply per question, in the order they were asked.
    on_answer: Callback<Vec<String>>,
    /// True while an answer is in flight, so the block cannot be sent twice.
    #[prop(optional, into)]
    busy: Signal<bool>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let picks: Vec<Picked> = questions.iter().map(|_| Picked::new()).collect();
    // One question with a fixed set of answers is the shape a click can settle outright. Anything
    // else needs the whole block read before it can be sent, so the button is the only way out.
    let one_tap =
        questions.len() == 1 && !questions[0].options.is_empty() && !questions[0].multi_select;

    let send = {
        let questions = questions.clone();
        let picks = picks.clone();
        move || {
            let replies: Vec<String> = questions
                .iter()
                .zip(&picks)
                .map(|(question, pick)| pick.reply(question))
                .collect();
            if replies.iter().all(|r| r.trim().is_empty()) {
                return;
            }
            on_answer.run(replies);
        }
    };
    let answered = {
        let questions = questions.clone();
        let picks = picks.clone();
        Signal::derive(move || {
            questions
                .iter()
                .zip(&picks)
                .any(|(question, pick)| !pick.reply(question).trim().is_empty())
        })
    };

    let rows: Vec<_> = questions
        .iter()
        .cloned()
        .zip(picks.clone())
        .enumerate()
        .map(|(index, (question, pick))| {
            let tap = one_tap.then(|| {
                let send = send.clone();
                Callback::new(move |()| send())
            });
            question_row(index, questions.len(), question, pick, tap, busy)
        })
        .collect();

    let send_click = send.clone();
    view! {
        <div class=merge("border-l-2 border-line-strong py-1 pl-[18px]", class)>
            <div class="mb-2 flex items-center gap-3 text-label text-ink-3">
                "Waiting on you"
                <span class="ml-auto">{move || deadline_note.get()}</span>
            </div>
            {(!note.trim().is_empty()).then(|| view! {
                <Markdown source=note class="mb-3 text-small text-ink-2"/>
            })}
            <div class="flex flex-col gap-4">{rows}</div>
            // One question answered by a click needs no button — it has already gone. Everything
            // else gets one, and it stays out until there is something to send.
            {(!one_tap).then(|| view! {
                <div class="mt-3 flex items-center gap-3">
                    <Button
                        size=ButtonSize::Medium
                        disabled=Signal::derive(move || busy.get() || !answered.get())
                        on:click=move |_| send_click()
                    >
                        "Answer"
                    </Button>
                    <span class="text-small text-ink-3">
                        {move || if answered.get() {
                            "a question left blank is answered “(no answer)”"
                        } else {
                            "pick an option or write an answer"
                        }}
                    </span>
                </div>
            })}
        </div>
    }
}

/// One question: its tag, its text, its buttons, and the box under them.
fn question_row(
    index: usize,
    total: usize,
    question: AskQuestion,
    pick: Picked,
    one_tap: Option<Callback<()>>,
    busy: Signal<bool>,
) -> impl IntoView {
    let multi = question.multi_select;
    let header = question.header.clone();
    let text = question.question.clone();
    let buttons: Vec<_> = question
        .options
        .iter()
        .cloned()
        .enumerate()
        .map(|(option_index, option)| {
            let tap = one_tap;
            let chosen = Signal::derive(move || pick.chosen.get().contains(&option_index));
            view! {
                // The chosen option is the answer chip (§6): the active surface, never orange.
                <button
                    type="button"
                    class=move || if chosen.get() {
                        "cursor-pointer rounded-md bg-active px-3 py-1.5 text-left text-ui \
                         font-medium text-ink"
                    } else {
                        "cursor-pointer rounded-md bg-btn px-3 py-1.5 text-left text-ui \
                         text-ink hover:bg-btn-hover"
                    }
                    prop:disabled=move || busy.get()
                    on:click=move |_| {
                        pick.chosen.update(|set| {
                            if multi {
                                // A second click on a chosen option takes it back — with several
                                // allowed, there is no other way to undo a misclick.
                                if !set.remove(&option_index) {
                                    set.insert(option_index);
                                }
                            } else {
                                set.clear();
                                set.insert(option_index);
                            }
                        });
                        // Only the one-question, one-choice block sends on the click. The update
                        // above has already landed, so the reply it reads is this option.
                        if let Some(tap) = tap {
                            tap.run(());
                        }
                    }
                >
                    <span class="block">{option.label.clone()}</span>
                    {(!option.description.trim().is_empty()).then(|| view! {
                        <span class="mt-0.5 block text-mini font-normal text-ink-3">
                            {option.description.clone()}
                        </span>
                    })}
                </button>
            }
        })
        .collect();

    view! {
        <div class="flex flex-col gap-2">
            <div class="flex flex-wrap items-baseline gap-2">
                // Numbered only when there is more than one: a lone "1." in front of a single
                // question is a list that isn't there.
                {(total > 1).then(|| view! {
                    <span class="text-label text-ink-3">{format!("{}.", index + 1)}</span>
                })}
                {(!header.trim().is_empty()).then(|| view! {
                    <span class="rounded-full bg-chip px-2 py-0.5 text-label text-ink-2">
                        {header.clone()}
                    </span>
                })}
                <span class="text-[15px] text-ink">{text.clone()}</span>
                {multi.then(|| view! {
                    <span class="text-mini text-ink-3">"(choose any)"</span>
                })}
            </div>
            {(!buttons.is_empty()).then(|| view! {
                <div class="flex flex-wrap gap-2">{buttons}</div>
            })}
            <input
                type="text"
                class="w-full rounded-md border border-line-strong bg-raise px-3 py-2 text-ui \
                       text-ink placeholder:text-ink-3 focus-visible:border-ink-3 \
                       focus-visible:outline-none"
                placeholder=if question.options.is_empty() {
                    "your answer"
                } else {
                    "…or say something else"
                }
                prop:value=move || pick.typed.get()
                prop:disabled=move || busy.get()
                on:input=move |ev| pick.typed.set(event_target_value(&ev))
            />
        </div>
    }
}
