//! The facts base: one plain sentence, as a row and as a card — plus what an edit made
//! stale, and what one fact has been through.
//!
//! # What a fact is here
//!
//! Three things and nothing else: a sentence, whose meaning it is (`author`), and who wrote
//! the record (`creator`). Everything a designer's instinct wants to add — a category, a
//! confidence, a status — was tried in the experiment behind this and dropped, so the
//! components below have nothing to draw but the sentence and its provenance, and that is
//! the point rather than an omission.
//!
//! **Author and creator are both shown, always, and quietly.** "said by igor, written by
//! `agent:extractor@1`" is not a headline — but a base whose records were transcribed by
//! agents is only trustworthy if a reader can see which hand each one came through, and a UI
//! that shows one of the two names has quietly picked which question matters.
//!
//! # These draw nothing they were not handed
//!
//! Every value is a signal in, every action a callback out. Nothing here fetches, nothing
//! computes staleness, nothing decides a verdict — see [`crate::Simulator`] for the same
//! discipline argued at length. A staleness sweep is one join and an integer comparison
//! server-side; a component that guessed at it would be a second, wrong copy.

use leptos::prelude::*;

use crate::{Badge, BadgeTone, Button, ButtonSize, ButtonVariant, Empty, Verdict, merge};

/// What kind of node this is.
///
/// The distinction that carries weight is **written** against **derived**: a `Fact` was
/// stated by somebody, while `Composed` and `Artifact` were built out of other nodes and can
/// therefore go out of date under them. That is why the latter two share a tone — the badge
/// says which of the two it is, the colour says whether it can rot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NodeKind {
    /// One sentence somebody stated. The base's ground truth.
    #[default]
    Fact,
    /// One sentence standing in for several, derived from them.
    Composed,
    /// Anything longer built on facts — a plan, a page, a brief.
    Artifact,
}

impl NodeKind {
    /// The word for this kind, as the store spells it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Composed => "composed",
            Self::Artifact => "artifact",
        }
    }

    /// Whether this node was built from other nodes, and so can be made stale by one of them
    /// moving. A stated fact never goes stale — it gets rewritten, and that is what makes
    /// everything downstream of it stale.
    #[must_use]
    pub fn derived(self) -> bool {
        matches!(self, Self::Composed | Self::Artifact)
    }

    /// The badge tone. Derived nodes take the accent; a stated fact takes none, because it is
    /// the ordinary case and a base is mostly this.
    #[must_use]
    pub fn tone(self) -> BadgeTone {
        match self {
            Self::Fact => BadgeTone::Neutral,
            Self::Composed | Self::Artifact => BadgeTone::Accent,
        }
    }
}

/// One node of the base.
///
/// `version` starts at 1 and is bumped on every rewrite; it is what every derived node's edge
/// is stamped against, so it is shown wherever the id is. A fact with no version on screen is
/// a fact nobody can check a reference against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    /// The id the base gave it, and the string an outside reference is written with.
    pub id: String,
    /// The sentence. The whole of what the node says.
    pub text: String,
    /// Whose meaning this is — usually the person who said it.
    pub author: String,
    /// Who physically wrote the record — usually an agent, with its version.
    pub creator: String,
    /// Bumped on every rewrite. Starts at 1.
    pub version: u32,
    pub kind: NodeKind,
}

impl Fact {
    /// A stated fact at v1, with nobody named yet.
    #[must_use]
    pub fn new(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            author: String::new(),
            creator: String::new(),
            version: 1,
            kind: NodeKind::Fact,
        }
    }

    /// Whose meaning it is, and whose hand wrote it down — in that order, because that is the
    /// order the record is read in.
    #[must_use]
    pub fn by(mut self, author: impl Into<String>, creator: impl Into<String>) -> Self {
        self.author = author.into();
        self.creator = creator.into();
        self
    }

    /// The version it is at now.
    #[must_use]
    pub fn at(mut self, version: u32) -> Self {
        self.version = version;
        self
    }

    #[must_use]
    pub fn kind(mut self, kind: NodeKind) -> Self {
        self.kind = kind;
        self
    }
}

/// The sentence, at the weight the thing itself deserves.
///
/// `pub(crate)` because the pair card draws its two facts with it too: a fact has to look the
/// same wherever it is read, or a reader learns to trust one surface more than another.
#[component]
pub(crate) fn Sentence(
    #[prop(into)] text: String,
    /// The size step. A row is scanned, a card and a pair are read.
    #[prop(default = "text-row")]
    size: &'static str,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let own = format!("m-0 leading-relaxed text-ink [overflow-wrap:anywhere] {size}");
    view! { <p class=merge(&own, class)>{text}</p> }
}

