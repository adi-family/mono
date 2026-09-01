//! The Facts page: a base of plain sentences, and the queue of decisions that keeps it honest.
//!
//! A fact is one sentence with two names on it — whose meaning it is, and whose hand wrote the
//! record. When facts are added the base finds the ones already near them by embedding
//! similarity and asks somebody to rule on each pair; nothing lands until every pair is
//! decided. Separately, anything derived from a fact goes stale the moment that fact is
//! rewritten, and a log keeps outside references honest.
//!
//! # The order of this page is the argument
//!
//! **Decide comes first, and the base comes last.** Measured on a real base, adding facts
//! raises about two pairs per fact — ten per dictated note — so working the queue is the job
//! and browsing is the exception. A page that opened on a searchable list of sentences would
//! be a page arranged around the thing people do least.
//!
//! Out-of-date derivations sit between the two: they are not a decision anybody is blocked on,
//! but they are the other thing this base knows that nobody else does.
//!
//! # Where the backend goes
//!
//! Everything the screen shows arrives through [`load`], and every action leaves through the
//! four functions beside it. That block is the **whole** seam — see the banner comment on it —
//! and it is fixture-driven until `adi-facts` publishes its HTTP surface.

use adi_ui::{
    Button, ButtonSize, ButtonVariant, Change, Decided, Empty, Fact, FactHistory, FactRow, Flash,
    FlashKind, Input, InputWidth, Moved, NodeKind, Pair, PairQueue, PairSide, Relation, Ruling,
    Stale, StaleList, Truncated, TxPanel, Verdict,
};
use leptos::prelude::*;

/// The open transaction, as this page needs it.
#[derive(Clone, PartialEq)]
pub(crate) struct TxView {
    pub(crate) id: String,
    /// How many facts are staged under it — not the same as the number of pairs, and a person
    /// about to commit wants both numbers.
    pub(crate) staged: usize,
    pub(crate) pairs: Vec<Pair>,
    /// What the pending list left out, if the store capped it.
    pub(crate) truncated: Option<Truncated>,
}

/// Everything the page draws, in one value.
#[derive(Clone, PartialEq)]
pub(crate) struct FactsData {
    /// The open transaction, or `None` when there is nothing staged.
    pub(crate) tx: Option<TxView>,
    pub(crate) stale: Vec<Stale>,
    /// The base itself, newest first.
    pub(crate) facts: Vec<Fact>,
    /// The log of the fact whose history is open, keyed by id.
    pub(crate) history: Vec<(String, Vec<Change>)>,
}

/// The page's own state: what it has loaded, what is typed into it, and who it is deciding as.
///
/// `Copy` so it threads into the view and into async handlers, the way every other console on
/// this shell does.
#[derive(Clone, Copy)]
pub(crate) struct FactsConsole {
    pub(crate) data: RwSignal<Option<FactsData>>,
    /// The base's filter box.
    pub(crate) filter: RwSignal<String>,
    /// The fact whose history is open, or empty for none.
    pub(crate) open: RwSignal<String>,
    /// The identity every verdict made on this screen is stamped with. Editable, and always on
    /// screen: a `coexist` confirmed by a person and one confirmed by `agent:verifier@3` are
    /// different records, and somebody about to make thirty of them should see which they are
    /// making.
    pub(crate) acting_as: RwSignal<String>,
    /// What the last action did, kept beside the page rather than in the shared flash.
    pub(crate) note: RwSignal<String>,
    pub(crate) error: RwSignal<Option<String>>,
    pub(crate) busy: RwSignal<bool>,
}

impl FactsConsole {
    pub(crate) fn new() -> Self {
        Self {
            data: RwSignal::new(None),
            filter: RwSignal::new(String::new()),
            open: RwSignal::new(String::new()),
            acting_as: RwSignal::new("igor".to_string()),
            note: RwSignal::new(String::new()),
            error: RwSignal::new(None),
            busy: RwSignal::new(false),
        }
    }
}

// ==========================================================================================
// THE BACKEND SEAM. Everything below this banner and above the next one is the only code on
// the page that knows where facts come from, and it is the only thing that changes when
// `adi-facts` publishes its HTTP surface: `load` becomes `fetch::facts().await`, and the four
// actions become one POST each. Nothing in the view below reaches past these functions.
// ==========================================================================================

