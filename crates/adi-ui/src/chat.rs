//! The chat — a transcript that reads **newest first**, and stays cheap however long it
//! gets.
//!
//! # Newest first
//!
//! Every chat you have used puts the newest message at the bottom and scrolls you there.
//! This one puts it at the top. The reason is what these transcripts *are*: an agent's run
//! is long, mostly tool calls, and you come back to it to find out what just happened —
//! not to re-read it from the beginning. Newest-first means the answer is where the eye
//! already is, no scroll anchoring to fight, and no jump when a message lands while you are
//! reading.
//!
//! It also removes the single worst thing about a live transcript: keeping the view pinned
//! to the bottom while content of unknown height streams in. At the top there is nothing to
//! pin — new content pushes down, away from you, and the thing you were reading does not
//! move.
//!
//! # Cheap at any length
//!
//! Two properties do the work, and neither is a virtual list:
//!
//! - **`content-visibility: auto`** on every said turn. The browser skips layout, paint and
//!   style for anything off screen — virtualisation done by the engine, with no windowing,
//!   no scroll maths, and no rows that pop in wrong.
//! - **`contain-intrinsic-size`** beside it, so a skipped turn still reserves a plausible
//!   height. That is what keeps the scrollbar honest and stops the page shuddering as turns
//!   enter and leave.
//!
//! Every turn also carries `shrink-0`, and that one is not an optimisation — see [`LAZY`].
//!
//! Find-in-page, `Ctrl+F`, anchors and screen readers all still see everything, which is
//! what a JS virtual list takes away.

use leptos::prelude::*;

use crate::{Markdown, merge};

/// Who said it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Role {
    /// You.
    User,
    /// The agent.
    #[default]
    Agent,
}

/// How a tool call ended, or that it has not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolState {
    /// Still going. At most one call in a transcript is, and it is the one worth showing
    /// without being asked. **Only while something is alive to finish it** — a call left
    /// green in a run that ended weeks ago is the transcript lying about the present.
    Running,
    /// It returned.
    #[default]
    Ok,
    /// It did not.
    Failed,
    /// Nothing came back: the run ended while this call was in flight. Amber rather than
    /// red, because the tool never got to fail — it was never answered.
    Unanswered,
}

impl ToolState {
    /// The dot beside the call's name.
    #[must_use]
    pub fn dot_classes(self) -> &'static str {
        match self {
            Self::Running => "bg-accent",
            Self::Ok => "bg-faint",
            Self::Failed => "bg-err",
            Self::Unanswered => "bg-attention",
        }
    }
}

/// One tool call, as the model wrote it.
///
/// `params` is a list rather than a map on purpose: the order the model wrote them in is
/// the order they are shown, because that is what it actually emitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub state: ToolState,
    /// What came back, if anything has. Shown as the result block the model is handed next.
    pub result: Option<String>,
}

impl ToolCall {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            params: Vec::new(),
            state: ToolState::default(),
            result: None,
        }
    }

    /// Add a parameter, in the order it was written.
    #[must_use]
    pub fn param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.push((key.into(), value.into()));
        self
    }

    #[must_use]
    pub fn state(mut self, state: ToolState) -> Self {
        self.state = state;
        self
    }

    #[must_use]
    pub fn result(mut self, result: impl Into<String>) -> Self {
        self.result = Some(result.into());
        self
    }

    /// A one-line preview: the call, flattened, with values cut short.
    ///
    /// This is what a folded run shows for the call that is still running — enough to know
    /// *what* is happening without opening anything.
    #[must_use]
    pub fn summary(&self) -> String {
        let args = self
            .params
            .iter()
            .map(|(k, v)| {
                let flat = v.split_whitespace().collect::<Vec<_>>().join(" ");
                let short = if flat.chars().count() > 42 {
                    format!("{}…", flat.chars().take(42).collect::<String>())
                } else {
                    flat
                };
                format!("{k}={short}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}({args})", self.name)
    }
}

/// One entry in a transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Turn {
    /// Something said, in Markdown.
    Said { role: Role, body: String },
    /// A run of tool calls with no words between them. **Text is the divider**: everything
    /// the agent did between one thing it said and the next folds into a single run, which
    /// is exactly the unit you want to open or ignore.
    Did(Vec<ToolCall>),
}

