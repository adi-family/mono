//! The pair — two facts the base found near each other, and the four ways a person can rule
//! on them.
//!
//! # This is a triage inbox, not a knowledge browser
//!
//! Measured on a real base: roughly **2 pairs need a decision per fact added**, about ten per
//! dictated note. Browsing the base is the rare case; working this queue is the whole job. So
//! the pair card is the surface that got designed first and best, and everything else in the
//! facts screen is arranged to serve it.
//!
//! # Four things here look wrong and are not
//!
//! **There is no bulk action, and there will not be one.** No "accept all", no "merge every
//! duplicate", no "auto-resolve above 0.85". In the measured base, pairs above 0.80 similarity
//! held **10 controversies against 8 duplicates** — more conflicts than merges at the very top
//! of the queue — and the single highest-ranked pair in the whole base (0.886) was "The company
//! supports all countries except the CIS" against "Within the CIS, the company supports
//! Ukraine". A bulk merge would have silently deleted the Ukraine carve-out. A control that
//! resolves many pairs at once destroys data here, so the absence is the feature.
//!
//! **Strength is a small plain number, never a bar and never a coloured meter.** A bar implies
//! a calibration the number does not have: `independent` pairs run 0.245–0.898 and `duplicate`
//! pairs 0.617–0.898 — the ceilings are identical, and the second-ranked pair in the base came
//! back `independent`. A [`Badge`] in mono says "this is a measurement" without claiming it
//! sorts anything.
//!
//! **Rank is not priority.** By median similarity duplicates sit at 0.821, qualifications at
//! 0.721 and contradictions at 0.672 — so a strictly top-down queue makes a person grind
//! through boring merges and meet the real conflicts last. [`PairQueue`] can therefore lead
//! with conflicts and filter by kind, and it says on screen that the rank is a similarity, not
//! an importance.
//!
//! **`coexist` is a decision, not a dismissal.** It is what turns "we have two notes" into "we
//! know both of these are true", it is recorded with its confirmer like any other verdict, and
//! it is therefore drawn as one of four equal buttons — never as a skip, a snooze, or an ×
//! parked off to one side.
//!
//! # And the machine's reason is subordinate on purpose
//!
//! Below about the 750th-ranked pair the classifier's stated reason routinely describes facts
//! that are **not in the pair it was given** — at rank 4279 it paired "the plan includes
//! launching a website" with "we can support China" and explained a conflict between them. So
//! the two sentences are the subject of the card and the reason is a quiet line under them,
//! clearly labelled as an opinion. A reader must never be able to act on the reason without
//! having read the facts.

use leptos::{ev, html, prelude::*, wasm_bindgen::JsCast, web_sys};

use crate::facts::{Provenance, Sentence, Stamp};
use crate::{Badge, BadgeTone, Button, ButtonSize, ButtonVariant, Empty, Fact, Kbd, Textarea, merge};

/// What the classifier thought the pair was.
///
/// Three arms, and `independent` is deliberately not one of them: a pair the classifier called
/// independent never reaches a queue, so a card that could draw it would be drawing a state
/// nobody is ever asked about. If that changes — the design has an open question about
/// confirming co-existence rather than assuming it — this enum grows an arm and the queue
/// grows a chip, and nothing else moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Relation {
    /// The two disagree. The most valuable finding and the most expensive to get wrong.
    #[default]
    Controversy,
    /// One qualifies the other — a carve-out, an exception, a narrower case.
    Narrows,
    /// They say the same thing.
    Duplicate,
}

