# Knowledge base — design notes (experiment)

Status: exploratory. Nothing here is implemented. Decisions are the operator's; the rationale
is recorded so a later reader can tell a choice from an accident.

## The problem

Everything an AI produces is a *prediction*. Everything a human states or explicitly approves
is *ground truth*. When ground truth changes, every prediction derived from it must become
known-stale — automatically, and without asking a model to re-read the world.

## The node

A node is three things:

```
author   — whose meaning this is. Usually the human who said it.
creator  — who physically wrote the record. Usually an agent.
fact     — one plain sentence.
```

That is the whole schema.

The fact is written the way a person would say it to someone who was not there. "We do not
support the CIS." "Igor does not like fish." "A company is already incorporated in the USA."

An agent transcribing a human's sentence is `author: human, creator: agent`, and the record is
ground truth. An agent inventing a sentence is `author: agent`, and the record is a prediction.

### What is deliberately NOT in the node

Earlier drafts split the fact into `subject / predicate / value` and pulled negation out into a
separate polarity field, so that "not" would stop being invisible to the embedder.

**Both are gone.** The operator rejected them and the experiment agrees with him: on the same
corpus the flat sentence matched the decomposed version on extraction (93% vs ~92%) and on pair
ranking (all related pairs inside the top 125 of 528, against 139), with none of the machinery.
See `RESULTS.md` §8.

Negation belongs inside the sentence, where the person put it. A base that stores "Igor likes
fish, polarity: minus" has made a person's plain sentence into a puzzle for no gain.

No subject, no predicate, no polarity, no arity, no taxonomy, no categories, no status enum.
A fact is a sentence.

## How facts relate

### Pairs are ranked, not gated

Two facts are compared by the cosine of their embeddings. There is no threshold: pairs are
ranked by similarity and the reviewer works the queue from the top.

An earlier draft gated on `subject` AND `predicate` both being above a threshold, which let one
weak field veto a pair however strong the rest was, and ignored the content entirely. With a
single sentence there is one number and nothing to gate.

The embedder matters more than anything else here. On identical input the same 14 related
pairs land inside the top 125 of 528 (`nomic-embed-text`), the top 166 (Google
`embeddinggemma`, document prefix), or the top 465 (`mxbai-embed-large`). For a prefix-trained
model the prefix moves the result as much as the model does — `embeddinggemma` ranges 166 to
423 across its own prefixes. Model and prefix are real design parameters, not details
(`RESULTS.md` §8). Paying does not fix it: every Google Vertex model tested, including
`gemini-embedding-001` at 3072 dimensions, ranked worse than free local `nomic-embed-text`,
because it compresses every pair into a narrow high band and ranking needs spread.

### The machine surfaces. The reviewer decides.

The machine ranks pairs. That is all it does. It does not label a pair a contradiction.

An earlier draft promised a mechanical label — same value, opposite polarity → contradiction.
Experiment 1 killed it: the rule fired only when two claims were worded identically and stayed
silent on everything else, while pairs it would call contradictions and pairs it would call
agreement sat six thousandths of cosine apart (`RESULTS.md` §3).

The verdict is always confirmed, never inferred:

- `merge` — one fact proposed in place of two
- `coexist` — both stand; **this is confirmed too**, not assumed
- `supersede` — one replaces the other
- `review` — unresolved, stays open

Confirming co-existence matters as much as confirming a merge: it is what turns "we have two
notes" into "we know these two are both true".

**Every confirmation carries the identity of its confirmer** — a human id, or an agent id plus
its version — so any decision can be found and revisited.

A recorded verdict is permanent; the same pair is never asked twice.

### Merging is a later cycle, not an extraction problem

An extractor that writes three sentences where one would do is not making a mistake. The three
are near each other, so they surface as a pair, and the merge cycle proposes one fact in their
place. Extraction does not need to get granularity right.

## The cut-off: a floor, not a classifier

Measured on the live base — 114 facts from 30 dictated notes, all 6441 pairs classified
(`RESULTS.md` §9). **The floor is 0.55.**

The distribution is near-normal around a fat middle: half of all pairs sit between 0.44 and
0.56, the median is 0.491, and the ceiling is 0.898 — `nomic-embed-text` never says 0.95 about
anything, so the top of the scale is simply unused. 0.55 sits at about the 73rd percentile.

**The floor spends compute, not attention.** That is why it can be generous. The classifier
stands between the floor and the human, and it filters hard:

| floor | pairs the classifier reads | flags reaching a human | local compute |
|---|---|---|---|
| 0.60 | 955 | 98 | ~7 min |
| **0.55** | **1768** | **109** | **~13 min** |
| 0.50 | 2925 | 113 | ~22 min |

