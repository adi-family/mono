# The fact base

`adi-mono facts` keeps plain sentences somebody said, and a graph over them that makes anything
built on a changed fact go stale. `crates/adi-facts` is the library; `crates/adi-cli/src/facts.rs`
is the argv adapter. The design it implements is `experiment/knowledge-base/DESIGN.md`, and the
measurements behind every number in it are `RESULTS.md` next door.

A **fact** is one sentence, written the way a person would say it to somebody who was not there.
It carries who meant it and who wrote it down, and nothing else.

```
$ printf '%s\n' \
    "The company supports all countries except the CIS." \
    "Within the CIS, the company supports Ukraine." \
    "We are not sure we can enter the China market." \
  | adi-mono facts add --author igor --creator agent:chat@1

tx_1a02cbc2914  3 staged, 1 to decide

[p0] 0.886  controversy
  new   #0   The company supports all countries except the CIS.
  base  #1   Within the CIS, the company supports Ukraine.
  why        Contradiction on whether CIS is supported

decide each, then commit:
  facts tx resolve tx_1a02cbc2914 <p> --verdict coexist|merge|supersede|drop --confirmer <who>
  facts tx commit tx_1a02cbc2914
```

That is the whole shape of the tool. `add` reads stdin, **one fact per line** — send fifty at
once if you have fifty — stages them under a transaction, ranks each against everything already
in the base *and against its siblings*, and prints what a reviewer has to rule on. Nothing is
visible to the base until `commit`, and `commit` refuses while a pair is open.

```
$ adi-mono facts tx resolve tx_1a02cbc2914 0 --verdict coexist --confirmer igor
p0 -> coexist by igor
all decided.
  facts tx commit tx_1a02cbc2914

$ adi-mono facts tx commit tx_1a02cbc2914
committed 3 new
  f_1a02cbfe28e_0  The company supports all countries except the CIS.
  f_1a02cbfe28e_1  Within the CIS, the company supports Ukraine.
  f_1a02cbfe28e_2  We are not sure we can enter the China market.
```

`coexist` is a **decision, not a skip**. It is what turns "we have two notes" into "we know these
two are both true", and like every other verdict it records who made it.

## What a fact looks like

```
Good:  We do not support the CIS.
       A company is already incorporated in the USA.
       Our audience is founders who cannot afford to hire a team.

Bad:   that direction is fine        (a pointer nobody can resolve later)
       run the marketer again        (an instruction, not a fact)
       audience: solo founders       (a field, not a sentence)
```

Negation goes **inside** the sentence, where the person put it. An earlier design pulled it out
into a polarity field so that "not" would stop being invisible to the embedder; it was built,
measured, and dropped, along with a subject / predicate / value split. On the same corpus the
flat sentence matched the decomposed version on extraction (93% against ~92%) and beat it on pair
ranking, with none of the machinery. A base that stores "Igor likes fish, polarity: minus" has
made a person's plain sentence into a puzzle for no gain.

Resolve every referent from what you know now. A fact is read months later by somebody who does
not have the note it came from, so "I would not want that direction" records nothing — while "I
would not want us to turn into ETN" names ETN and is perfectly recordable. A rejection is still a
fact.

One fact per sentence: three markets are three facts. If the extractor writes three sentences
where one would do, that is not a mistake — the three are near each other, so they surface as a
pair and the merge cycle proposes one fact in their place.

## Nothing is ever merged automatically

The machine embeds, ranks, and asks a classifier which close pairs are worth a look. **It decides
nothing at any similarity**, and that is not caution, it is a measurement. Above 0.80 cosine the
live base held ten controversies against eight duplicates — more contradictions than merges, at
the very top — and the ceilings are indistinguishable: 0.898 for a duplicate, 0.886 for a
controversy, 0.898 for a pair that turned out to be about two different things.

The case that settles it is rank 6 of 6441:

```
The company supports all countries except the CIS.
Within the CIS, the company supports Ukraine.
```

Merging on similarity would erase the Ukraine carve-out. The floor bounds the *search*; it
decides nothing.

## The verdicts, and the one mechanism behind two of them