impl Relation {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Controversy => "controversy",
            Self::Narrows => "narrows",
            Self::Duplicate => "duplicate",
        }
    }

    /// What the classifier is claiming, in words — a chip's worth, for the filter.
    #[must_use]
    pub fn says(self) -> &'static str {
        match self {
            Self::Controversy => "these two disagree",
            Self::Narrows => "one carves an exception out of the other",
            Self::Duplicate => "these two say the same thing",
        }
    }

    /// The badge tone. Red for a disagreement, amber for a qualification, nothing for a
    /// duplicate — which is the only one of the three that is ordinary housekeeping.
    #[must_use]
    pub fn tone(self) -> BadgeTone {
        match self {
            Self::Controversy => BadgeTone::Down,
            Self::Narrows => BadgeTone::Warn,
            Self::Duplicate => BadgeTone::Neutral,
        }
    }

    /// Where this kind sits when the queue leads with conflicts. Lower comes first.
    ///
    /// `Narrows` above `Duplicate` because a carve-out is the shape a lost fact hides in — the
    /// Ukraine case was a `narrows` at the top of the base — while a duplicate is the one kind
    /// where getting it wrong costs a sentence somebody can write again.
    fn urgency(self) -> u8 {
        match self {
            Self::Controversy => 0,
            Self::Narrows => 1,
            Self::Duplicate => 2,
        }
    }
}

/// What a person can rule.
///
/// Four, and all four are decisions. `merge` and `supersede` are one mechanism underneath —
/// both retire the losing sentence by rewriting the winner in place — and differ only in where
/// the winning sentence comes from: `merge` takes one that was written, `supersede` takes the
/// side that won.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Verdict {
    /// Both stand, and somebody said so. **Confirmed, not assumed.**
    #[default]
    Coexist,
    /// One sentence in place of two, written by the person ruling.
    Merge,
    /// One side's sentence replaces the other's.
    Supersede,
    /// The new fact never lands. The base was already right.
    Drop,
}

impl Verdict {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Coexist => "coexist",
            Self::Merge => "merge",
            Self::Supersede => "supersede",
            Self::Drop => "drop",
        }
    }

    /// What it does to the facts, for the button's title.
    #[must_use]
    pub fn says(self) -> &'static str {
        match self {
            Self::Coexist => "both stand, and the base now knows both are true",
            Self::Merge => "one sentence you write, in place of both",
            Self::Supersede => "the side you pick is written into the other, which goes stale",
            Self::Drop => "the new fact never lands",
        }
    }

    /// The key that rules this way while a card has focus.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Coexist => "c",
            Self::Merge => "m",
            Self::Supersede => "s",
            Self::Drop => "d",
        }
    }

    /// The tone a *recorded* verdict wears. Neutral for the three that keep everything, error
    /// ink for the one that ends a sentence's life in the base.
    #[must_use]
    pub fn tone(self) -> BadgeTone {
        match self {
            Self::Coexist | Self::Merge | Self::Supersede => BadgeTone::Neutral,
            Self::Drop => BadgeTone::Down,
        }
    }

    /// Every verdict, in the order the card draws them. `coexist` leads because it is the one
    /// that keeps both sentences, not because it is a default — the card gives none of the
    /// four the primary weight.
    pub const ALL: [Verdict; 4] =
        [Verdict::Coexist, Verdict::Merge, Verdict::Supersede, Verdict::Drop];
}

/// One half of a pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairSide {
    pub fact: Fact,
    /// This fact is staged in the open transaction rather than already in the base. It is the
    /// difference between "drop" having an obvious target and having none.
    pub staged: bool,
}

impl PairSide {
    /// A fact already in the base.
    #[must_use]
    pub fn base(fact: Fact) -> Self {
        Self { fact, staged: false }
    }

    /// A fact staged in the open transaction.
    #[must_use]
    pub fn staged(fact: Fact) -> Self {
        Self { fact, staged: true }
    }

    fn label(&self) -> &'static str {
        if self.staged { "new" } else { "base" }
    }
}

/// A verdict already recorded, with the identity that made it.
///
/// Two `coexist`s are not the same record if one was confirmed by a person and the other by
/// `agent:verifier@3`, so the confirmer travels with the verdict everywhere it is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decided {
    pub verdict: Verdict,
    /// A human id, or an agent id with its version.
    pub by: String,
}

