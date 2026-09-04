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
//!
//! # What it looks like
//!
//! The transcript is the product (`design/DESIGN.md` §2.1): it sits on the lightest surface,
//! in the largest body type, at the widest measure. The agent's words are plain paragraphs —
//! no bubble. A run of tool calls is a receipt, not a message: one collapsed line with a
//! hairline round it, opened on demand.

use leptos::prelude::*;

use crate::icon::{Icon, IconSize, Lucide};
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
    /// live in a run that ended weeks ago is the transcript lying about the present.
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
    /// The 6px dot beside the call's name (§3: a state is a dot, never a fill).
    #[must_use]
    pub fn dot_classes(self) -> &'static str {
        match self {
            Self::Running => "bg-accent",
            Self::Ok => "bg-ink-3",
            Self::Failed => "bg-err",
            Self::Unanswered => "bg-warn",
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

    /// What the receipt line shows of this call: its first argument, flattened — the command
    /// of a `Bash`, the path of a `Read`. The line is cut by the box it sits in, so nothing is
    /// truncated here; a call with no arguments shows nothing.
    #[must_use]
    pub fn preview(&self) -> String {
        self.params
            .first()
            .map(|(_, v)| v.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default()
    }
}

/// Something that was attached to a message: where to fetch it, what to call it, and whether it is
/// a picture at all.
///
/// A URL rather than bytes. A transcript is re-rendered on every poll, and the browser is already
/// the thing that caches a file by its address — handing it one is how a chat with a dozen
/// screenshots in it stays a chat rather than a download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub url: String,
    /// The alt text, and what a reader sees on hover — the file's own name in this tree.
    pub name: String,
    pub kind: AttachmentKind,
}

