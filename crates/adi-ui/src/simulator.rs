//! [`Simulator`] — the screen where a person takes the model's seat in a real agent run.
//!
//! # What it arranges
//!
//! Two columns, and the split is the idea. On the left, **what the model sees**: the composed
//! prompt, as tokens or as text, with the whole conversation already in it — there is no
//! separate transcript, because to a model there is no separate transcript. On the right,
//! **what the model does**: the blocks staged into the open turn, the two ways to add one, and
//! the control that closes it.
//!
//! # It holds no fetch logic, and that is load-bearing
//!
//! Every value comes in as a signal and every action goes out as a callback. Not for purity —
//! because the one rule the feature rests on is that the simulator never grows its own copy of
//! anything a real run does. The prompt is composed by the runner's own composer, the tools are
//! the runner's own declarations, and a call is executed by the runner's own `execute`. A
//! component that fetched, or that built a call's JSON, would be the second copy, and a second
//! copy is a screen that shows something the agent does not actually see.
//!
//! The one thing it *does* own is view state — which column tab is up, which tool is selected,
//! whether the prompt is read as tokens or as text. None of that is data anybody else has an
//! opinion about.

use leptos::prelude::*;

use crate::{
    Badge, BadgeTone, Block, Button, ButtonSize, ButtonVariant, Composer, Empty, Flag, FlagList,
    FlagMark, Panel, Param, PromptText, Select, Stop, StopLine, Textarea, Token, TokenStream,
    ToolForm, TurnBlocks, merge,
};

/// A tool as this screen offers it: what it is called, what it says it does, and the fields a
/// call to it is written into.
///
/// `params` are [`Param`]s the **caller** built from the tool's declared schema and whose
/// signals the caller owns. That is what keeps this component out of the business of building
/// a call: when a call is added it reports the tool's name, and whoever owns the params reads
/// them and composes the wire form.
#[derive(Debug, Clone)]
pub struct ToolDecl {
    /// The name the model writes in the call.
    pub name: String,
    /// The description the model is given, verbatim.
    pub description: String,
    /// The parameters, in the order the tool declares them.
    pub params: Vec<Param>,
}

impl ToolDecl {
    #[must_use]
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self { name: name.into(), description: description.into(), params: Vec::new() }
    }

    /// The tool's parameters, in declaration order.
    #[must_use]
    pub fn params(mut self, params: Vec<Param>) -> Self {
        self.params = params;
        self
    }
}

/// Which composer is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    /// Prose — what the model says.
    #[default]
    Prose,
    /// A call to one of the agent's tools.
    Call,
}