impl Decided {
    #[must_use]
    pub fn new(verdict: Verdict, by: impl Into<String>) -> Self {
        Self { verdict, by: by.into() }
    }
}

/// Two facts that need ruling on.
#[derive(Debug, Clone, PartialEq)]
pub struct Pair {
    pub id: String,
    /// Cosine of the two embeddings. Shown as a number to three places and nothing else.
    pub strength: f32,
    pub relation: Relation,
    /// The classifier's stated reason. May be empty, and is not trusted — see the module doc.
    pub reason: String,
    /// The two sides, as an array because they are of equal weight by construction. Which one
    /// is which is said by [`PairSide::staged`], not by position.
    pub sides: [PairSide; 2],
    /// The verdict, once one has been recorded. A recorded verdict is permanent and the same
    /// pair is never asked twice, so this is the end state of a card rather than a draft.
    pub decided: Option<Decided>,
}

impl Pair {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        strength: f32,
        relation: Relation,
        a: PairSide,
        b: PairSide,
    ) -> Self {
        Self {
            id: id.into(),
            strength,
            relation,
            reason: String::new(),
            sides: [a, b],
            decided: None,
        }
    }

    /// The classifier's stated reason.
    #[must_use]
    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = reason.into();
        self
    }

    /// The verdict already on the record.
    #[must_use]
    pub fn decided(mut self, decided: Decided) -> Self {
        self.decided = Some(decided);
        self
    }

    /// How many of the two sides are staged in the open transaction.
    fn staged_count(&self) -> usize {
        self.sides.iter().filter(|s| s.staged).count()
    }
}

/// A decision leaving the card.
///
/// One shape for all four verdicts, mapping onto what the store is asked to do:
/// `resolve <pair> --verdict <v> [--keep <id>] [--fact <text>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ruling {
    /// Which pair.
    pub pair: String,
    pub verdict: Verdict,
    /// The id of the fact whose sentence **stands**. Set for `supersede` always, and for
    /// `drop` only where the card had to ask — when both sides are staged there is no base
    /// fact for "the new one never lands" to mean.
    pub keep: Option<String>,
    /// `merge` only: the sentence written in place of both.
    pub fact: Option<String>,
}

/// What the pending list left out.
///
/// **Truncation is always reported.** A silent cap reads as "nothing else to see", which is
/// the one lie this interface must never tell — so [`PairQueue`] renders this when it is
/// `Some` and says the list is complete when it is `None`, rather than saying nothing either
/// way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Truncated {
    /// How many pairs above the floor were never examined.
    pub not_examined: usize,
    /// The strength below which they sit.
    pub below: f32,
}

impl Truncated {
    #[must_use]
    pub fn new(not_examined: usize, below: f32) -> Self {
        Self { not_examined, below }
    }
}

/// What the card is asking for right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Mode {
    /// The four verdicts.
    #[default]
    Idle,
    /// `merge` was chosen and wants a sentence.
    Merging,
    /// A verdict was chosen that wants a side picked.
    Picking(Verdict),
}