/// The transcript.
///
/// Hand it turns oldest-first — the order they happened, which is the order a store keeps
/// them — and it reverses them for the screen. Nothing else in the app has to think
/// backwards.
///
/// ```ignore
/// <Chat turns=Signal::derive(move || store.get())/>
/// ```
#[component]
pub fn Chat(
    /// Oldest first. The component flips it.
    #[prop(into)]
    turns: Signal<Vec<Turn>>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    view! {
        <div class=merge("flex flex-col gap-3 overflow-y-auto", class)>
            {move || {
                let mut turns = turns.get();
                turns.reverse();
                turns
                    .into_iter()
                    .map(|turn| match turn {
                        Turn::Said { role, body } => view! { <Said role=role body=body/> }
                            .into_any(),
                        Turn::Did(calls) => view! { <Did calls=calls/> }.into_any(),
                    })
                    .collect::<Vec<_>>()
            }}
        </div>
    }
}

/// What every top-level entry wears, and both halves of it are load-bearing.
///
/// **`shrink-0`** first, because this is a flex column with a height: without it a turn is a
/// flex item that can be squeezed, and one that opens — a tool run — gets squeezed. Measured
/// in the browser: an opened run came out 343px tall around 607px of content, with the
/// second half of it simply gone. `overflow-hidden` on the run then hid the evidence.
///
/// **`content-visibility: auto`** second, with an intrinsic height beside it so a skipped
/// turn still reserves room and the scrollbar does not lie. That pair is the whole
/// virtualisation story: the engine skips what is off screen, and find-in-page, anchors and
/// screen readers keep seeing everything — which is what a JS virtual list takes away.
const LAZY: &str = "shrink-0 [content-visibility:auto] [contain-intrinsic-size:auto_120px]";

/// One thing said.
#[component]
fn Said(role: Role, body: String) -> impl IntoView {
    let own = match role {
        // The user's own words are a bubble, because they are the short thing you scan for
        // to find where a stretch of work began.
        Role::User => "island ml-8 bg-bubble px-3 py-2",
        Role::Agent => "px-1",
    };
    view! {
        <div class=format!("{LAZY} {own}")>
            <Markdown source=body/>
        </div>
    }
}

/// A message said but not yet asked: your own bubble, hollowed out.
///
/// It is not a [`Turn`], and deliberately so — a queue is not transcript. The turns a [`Chat`]
/// holds are things that happened; this is a thing that has not, and it sits *outside* the
/// transcript (below it on screen, where the newest-first order puts what comes next). Keeping
/// it out of `Vec<Turn>` is what stops a queued message from being indistinguishable from a
/// sent one the moment anything reads the list.
///
/// Same geometry as the user bubble in [`Said`], then emptied: dashed edge, no fill, dimmed
/// text — intent rather than history.
///
/// ```ignore
/// <Queued body=text on_unqueue=Callback::new(move |()| drop_from_queue(place))/>
/// ```
#[component]
pub fn Queued(
    /// Markdown, the same as anything else said.
    #[prop(into)]
    body: String,
    /// Take it back before the agent ever sees it. With no handler the × is not drawn, which
    /// is the honest rendering of a queue you cannot edit.
    #[prop(optional, into)]
    on_unqueue: Option<Callback<()>>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    view! {
        <div class=merge(
            &format!("{LAZY} island ml-8 border-dashed bg-transparent px-3 py-2 text-meta"),
            class,
        )>
            <div class="caps mb-1 flex items-center gap-1 text-faint">
                "you · queued"
                {on_unqueue.map(|cb| view! {
                    <button
                        class="-my-0.5 ml-auto cursor-pointer rounded-sm px-1 py-0.5 leading-none \
                               text-faint hover:text-err focus-visible:outline-2 \
                               focus-visible:outline-offset-1 focus-visible:outline-accent"
                        type="button"
                        title="don't send this after all"
                        aria-label="Remove from queue"
                        on:click=move |_| cb.run(())
                    >
                        "\u{2715}"
                    </button>
                })}
            </div>
            <Markdown source=body class="text-meta"/>
        </div>
    }
}