/// "said by igor, written by `agent:chat@1`" — the two identities, spelled out.
///
/// Both names are mono: they are actor ids the store interned, not prose, and one of them is
/// nearly always an agent with a version stuck to it.
#[component]
pub(crate) fn Provenance(
    #[prop(into)] author: String,
    #[prop(into)] creator: String,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let own = "flex flex-wrap items-baseline gap-x-1 text-mini text-meta";
    view! {
        <div class=merge(own, class)>
            {(!author.is_empty())
                .then(|| view! {
                    <span>"said by"</span>
                    <span class="font-mono text-secondary">{author}</span>
                })}
            {(!creator.is_empty())
                .then(|| view! {
                    <span>"written by"</span>
                    <span class="font-mono text-secondary">{creator}</span>
                })}
        </div>
    }
}

/// The id and version, together, as one unbreakable token — `f091 v2`.
///
/// They are shown together everywhere because separately neither is checkable: an outside
/// reference is written `f091@1`, and the only way to see that it has drifted is to read the
/// version that is standing next to the id now.
#[component]
pub(crate) fn Stamp(
    #[prop(into)] id: String,
    version: u32,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let own = "font-mono text-caps whitespace-nowrap tabular-nums text-fainter";
    view! {
        <span class=merge(own, class)>
            {id}
            <span class="text-faint">{format!(" v{version}")}</span>
        </span>
    }
}

/// A fact as a **row**, for a list of them.
///
/// The sentence leads and everything else is the line under it, because a list of facts is
/// scanned by reading, not by id. Handlers attach the ordinary Leptos way —
/// `<FactRow on:click=open/>` lands on the underlying `<button>` — so, as with
/// [`crate::Button`], there is no callback prop.
///
/// It draws no border of its own: a row is part of a list, and the panel around it is the
/// island.
#[component]
pub fn FactRow(
    fact: Fact,
    /// The one the screen is currently showing.
    #[prop(optional, into)]
    selected: Signal<bool>,
    #[prop(optional, into)] class: String,
    /// A `<span>` after the stamp — a stale mark, a neighbour count.
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    let Fact {
        id,
        text,
        author,
        creator,
        version,
        kind,
    } = fact;
    // One reactive `class`, because an element may only be given the attribute once: the
    // selected/idle fill and the call site's own utilities are merged in the same closure.
    let fill = move || {
        let own = if selected.get() {
            "w-full cursor-pointer rounded-sm border border-edge bg-selected px-2.5 py-2 text-left"
        } else {
            "w-full cursor-pointer rounded-sm border border-transparent px-2.5 py-2 text-left \
             hover:bg-card"
        };
        merge(own, class.clone())
    };

    view! {
        <button class=fill type="button">
            <Sentence text=text/>
            <div class="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1">
                <Stamp id=id version=version/>
                <Badge tone=kind.tone() mono=true>{kind.label()}</Badge>
                {children.map(|c| c())}
                <Provenance author=author creator=creator/>
            </div>
        </button>
    }
}

/// A fact as a **card**, for when it is the thing on screen rather than one of a list.
///
/// Same three parts in the same order, one step louder, on its own island.
#[component]
pub fn FactCard(
    fact: Fact,
    /// Controls pinned to the right of the head — open the history, copy the reference.
    #[prop(optional, into)]
    actions: Option<ViewFn>,
    #[prop(optional, into)] class: String,
    /// Anything under the sentence — the neighbours, an edit box.
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    let Fact {
        id,
        text,
        author,
        creator,
        version,
        kind,
    } = fact;
    view! {
        <div class=merge("island bg-card", class)>
            <div class="flex items-center gap-2 border-b border-divider px-3 py-1.5">
                <Stamp id=id version=version/>
                <Badge tone=kind.tone() mono=true>{kind.label()}</Badge>
                <span class="flex-1"></span>
                {actions.map(|a| a.run())}
            </div>
            <div class="flex flex-col gap-2 p-3">
                <Sentence text=text size="text-msg"/>
                <Provenance author=author creator=creator/>
                {children.map(|c| c())}
            </div>
        </div>
    }
}

/// What a sentence used to say, and what it says now.
///
/// **Deliberately not a strike-through.** Nothing was deleted: the node was rewritten in
/// place and kept its id, which is the whole mechanism — a struck-out line would tell a
/// reader the record is gone when in fact it is the same record, saying something else.
#[component]
pub(crate) fn WasNow(
    #[prop(into)] was: String,
    #[prop(into)] now: String,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let own = "grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-2 gap-y-1";
    view! {
        <div class=merge(own, class)>
            <span class="caps text-fainter">"was"</span>
            <span class="text-mini leading-relaxed text-meta [overflow-wrap:anywhere]">{was}</span>
            <span class="caps text-fainter">"now"</span>
            <span class="text-mini leading-relaxed text-ink [overflow-wrap:anywhere]">{now}</span>
        </div>
    }
}