/// One pair, and the four ways to rule on it. **The decision atom of the whole subsystem.**
///
/// The two facts are the subject of the card: equal weight, equal size, side by side, each
/// with its own provenance, because deciding between them means reading both. Everything else
/// — the strength, the classifier's guess at the relation, its stated reason — is furniture
/// around them.
///
/// Two of the verdicts need something more than a click, and the card asks for it in place
/// rather than in a dialog:
///
/// - **`merge` opens a box.** It starts empty. The merged sentence is the one that says what
///   both sides said, and there is no way to pick that off a menu — the two "start from"
///   controls seed the box with one side's wording, which is a shortcut to typing and not a
///   choice of outcome.
/// - **`supersede` makes picking a side an act.** The two "this one stands" controls sit on
///   the sentences themselves, so the click lands on the words that will survive rather than
///   on an id in a dropdown. That click *is* the confirmation; a third one would only teach
///   people to click through.
///
/// Keyboard-first, because this is a queue somebody works: the card takes focus, `c` `m` `s`
/// `d` rule, `↓`/`↑` (or `j`/`k`) walk to the next card, `Escape` backs out of a mode.
///
/// ```ignore
/// <PairCard pair=pair rank=6 on_rule=Callback::new(move |r: Ruling| resolve(r))/>
/// ```
#[component]
pub fn PairCard(
    pair: Pair,
    /// The decision, on its way to whoever will record it. The card never learns whether it
    /// landed — the caller replaces the pair with a decided one, which is what draws the
    /// verdict on the card.
    #[prop(into)]
    on_rule: Callback<Ruling>,
    /// Where the pair sat in the ranking, if the caller is showing one. Drawn as `rank 6` and
    /// never as a position in a priority list — see the module doc.
    #[prop(optional)]
    rank: Option<usize>,
    /// The caller's own check found the stated reason naming facts that are not in this pair.
    /// That mismatch is a free false-positive detector — no extra model call — and it is worth
    /// saying out loud, because it is the failure mode of everything below the top of the
    /// queue.
    #[prop(optional, into)]
    reason_suspect: Signal<bool>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let root = NodeRef::<html::Div>::new();
    let mode = RwSignal::new(Mode::default());
    let draft = RwSignal::new(String::new());

    let id = pair.id.clone();
    let decided = pair.decided.clone();
    let open = decided.is_none();
    let reason = pair.reason.clone();
    let strength = pair.strength;
    let relation = pair.relation;
    let sides = pair.sides.clone();
    // `drop` means "the new fact never lands", which needs exactly one new fact to point at.
    // With both sides staged — two facts from the same batch — there is no base side, so the
    // card has to ask which one lands instead of guessing.
    let drop_is_obvious = pair.staged_count() == 1;

    let rule = {
        let id = id.clone();
        move |verdict: Verdict, keep: Option<String>, fact: Option<String>| {
            on_rule.run(Ruling { pair: id.clone(), verdict, keep, fact });
            mode.set(Mode::Idle);
            // Walk on before the caller's re-render lands. A queue you have to re-aim after
            // every decision is a queue worked with the mouse.
            step(root, 1);
        }
    };

    let start = {
        let rule = rule.clone();
        move |verdict: Verdict| match verdict {
            Verdict::Coexist => rule(verdict, None, None),
            Verdict::Merge => {
                mode.set(Mode::Merging);
            }
            Verdict::Supersede => mode.set(Mode::Picking(Verdict::Supersede)),
            Verdict::Drop => {
                if drop_is_obvious {
                    rule(verdict, None, None);
                } else {
                    mode.set(Mode::Picking(Verdict::Drop));
                }
            }
        }
    };

    let on_key = {
        let start = start.clone();
        move |ev: ev::KeyboardEvent| {
            let key = ev.key();
            if key == "Escape" {
                mode.set(Mode::Idle);
                return;
            }
            // A verdict key typed into the merge box is a letter, not a verdict. Without this
            // guard the first `s` of a merged sentence supersedes the pair.
            if typing_in(&ev) || !open {
                return;
            }
            match key.as_str() {
                "ArrowDown" | "j" => {
                    ev.prevent_default();
                    step(root, 1);
                }
                "ArrowUp" | "k" => {
                    ev.prevent_default();
                    step(root, -1);
                }
                other => {
                    if let Some(v) = Verdict::ALL.into_iter().find(|v| v.key() == other) {
                        ev.prevent_default();
                        start(v);
                    }
                }
            }
        }
    };

    view! {
        <div
            node_ref=root
            // The queue walks between cards by DOM sibling, so the marker is what says
            // "this is another card" rather than a spacer the caller slipped in.
            data-pair=id.clone()
            tabindex="0"
            class=merge(
                "group island bg-card focus-visible:outline-2 focus-visible:outline-offset-2 \
                 focus-visible:outline-accent",
                class,
            )
            on:keydown=on_key
        >
            <div class="flex flex-wrap items-center gap-2 border-b border-divider px-3 py-1.5">
                {rank.map(|n| view! {
                    <span class="caps text-fainter">{format!("rank {n}")}</span>
                })}
                // The number, plain. Three places because that is the resolution the pairs are
                // actually ordered at; a bar or a tinted meter would claim a calibration the
                // cosine does not have.
                <Badge mono=true>{format!("{strength:.3}")}</Badge>
                <Badge tone=relation.tone() mono=true>{relation.label()}</Badge>
                <span class="flex-1"></span>
                {decided.clone().map(|d| view! {
                    <Badge tone=d.verdict.tone() mono=true>{d.verdict.label()}</Badge>
                })}
            </div>

            <div class="grid grid-cols-1 sm:grid-cols-2">
                {sides
                    .clone()
                    .into_iter()
                    .enumerate()
                    .map(|(i, side)| {
                        // Both literals spelled out: Tailwind reads this file as text.
                        let edge = if i == 0 {
                            "flex flex-col gap-2 border-b border-divider p-3 sm:border-r \
                             sm:border-b-0"
                        } else {
                            "flex flex-col gap-2 p-3"
                        };
                        let label = side.label();
                        let fact = side.fact;
                        let keep_id = fact.id.clone();
                        let rule = rule.clone();
                        view! {
                            <div class=edge>
                                <div class="flex flex-wrap items-center gap-2">
                                    <Badge mono=true>{label}</Badge>
                                    <Stamp id=fact.id.clone() version=fact.version/>
                                </div>
                                <Sentence text=fact.text.clone() size="text-msg"/>
                                <Provenance author=fact.author.clone() creator=fact.creator.clone()/>
                                {move || match mode.get() {
                                    Mode::Picking(v) => {
                                        let keep_id = keep_id.clone();
                                        let rule = rule.clone();
                                        Some(view! {
                                            <Button
                                                size=ButtonSize::Small
                                                class="self-start"
                                                on:click=move |_| rule(
                                                    v,
                                                    Some(keep_id.clone()),
                                                    None,
                                                )
                                            >
                                                {if v == Verdict::Supersede {
                                                    "this one stands"
                                                } else {
                                                    "this one lands"
                                                }}
                                            </Button>
                                        })
                                    }
                                    _ => None,
                                }}
                            </div>
                        }
                    })
                    .collect::<Vec<_>>()}
            </div>

            // The classifier's opinion, under the facts and quieter than them. It is labelled
            // rather than merely styled down, because a sentence in an interface reads as a
            // finding unless something says whose sentence it is.
            {(!reason.is_empty()).then(|| view! {
                <div class="flex flex-col gap-1 border-t border-divider bg-panel-alt px-3 py-2">
                    <span class="caps text-fainter">"the classifier says"</span>
                    <p class="m-0 text-mini leading-relaxed text-meta">{reason.clone()}</p>
                    <Show when=move || reason_suspect.get()>
                        <p class="m-0 text-mini leading-relaxed text-queue">
                            "This reason names facts that are not in this pair. Read the two \
                             sentences; the explanation is about something else."
                        </p>
                    </Show>
                </div>
            })}

            {move || match (decided.clone(), mode.get()) {
                // A verdict on the record. Nothing to press: the same pair is never asked
                // twice, and a card that still offered its four buttons would be inviting a
                // second answer to a settled question.
                (Some(d), _) => view! {
                    <div class="flex flex-wrap items-baseline gap-x-2 gap-y-1 border-t \
                                border-divider bg-bar px-3 py-2.5 text-mini text-meta">
                        <span>"recorded"</span>
                        <span class="font-mono text-secondary">{d.verdict.label()}</span>
                        <span>"by"</span>
                        <span class="font-mono text-secondary">{d.by.clone()}</span>
                        <span class="w-full text-fainter">
                            "A recorded verdict is permanent. This pair is never asked again."
                        </span>
                    </div>
                }
                .into_any(),
                (None, Mode::Merging) => view! {
                    <MergeBox
                        draft=draft
                        sides=sides.clone()
                        on_cancel=Callback::new(move |()| mode.set(Mode::Idle))
                        on_merge={
                            let rule = rule.clone();
                            Callback::new(move |text: String| rule(Verdict::Merge, None, Some(text)))
                        }
                    />
                }
                .into_any(),
                (None, Mode::Picking(v)) => view! {
                    <div class="flex flex-wrap items-center gap-2 border-t border-divider \
                                bg-bar px-3 py-2.5">
                        <span class="min-w-0 flex-1 text-mini text-meta">
                            {if v == Verdict::Supersede {
                                "Pick the sentence that stands. It is written into the other \
                                 node in place, so everything derived from that node goes stale."
                            } else {
                                "Both of these are new. Pick the one that lands; the other \
                                 never reaches the base."
                            }}
                        </span>
                        <Button
                            size=ButtonSize::Small
                            variant=ButtonVariant::Ghost
                            on:click=move |_| mode.set(Mode::Idle)
                        >
                            "cancel"
                        </Button>
                    </div>
                }
                .into_any(),
                (None, Mode::Idle) => view! {
                    <div class="flex flex-wrap items-center gap-2 border-t border-divider \
                                bg-bar px-3 py-2.5">
                        {Verdict::ALL
                            .into_iter()
                            .map(|v| {
                                let start = start.clone();
                                view! {
                                    // All four the same variant, in one row. `coexist` is a
                                    // decision — "we know both of these are true" — so drawing
                                    // it as a skip, or parking it away from the other three,
                                    // would misreport what it does. And none of the four is
                                    // Primary: the card has no recommendation to make.
                                    <Button
                                        size=ButtonSize::Small
                                        attr:title=v.says()
                                        on:click=move |_| start(v)
                                    >
                                        {v.label()}
                                        // Hidden until the card has the keyboard, in CSS
                                        // rather than in a signal: the queue re-creates every
                                        // card component when one of them is ruled, so a
                                        // signal would come back `false` under a card the
                                        // browser is still focusing.
                                        <span class="hidden group-focus-within:inline-flex">
                                            <Kbd>{v.key()}</Kbd>
                                        </span>
                                    </Button>
                                }
                            })
                            .collect::<Vec<_>>()}
                    </div>
                }
                .into_any(),
            }}
        </div>
    }
}