| verdict | what it does |
| --- | --- |
| `coexist` | nothing. Both land — and you are confirming that, not skipping it. |
| `drop` | the new fact never lands. The base was already right. |
| `merge --fact "…"` | one sentence replaces both. |
| `supersede --keep <id>` | the winner replaces the loser. |

**`merge` and `supersede` are one mechanism.** Both write the winning sentence into the *losing*
node **in place** and bump its version, so everything derived from it goes stale. Neither ever
leaves two rows. They differ only in where the winning sentence comes from — `merge` takes it
from `--fact`, `supersede` from whichever side won.

Superseding rewrites rather than deletes for a specific reason: dropping the old row cascades its
edges away, and everything derived from it becomes *orphaned* rather than *stale*. That is the
one outcome this whole design exists to prevent.

Three bugs lived in that mechanism in the prototype. All three were silent — the failure mode of
this tool is not a crash, it is a base that quietly holds the wrong thing — and all three are
fixed here and covered by tests:

- `merge` rewrote only the incoming fact and left the base one alive, so committing produced the
  merged sentence **and** the original it was meant to replace;
- when both sides of a pair were in the same incoming batch, `merge` and `supersede` did nothing
  at all and both landed;
- a typo in `--keep` was read as "the base side won", so the incoming fact was discarded without
  a word. Now:

```
$ adi-mono facts tx resolve tx_1a02cc00a73 0 --verdict supersede --keep f_typo --confirmer igor
error: --keep f_typo is neither side of p0. It must be #0 or f_1a02cbfe28e_2.
```

`#N` names a fact staged in this batch; a bare id names one already in the base.

## Staleness is mechanical, and never a timestamp

Record something built *on* facts, and the graph does the rest:

```
$ adi-mono facts derive --from f_1a02cbfe28e_2 \
    --fact "Market entry plan: skip China for now." --creator agent:planner@1
d_1a02cc00a57  derived from f_1a02cbfe28e_2

$ adi-mono facts stale
everything is up to date
```

Each edge stores the exact `version` its source was at when the derivation happened. Now a
reversal arrives:

```
$ echo "We can support China after all." | adi-mono facts add --author igor --creator agent:chat@1
tx_1a02cc00a73  1 staged, 3 to decide

[p0] 0.734  narrows
  new   #0   We can support China after all.
  base  f_1a02cbfe28e_2 We are not sure we can enter the China market.
  why        Support possible but entry uncertain implies hesitation

[p1] 0.657  controversy
  new   #0   We can support China after all.
  base  d_1a02cc00a57 Market entry plan: skip China for now.
  why        Supporting contradicts skipping market entry for China
```

`p1` is worth pausing on: **a derived artifact is a node, so it gets checked like everything
else.** Nobody designed that; it falls out of the plan being in the same table as the facts.

Rule `p0` as a supersede keeping the incoming side, and:

```
$ adi-mono facts tx commit tx_1a02cc00a73
committed 0 new, rewrote 1 in place (f_1a02cbfe28e_2), dropped 1

$ adi-mono facts stale
d_1a02cc00a57  Market entry plan: skip China for now.
    out of date because f_1a02cbfe28e_2 changed
```

No duplicate row exists, the base fact is at v2 with the new text, and the plan is out of date
with the fact that reversed it named as the cause. Nothing had to be remembered by hand.
`adi-mono facts refresh d_1a02cc00a57` says the plan has been regenerated: it brings that node's
incoming edges up to its sources' current versions and bumps its own, so anything built on *the
plan* goes stale in turn. Refreshing one artifact disturbs no sibling.

### Why a counter and not a timestamp

`version` is a plain per-node integer, bumped on every edit. `updated_at` exists so a human can
see when something last moved, and **nothing compares it**.

That is the single silent failure this design was rebuilt to remove. An earlier draft compared
wall-clock stamps; the prototype's first run wrote a fact and edited it inside the same
millisecond, `updated_at` never moved, and the edit was invisible to every dependent with no
error anywhere. Milliseconds are fine for a human typing and are not fine for an agent writing in
a loop. A counter is monotonic by construction, cannot collide with itself, and needs no clock:
five edits inside one millisecond take `version` from 1 to 6 and every dependent goes stale.

