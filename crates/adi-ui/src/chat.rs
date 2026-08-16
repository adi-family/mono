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
//! Three properties do the work, and none of them is a virtual list:
//!
//! - **A keyed list.** Turns arrive as [`Entry`]s carrying an identity, and are rendered
//!   through `<For>`. A key that has not changed is not re-rendered *at all* — Leptos'
//!   keyed diff only ever calls the view function for keys it has not seen — so a settled
//!   turn's DOM is untouched by the poll that lands the next one. This is the load-bearing
//!   one: see [`Entry`] for what goes wrong without it.
//! - **`content-visibility: auto`** on every said turn. The browser skips layout, paint and
//!   style for anything off screen — virtualisation done by the engine, with no windowing,
//!   no scroll maths, and no rows that pop in wrong.
//! - **`contain-intrinsic-size`** beside it, so a skipped turn still reserves a plausible
//!   height. That is what keeps the scrollbar honest and stops the page shuddering as turns
//!   enter and leave. It only tells the truth because the list is keyed: the size the
//!   browser remembers is remembered *per element*, so an element that keeps its identity
//!   keeps a size that still describes what is inside it.
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
    /// A DOM id for this one call, so something outside the transcript can send the reader to
    /// it. A failed call forty turns down a folded run is otherwise unreachable except by
    /// scrolling — the id is what makes a summary that counts failures able to point at one.
    pub anchor: Option<String>,
}

impl ToolCall {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            params: Vec::new(),
            state: ToolState::default(),
            result: None,
            anchor: None,
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

    /// Name this call in the DOM, so a link elsewhere can jump to it.
    #[must_use]
    pub fn anchor(mut self, anchor: impl Into<String>) -> Self {
        self.anchor = Some(anchor.into());
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

/// A picture that was part of a message: where to fetch it, and what to call it.
///
/// A URL rather than bytes. A transcript is re-rendered on every poll, and the browser is already
/// the thing that caches an image by its address — handing it one is how a chat with a dozen
/// screenshots in it stays a chat rather than a download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub url: String,
    /// The alt text, and what a reader sees on hover — the file's own name in this tree.
    pub name: String,
}

/// One entry in a transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Turn {
    /// Something said, in Markdown, with whatever was attached to it.
    Said {
        role: Role,
        body: String,
        /// Attached images, drawn above the words. Only ever on a [`Role::User`] turn in this
        /// tree — pictures travel from the person to the model, not back.
        images: Vec<Image>,
    },
    /// A run of tool calls with no words between them. **Text is the divider**: everything
    /// the agent did between one thing it said and the next folds into a single run, which
    /// is exactly the unit you want to open or ignore.
    Did(Vec<ToolCall>),
}

/// A turn with the identity that keeps it still.
///
/// The key is why this type exists. A transcript is re-derived from a snapshot on every poll,
/// and the list is drawn newest-first — so **every** arriving turn shifts the position of
/// every turn already on screen. Diffed by position, that means each slot is rebuilt against
/// a different turn than it held a moment ago: markdown re-parsed and re-highlighted from
/// scratch for the whole transcript once a second, a `<details>` you had opened now standing
/// over somebody else's tool run, and the height the browser remembered for a skipped turn
/// now describing the wrong one. Diffed by key, none of that happens — an unchanged key is
/// not rendered again at all.
///
/// The key is also the entry's **DOM id**, so whatever names an entry can also link to it.
/// It therefore has to be unique within the transcript and valid as an id; the caller picks
/// it, because only the caller knows what an entry *is*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Stable across polls, unique in the transcript, and used verbatim as the DOM id.
    pub key: String,
    pub turn: Turn,
}

impl Entry {
    #[must_use]
    pub fn new(key: impl Into<String>, turn: Turn) -> Self {
        Self {
            key: key.into(),
            turn,
        }
    }
}

/// Key a list by position — for a transcript that is not going to change, which is what a
/// fixture and a finished run both are.
///
/// **Never for a live one.** A position is not an identity the moment anything is prepended,
/// and newest-first prepends every time: use it there and every key shifts onto the wrong
/// turn, which is the exact failure keying exists to prevent.
#[must_use]
pub fn by_position(turns: Vec<Turn>) -> Vec<Entry> {
    turns
        .into_iter()
        .enumerate()
        .map(|(i, turn)| Entry::new(format!("turn-{i}"), turn))
        .collect()
}