Doubling the search doubles machine time and adds eleven items to a person's queue. On a local
model that time is free, so the only real question is whether the extra flags are worth reading.

**They are, narrowly.** Of the 11 new flags in the 0.55–0.60 band, roughly 4 are genuine — the
sharpest being "the enterprise license would include user management" against "we decided not to
add user management to Mesh", a real contradiction six notes apart sitting at **0.551**. An
earlier pass claimed every genuine finding lived above 0.631; that claim was wrong, and this pair
is why. The other 7 are the familiar low-similarity noise, where the classifier's stated reason
describes facts that are not in the pair it was given.

Below 0.50 the return collapses: 1157 more pairs bought 4 more flags, none of which survived
reading. That is where the floor stops being generous and starts being pointless.

### Above the floor, nothing is decided by the number

A high cosine is not a duplicate. Above 0.80 the base holds **10 controversies against 8
duplicates**, and the ceilings are indistinguishable — 0.898 for a duplicate, 0.886 for a
controversy, 0.898 for an `independent` pair. Duplicates skew high only because there are 11 of
them in 6441 pairs.

The case that settles it — rank 6 overall, cosine 0.886:

```
The company supports all countries except the CIS.
Within the CIS, the company supports Ukraine.
```

Merging on similarity would erase the Ukraine carve-out. And the classifier mislabels at the top
too: the second-ranked pair in the whole base (0.898), "China is one of the main target markets"
against "China is a great market", came back `independent`.

The three relation types also sit in different bands, which is worth knowing when building the
queue: `duplicate` only appears above 0.80, `narrows` peaks at 0.70–0.75, and `controversy` peaks
lower, at 0.65–0.70. A reviewer working strictly top-down meets merges first and qualifications
second.

### The number belongs to the embedder, not to this design

0.60 is a property of `nomic-embed-text` and nothing else. `gemini-embedding-001` puts every
pair in the base above 0.645, so the same threshold there discards nothing at all
(`RESULTS.md` §8). Swap the embedder and the floor must be measured again, from scratch.

Recalibration is the same procedure every time: classify every pair once, find the lowest cosine
at which a genuine finding still appears, and set the floor below it. It is a day of compute,
not a judgement call.

And it is calibrated on one corpus, one speaker, one language. Treat 0.60 as a starting point
with a known provenance, not as a constant.

## Staleness is mechanical, never semantic

A derived node records the ids of the sources it was built from, and — on each edge — the exact
version those sources were at. Recompute; if a version no longer matches, the node is stale.
Transitively, instantly, with no model in the loop.

A model is consulted only afterwards, and only about nodes the mechanical pass already flagged:
"does this edit actually affect this text?"

### The graph

Two tables (`graph.sql`, working implementation in `facts`, worked example in `demo.py`):

```sql
actors(id, name, kind)                       -- every person and agent, spelled out once
nodes(id, fact, author, creator, version, updated_at, kind)   -- author/creator are actor ids
edges(src, dst, src_version, created_at)     -- dst was derived from src, at that version of src
```

Identities are interned rather than inlined. One operator and a handful of agents produce a whole
base, so the same few strings would otherwise repeat on every row — twice on `nodes`, again on
`history`. Measured on 200,000 facts with five distinct identities: 33.3 MB inlined against
28.4 MB by reference, **15% of the file for a table with five rows in it**. Read facts through
the `facts_v` view, which joins the names back.

`version` starts at 1 and is bumped on every edit. `src_version` is the source's `version` at the
moment the derived node was built. That is the entire mechanism.

Direct staleness is one join and an integer comparison:

```sql
SELECT e.dst FROM edges e JOIN nodes s ON s.id = e.src
WHERE  e.src_version <> s.version
```

Transitive staleness is that seeded into a recursive CTE. Regenerating a derived node means
bringing its incoming edges up to the sources' current versions — nothing else in the graph
moves, so refreshing one artifact never disturbs its siblings.

### Why a counter, and not a timestamp or a hash

Two earlier drafts were worse, and the second one was dangerous.

The first stored a composite stamp, `"<src id>_<src updated_at>"`. The id half was redundant —
the edge already carries `src` — and it cost a string concatenation per row on every check:
81 ms against 69 ms over 180k edges, and 9% more file.

The second kept the wall-clock timestamp alone. That can fail **silently**, and did: the first
run of `demo.py` wrote a fact and edited it inside the same millisecond, `updated_at` never
moved, and the edit was invisible to every dependent with no error anywhere. Milliseconds are
fine for a human typing and are not fine for an agent writing in a loop.

So it is a plain per-node counter. Monotonic by construction, unable to collide with itself, and
independent of any clock — five edits inside one millisecond take `version` from 1 to 6 and every
dependent goes stale. `updated_at` survives only so a human can see when something last moved;
nothing compares it, and it may be as coarse as you like.

