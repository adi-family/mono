//! The open turn: [`TurnBlocks`], what has been emitted into it, and [`StopLine`], how the
//! last one ended.
//!
//! # Why a turn is a list and not a message
//!
//! A model does not answer, then call a tool, then answer again. It emits **one turn** made
//! of blocks — some prose, some calls, in whatever order it wrote them — and the turn is over
//! only when it stops emitting. Everything downstream depends on that shape: whether the loop
//! runs again is decided by whether the turn contained a call, and the results of *every* call
//! in it come back together, before the next turn starts.
//!
//! So the staging area is a list, and the person taking the model's seat stacks blocks into it.
//! It is not a transcript ([`Chat`](crate::Chat) is), and nothing in it has happened yet: a
//! block can still be dropped, because the model that has not emitted its turn has not
//! committed to anything either.

use leptos::prelude::*;

use crate::icon::{Icon, IconSize, Lucide};
use crate::{Empty, chat::Invoke, merge};

/// One thing emitted into the open turn.
///
/// Two kinds, because a model emits two kinds. Nothing else belongs here — a result is not a
/// block the model emits, it is what comes back after the turn is closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// Prose. Shown verbatim rather than rendered, because in the staging area what matters
    /// is the exact characters going out, not how they will look once something renders them.
    Text(String),
    /// A call, with its arguments in the order they were written.
    Call {
        name: String,
        params: Vec<(String, String)>,
    },
}

impl Block {
    /// A prose block.
    #[must_use]
    pub fn text(body: impl Into<String>) -> Self {
        Self::Text(body.into())
    }

    /// A call block.
    #[must_use]
    pub fn call(name: impl Into<String>, params: Vec<(String, String)>) -> Self {
        Self::Call {
            name: name.into(),
            params,
        }
    }

    /// Whether this block is a call — which is the whole question the stop reason turns on.
    #[must_use]
    pub fn is_call(&self) -> bool {
        matches!(self, Self::Call { .. })
    }
}

/// What has been staged into the turn that is still open.
///
/// Each row can be dropped. Ordering is the order they were added and cannot be changed here:
/// a model writes its turn front to back and does not go and reorder it, so an affordance to
/// do that would be one the seat being simulated does not have.
///
/// ```ignore
/// <TurnBlocks
///     blocks=Signal::derive(move || staged.get())
///     on_drop=Callback::new(move |i| staged.update(|b| { b.remove(i); }))
/// />
/// ```
#[component]
pub fn TurnBlocks(
    /// The staged blocks, in the order they were emitted.
    #[prop(into)]
    blocks: Signal<Vec<Block>>,
    /// Take a block back out. With no handler the × is not drawn — the honest rendering of a
    /// turn already on its way out.
    #[prop(optional, into)]
    on_drop: Option<Callback<usize>>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let rows = move || {
        let blocks = blocks.get();
        if blocks.is_empty() {
            // Not a failure and not an error — a turn nobody has written into yet. It says
            // what to do rather than that there is nothing, because there is nothing *yet*.
            return view! {
                <Empty>"Nothing emitted yet. Say something, call a tool, or both."</Empty>
            }
            .into_any();
        }
        blocks
            .into_iter()
            .enumerate()
            .map(|(i, block)| view! { <Staged i=i block=block on_drop=on_drop/> })
            .collect::<Vec<_>>()
            .into_any()
    };

    view! { <div class=merge("flex flex-col", class)>{rows}</div> }
}