An even earlier draft stored a composite stamp, `"<src id>_<src updated_at>"`. The id half was
redundant — the edge already carries `src` — and it cost a string concatenation per row on every
check: 81 ms against 69 ms over 180k edges, and 9% more file.

It is deliberately not a hash either. Nothing here is adversarial, collisions are not a threat
model, and an integer you can read beats a digest you cannot.

## References from outside

A fact id gets written down where the base cannot see it — in a plan, in a comment, in another
agent's notes. The surprising part is what does *not* go wrong: because `merge` and `supersede`
rewrite the winner in place, a committed id is never destroyed and a reference never dangles.
What changes underneath it is the **meaning**. A dangling pointer announces itself; a pointer
whose target quietly changed does not.

So every change is logged, and `facts get` replays it. Give it the version you wrote down and it
checks itself:

```
$ adi-mono facts get f_1a02cbfe28e_2@1
f_1a02cbfe28e_2  v2  [fact]
  We can support China after all.
  said by igor, written by agent:chat@1

  STALE REFERENCE — written against v1, the fact is now v2.

what changed since:
  v2   supersede   by igor
        was: We are not sure we can enter the China market.
        now: We can support China after all.
```

Without a version it prints the whole log and says plainly that the id still resolves but no
longer means what it did. The tool never goes looking for references and has no opinion about
where they live: whoever holds one brings it here.

The log is append-only, and it is **not** the version history `DESIGN.md` declines to keep. It
records *decisions*, not revisions, and it exists to keep outside references honest rather than
to let anyone browse the past.

**One case does destroy a record: two facts merged inside the same batch.** The loser never
reaches the base, so it never gets an id anybody could have referenced. It is logged against the
survivor as `absorbed`, with its text, so searching the log for the wording somebody remembers
finds the id that swallowed it. The winner's own staged wording is *not* kept when `--fact`
replaces it — that is the design's accepted edge, not an oversight, and it is the one place a
sentence can leave no trace.

## Working the queue

```
$ adi-mono facts near f_1a02cbfe28e_0 --top 5
0.886  f_1a02cbfe28e_1  Within the CIS, the company supports Ukraine.
0.591  f_1a02cbfe28e_2  We can support China after all.
```

`near` is the queue around one fact, for a verifier agent to work through. The confirmation load
is deliberately not the operator's: a verifier agent works it, its confirmations carry its
identity and version (`--confirmer agent:verifier@3`), and only what it cannot settle reaches a
person. Nothing yet caps that queue's growth — that is a known gap, not a solved problem.

## The floor, and what measuring it again showed

The similarity floor is **0.55**, and unlike most numbers in a ported design it did not have to
be re-derived: it belongs to the embedder, and this crate embeds with the same
`nomic-embed-text` the whole calibration was measured on. `RESULTS.md` §9 arrived at it by
classifying all 6441 pairs of a 114-fact base, finding the lowest cosine at which a genuine
finding still appeared (0.551, a real contradiction six notes apart), and setting the floor just
below it.

The floor spends **compute, not attention** — the classifier stands between it and a person and
filters hard — which is why it can be generous. Dropping it from 0.60 to 0.55 doubled machine
time and added eleven items to a reviewer's queue, of which about four were real. Below 0.50 the
return collapses: 1157 more pairs bought 4 more flags, none of which survived reading.

### It reproduces

`golden-flat.json` is §8's fixture: 33 extracted facts, 528 pairs, 14 hand-labelled as related.
Re-run through this crate's embedder:

```bash
$ cargo test -p adi-facts --lib -- --ignored --nocapture the_floor_admits
nomic-embed-text on golden-flat: 14 of 14 labelled pairs found; deepest rank 125 of 528;
weakest labelled cosine 0.520
  median cosine over all 528 pairs: 0.456
  floor 0.50: 153 pairs above (29.0%), 14/14 labelled kept
  floor 0.55:  88 pairs above (16.7%), 12/14 labelled kept
  floor 0.60:  53 pairs above (10.0%), 12/14 labelled kept
  floor 0.65:  27 pairs above ( 5.1%),  8/14 labelled kept
```