/// Fill the page. Today from a fixture built out of the measured base — the pair at 0.886 is
/// the real one, and it is here because it is the case the whole design turns on.
// `async` with nothing awaited on purpose: this is the shape the fetch will have, and keeping
// it means swapping the body for `fetch::facts().await` touches no caller.
#[allow(clippy::unused_async)]
async fn load(facts: FactsConsole) {
    facts.data.set(Some(fixture()));
}

/// Record one verdict. The pair keeps its place in the queue wearing the verdict, because a
/// recorded verdict is permanent and a pair that vanished when you decided it would take the
/// record of the decision with it.
fn resolve(facts: FactsConsole, ruling: &Ruling) {
    let by = facts.acting_as.get_untracked();
    facts.data.update(|data| {
        let Some(tx) = data.as_mut().and_then(|d| d.tx.as_mut()) else {
            return;
        };
        if let Some(pair) = tx.pairs.iter_mut().find(|p| p.id == ruling.pair) {
            pair.decided = Some(Decided::new(ruling.verdict, by));
        }
    });
}

/// Land everything staged.
fn commit(facts: FactsConsole) {
    let staged = tx_of(facts).map_or(0, |tx| tx.staged);
    facts.data.update(|data| {
        if let Some(data) = data.as_mut() {
            data.tx = None;
        }
    });
    facts
        .note
        .set(format!("Committed \u{2014} {staged} facts landed."));
}

/// Throw the transaction away.
fn abort(facts: FactsConsole) {
    facts.data.update(|data| {
        if let Some(data) = data.as_mut() {
            data.tx = None;
        }
    });
    facts
        .note
        .set("Transaction aborted. Nothing landed.".to_string());
}

/// A derived node was regenerated: re-stamp its edges at its sources' current versions.
fn refresh(facts: FactsConsole, id: &str) {
    facts.data.update(|data| {
        if let Some(data) = data.as_mut() {
            data.stale.retain(|s| s.node.id != id);
        }
    });
    facts
        .note
        .set(format!("{id} re-stamped at its sources' current versions."));
}

// ==========================================================================================
// END OF THE SEAM. Everything below draws what is above and asks it for nothing else.
// ==========================================================================================

/// The Facts page.
pub(crate) fn facts_view(facts: FactsConsole) -> AnyView {
    Effect::new(move |loaded: Option<()>| {
        if loaded.is_none() {
            leptos::task::spawn_local(load(facts));
        }
    });

    view! {
        {decide_panel(facts)}
        {stale_panel(facts)}
        {base_panel(facts)}
        {history_panel(facts)}
    }
    .into_any()
}

/// The transaction and its queue — the first thing on the page, because it is the job.
fn decide_panel(facts: FactsConsole) -> AnyView {
    let tx = Signal::derive(move || tx_of(facts));
    let pairs = Signal::derive(move || tx.get().map(|t| t.pairs).unwrap_or_default());
    let staged = Signal::derive(move || tx.get().map_or(0, |t| t.staged));
    let pending =
        Signal::derive(move || pairs.get().iter().filter(|p| p.decided.is_none()).count());
    let truncated = Signal::derive(move || tx.get().and_then(|t| t.truncated));

    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Decide"</h2>
                <span class="adi-spacer"></span>
                // The identity, editable and never hidden. Every verdict carries it.
                <span class="adi-updated">"acting as"</span>
                <Input value=facts.acting_as width=InputWidth::Default placeholder="igor"/>
            </div>

            <div class="adi-panel__body">
                {move || match tx.get() {
                    None => view! {
                        <Empty>
                            "Nothing staged. An insert opens a transaction and the pairs it \
                             raises land here."
                        </Empty>
                    }
                    .into_any(),
                    Some(tx) => view! {
                        <TxPanel
                            id=tx.id
                            staged=staged
                            pending=pending
                            busy=facts.busy
                            on_commit=Callback::new(move |()| commit(facts))
                            on_abort=Callback::new(move |()| abort(facts))
                        >
                            <PairQueue
                                pairs=pairs
                                acting_as=facts.acting_as
                                truncated=truncated
                                on_rule=Callback::new(move |r: Ruling| resolve(facts, &r))
                            />
                        </TxPanel>
                    }
                    .into_any(),
                }}

                {move || (!facts.note.get().is_empty())
                    .then(|| view! {
                        <Flash kind=FlashKind::Ok card=true>{facts.note.get()}</Flash>
                    })}
                {move || facts.error.get()
                    .map(|e| view! { <Flash kind=FlashKind::Err card=true>{e}</Flash> })}
            </div>
        </section>
    }
    .into_any()
}