/// The box `merge` opens.
#[component]
fn MergeBox(
    draft: RwSignal<String>,
    sides: [PairSide; 2],
    on_cancel: Callback<()>,
    on_merge: Callback<String>,
) -> impl IntoView {
    view! {
        <div class="flex flex-col gap-2 border-t border-divider bg-bar px-3 py-2.5">
            <span class="text-mini text-meta">
                "One sentence in place of both \u{2014} the one that says what each of them said."
            </span>
            <Textarea
                value=draft
                rows=2
                prose=true
                placeholder="The company supports all countries except the CIS, where it supports Ukraine."
            />
            <div class="flex flex-wrap items-center gap-2">
                // Seeding the box from a side is a shortcut to typing, not a choice of
                // outcome: what lands is whatever is in the box when `merge` is pressed.
                {sides
                    .into_iter()
                    .map(|side| {
                        let label = side.label();
                        let text = side.fact.text;
                        view! {
                            <Button
                                size=ButtonSize::Small
                                variant=ButtonVariant::Ghost
                                on:click=move |_| draft.set(text.clone())
                            >
                                {format!("start from {label}")}
                            </Button>
                        }
                    })
                    .collect::<Vec<_>>()}
                <span class="flex-1"></span>
                <Button
                    size=ButtonSize::Small
                    variant=ButtonVariant::Ghost
                    on:click=move |_| on_cancel.run(())
                >
                    "cancel"
                </Button>
                <Button
                    size=ButtonSize::Small
                    disabled=Signal::derive(move || draft.get().trim().is_empty())
                    on:click=move |_| on_merge.run(draft.get_untracked())
                >
                    "record merge"
                </Button>
            </div>
        </div>
    }
}