**Deepest rank 125 of 528** is `RESULTS.md` §8's published number, to the rank. The median at
0.456 and the fat middle are §9's distribution. Whatever else is true, the vectors this crate
compares are the vectors the design was measured with — which is the thing a port can most
easily get wrong and least easily notice.

### And it shows something the design did not

12 of the 14 labelled pairs clear 0.55. The two that do not are not the same kind of loss:

- **`mkt-04` / `mkt-09` at 0.532** — *"India and the post-CIS countries are secondary markets to
  consider"* against *"We do not support the CIS"*, labelled `supersede`, and annotated in the
  fixture itself as **"the hard one"**: different predicates on paper, the same real-world claim.
  This is a genuine finding, and it is the very pair §9 held up as what flat facts recovered that
  the subject/predicate schema had recorded as unfindable.
- **`msh-01` / `msh-03` at 0.520** — labelled `coexist`, "unrelated predicates under one
  subject". Losing a `coexist` costs nothing actionable.

So the design's own procedure, applied to two corpora, gives two answers: 0.55 on the 114-fact
live base, about 0.50 on this 33-fact fixture. They disagree by 0.02 — at exactly the resolution
that decides whether a finding is seen. That is not a defect in either measurement; it is
evidence for the conclusion `DESIGN.md` already reached, that **top-K neighbours will have to
replace the threshold**, and it is the strongest single argument in the tree for building that.

**The floor is left at 0.55.** Fitting a threshold to whichever corpus was measured last is how a
calibration stops meaning anything, and the safe error on a floor is the generous one — too low
costs compute, too high loses findings silently. Anyone who wants the fixture's answer instead
can have it per process:

```
$ ADI_FACTS_FLOOR=0.50 adi-mono facts add …
```

Two things still invalidate it outright, and both are one environment variable away:

1. **A different embedder.** `ADI_FACTS_EMBED` changes the model, and with it every number on
   this page. The same fourteen pairs land inside the top 125 with `nomic-embed-text`, the top
   166 with `embeddinggemma`, and the top 465 with `mxbai-embed-large`; paying does not help,
   because every hosted model tried compresses the pairs into a narrow high band and ranking
   needs spread.
2. **A different extraction prompt.** The floor depends on how verbosely facts are written: the
   same relation moves from 0.583 to 0.886 across four phrasings, and adding a shared subject —
   "The company…" — is worth 0.11 on its own.

## The three levels

A fact base is addressed exactly as a knowledge base is, and by the same code — `BaseId`,
`Scope`, and `Reader` are `adi_knowledge`'s types, not lookalikes.

| id | who reads it | who writes it |
| --- | --- | --- |
| `global/<name>` | everyone | everyone |
| `project:<id>/<name>` | whoever is working in that project | same |
| `agent:<name>/<base>` | **every agent** | that agent alone |

`--base` is stated once, before the verb, like the identity flags. With none of them the base is
`global/default`, or whatever `ADI_FACTS_BASE` says.

```
$ adi-mono facts --base project:acme/default --as-agent solver stale
$ adi-mono facts bases
global/default                     global   4 fact(s)
project:acme/default               project  2 fact(s)
```

`--as-agent` / `--as-project` run a command as somebody in particular, and with neither they fall
back to the `ADI_AGENT` / `ADI_PROJECT` an agent run already carries. `--root` runs as the owner
of the store whatever the environment says, and is answered *before* the flags and the
environment — a run cannot unset a variable its launcher exported. As with `knowledge`, **these
levels are not a sandbox**: they organize facts and decide what a run reaches by default.

`facts add` is the only command that creates a base. Everything else refuses a base that is not
there, so a mistyped id is an error rather than an empty base answering "nothing here".

## The embedder is `nomic-embed-text`, and that is not a preference

Facts are embedded by **`nomic-embed-text`, served by the same local ollama as the classifier** —
not by the jina-embeddings-v2-base-code model on candle that `adi-indexer` and `adi-knowledge`
share.