/// The transcript.
///
/// Hand it entries oldest-first — the order they happened, which is the order a store keeps
/// them — and it reverses them for the screen. Nothing else in the app has to think
/// backwards.
///
/// ```ignore
/// <Chat turns=settled live=streaming/>
/// ```
#[component]
pub fn Chat(
    /// The settled transcript, oldest first. The component flips it.
    ///
    /// Settled is the point: these are keyed, and a key that stays is a turn that is never
    /// re-rendered. Anything still being written belongs in `live` instead — left in here it
    /// would keep its key while its content changed, and a keyed list would simply never
    /// show the change.
    #[prop(into)]
    turns: Signal<Vec<Entry>>,
    /// The turn still being written, if there is one — the only part of a transcript whose
    /// content changes between polls.
    ///
    /// It is drawn outside the keyed list, in a reactive closure of its own, so a streaming
    /// answer costs one card's worth of work per poll instead of the whole transcript's. Its
    /// entries are keyed too, so the moment it settles and moves into `turns` the list
    /// recognises it rather than treating it as new.
    #[prop(optional, into)]
    live: Signal<Vec<Entry>>,
    /// What sits at the head of the transcript — **inside** its scroll, above the newest turn.
    ///
    /// This exists because the alternative does not work. A card pinned *outside* the scroll (a
    /// pending question, say) is a flex item in a column with a height: the moment it is taller
    /// than the pane its bottom is simply cut off, and there is no scrollbar anywhere that
    /// reaches it. Put it in here and it is one more thing in the feed — it scrolls like a
    /// message, because it is where the messages are.
    #[prop(optional, into)]
    lead: Option<ViewFn>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    view! {
        <div class=merge("flex flex-col gap-3 overflow-y-auto", class)>
            // `shrink-0` for exactly the reason every turn carries it (see [`LAZY`]): this is a
            // flex item in a column with a height, and a lead that can be squeezed is a lead with
            // its second half missing. `gap-3` inside, so several lead items sit apart the way the
            // turns below them do.
            {lead.map(|lead| view! {
                <div class="flex shrink-0 flex-col gap-3">{lead.run()}</div>
            })}
            // Above the settled list, because it is the newest thing there is. Rebuilt in
            // place on every poll — which is affordable precisely because it is one turn.
            //
            // Reversed in CSS rather than in Rust, and that is the same argument as the keys
            // below, in miniature. A turn's parts are append-only, so in document order a new
            // one lands at the end and disturbs nothing before it, and the unkeyed diff here
            // rebuilds only what actually changed. Reverse the vector instead and every part
            // shifts by one — which would leave a tool run you had opened standing over a
            // different run's calls, once a second, for as long as the answer takes.
            {move || {
                let parts = live.get();
                (!parts.is_empty()).then(|| view! {
                    <div class="flex shrink-0 flex-col-reverse gap-3">
                        {parts.into_iter().map(entry).collect::<Vec<_>>()}
                    </div>
                })
            }}
            <For
                each=move || {
                    let mut turns = turns.get();
                    turns.reverse();
                    turns
                }
                key=|entry: &Entry| entry.key.clone()
                let:settled
            >
                {entry(settled)}
            </For>
        </div>
    }
}