/// A folded run of tool calls.
///
/// Closed by default, always: a run is what you skip. It is one `<details>` for the whole
/// run rather than one per call, so opening it shows the *sequence*, which is the thing
/// worth reading — a single call out of context rarely explains anything.
///
/// The summary line carries the one call worth seeing without opening: whatever is running
/// now, or the last one that ran.
#[component]
fn Did(calls: Vec<ToolCall>) -> impl IntoView {
    let n = calls.len();
    // What the closed run shows: the live call if there is one, else the last to have run.
    let head = calls
        .iter()
        .find(|c| c.state == ToolState::Running)
        .or_else(|| calls.last())
        .cloned();
    // The one word the closed run says about its head call, when there is one worth saying.
    // "running" is a claim about right now and must be true; a run that ended on a call says
    // so instead, which is the honest version of the flag it used to leave behind.
    let chip = match head.as_ref().map(|c| c.state) {
        Some(ToolState::Running) => Some(("running", "text-accent")),
        Some(ToolState::Unanswered) => Some(("no result", "text-attention")),
        _ => None,
    };

    view! {
        <details class=format!("{LAZY} group island overflow-hidden bg-card")>
            <summary class="flex cursor-pointer list-none items-center gap-2 px-3 py-2 \
                            select-none hover:bg-bubble focus-visible:outline-2 \
                            focus-visible:outline-offset-[-2px] focus-visible:outline-accent \
                            [&::-webkit-details-marker]:hidden">
                <svg
                    class="size-3 shrink-0 text-meta transition-transform duration-100 \
                           group-open:rotate-90"
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
                <span class="caps shrink-0 text-faint">
                    {if n == 1 { "1 call".to_string() } else { format!("{n} calls") }}
                </span>
                {head.map(|c| {
                    let dot = c.state.dot_classes();
                    view! {
                        <span
                            class=format!("size-1.5 shrink-0 rounded-full {dot}")
                            aria-hidden="true"
                        ></span>
                        <span class="truncate font-mono text-mini text-meta">{c.summary()}</span>
                    }
                })}
                {chip.map(|(word, ink)| view! {
                    <span class=format!("caps ml-auto shrink-0 {ink}")>{word}</span>
                })}
            </summary>
            <div class="flex flex-col gap-2 border-t border-divider p-3">
                {calls.into_iter().map(|c| view! { <Call call=c/> }).collect::<Vec<_>>()}
            </div>
        </details>
    }
}

/// One call, written the way the model wrote it.
///
/// **Not JSON.** A tool call is not a JSON document to the model — it is a block of tagged
/// text it emits into its own stream, and the result comes back as another one. Showing the
/// wire format of some transport instead teaches the reader a shape the model never saw,
/// and then they debug against it.
///
/// So this is the block: an `invoke` with a `parameter` per argument, values verbatim on
/// their own lines. It is longer than JSON and it is what is actually there.
#[component]
fn Call(call: ToolCall) -> impl IntoView {
    let dot = call.state.dot_classes();
    view! {
        <div class="flex flex-col gap-1">
            <div class="flex items-center gap-2">
                <span class=format!("size-1.5 shrink-0 rounded-full {dot}") aria-hidden="true">
                </span>
                <span class="font-mono text-mini text-syn-func">{call.name.clone()}</span>
            </div>
            <pre class="m-0 shrink-0 overflow-x-auto rounded-sm border border-edge bg-bubble \
                        p-2.5 font-mono text-mini leading-[1.55] whitespace-pre-wrap \
                        [word-break:break-word] text-syn-plain">
                <span class="text-syn-punct">"<invoke name="</span>
                <span class="text-syn-str">{format!("\"{}\"", call.name)}</span>
                <span class="text-syn-punct">">\n"</span>
                {call.params.into_iter().map(|(k, v)| view! {
                    <span class="text-syn-punct">"  <parameter name="</span>
                    <span class="text-syn-str">{format!("\"{k}\"")}</span>
                    <span class="text-syn-punct">">"</span>
                    {v}
                    <span class="text-syn-punct">"</parameter>\n"</span>
                }).collect::<Vec<_>>()}
                <span class="text-syn-punct">"</invoke>"</span>
            </pre>
            {call.result.map(|r| view! {
                <pre class="m-0 shrink-0 overflow-x-auto rounded-sm border border-edge \
                            bg-stage p-2.5 font-mono text-mini leading-[1.55] \
                            whitespace-pre-wrap [word-break:break-word] text-syn-comment">
                    <span class="text-syn-punct">"<result>\n"</span>
                    {r}
                    <span class="text-syn-punct">"\n</result>"</span>
                </pre>
            })}
        </div>
    }
}