/// Whether an attachment can be *shown* or only linked to.
///
/// The difference is not decoration: a PDF drawn as an `<img>` is a broken image with a filename
/// nobody can read, where the same row as a link is a thing you can open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    /// A picture the browser draws — the four types a message can carry into a model's request.
    Picture,
    /// Anything else: shown as a named link, and opened by whatever the reader has for it.
    File,
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
        images: Vec<Attachment>,
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
    /// answer costs one turn's worth of work per poll instead of the whole transcript's. Its
    /// entries are keyed too, so the moment it settles and moves into `turns` the list
    /// recognises it rather than treating it as new.
    #[prop(optional, into)]
    live: Signal<Vec<Entry>>,
    /// What sits at the head of the transcript — **inside** its scroll, above the newest turn.
    ///
    /// This exists because the alternative does not work. A block pinned *outside* the scroll (a
    /// pending question, say) is a flex item in a column with a height: the moment it is taller
    /// than the pane its bottom is simply cut off, and there is no scrollbar anywhere that
    /// reaches it. Put it in here and it is one more thing in the feed — it scrolls like a
    /// message, because it is where the messages are.
    #[prop(optional, into)]
    lead: Option<ViewFn>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    view! {
        <div class=merge("flex flex-col gap-4 overflow-y-auto bg-bg", class)>
            // `shrink-0` for exactly the reason every turn carries it (see [`LAZY`]): this is a
            // flex item in a column with a height, and a lead that can be squeezed is a lead with
            // its second half missing.
            {lead.map(|lead| view! {
                <div class="flex max-w-[80ch] shrink-0 flex-col gap-4">{lead.run()}</div>
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
                    <div class="flex shrink-0 flex-col-reverse gap-4">
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

/// What every top-level entry wears, and every half of it is load-bearing.
///
/// **`shrink-0`** first, because this is a flex column with a height: without it a turn is a
/// flex item that can be squeezed, and one that opens — a tool run — gets squeezed. Measured
/// in the browser: an opened run came out 343px tall around 607px of content, with the
/// second half of it simply gone.
///
/// **`content-visibility: auto`** second, with an intrinsic height beside it so a skipped
/// turn still reserves room and the scrollbar does not lie. That pair is the whole
/// virtualisation story: the engine skips what is off screen, and find-in-page, anchors and
/// screen readers keep seeing everything — which is what a JS virtual list takes away.
///
/// **`max-w-[80ch]`** last: the transcript's measure (§4). Wider than that and a line of
/// prose is a line nobody finishes.
const LAZY: &str =
    "max-w-[80ch] shrink-0 [content-visibility:auto] [contain-intrinsic-size:auto_120px]";

/// One thing said.
#[component]
fn Said(
    id: String,
    role: Role,
    body: String,
    #[prop(optional)] images: Vec<Attachment>,
) -> impl IntoView {
    let own = match role {
        // Your own words are on the raised surface so a stretch of work can be scanned for
        // where it began; a tone, not a bubble — no border, and never a card in a card.
        Role::User => "rounded-lg bg-raise px-4 py-3",
        Role::Agent => "",
    };
    let said = !body.trim().is_empty();
    view! {
        <div id=id class=format!("{LAZY} {own}")>
            {(!images.is_empty()).then(|| view! { <Pictures images=images said=said/> })}
            {said.then(|| view! { <Markdown source=body/> })}
        </div>
    }
}

/// What was attached to a message, above its words.
///
/// A picture is capped in height rather than shown whole: a screenshot of a full window is taller
/// than the transcript pane, and a message you have to scroll past to reach the reply is a message
/// that has taken over the conversation. The whole image is one click away — each opens in its own
/// tab, which is the browser's own zoom, pan and save rather than a lightbox that reimplements all
/// three.
///
/// Anything that is not a picture is a named link instead, for the same reason it reaches the model
/// as a path: there is nothing to draw. It opens in a tab too, where the browser shows a PDF and
/// downloads what it cannot.
#[component]
fn Pictures(images: Vec<Attachment>, said: bool) -> impl IntoView {
    let gap = if said { "mb-3" } else { "" };
    view! {
        <div class=format!("flex flex-wrap items-start gap-2 {gap}")>
            {images
                .into_iter()
                .map(|image| {
                    let Attachment { url, name, kind } = image;
                    match kind {
                        AttachmentKind::Picture => {
                            let title = format!("{name} — open full size");
                            let href = url.clone();
                            view! {
                                <a
                                    class="block max-w-full overflow-hidden rounded-md border \
                                           border-line"
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
                                .into_any()
                        }
                        AttachmentKind::File => {
                            let title = format!("{name} — open this file");
                            view! {
                                <a
                                    class="flex max-w-full items-center gap-2 rounded-md border \
                                           border-line bg-raise px-3 py-2 text-mini text-ink-2 \
                                           hover:text-ink"
                                    href=url
                                    target="_blank"
                                    rel="noreferrer"
                                    title=title
                                >
                                    <Icon
                                        icon=Lucide::Paperclip
                                        size=IconSize::Sm
                                        label="Attached file"
                                    />
                                    <span class="truncate">{name}</span>
                                </a>
                            }
                                .into_any()
                        }
                    }
                })
                .collect::<Vec<_>>()}
        </div>
    }
}

/// A message said but not yet asked: your own block, hollowed out.
///
/// It is not a [`Turn`], and deliberately so — a queue is not transcript. The turns a [`Chat`]
/// holds are things that happened; this is a thing that has not, and it sits *outside* the
/// transcript (below it on screen, where the newest-first order puts what comes next). Keeping
/// it out of `Vec<Turn>` is what stops a queued message from being indistinguishable from a
/// sent one the moment anything reads the list.
///
/// Same geometry as the user's block in [`Said`], then emptied: dashed hairline, no fill, dimmed
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
    images: Vec<Attachment>,
    /// Take it back before the agent ever sees it. With no handler the × is not drawn, which
    /// is the honest rendering of a queue you cannot edit.
    #[prop(optional, into)]
    on_unqueue: Option<Callback<()>>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    view! {
        <div class=merge(
            &format!("{LAZY} rounded-lg border border-dashed border-line-strong px-4 py-3 text-ink-3"),
            class,
        )>
            <div class="label mb-1 flex items-center gap-1 text-ink-3">
                "You · queued"
                {on_unqueue.map(|cb| view! {
                    <button
                        class="-my-0.5 ml-auto grid size-6 place-items-center rounded-md \
                               text-ink-3 hover:bg-hover hover:text-ink"
                        type="button"
                        title="don't send this after all"
                        on:click=move |_| cb.run(())
                    >
                        <Icon icon=Lucide::X size=IconSize::Sm label="Remove from queue"/>
                    </button>
                })}
            </div>
            {(!images.is_empty())
                .then(|| view! { <Pictures images=images said=!body.trim().is_empty()/> })}
            <Markdown source=body class="text-ink-3"/>
        </div>
    }
}

/// The tools a run used, for its receipt line: one name when they are all the same, the first
/// three otherwise. A run that read, grepped and edited is "Read, Grep, Edit"; one that only ran
/// shell is "Bash".
fn tools_of(calls: &[ToolCall]) -> String {
    let mut names: Vec<&str> = Vec::new();
    for call in calls {
        if !names.contains(&call.name.as_str()) {
            names.push(&call.name);
        }
    }
    match names.len() {
        0..=3 => names.join(", "),
        _ => format!("{}, …", names[..3].join(", ")),
    }
}

/// A folded run of tool calls: a receipt, not a message (§6).
///
/// Closed by default, always: a run is what you skip. It is one `<details>` for the whole
/// run rather than one per call, so opening it shows the *sequence*, which is the thing
/// worth reading — a single call out of context rarely explains anything.
///
/// The line carries the count and the tool, and the one call worth seeing without opening:
/// whatever is running now, or the last one that ran.
#[component]
fn Did(id: String, calls: Vec<ToolCall>) -> impl IntoView {
    let n = calls.len();
    let count = if n == 1 {
        "1 call".to_string()
    } else {
        format!("{n} calls")
    };
    let tools = tools_of(&calls);
    // What the closed run shows: the live call if there is one, else the last to have run.
    let head = calls
        .iter()
        .find(|c| c.state == ToolState::Running)
        .or_else(|| calls.last())
        .cloned();
    // The one word the closed run says about its head call, when there is one worth saying.
    // "running" is a claim about right now and must be true; a run that ended on a call says
    // so instead, which is the honest version of the flag it used to leave behind.
    let note = match head.as_ref().map(|c| c.state) {
        Some(ToolState::Running) => Some(("running", "bg-accent")),
        Some(ToolState::Unanswered) => Some(("no result", "bg-warn")),
        Some(ToolState::Failed) => Some(("failed", "bg-err")),
        _ => None,
    };

    view! {
        <details id=id class=format!("{LAZY} group rounded-lg border border-line text-ink-3")>
            <summary class="flex cursor-pointer list-none items-center gap-2.5 rounded-lg px-3 \
                            py-2 text-small select-none hover:bg-hover hover:text-ink-2 \
                            [&::-webkit-details-marker]:hidden">
                <Icon
                    icon=Lucide::ChevronRight
                    size=IconSize::Sm
                    class="transition-transform duration-100 group-open:rotate-90"
                />
                <span class="shrink-0 font-medium text-ink-2">{format!("{count} · {tools}")}</span>
                {head.map(|c| view! {
                    <span class="min-w-0 truncate font-mono text-mini">{c.preview()}</span>
                })}
                {note.map(|(word, dot)| view! {
                    <span class="ml-auto flex shrink-0 items-center gap-1.5 text-label">
                        <span class=format!("size-1.5 rounded-full {dot}") aria-hidden="true"></span>
                        {word}
                    </span>
                })}
            </summary>
            <div class="flex flex-col gap-3 border-t border-line px-3 py-3">
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
        <div id=call.anchor.clone() class="flex flex-col gap-1.5">
            <div class="flex items-center gap-2">
                <span class=format!("size-1.5 shrink-0 rounded-full {dot}") aria-hidden="true">
                </span>
                <span class="text-small font-medium text-ink-2">{call.name.clone()}</span>
            </div>
            <Invoke name=call.name params=call.params/>
            {call.result.map(|r| view! {
                <pre class="m-0 shrink-0 overflow-x-auto rounded-lg border border-line \
                            bg-raise px-3.5 py-3 font-mono text-mono leading-[1.6] \
                            whitespace-pre-wrap [word-break:break-word] text-ink-2">
                    <span class="text-ink-3">"<result>\n"</span>
                    {r}
                    <span class="text-ink-3">"\n</result>"</span>
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
            "m-0 shrink-0 overflow-x-auto rounded-lg border border-line bg-raise px-3.5 py-3 \
             font-mono text-mono leading-[1.6] whitespace-pre-wrap [word-break:break-word] \
             text-code",
            class,
        )>
            <span class="text-ink-3">"<invoke name="</span>
            <span class="text-syn-str">{format!("\"{name}\"")}</span>
            <span class="text-ink-3">">\n"</span>
            {params.into_iter().map(|(k, v)| view! {
                <span class="text-ink-3">"  <parameter name="</span>
                <span class="text-syn-str">{format!("\"{k}\"")}</span>
                <span class="text-ink-3">">"</span>
                {v}
                <span class="text-ink-3">"</parameter>\n"</span>
            }).collect::<Vec<_>>()}
            <span class="text-ink-3">"</invoke>"</span>
        </pre>
    }
}

#[cfg(test)]
mod tests {
    use super::{ToolCall, tools_of};

    #[test]
    fn a_receipt_names_its_tools() {
        let one = vec![ToolCall::new("Bash"), ToolCall::new("Bash")];
        assert_eq!(tools_of(&one), "Bash");
        let mixed = vec![
            ToolCall::new("Read"),
            ToolCall::new("Grep"),
            ToolCall::new("Read"),
            ToolCall::new("Edit"),
            ToolCall::new("Bash"),
        ];
        assert_eq!(tools_of(&mixed), "Read, Grep, Edit, …");
    }

    #[test]
    fn the_preview_is_the_first_argument_flattened() {
        let call = ToolCall::new("Bash")
            .param("command", "cd /tmp &&\n  ls")
            .param("description", "list");
        assert_eq!(call.preview(), "cd /tmp && ls");
        assert_eq!(ToolCall::new("thinking").preview(), "");
    }
}