/// One entry, drawn under its own key as its own DOM id.
fn entry(entry: Entry) -> AnyView {
    let Entry { key, turn } = entry;
    match turn {
        Turn::Said { role, body, images } => {
            view! { <Said id=key role=role body=body images=images/> }.into_any()
        }
        Turn::Did(calls) => view! { <Did id=key calls=calls/> }.into_any(),
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
fn Said(
    id: String,
    role: Role,
    body: String,
    #[prop(optional)] images: Vec<Image>,
) -> impl IntoView {
    let own = match role {
        // The user's own words are a bubble, because they are the short thing you scan for
        // to find where a stretch of work began.
        Role::User => "island ml-8 bg-bubble px-3 py-2",
        Role::Agent => "px-1",
    };
    let said = !body.trim().is_empty();
    view! {
        <div id=id class=format!("{LAZY} {own}")>
            {(!images.is_empty()).then(|| view! { <Pictures images=images said=said/> })}
            {said.then(|| view! { <Markdown source=body/> })}
        </div>
    }
}

/// The pictures attached to a message, above its words.
///
/// Capped in height rather than shown whole: a screenshot of a full window is taller than the
/// transcript pane, and a message you have to scroll past to reach the reply is a message that has
/// taken over the conversation. The whole image is one click away — each opens in its own tab,
/// which is the browser's own zoom, pan and save rather than a lightbox that reimplements all
/// three.
#[component]
fn Pictures(images: Vec<Image>, said: bool) -> impl IntoView {
    let gap = if said { "mb-2" } else { "" };
    view! {
        <div class=format!("flex flex-wrap gap-2 {gap}")>
            {images
                .into_iter()
                .map(|image| {
                    let Image { url, name } = image;
                    let title = format!("{name} — open full size");
                    let href = url.clone();
                    view! {
                        <a
                            class="block max-w-full overflow-hidden rounded-sm border border-dim \
                                   focus-visible:outline-2 focus-visible:outline-offset-2 \
                                   focus-visible:outline-accent"
                            href=href
                            target="_blank"
                            rel="noreferrer"
                            title=title
                        >
                            <img
                                class="max-h-64 max-w-full object-contain"
                                src=url
                                alt=name
                                loading="lazy"
                            />
                        </a>
                    }
                })
                .collect::<Vec<_>>()}
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
    /// What was attached to it. A picture pasted while the agent was still answering waits in the
    /// queue with its message, and showing the words without it would misdescribe what is about to
    /// be sent.
    #[prop(optional)]
    images: Vec<Image>,
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
            {(!images.is_empty())
                .then(|| view! { <Pictures images=images said=!body.trim().is_empty()/> })}
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
fn Did(id: String, calls: Vec<ToolCall>) -> impl IntoView {
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
        <details id=id class=format!("{LAZY} group island overflow-hidden bg-card")>
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
#[component]
fn Call(call: ToolCall) -> impl IntoView {
    let dot = call.state.dot_classes();
    view! {
        <div id=call.anchor.clone() class="flex flex-col gap-1">
            <div class="flex items-center gap-2">
                <span class=format!("size-1.5 shrink-0 rounded-full {dot}") aria-hidden="true">
                </span>
                <span class="font-mono text-mini text-syn-func">{call.name.clone()}</span>
            </div>
            <Invoke name=call.name params=call.params/>
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

/// A call as the model actually emits it.
///
/// **Not JSON.** A tool call is not a JSON document to the model — it is a block of tagged
/// text it emits into its own stream, and the result comes back as another one. Showing the
/// wire format of some transport instead teaches the reader a shape the model never saw,
/// and then they debug against it.
///
/// So this is the block: an `invoke` with a `parameter` per argument, values verbatim on
/// their own lines. It is longer than JSON and it is what is actually there.
///
/// It lives here, shared, rather than once in the transcript and again in
/// [`TurnBlocks`](crate::TurnBlocks): a simulated call that rendered differently from a real
/// one would be teaching the reader the wrong shape in exactly the screen built to show them
/// the right one.
#[component]
pub(crate) fn Invoke(
    /// The tool's name, in the tag and beside it.
    name: String,
    /// The arguments, in the order they were written.
    params: Vec<(String, String)>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    view! {
        <pre class=merge(
            "m-0 shrink-0 overflow-x-auto rounded-sm border border-edge bg-bubble p-2.5 \
             font-mono text-mini leading-[1.55] whitespace-pre-wrap [word-break:break-word] \
             text-syn-plain",
            class,
        )>
            <span class="text-syn-punct">"<invoke name="</span>
            <span class="text-syn-str">{format!("\"{name}\"")}</span>
            <span class="text-syn-punct">">\n"</span>
            {params.into_iter().map(|(k, v)| view! {
                <span class="text-syn-punct">"  <parameter name="</span>
                <span class="text-syn-str">{format!("\"{k}\"")}</span>
                <span class="text-syn-punct">">"</span>
                {v}
                <span class="text-syn-punct">"</parameter>\n"</span>
            }).collect::<Vec<_>>()}
            <span class="text-syn-punct">"</invoke>"</span>
        </pre>
    }
}