/// Whether the key went into a control that takes text. A verdict shortcut typed into the
/// merge box has to stay a letter.
fn typing_in(ev: &ev::KeyboardEvent) -> bool {
    ev.target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .is_some_and(|el| matches!(el.tag_name().as_str(), "TEXTAREA" | "INPUT" | "SELECT"))
}

/// Move the keyboard to the next card, or the previous one.
///
/// By DOM sibling rather than by index: the queue owns the list, the card owns the keys, and
/// neither has to know how many pairs there are. The `data-pair` marker is what stops a walk
/// landing on a truncation notice or a spacer.
fn step(root: NodeRef<html::Div>, delta: i32) {
    let Some(el) = root.get_untracked() else {
        return;
    };
    let next = if delta > 0 { el.next_element_sibling() } else { el.previous_element_sibling() };
    let Some(next) = next.filter(|n| n.has_attribute("data-pair")) else {
        return;
    };
    if let Ok(next) = next.dyn_into::<web_sys::HtmlElement>() {
        let _ = next.focus();
    }
}

/// How the queue is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Order {
    /// As the base ranked it — by similarity, strongest first.
    #[default]
    Ranked,
    /// Contradictions, then qualifications, then duplicates; ranked within each.
    ConflictsFirst,
}

/// The queue: every pair waiting on a decision, and the two ways to come at it.
///
/// It draws **no island of its own** — it is a list, and the panel it sits in is the object on
/// the screen. See [`crate::TxPanel`], which is usually that panel.
///
/// The controls at the top exist because rank is not priority. Sorting strictly by strength
/// makes a person work through duplicates — median 0.821 — before ever reaching the
/// contradictions at 0.672, which are the findings worth a human. So the queue can lead with
/// conflicts, and it can be narrowed to one kind. Neither reorders anything in the store: this
/// is which end of the same list you start from.
///
/// ```ignore
/// <PairQueue
///     pairs=Signal::derive(move || pending.get())
///     acting_as=Signal::derive(move || whoami.get())
///     truncated=Signal::derive(move || cap.get())
///     on_rule=resolve
/// />
/// ```
#[component]
pub fn PairQueue(
    /// Every pair, in the order the base ranked them.
    #[prop(into)]
    pairs: Signal<Vec<Pair>>,
    #[prop(into)] on_rule: Callback<Ruling>,
    /// The identity every verdict made here will be stamped with. Shown, always: a `coexist`
    /// confirmed by a person and one confirmed by `agent:verifier@3` are different records,
    /// and somebody about to make thirty of them should be able to see which they are making.
    #[prop(optional, into)]
    acting_as: Signal<String>,
    /// What the pending list left out, if it was capped.
    #[prop(optional, into)]
    truncated: Signal<Option<Truncated>>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let kind = RwSignal::new(None::<Relation>);
    let order = RwSignal::new(Order::default());

    let shown = Signal::derive(move || {
        let mut rows: Vec<Pair> = pairs
            .get()
            .into_iter()
            .filter(|p| kind.get().is_none_or(|k| p.relation == k))
            .collect();
        if order.get() == Order::ConflictsFirst {
            // Stable, so the ranking survives inside each band — the two orderings are the
            // same list read from two ends, not two different lists.
            rows.sort_by_key(|p| p.relation.urgency());
        }
        rows
    });
    let open = Signal::derive(move || pairs.get().iter().filter(|p| p.decided.is_none()).count());

    view! {
        <div class=merge("flex flex-col", class)>
            <div class="flex flex-wrap items-center gap-2 border-b border-divider bg-panel-alt \
                        px-3 py-2">
                <span class="text-row font-medium text-ink">
                    {move || format!("{} to decide", open.get())}
                </span>
                <span class="flex-1"></span>
                <span class="caps text-fainter">"deciding as"</span>
                <span class="font-mono text-mini text-secondary">
                    {move || {
                        let who = acting_as.get();
                        if who.is_empty() { "\u{2014} nobody set".to_string() } else { who }
                    }}
                </span>
            </div>

            <div class="flex flex-wrap items-center gap-1.5 border-b border-divider px-3 py-2">
                <Chip on:click=move |_| kind.set(None) active=Signal::derive(move || kind.get().is_none())>
                    {move || format!("all {}", pairs.get().len())}
                </Chip>
                {[Relation::Controversy, Relation::Narrows, Relation::Duplicate]
                    .into_iter()
                    .map(|r| {
                        let n = Signal::derive(move || {
                            pairs.get().iter().filter(|p| p.relation == r).count()
                        });
                        view! {
                            <Chip
                                active=Signal::derive(move || kind.get() == Some(r))
                                title=r.says()
                                on:click=move |_| kind.set(Some(r))
                            >
                                {move || format!("{} {}", r.label(), n.get())}
                            </Chip>
                        }
                    })
                    .collect::<Vec<_>>()}
                <span class="flex-1"></span>
                <Button
                    size=ButtonSize::Small
                    variant=ButtonVariant::Ghost
                    attr:title="Duplicates score highest and matter least. This starts at the \
                                other end."
                    on:click=move |_| order.update(|o| {
                        *o = if *o == Order::Ranked { Order::ConflictsFirst } else { Order::Ranked };
                    })
                >
                    {move || match order.get() {
                        Order::Ranked => "as ranked",
                        Order::ConflictsFirst => "conflicts first",
                    }}
                </Button>
            </div>

            <p class="m-0 px-3 py-1.5 text-mini text-meta">
                "Rank is similarity, not importance \u{2014} above 0.80 this base holds more \
                 contradictions than duplicates. Nothing here resolves in bulk; every pair is \
                 read."
            </p>

            <div class="flex flex-col gap-3 p-3">
                {move || {
                    let rows = shown.get();
                    if rows.is_empty() {
                        return view! { <Empty>"No pairs waiting."</Empty> }.into_any();
                    }
                    rows.into_iter()
                        .enumerate()
                        .map(|(i, pair)| view! {
                            <PairCard pair=pair rank={i + 1} on_rule=on_rule/>
                        })
                        .collect::<Vec<_>>()
                        .into_any()
                }}
            </div>

            // Said either way. "Nothing more" and "we stopped looking" are different facts and
            // an interface that only ever prints one of them has picked which one you assume.
            <div class="border-t border-divider bg-bar px-3.5 py-2.5 text-mini text-meta">
                {move || match truncated.get() {
                    Some(t) => view! {
                        <span class="text-queue">
                            {format!(
                                "{} more pairs were not examined \u{2014} everything below \
                                 {:.3}.",
                                t.not_examined, t.below,
                            )}
                        </span>
                    }
                    .into_any(),
                    None => view! {
                        <span>"Every pair above the floor was examined."</span>
                    }
                    .into_any(),
                }}
            </div>
        </div>
    }
}

/// A filter chip. Same shape as the simulator's tabs, in the queue's own row.
#[component]
fn Chip(
    #[prop(into)] active: Signal<bool>,
    #[prop(optional, into)] title: String,
    children: Children,
) -> impl IntoView {
    let look = move || {
        if active.get() {
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
            title=title
            aria-pressed=move || active.get().to_string()
        >
            {children()}
        </button>
    }
}