/// The simulator.
///
/// ```ignore
/// <Simulator
///     prompt=Signal::derive(move || tokens.get())
///     blocks=Signal::derive(move || staged.get())
///     tools=Signal::derive(move || tools.get())
///     on_text=stage_text on_call=stage_call on_drop_block=drop_block
///     on_end_turn=end_turn on_user=user_turn
///     flags=Signal::derive(move || flags.get()) on_flag=take_flag
/// />
/// ```
#[component]
pub fn Simulator(
    /// The composed prompt, tokenized server-side — instructions, tool declarations and the
    /// conversation so far, in one stream, because that is how it arrives.
    #[prop(into)]
    prompt: Signal<Vec<Token>>,
    /// What has been staged into the turn that is still open.
    #[prop(into)]
    blocks: Signal<Vec<Block>>,
    /// The agent's tools, with their fields.
    #[prop(into)]
    tools: Signal<Vec<ToolDecl>>,
    /// Stage a prose block. Called with the text; clearing the box is the caller's.
    #[prop(into)]
    on_text: Callback<String>,
    /// Stage a call, named. The caller reads the [`Param`]s it owns to build it.
    #[prop(into)]
    on_call: Callback<String>,
    /// Drop a staged block by index.
    #[prop(optional, into)]
    on_drop_block: Option<Callback<usize>>,
    /// Close the turn: execute every staged call in order, append the results, and ask again.
    #[prop(into)]
    on_end_turn: Callback<()>,
    /// Answer as yourself, once the run has yielded.
    ///
    /// Unlike everywhere else in this crate, the draft is owned **here** and cleared on send.
    /// [`Composer`] leaves clearing to its caller so a failed send keeps its text, and that is
    /// right for a box posting to a network; this one appends a turn to a conversation the
    /// caller already has in hand, and a message left sitting in the box after it has also
    /// appeared in the prompt above reads as a send that did not happen.
    #[prop(into)]
    on_user: Callback<String>,
    /// Flags taken while reading.
    #[prop(optional, into)]
    flags: Signal<Vec<Flag>>,
    /// A passage was flagged.
    #[prop(optional, into)]
    on_flag: Option<Callback<String>>,
    /// Unflag one.
    #[prop(optional, into)]
    on_unflag: Option<Callback<usize>>,
    /// How the last turn ended. `None` before the first one has.
    #[prop(optional, into)]
    stop: Signal<Option<Stop>>,
    /// A turn is executing. Everything that would emit another one goes out while it is.
    #[prop(optional, into)]
    busy: Signal<bool>,
    /// The encoding the token count is in. Shown, because a token number without its encoding
    /// is a number nobody can check.
    #[prop(default = "o200k_base".into(), into)]
    encoding: Signal<String>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let tab = RwSignal::new(Tab::default());
    // Tokens are the honest view and text is the readable one; text is the default, because
    // the reason to be on this screen is to read the prompt, and the per-token colour is
    // there for the moment you stop believing what you are reading.
    let as_tokens = RwSignal::new(false);
    let prose = RwSignal::new(String::new());
    let reply = RwSignal::new(String::new());
    let picked = RwSignal::new(String::new());

    // The tool whose form is up: whatever is picked, else the first one the agent has. Falling
    // back rather than showing nothing means the tab is usable the moment it is opened.
    let current = Signal::derive(move || {
        let tools = tools.get();
        let picked = picked.get();
        tools.iter().find(|t| t.name == picked).or_else(|| tools.first()).cloned()
    });
    // Name the fallback out loud, once the tools have arrived. Without this the `<select>` sits
    // on a value no option has and renders blank, while the form under it is already the first
    // tool's — a control disagreeing with the form it controls. An effect rather than an
    // initial value because the tools are fetched, so at mount there are none.
    Effect::new(move |_| {
        if picked.get_untracked().is_empty()
            && let Some(first) = tools.get().first()
        {
            picked.set(first.name.clone());
        }
    });

    // What ending the turn right now would do. Derived from the staged blocks rather than
    // stored, because it is not a decision anybody makes — it is what having called a tool
    // means.
    let outcome = Signal::derive(move || Stop::of(&blocks.get()));
    // The run has yielded: the model's seat is empty until a person says something as
    // themselves.
    let yielded = Signal::derive(move || stop.get() == Some(Stop::EndTurn));

    let add_text = move || {
        let text = prose.get_untracked();
        if text.trim().is_empty() || busy.get_untracked() {
            return;
        }
        on_text.run(text);
        prose.set(String::new());
    };

    view! {
        <div class=merge(
            "grid grid-cols-1 items-start gap-4 lg:grid-cols-[minmax(0,1fr)_minmax(360px,26rem)]",
            class,
        )>
            <div class="flex min-w-0 flex-col gap-4">
                <Panel
                    title="what the model sees"
                    flush=true
                    actions=move || view! {
                        <span class="font-mono text-caps text-fainter">
                            {move || format!("{} tok · {}", prompt.get().len(), encoding.get())}
                        </span>
                        <Button
                            size=ButtonSize::Small
                            variant=if as_tokens.get() {
                                ButtonVariant::Default
                            } else {
                                ButtonVariant::Ghost
                            }
                            on:click=move |_| as_tokens.update(|t| *t = !*t)
                        >
                            "tokens"
                        </Button>
                    }
                >
                    <div class="px-4 pt-3 text-mini text-meta">
                        "Everything below is one document to the model: its instructions, the \
                         tools it was declared, and every turn so far. Select any of it to flag \
                         it."
                    </div>
                    <FlagMark
                        on_flag=on_flag.unwrap_or_else(|| Callback::new(|_: String| ()))
                        class="p-4"
                    >
                        {move || if as_tokens.get() {
                            view! {
                                <TokenStream
                                    tokens=prompt
                                    class="max-h-[32rem] overflow-auto rounded-sm border \
                                           border-edge bg-panel-alt p-3"
                                />
                            }
                            .into_any()
                        } else {
                            view! {
                                <PromptText
                                    tokens=prompt
                                    class="max-h-[32rem] overflow-auto rounded-sm border \
                                           border-edge bg-panel-alt p-3"
                                />
                            }
                            .into_any()
                        }}
                    </FlagMark>
                </Panel>

                <Panel title="flagged" flush=true>
                    // Two branches rather than forwarding the `Option`: the × is drawn only
                    // where a handler exists, and a prop that takes one cannot be handed a
                    // maybe.
                    {match on_unflag {
                        Some(cb) => view! { <FlagList flags=flags on_drop=cb class="p-4"/> }
                            .into_any(),
                        None => view! { <FlagList flags=flags class="p-4"/> }.into_any(),
                    }}
                </Panel>
            </div>

            <div class="flex min-w-0 flex-col gap-4">
                {move || stop.get().map(|stop| view! { <StopLine stop=stop/> })}

                <Panel
                    title="this turn"
                    flush=true
                    actions=move || view! {
                        <Badge tone=BadgeTone::Neutral mono=true>
                            {move || {
                                let n = blocks.get().len();
                                if n == 1 { "1 block".to_string() } else { format!("{n} blocks") }
                            }}
                        </Badge>
                    }
                >
                    {match on_drop_block {
                        Some(cb) => view! {
                            <TurnBlocks blocks=blocks on_drop=cb class="p-4"/>
                        }
                        .into_any(),
                        None => view! { <TurnBlocks blocks=blocks class="p-4"/> }.into_any(),
                    }}

                    <div class="flex items-center gap-1 border-t border-divider px-4 pt-3">
                        <TabButton tab=tab mine=Tab::Prose>"say something"</TabButton>
                        <TabButton tab=tab mine=Tab::Call>"call a tool"</TabButton>
                    </div>

                    <div class="p-4 pt-3">
                        <Show
                            when=move || tab.get() == Tab::Prose
                            fallback=move || view! {
                                <CallTab
                                    tools=tools
                                    picked=picked
                                    current=current
                                    busy=busy
                                    on_call=on_call
                                />
                            }
                        >
                            <div class="flex flex-col gap-2">
                                <Textarea
                                    value=prose
                                    rows=4
                                    prose=true
                                    disabled=busy
                                    placeholder="What the model says this turn…"
                                />
                                <Button
                                    size=ButtonSize::Small
                                    class="self-end"
                                    disabled=Signal::derive(move || {
                                        prose.get().trim().is_empty() || busy.get()
                                    })
                                    on:click=move |_| add_text()
                                >
                                    "add block"
                                </Button>
                            </div>
                        </Show>
                    </div>

                    <div class="flex items-center gap-3 border-t border-divider bg-panel-alt \
                                px-4 py-3">
                        <span class="min-w-0 flex-1 text-mini text-meta">
                            {move || {
                                let outcome = outcome.get();
                                format!("ends as {} — {}", outcome.wire(), outcome.says())
                            }}
                        </span>
                        <Button
                            variant=ButtonVariant::Primary
                            size=ButtonSize::Small
                            disabled=Signal::derive(move || busy.get() || blocks.get().is_empty())
                            on:click=move |_| on_end_turn.run(())
                        >
                            "end turn"
                        </Button>
                    </div>
                </Panel>

                <Panel title="as yourself" flush=true>
                    <div class="flex flex-col gap-2 p-4">
                        <p class="m-0 text-mini text-meta">
                            {move || if yielded.get() {
                                "The run yielded. Answer as the person the agent is working for."
                            } else {
                                "Not your turn — the model's. End a turn without a call and the \
                                 run yields to you."
                            }}
                        </p>
                        <Composer
                            value=reply
                            busy=Signal::derive(move || busy.get() || !yielded.get())
                            placeholder="Reply as the user…"
                            on_send=Callback::new(move |text: String| {
                                on_user.run(text);
                                reply.set(String::new());
                            })
                        />
                    </div>
                </Panel>
            </div>
        </div>
    }
}