/// What is out of date, and what changed under it.
fn stale_panel(facts: FactsConsole) -> AnyView {
    let items = Signal::derive(move || facts.data.get().map(|d| d.stale).unwrap_or_default());
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Out of date"</h2>
                <span class="adi-chip adi-mono" title="Derived nodes with a source that moved">
                    {move || items.get().len().to_string()}
                </span>
                <span class="adi-spacer"></span>
                <span class="adi-updated">
                    "mechanical \u{2014} a version comparison, no model in the loop"
                </span>
            </div>
            <div class="adi-panel__body">
                <StaleList
                    items=items
                    on_refresh=Callback::new(move |id: String| refresh(facts, &id))
                />
            </div>
        </section>
    }
    .into_any()
}

/// The base itself. Last on the page and deliberately plain: it is a list of sentences, and
/// looking one up is the rare visit.
fn base_panel(facts: FactsConsole) -> AnyView {
    let all = Signal::derive(move || facts.data.get().map(|d| d.facts).unwrap_or_default());
    let shown = Signal::derive(move || {
        let needle = facts.filter.get().trim().to_lowercase();
        all.get()
            .into_iter()
            .filter(|f| needle.is_empty() || f.text.to_lowercase().contains(&needle))
            .collect::<Vec<_>>()
    });

    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"The base"</h2>
                <span class="adi-chip adi-mono" title="Facts in the base">
                    {move || all.get().len().to_string()}
                </span>
                <span class="adi-spacer"></span>
                <Input
                    value=facts.filter
                    input_type="search"
                    placeholder="filter by words"
                    width=InputWidth::Default
                />
            </div>
            <div class="adi-panel__body">
                {move || {
                    let rows = shown.get();
                    if rows.is_empty() {
                        return view! { <Empty>"No fact matches that."</Empty> }.into_any();
                    }
                    rows.into_iter()
                        .map(|fact| {
                            let id = fact.id.clone();
                            let selected =
                                Signal::derive(move || facts.open.get() == id);
                            let open_id = fact.id.clone();
                            view! {
                                <FactRow
                                    fact=fact
                                    selected=selected
                                    on:click=move |_| facts.open.set(open_id.clone())
                                />
                            }
                        })
                        .collect::<Vec<_>>()
                        .into_any()
                }}
            </div>
        </section>
    }
    .into_any()
}

/// The history of the open fact — only drawn once one is open.
fn history_panel(facts: FactsConsole) -> AnyView {
    let open = Signal::derive(move || {
        let id = facts.open.get();
        if id.is_empty() {
            return None;
        }
        let data = facts.data.get()?;
        let fact = data.facts.iter().find(|f| f.id == id)?.clone();
        let changes = data
            .history
            .iter()
            .find(|(fid, _)| *fid == id)
            .map(|(_, log)| log.clone())
            .unwrap_or_default();
        Some((fact, changes))
    });

    view! {
        {move || open.get().map(|(fact, changes)| view! {
            <section class="adi-panel">
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">"History"</h2>
                    <span class="adi-spacer"></span>
                    <Button
                        size=ButtonSize::Small
                        variant=ButtonVariant::Ghost
                        on:click=move |_| facts.open.set(String::new())
                    >
                        "close"
                    </Button>
                </div>
                <div class="adi-panel__body">
                    <FactHistory fact=fact changes=changes/>
                </div>
            </section>
        })}
    }
    .into_any()
}

/// The open transaction, if there is one.
fn tx_of(facts: FactsConsole) -> Option<TxView> {
    facts.data.get().and_then(|d| d.tx)
}

// ---- the fixture -------------------------------------------------------------------------
// Real sentences off the measured base. The pair at 0.886 is the one the whole design turns
// on: merging it on similarity would have deleted the Ukraine carve-out, which is why nothing
// on this screen resolves in bulk.