This is the single most load-bearing choice in the crate, and it is not about prose versus code.
*Every threshold in this design was measured against `nomic-embed-text`*: the 0.55 floor, the
recall table, the band structure where `duplicate` sits around 0.82 and `controversy` around
0.67. A different embedder does not shift those numbers, it invalidates them. The experiment
tried several and the spread is enormous — the same fourteen related pairs inside the top 125 of
528 with this model, the top 465 with `mxbai-embed-large` — so the model is a design parameter,
not a detail to be settled by what the workspace already loads.

Reaching it over HTTP rather than in-process is the simplifying consequence, not a compromise:
the classifier was already an ollama client on the same host, so the embedder joins it there and
`adi-facts` carries **no model stack at all** — no candle, no weights, no download, and no
first-call pause while 550MB loads. The crate has no Cargo features because there is nothing to
opt out of.

`adi_facts::embed::OllamaEmbedder` implements `adi_indexer::embed::Embedder` — the workspace's
trait, not a lookalike — so a caller can inject any embedder the workspace has, and the tests
inject `HashEmbedder` and never touch a network.

```bash
ADI_FACTS_OLLAMA=http://127.0.0.1:11434   # moves the embedder and the classifier together
ADI_FACTS_EMBED=nomic-embed-text          # changing this invalidates the floor
```

**Vectors from two models must never be compared**, and nothing relies on remembering that: every
cached vector records the model that made it, and a row from any other model is treated as absent
and re-embedded. Rewriting a fact in place throws its cached vector away in the same transaction,
so a node can never be ranked by a sentence it no longer says.

One quiet benefit of embedding over HTTP: `/api/embeddings` takes one prompt per request, so a
fact's vector never depends on what else shared its batch. The candle path has the opposite
property — `jina_bert`'s forward takes no attention mask, so padding leaks into every shorter
text in a batch and two symbols' similarity depends on what they were indexed alongside (measured
at 0.507 against 0.573 for one pair). That is a real problem for the code index and is documented
and pinned by a test in `adi-indexer`; it does not reach facts.

## The classifier is injectable

Deciding whether a close pair is a `duplicate`, `narrows`, `independent`, or `controversy` is a
model's job, and which model is a deployment question — so it sits behind
`adi_facts::judge::Judge`, exactly as the embedder sits behind `Embedder`. The default talks to
the same local ollama the embedder does, through the same client, so `ADI_FACTS_OLLAMA` moves
both halves of the model work at once:

```bash
ADI_FACTS_JUDGE=qwen3.6   # the model the measurements were taken with
```

Local because the classifier reads roughly two pairs per inserted fact, and the full measured
sweep was 6441 pairs in 53 minutes at zero cost. A hosted model would make the floor a budget
decision instead of a compute one.

The two prompts — extraction and classification — are **verbatim** from the prototype. Their
wording was iterated against a hand-labelled corpus and every measurement in `RESULTS.md` was
taken with them; change the extraction prompt and the floor needs re-measuring just as surely as
if the embedder had changed.

**A classifier that cannot be reached is reported, never assumed.** The prototype caught every
error and defaulted the batch to `independent`, which means "nothing to do", which means an
unreachable model quietly emptied the review queue. Here those pairs come back marked
`unclassified`, they *do* reach the reviewer, and the reason travels with them:

```
tx_1a02d0  4 staged, 6 to decide
   [the classifier could not be reached: connection refused]
   every close pair is listed below, unread — decide them or abort.
```

The same rule governs the cap. If more pairs cleared the floor than one transaction shows, the
output says how many were dropped and below what strength — a silent cut reads as "nothing else
to see", which is the one lie this interface must never tell.

```
   [capped: 12 more pair(s) below 0.612 not examined]
```

`--text` is the one path that fails outright without a model: with nothing extracted there is
nothing to stage, and an empty transaction would read as "this note said nothing".

## Extraction: the caller emits facts, `--text` is the fallback

```
$ cat dictation.txt | adi-mono facts add --text --author igor --creator agent:chat@1
```

`--text` stores the raw note verbatim, extracts facts from it, and stages those. The note becomes
a node with an edge to every fact drawn from it, so editing the note makes those facts stale —
the same mechanism, with no special case for "came from prose".