/// One of the two composer tabs.
#[component]
fn TabButton(tab: RwSignal<Tab>, mine: Tab, children: Children) -> impl IntoView {
    // Both halves spelled out per branch — Tailwind reads this file as text and never runs it.
    let look = move || {
        if tab.get() == mine {
            "caps cursor-pointer rounded-sm border border-accent-soft-edge bg-accent-soft \
             px-2 py-1 text-accent"
        } else {
            "caps cursor-pointer rounded-sm border border-transparent px-2 py-1 text-faint \
             hover:text-secondary"
        }
    };
    view! {
        <button
            class=look
            type="button"
            aria-pressed=move || (tab.get() == mine).to_string()
            on:click=move |_| tab.set(mine)
        >
            {children()}
        </button>
    }
}

/// The tool-call tab: pick a tool, fill its declared fields, stage the call.
#[component]
fn CallTab(
    tools: Signal<Vec<ToolDecl>>,
    picked: RwSignal<String>,
    current: Signal<Option<ToolDecl>>,
    busy: Signal<bool>,
    on_call: Callback<String>,
) -> impl IntoView {
    view! {
        <Show
            when=move || !tools.get().is_empty()
            fallback=|| view! { <Empty>"This agent was declared no tools."</Empty> }
        >
            <div class="flex flex-col gap-3">
                <Select value=picked disabled=busy class="w-full">
                    {move || tools
                        .get()
                        .into_iter()
                        .map(|t| view! { <option value=t.name.clone()>{t.name.clone()}</option> })
                        .collect::<Vec<_>>()}
                </Select>
                {move || current.get().map(|tool| {
                    let name = tool.name.clone();
                    view! {
                        <p class="m-0 text-mini leading-relaxed text-meta">{tool.description}</p>
                        <ToolForm params=tool.params/>
                        <Button
                            size=ButtonSize::Small
                            class="self-end"
                            disabled=busy
                            on:click=move |_| on_call.run(name.clone())
                        >
                            "add call"
                        </Button>
                    }
                })}
            </div>
        </Show>
    }
}