fn fixture() -> FactsData {
    let cis = Fact::new("f091", "The company supports all countries except the CIS.")
        .by("igor", "agent:chat@1")
        .at(2);
    let ukraine = Fact::new("f104", "Within the CIS, the company supports Ukraine.")
        .by("igor", "agent:chat@1");
    let china_market = Fact::new(
        "f044",
        "China is one of the operator's main target markets.",
    )
    .by("igor", "agent:extractor@1");
    let china_unsure = Fact::new(
        "f038",
        "The company is not sure it can enter the China market.",
    )
    .by("igor", "agent:extractor@1");
    let licence = Fact::new(
        "f052",
        "The enterprise licence would include user management.",
    )
    .by("igor", "agent:extractor@1");
    let no_users = Fact::new(
        "f061",
        "The company decided not to add user management to Mesh.",
    )
    .by("igor", "agent:extractor@1");
    let plan = Fact::new(
        "a012",
        "Market entry plan: skip China for now, open the EU first.",
    )
    .by("igor", "agent:planner@2")
    .at(3)
    .kind(NodeKind::Artifact);
    let composed = Fact::new(
        "c003",
        "The company supports every country outside the CIS, and Ukraine inside it.",
    )
    .by("igor", "agent:composer@1")
    .at(4)
    .kind(NodeKind::Composed);

    let pairs = vec![
        Pair::new(
            "p1",
            0.886,
            Relation::Narrows,
            PairSide::staged(cis.clone()),
            PairSide::base(ukraine.clone()),
        )
        .reason("one excludes the CIS, the other carves Ukraine out of it"),
        Pair::new(
            "p2",
            0.821,
            Relation::Duplicate,
            PairSide::staged(
                Fact::new("n#7", "China is a great market.").by("igor", "agent:chat@1"),
            ),
            PairSide::base(china_market.clone()),
        )
        .reason("both name China as a market the company wants"),
        Pair::new(
            "p3",
            0.712,
            Relation::Controversy,
            PairSide::staged(
                Fact::new("n#9", "The company can support China after all.")
                    .by("igor", "agent:chat@1"),
            ),
            PairSide::base(china_unsure.clone()),
        )
        .reason("one asserts capability, the other doubts it"),
        // The finding that moved the floor from 0.60 to 0.55: a real contradiction six notes
        // apart, sitting at 0.551. It is in the fixture because it is the case that proves a
        // queue cut off at the top would have missed it.
        Pair::new(
            "p4",
            0.551,
            Relation::Controversy,
            PairSide::staged(licence.clone()),
            PairSide::base(no_users.clone()),
        )
        .reason("one plans user management, the other says it was dropped"),
        // Both sides staged, so `drop` has no base fact to point at and the card has to ask.
        Pair::new(
            "p5",
            0.664,
            Relation::Duplicate,
            PairSide::staged(
                Fact::new("n#12", "The company was incorporated in Delaware.")
                    .by("igor", "agent:chat@1"),
            ),
            PairSide::staged(
                Fact::new("n#13", "The company is a Delaware C-corp.").by("igor", "agent:chat@1"),
            ),
        )
        .reason("the same incorporation, said twice"),
    ];

    FactsData {
        tx: Some(TxView {
            id: "tx_7f3a91".to_string(),
            staged: 12,
            pairs,
            truncated: Some(Truncated::new(214, 0.601)),
        }),
        stale: vec![Stale::new(
            plan.clone(),
            vec![
                Moved::new(
                    "f038",
                    "The company is not sure it can enter the China market.",
                    "The company can support China after all.",
                )
                .versions(1, 2),
            ],
        )],
        facts: vec![
            cis.clone(),
            ukraine,
            composed,
            china_market,
            china_unsure,
            licence,
            no_users,
            plan,
        ],
        history: vec![(
            cis.id.clone(),
            vec![
                Change::rewritten(
                    2,
                    Verdict::Supersede,
                    "igor",
                    "The company supports all countries.",
                    "The company supports all countries except the CIS.",
                ),
                Change::created("agent:chat@1", "The company supports all countries."),
            ],
        )],
    }
}