/// One source fact that moved under a derived node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    /// The source fact's id.
    pub source: String,
    /// What it said when the derived node was built.
    pub was: String,
    /// What it says now.
    pub now: String,
    /// The version the edge was stamped at — what the derived node was built against.
    pub built_at: u32,
    /// The source's version now. The mismatch with `built_at` *is* the staleness.
    pub version: u32,
}

impl Moved {
    #[must_use]
    pub fn new(source: impl Into<String>, was: impl Into<String>, now: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            was: was.into(),
            now: now.into(),
            built_at: 1,
            version: 2,
        }
    }

    /// The two versions: what it was built against, and where the source is now.
    #[must_use]
    pub fn versions(mut self, built_at: u32, version: u32) -> Self {
        self.built_at = built_at;
        self.version = version;
        self
    }
}

/// A derived node that is out of date, and every source that moved under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stale {
    /// The derived node, as it stands — text included, because deciding whether it needs
    /// rewriting means reading it against the change.
    pub node: Fact,
    /// What moved. More than one source can move before anybody looks.
    pub causes: Vec<Moved>,
}

impl Stale {
    #[must_use]
    pub fn new(node: Fact, causes: Vec<Moved>) -> Self {
        Self { node, causes }
    }
}

/// What is out of date, and what changed under it.
///
/// **This is a was/now surface, not a list of names.** Knowing that `f091` moved says nothing
/// about whether the plan built on it needs rewriting; knowing that it went from "we are not
/// sure we can enter the China market" to "we can support China after all" answers the
/// question in one read. So both texts are always here, and the id is the small print.
///
/// The staleness itself is mechanical and belongs server-side — one join and an integer
/// comparison, no model in the loop. This component is handed the answer.
#[component]
pub fn StaleList(
    #[prop(into)] items: Signal<Vec<Stale>>,
    /// The derived node was regenerated: re-stamp its edges at the sources' current versions.
    /// With no handler the control is not drawn, which is the honest rendering of a read-only
    /// view.
    #[prop(optional, into)]
    on_refresh: Option<Callback<String>>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let rows = move || {
        let items = items.get();
        if items.is_empty() {
            return view! { <Empty>"Nothing is out of date."</Empty> }.into_any();
        }
        items
            .into_iter()
            .map(|item| view! { <StaleCard item=item on_refresh=on_refresh/> })
            .collect::<Vec<_>>()
            .into_any()
    };
    view! { <div class=merge("flex flex-col gap-3", class)>{rows}</div> }
}

/// One stale node.
#[component]
fn StaleCard(item: Stale, on_refresh: Option<Callback<String>>) -> impl IntoView {
    let Stale { node, causes } = item;
    let id = node.id.clone();
    view! {
        <div class="island bg-card">
            <div class="flex items-center gap-2 border-b border-divider px-3 py-1.5">
                <Stamp id=node.id.clone() version=node.version/>
                <Badge tone=node.kind.tone() mono=true>{node.kind.label()}</Badge>
                <Badge tone=BadgeTone::Warn mono=true>"stale"</Badge>
                <span class="flex-1"></span>
                {on_refresh.map(|cb| view! {
                    <Button
                        size=ButtonSize::Small
                        variant=ButtonVariant::Default
                        on:click=move |_| cb.run(id.clone())
                    >
                        "mark refreshed"
                    </Button>
                })}
            </div>
            <div class="flex flex-col gap-3 p-3">
                <Sentence text=node.text size="text-msg"/>
                <div class="flex flex-col gap-2">
                    {causes
                        .into_iter()
                        .map(|c| view! {
                            <div class="rounded-sm border border-edge bg-panel-alt p-2.5">
                                <div class="mb-1.5 flex flex-wrap items-center gap-2">
                                    <span class="caps text-fainter">"built on"</span>
                                    <Stamp id=c.source version=c.version/>
                                    <span class="font-mono text-caps text-fainter">
                                        {format!("stamped at v{}", c.built_at)}
                                    </span>
                                </div>
                                <WasNow was=c.was now=c.now/>
                            </div>
                        })
                        .collect::<Vec<_>>()}
                </div>
            </div>
        </div>
    }
}

/// One step in a fact's log: the version it produced, what caused it, and who confirmed that.
///
/// `verdict` is `None` for v1 — creation is not a verdict, and calling it one would put the
/// moment a fact was first stated in the same column as the moment somebody overruled it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The version this step produced.
    pub version: u32,
    /// The verdict that caused it; `None` for the creation of v1.
    pub verdict: Option<Verdict>,
    /// Who confirmed it — a human id, or an agent id with its version.
    pub by: String,
    pub was: String,
    pub now: String,
}