But it is the fallback, not the default, and the reason is the one thing the experiment could not
fix by any other means. Extraction from raw dictated notes reaches ~93%, and every remaining
failure is **anaphora** — a fact whose referent lives in a different note. Handing the extractor
the two previous notes lifted recall from 87% to 93%, and *a caller in a live conversation has
strictly more context than that*: it has the whole thread, it knows what "that direction" meant,
and it is the only party that ever will. Moving extraction to the background throws away the only
information that solves the problem.

The source text is stored either way. Extraction will get better, and re-projecting facts from
the original is only possible if the original survived. A side effect worth naming: every
resolved transaction is a labelled training example — these facts, this pair, this verdict, this
confirmer — so the system produces its own training set as a by-product of being used.

## No JSON, anywhere

There is no `--json` flag on any subcommand, and that is deliberate rather than unfinished. The
reader is a language model; JSON costs it tokens to unwrap something a laid-out line already
said. The prototype had `--json` on everything and dropped all of it.

Every view ends with the command to run next, for the same reason — a caller that has to work out
its own next step from a data structure is a caller spending a round trip on nothing. `add` prints
the pairs needing a decision immediately: there is no envelope to parse and no second call to
discover the work.

## Deliberately not built

**Composition.** A `compose` verdict deriving a combined node from several atomic facts by
ordinary edges was built, worked, and was removed. It added a concept every caller has to
understand for a payoff that only appears once the base is systematically splitting facts into
atoms — which is itself deferred, because terse atomic sentences score *lower* than verbose ones
on an identical relation, so splitting would hide facts from the base's own search unless both
layers were indexed. Do not reintroduce it.

**Top-K neighbours instead of a floor.** A threshold does not scale: 0.55 admits about a quarter
of all pairs, so a new fact is compared against a quarter of the base and cost grows linearly with
it — 31 pairs per fact at 114 facts, ~2740 at 10,000. Top-K is constant at any base size, and
K=20 caught 108 of 124 actionable pairs at a fixed 100 pairs per note. **This is the one that will
have to be built** — not because the floor is wrong, but because it stops working somewhere
between here and a base ten times this size.

**A two-tier floor** — a high one at insert time for a fast answer, a low one in a background
sweep for completeness. The fast pass catches a minority (17 of 48 actionable pairs at 0.75), so
its value would be "don't let an agent write something obviously contradictory unnoticed", not
completeness. One floor is simpler than two.

**Gaps are accepted.** Some pairs will never surface, usually because the same real subject was
framed two different ways in two notes. That is a decision, not a defect: the base does not
promise to have found every contradiction the moment it was written. It promises never to lose a
fact, and to surface what it can.

## On disk

```
~/.adi/mono/facts/
  global/<base>/base.toml           # provider, description, timestamps
  global/<base>/facts.db            # nodes, edges, actors, staging, history, vectors
  projects/<project-id>/<base>/…
  agents/<agent-name>/<base>/…
```

One SQLite file per base, with the same WAL + `busy_timeout` settings as the rest of the platform
because agents, their tools, and a person at a terminal reach one base at once. The schema is
`crates/adi-facts/src/graph.sql`, ported from the prototype's and commented with the reason for
each shape.

Identities are **interned** in an `actors` table and referenced by integer from `nodes.author`,
`nodes.creator`, `history.confirmer`, `pending.confirmer`, `notes.author` and
`transactions.author`/`creator`. One operator and a handful of agents produce a whole base, so
the same few strings would otherwise repeat on every row twice over: measured on 200,000 facts
with five distinct identities, 33.3 MB inlined against 28.4 MB by reference — **15% of the file
for a table with five rows in it**. Read facts through the `facts_v` view, which joins the names
back.

Unlike `adi-knowledge`, storage here is **not** behind a provider trait. There the trait earns its
keep because a base is a bag of notes and a hosted vector store could hold one just as well. Here
the schema *is* the design — the version counter, the edge stamp, and a recursive CTE that answers
transitive staleness in one query are what the whole thing is for — and a trait over them would be
thirty methods with one implementation and no second one in sight.