Deliberately not a hash, either. Nothing here is adversarial, collisions are not a threat model,
and an integer you can read beats a digest you cannot.

### Cost

100,000 nodes and 180,000 edges in a 21 MB SQLite file. Editing one fact is a single `version = version + 1`. The full
stale sweep — direct or transitive over the whole graph — takes **69 ms**, and takes the same
69 ms whether nothing is stale or 63 nodes are. It can simply be run after every write.

## Hierarchy is a fact, not a structure

There is no tree. "The hero section is part of the main landing page" is an ordinary fact. Flat
records, related by meaning.

## No spans, no version history

A fact records which notes it came from, and is **edited in place**. No copy-on-write, no chain
of superseded revisions — that multiplies records without bound for a reader who almost never
comes. History is explicitly out of scope: restore a backup.

## Facts must stand alone

A fact is read months later by someone who does not have the note it came from. "I would not
want that direction" records nothing.

The important half: a rejection is still a fact. "I would not want us to turn into ETN" names
ETN, so it is recordable. Only a fact whose referent lives in *another* note is not.

Two things the experiment showed about getting this (`RESULTS.md` §5, §8): a prompt rule that
orders the model to resolve every pointer is not the mechanism — it drops real facts. But
giving the extractor the previous notes as read-only context does work, and it is what lifts
recall from 87% to 93%: a two-word note like "Да, лицензия на компанию" is unrecordable alone
and perfectly clear after the question it answers.

## Gaps are accepted

Some pairs will never surface — usually because the operator framed the same real subject two
different ways in two notes. `golden-flat.json` carries these as `accepted_gaps`.

This is a decision, not a defect. A human, or a verifier agent sweeping the base, notices later
that two facts disagree and marks the link. The base does not promise to have found every
contradiction the moment it was written; it promises never to lose a fact, and to surface what
it can.

## Confirmation load goes to a verifier

Experiment 1 measured a queue that grows with the base. The operator does not work it — a
verifier agent does, and its confirmations carry its identity and version. Only what the
verifier cannot settle reaches a human.

Deferred, not solved: nothing yet caps the queue's growth.

## Measured, and deliberately not built yet

Three things were measured on the live base and then set aside. The numbers are here so the
decision can be revisited without re-running the work.

**A two-tier floor.** A high floor at insert time for a fast answer, a low floor in a background
sweep for completeness. Inserting 30 facts into a base of 84 is 2955 candidate pairs, of which 48
are actionable:

| floor | pairs | classify time | actionable caught |
|---|---|---|---|
| 0.80 | 20 | 10 s | 13 / 48 |
| 0.75 | 33 | 16 s | 17 / 48 |
| 0.65 | 114 | 57 s | 30 / 48 |
| **0.55 (chosen)** | **467** | **234 s** | **36 / 48** |
| 0.50 | 905 | 452 s | 40 / 48 |

The fast pass catches a minority — 17 of 48 at 0.75 — so its value would be "don't let an agent
write something obviously contradictory unnoticed", not completeness. Deferred: 234 seconds is
acceptable for now, and one floor is simpler than two.

**Top-K neighbours instead of a floor.** A threshold does not scale: 0.55 admits 27% of all
pairs, so a new fact is compared against a quarter of the base and cost grows linearly with it —
31 pairs per fact at 114 facts, ~2740 at 10,000. Top-K is constant at any base size: K=20 catches
108 of 124 actionable pairs at a fixed 100 pairs per note. **This is the one that will have to be
built** — not because the floor is wrong, but because it stops working somewhere between here and
a base ten times this size.

**Splitting facts into atoms in the background.** Attractive, and it has a measured cost: terse
atomic sentences score lower than verbose ones on an identical relation, so splitting hides facts
from the base's own search. Composition restores it — against a broad probe the composed sentence
scored 0.715 where the best atom managed 0.677 — but against a *precise* probe the atom wins
(0.883 against 0.853). Neither layer is redundant, so splitting would require indexing both, and
that is the complexity being deferred.

**Composition was built and then removed.** A `compose` verdict derived a combined node from the
atoms by ordinary edges, and it worked. It came out because it added a concept the caller has to
understand for a payoff that only appears once the base is systematically splitting facts — which
is itself deferred. The measurements above are kept because they are what a future attempt needs;
the mechanism is not.

## Open questions

- Half of the extracted facts start with a template subject ("The operator…", "The company…").
  That is 50% of every sentence being identical boilerplate, which inflates every pairwise
  similarity. Worth removing at extraction.
- How far the fact sentence can carry before something structured is genuinely needed.
- Whether a better embedder closes the remaining pair-ranking gap on its own.