/// One staged block: a row under a hairline — what it is, and a way to take it back.
#[component]
fn Staged(i: usize, block: Block, on_drop: Option<Callback<usize>>) -> impl IntoView {
    let kind = match &block {
        Block::Text(_) => "text",
        Block::Call { .. } => "call",
    };
    let name = match &block {
        Block::Text(_) => String::new(),
        Block::Call { name, .. } => name.clone(),
    };

    view! {
        <div class="border-t border-line py-3 first:border-t-0 first:pt-0">
            <div class="mb-2 flex items-center gap-2">
                <span class="w-4 shrink-0 text-right text-label text-ink-3 tabular-nums">
                    {i + 1}
                </span>
                <span class="rounded-full bg-chip px-2 py-0.5 text-label text-ink-2">{kind}</span>
                {(!name.is_empty()).then(|| view! {
                    <span class="truncate font-mono text-mono text-code">{name}</span>
                })}
                {on_drop.map(|cb| view! {
                    <button
                        class="ml-auto grid size-6 shrink-0 cursor-pointer place-items-center \
                               rounded-md text-ink-3 hover:bg-hover hover:text-ink"
                        type="button"
                        title="drop this block"
                        on:click=move |_| cb.run(i)
                    >
                        <Icon icon=Lucide::X size=IconSize::Sm label="Drop this block"/>
                    </button>
                })}
            </div>
            {match block {
                Block::Text(body) => view! {
                    <div class="font-mono text-mono leading-[1.6] whitespace-pre-wrap \
                                [word-break:break-word] text-code">
                        {body}
                    </div>
                }
                .into_any(),
                // The same block a real call is drawn as, from the same component — see
                // [`Invoke`](crate::chat::Invoke).
                Block::Call { name, params } => view! {
                    <Invoke name=name params=params/>
                }
                .into_any(),
            }}
        </div>
    }
}

/// How a turn ended, which is the same thing as what happens next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stop {
    /// The turn contained calls. They run, their results are appended, and the model is asked
    /// again — the loop goes round without anyone being consulted.
    #[default]
    ToolUse,
    /// The turn was words only. The run yields, and the next thing it hears is a person.
    EndTurn,
}

impl Stop {
    /// The value as Anthropic spells it, which is the one the runner actually reads.
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Self::ToolUse => "tool_use",
            Self::EndTurn => "end_turn",
        }
    }

    /// The value as OpenAI spells it. Shown beside the other because the same event has two
    /// names in the wild, and somebody reading a provider's logs next to this screen needs to
    /// know they are looking at one thing.
    #[must_use]
    pub fn openai(self) -> &'static str {
        match self {
            Self::ToolUse => "tool_calls",
            Self::EndTurn => "stop",
        }
    }

    /// What it means for the run, in words.
    #[must_use]
    pub fn says(self) -> &'static str {
        match self {
            Self::ToolUse => "results append and the loop runs again",
            Self::EndTurn => "the run yields — the next turn is yours, as yourself",
        }
    }

    /// Which way a turn holding these blocks would end.
    ///
    /// One call anywhere in the turn is enough: the loop's question is whether there is
    /// anything to answer, not how much of the turn was prose.
    #[must_use]
    pub fn of(blocks: &[Block]) -> Self {
        if blocks.iter().any(Block::is_call) {
            Self::ToolUse
        } else {
            Self::EndTurn
        }
    }
}

/// How the last turn ended, drawn as a rule across the conversation.
///
/// **Deliberately not mono prose, and deliberately not in a box.** A stop reason is response
/// metadata — the model never sees it, it is never in anybody's prompt, and no token was spent
/// on it. Everything in this screen that is set in mono on a raised surface is text the model
/// was actually handed, and the moment this joined that set a reader would start believing the
/// model was told how its own turn ended. So it is a hairline and a chip in the margin between
/// documents, which is what it is.
///
/// ```ignore
/// <StopLine stop=Stop::ToolUse/>
/// ```
#[component]
pub fn StopLine(
    /// How it ended.
    stop: Stop,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    view! {
        <div
            class=merge("flex items-center gap-2 py-1 text-label text-ink-3", class)
            role="separator"
            aria-label=format!("stop reason {}", stop.wire())
        >
            <span class="h-px w-4 shrink-0 bg-line" aria-hidden="true"></span>
            <span class="shrink-0">"stop reason"</span>
            <span class="shrink-0 rounded-full bg-chip px-2 py-0.5 font-mono text-code">
                {stop.wire()}
            </span>
            <span class="hidden shrink-0 font-mono sm:inline">
                {format!("openai: {}", stop.openai())}
            </span>
            <span class="truncate">{stop.says()}</span>
            <span class="h-px flex-1 bg-line" aria-hidden="true"></span>
        </div>
    }
}