impl Change {
    /// The step that created the fact.
    #[must_use]
    pub fn created(by: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            version: 1,
            verdict: None,
            by: by.into(),
            was: String::new(),
            now: text.into(),
        }
    }

    /// A step that rewrote it.
    #[must_use]
    pub fn rewritten(
        version: u32,
        verdict: Verdict,
        by: impl Into<String>,
        was: impl Into<String>,
        now: impl Into<String>,
    ) -> Self {
        Self {
            version,
            verdict: Some(verdict),
            by: by.into(),
            was: was.into(),
            now: now.into(),
        }
    }
}

/// The history of one fact — what a reference to it resolves to now, and what it has been.
///
/// The case this exists for is the one that does not announce itself. Because `merge` and
/// `supersede` rewrite the winner **in place**, a committed id is never destroyed and an
/// outside reference never dangles; what changes under it is the *meaning*. A dangling
/// pointer throws; a pointer whose target quietly changed does not, so this screen has to say
/// it in words.
///
/// Give `against` the version a reference was written at (`f091@1`) and the drift is reported
/// rather than left for a reader to notice.
#[component]
pub fn FactHistory(
    /// The fact as it stands now.
    fact: Fact,
    /// Every step, newest first — the order somebody chasing "what happened to this" reads in.
    #[prop(into)]
    changes: Signal<Vec<Change>>,
    /// The version an outside reference was written against, if this is being read on behalf
    /// of one.
    #[prop(optional, into)]
    against: Signal<Option<u32>>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let head = fact.clone();
    let now = fact.version;
    let rewritten = now > 1;

    view! {
        <div class=merge("island bg-card", class)>
            <div class="flex items-center gap-2 border-b border-divider px-3 py-1.5">
                <Stamp id=head.id.clone() version=now/>
                <Badge tone=head.kind.tone() mono=true>{head.kind.label()}</Badge>
            </div>

            <div class="flex flex-col gap-2 p-3">
                <Sentence text=head.text.clone() size="text-msg"/>
                <Provenance author=head.author.clone() creator=head.creator.clone()/>
            </div>

            // The reference check. Drawn in the queue tones rather than the error ones: a
            // drifted reference is not a failure of anything, it is news, and it is news
            // whether the reader likes it or not.
            {move || against.get().map(|at| if at < now {
                view! {
                    <div class="mx-3 mb-3 rounded-sm border border-queue-edge bg-queue-bg \
                                px-2.5 py-2 text-mini text-queue">
                        <span class="caps">"stale reference"</span>
                        <span>
                            {format!(
                                " \u{2014} written against v{at}, the fact is now v{now}.",
                            )}
                        </span>
                    </div>
                }
                .into_any()
            } else {
                view! {
                    <div class="mx-3 mb-3 text-mini text-meta">
                        {format!("Reference written against v{at} \u{2014} still current.")}
                    </div>
                }
                .into_any()
            })}

            <div class="border-t border-divider">
                <div class="px-3 pt-2 pb-1">
                    <span class="caps text-faint">"what changed"</span>
                </div>
                <div class="flex flex-col gap-2 px-3 pb-3">
                    {move || changes
                        .get()
                        .into_iter()
                        .map(|c| view! { <Step change=c/> })
                        .collect::<Vec<_>>()}
                </div>
            </div>

            // The sentence a reader has to be told rather than shown. Everything above is
            // true of an id that still works, which is exactly why nothing above makes the
            // point on its own.
            {rewritten.then(|| view! {
                <p class="m-0 border-t border-divider bg-bar px-3.5 py-2.5 text-mini text-meta">
                    "The id still resolves. It no longer means what it did."
                </p>
            })}
        </div>
    }
}

/// One step of a fact's log.
#[component]
fn Step(change: Change) -> impl IntoView {
    let Change {
        version,
        verdict,
        by,
        was,
        now,
    } = change;
    view! {
        <div class="rounded-sm border border-edge bg-panel-alt p-2.5">
            <div class="mb-1.5 flex flex-wrap items-center gap-2">
                <span class="font-mono text-caps tabular-nums text-faint">
                    {format!("v{version}")}
                </span>
                {match verdict {
                    Some(v) => view! { <Badge tone=v.tone() mono=true>{v.label()}</Badge> }
                        .into_any(),
                    None => view! { <Badge mono=true>"created"</Badge> }.into_any(),
                }}
                <span class="text-mini text-meta">"by"</span>
                <span class="font-mono text-mini text-secondary">{by}</span>
            </div>
            {if was.is_empty() {
                view! {
                    <div class="text-mini leading-relaxed text-ink [overflow-wrap:anywhere]">
                        {now}
                    </div>
                }
                .into_any()
            } else {
                view! { <WasNow was=was now=now/> }.into_any()
            }}
        </div>
    }
}
