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

Merging on similarity would erase the Ukraine carve-out. Selection bounds the *search* — the
closest twenty and no more; it decides nothing.

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

### Provenance and checking are the same operation

An agent that draws a conclusion from what a person said needs two things at once: the conclusion
**linked** to the facts it was built on, so it goes stale when they move, and the conclusion
**checked** against everything already in the base, so it cannot quietly contradict something.
Those are not two features. They are one, and `--from` is how you ask for it:

```
$ adi-mono facts derive --from f_1a02cbfe28e_2 \
    --fact "Market entry plan: skip China for now." --creator agent:planner@1

tx_1a02cc00a57  1 staged, 1 to decide

[p0] 0.793  narrows
  new   #0   Market entry plan: skip China for now.
  base  f_1a02cbfe28e_2 We are not sure we can enter the China market.
  why        Uncertainty supports skipping; compatible

decide each, then commit:
  facts tx resolve tx_1a02cc00a57 <p> --verdict coexist|merge|supersede|drop --confirmer <who>
```

A derived node goes through **exactly the transaction a stated fact does** — staged, ranked
against the base, refused until every pair is ruled on. `derive` is `add --from … --kind
artifact` with the sentence given as a flag rather than on stdin, and nothing more. `--from` is
equally available on `add`: repeatable, applying to the whole batch, and taking either a
committed fact id or `#N` for a fact staged in the same call.

```
$ adi-mono facts tx resolve tx_1a02cc00a57 0 --verdict coexist --confirmer igor
$ adi-mono facts tx commit tx_1a02cc00a57
committed 1 new, linked to 1 source(s)
  d_1a02cc00a57_0  Market entry plan: skip China for now.

$ adi-mono facts stale
everything is up to date
```

Each edge stores the exact `version` its source was at **when the batch committed**, not when it
was staged. A source that moved while the caller was deciding must not be recorded at the version
it had when they started typing, or the new node would be born claiming to be current against
text it never saw.

Now a reversal arrives:

```
$ echo "We can support China after all." | adi-mono facts add --author igor --creator agent:chat@1
tx_1a02cc00a73  1 staged, 3 to decide

[p0] 0.734  narrows
  new   #0   We can support China after all.
  base  f_1a02cbfe28e_2 We are not sure we can enter the China market.
  why        Support possible but entry uncertain implies hesitation

[p1] 0.657  controversy
  new   #0   We can support China after all.
  base  d_1a02cc00a57_0 Market entry plan: skip China for now.
  why        Supporting contradicts skipping market entry for China
```

`p1` is worth pausing on: **a derived artifact is a node, so it gets checked like everything
else.** Nobody designed that; it falls out of the plan being in the same table as the facts.

Rule `p0` as a supersede keeping the incoming side, and:

```
$ adi-mono facts tx commit tx_1a02cc00a73
committed 0 new, rewrote 1 in place (f_1a02cbfe28e_2), dropped 1

$ adi-mono facts stale
d_1a02cc00a57_0  Market entry plan: skip China for now.
    out of date because f_1a02cbfe28e_2 changed
```

No duplicate row exists, the base fact is at v2 with the new text, and the plan is out of date
with the fact that reversed it named as the cause. Nothing had to be remembered by hand.

**There is exactly one door into the base, and this is why.** `derive` briefly wrote a node and
its edges straight in — no transaction, no neighbour scan, no pair — and that was a hole in the
whole design: this same conclusion, drawn from this same fact, could contradict something already
recorded and land beside it in silence. Nothing errored; the base just quietly held both. So
`--from` became an argument to staging rather than a command of its own, and the library method
that wrote directly is gone. A source that does not exist, or that a verdict in the same
transaction threw away, is an error that names it:

```
$ adi-mono facts derive --from f_nope --fact "A conclusion."
error: no such fact: f_nope
```

An edge quietly not written is a derived node that never goes stale — the one outcome this design
exists to prevent, arriving as silence rather than as a message.

`adi-mono facts refresh d_1a02cc00a57_0` says the plan has been regenerated: it brings that node's
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

## Reading the base

**`search` is the command to run before `add`, not after.**

```
$ adi-mono facts search "pricing in China" --top 5
0.788  f_1a02d59d0f8_0  China pricing is set per seat, not per workspace.
0.682  f_1a02d59d0f8_1  We are not sure we can enter the China market.
0.363  f_1a02d59d0f8_2  The office is in Warsaw.
```

Everything you add is compared against the whole base, and every near pair comes back to be ruled
on by hand. Asking what the base already knows *before* writing is the review queue reduced at the
source rather than worked through afterwards — which is worth more than any amount of skill at
working it. The first agent to use this tool had no way to ask, so every fact went in blind and it
burned four aborted transactions on the consequences.

**Nothing is cut for scoring low,** here or anywhere else. Every fact is ranked, the top `--top`
come back, and the scores travel with them — the 0.363 line above is a weak match shown honestly
rather than hidden. An answer of "nothing found" about a base that plainly holds something closest
is not one a caller can act on. `near <id>` is the same, starting from an id you already have
instead of from words.

```
$ adi-mono facts list --limit 3
f_1a02d59d0f8_0    v1   fact      China pricing is set per seat, not per workspace.
f_1a02d59d0f8_1    v1   fact      We are not sure we can enter the China market.
d_1a02d292fdb_0    v1   artifact  Market entry plan: skip China for now.
```

`list` is everything, most recently changed first. Between them these are what stop a caller
dropping to `sqlite3` against the store — which the first agent did, and which is a missing
command rather than resourcefulness.

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

## Neighbour selection: top-K, and the floor that used to be here

Every fact you add is compared against its **20 nearest neighbours** and nothing else. A pair
surfaces when *either* side holds the other in its top K, and nothing is ever discarded for
scoring low.

```
$ ADI_FACTS_TOP_K=30 adi-mono facts add …
```

**The symmetry is load-bearing, not tidiness.** A fact in a sparse neighbourhood keeps a busy fact
in its own top K while the busy one, surrounded by closer things, does not reciprocate. Selecting
on one direction only would lose exactly those pairs — and the measured recall assumes both
directions.

**K = 20 is the knee**, measured on the 114-fact base with 124 actionable pairs: K=10 caught 96,
K=20 caught 108 (87%), K=30 caught 112. Thirty buys three points for half again the work.

### Why there is no similarity floor

There was one, at 0.55, for most of this design's life. It is gone — removed rather than defaulted
to zero, so it cannot come back as a mystery constant. The reasoning is worth keeping, because a
threshold is the obvious tool here and the next person to reach for it should know it was tried.

It was doing neither of the two jobs it appeared to do.

**It was not bounding cost.** A threshold admits a roughly constant *fraction* of a base, so what
it costs grows with the base. On the live 97-fact `project:adi/business` base, one inserted fact
drew 43 pairs above 0.55; another drew **76 of 96 — 79% of everything there**. At a thousand facts
that is hundreds of pairs per insert. K is constant at any size, which is the property the queue
actually needs.

**It was not judging quality.** What filters is the classifier: everything selected goes to it, it
answers `independent` for the weak ones, and only what it flags reaches a person. That was the
argument for *lowering* the floor to 0.55 in the first place — it spends machine time, not
attention — and it cuts the other way too. If the classifier is what filters, a floor on top of it
was not filtering; it was declining to look.

**And it would not hold still.** 0.55 was measured with `nomic-embed-text` on one corpus. On
`jina-embeddings-v2-base-en` it admitted 100% of the same fixture. On a real base it admitted
between 3% and 79% depending on which fact was being inserted. A number that has to be re-measured
whenever the model or the corpus changes, and that nobody can set correctly without redoing §9's
day of compute, is a trap rather than a setting.

The scores are still shown everywhere they were. They inform a reader; they gate nothing.

### What is still measured, and what the fixture cannot tell you

```bash
$ cargo test -p adi-facts --lib -- --ignored --nocapture top_k_recall
nomic-embed-text on golden-flat: 14 labelled pairs, 528 pairs in all; deepest labelled at rank 125
  K=5   14/14 labelled caught, 108 of 528 pairs selected (20%)
  K=10  14/14 labelled caught, 201 of 528 pairs selected (38%)
  K=20  14/14 labelled caught, 393 of 528 pairs selected (74%)
  K=30  14/14 labelled caught, 517 of 528 pairs selected (98%)
```

**Deepest labelled pair at rank 125 of 528 is `RESULTS.md` §8's published number, to the rank** —
which is the thing worth asserting, because a drift there would mean the vectors this crate
compares are not the vectors the design was measured with.

The K column, read honestly, cannot choose K. Thirty-three facts is smaller than the regime top-K
exists for: K=20 already reaches most of that base, so every K from 5 up catches all fourteen and
what actually varies is cost. The knee that picked 20 was measured on the 114-fact live base,
which is not in this tree. Two things still invalidate all of it, and both are one environment
variable away: a different embedder (`ADI_FACTS_EMBED`), and a different extraction prompt, which
changes how verbosely facts are written and so what they score against each other.

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
*Every number in this design was measured against `nomic-embed-text`*: the recall table, the
band structure where `duplicate` sits around 0.82 and `controversy` around 0.67, and the ranking
that top-K reads. A different embedder does not shift those numbers, it invalidates them. The experiment
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
ADI_FACTS_EMBED=nomic-embed-text          # changing this invalidates every measured number
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
sweep was 6441 pairs in 53 minutes at zero cost. A hosted model would make review a budget
decision instead of a compute one.

The **extraction** prompt is verbatim from the prototype. Its wording was iterated against a
hand-labelled corpus, and changing it means re-measuring just as surely as changing the embedder
would — it changes how verbosely facts are written, and therefore what they score against each
other.

The **classification** prompt is the prototype's plus two changes, and both were forced by a real
run. That does *not* touch neighbour selection: a classifier prompt moves no cosine.

### `num_predict` — do not tidy this away

`options` carries an explicit `num_predict`, and it must stay there. Without it ollama applies its
own output cap; a full batch of 60 pairs needs roughly 1,800 tokens of JSON, so the array arrives
**truncated** — deterministically, for any input long enough to reach the cap. The first agent to
use this tool hit it four times in one run, aborted four transactions over it, and reproduced it
byte for byte. It costs nothing on a local model. It is exactly the kind of line somebody removes
while tidying an options map.

The second half of that fix is in the parser. A truncated response is a well-formed prefix and a
broken tail, and handing the whole span to a JSON parser loses **every** judgement in it — 59 good
verdicts thrown away because the sixtieth was cut mid-word, surfacing as "the classifier could not
be reached" and 40 unclassified pairs. So the parser now walks the response and takes each
complete object: a malformed tail costs the pairs in the tail and nothing else, and what cannot be
recovered stays `unclassified`, which already reaches the reviewer.

### The classifier is shown who said each side

Each side arrives labelled — `said by igor, written by agent:chat@1 [fact]` — because judged on
wording alone the classifier called a person's statement and an agent's conclusion drawn from it a
`duplicate`, at cosine 0.954. It could not have done better; it was never shown who said either
sentence. A `merge` on that verdict deletes what somebody actually said.

Where the rule sits was measured rather than guessed. As its own paragraph after the verdict list,
the model ignored it. Moved *into the definition of `duplicate`* — read at the moment the verdict
is chosen — the pair came back `narrows`. And a prohibition alone did not hold either: the rule
only stuck once it also said what to answer instead.

**Even so, the prompt is the belt and not the brace.** The same pair came back `narrows` alone and
`duplicate` when a second pair shared the batch. A rule decidable from data already in hand should
not depend on a model obeying it, so `duplicate` across two different kinds of record is
downgraded to `narrows` in code. It downgrades a label and never drops a pair — both kinds reach
the reviewer — so nothing is hidden; what changes is the hint, and the hint is what sent a real
agent toward the wrong verdict.

**A classifier that cannot be reached is reported, never assumed.** The prototype caught every
error and defaulted the batch to `independent`, which means "nothing to do", which means an
unreachable model quietly emptied the review queue. Here those pairs come back marked
`unclassified`, they *do* reach the reviewer, and the reason travels with them:

```
tx_1a02d0  4 staged, 6 to decide
   [the classifier could not be reached: connection refused]
   every close pair is listed below, unread — decide them or abort.
```

The same rule governs the cap. If more pairs were selected than one transaction shows, the output
says how many were dropped and below what strength — a silent cut reads as "nothing else to see",
which is the one lie this interface must never tell.

The cap is **200**, and it is a backstop against a runaway queue rather than a workload control.
What bounds the queue in the normal case is K: a batch of `n` facts selects at most `n × K` pairs
however large the base is. It binds only when a batch is itself enormous — fifty facts at K=20 can
reach a thousand pairs — which is the runaway it exists to catch. Under the old similarity floor
it bound at *ordinary* size (one fact against a 97-fact base drew 43 pairs, another 76), which is
how a cap ended up quietly deciding what a reviewer saw. It no longer does.

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

## What an agent is told

An agent gets this tool as `adi-facts`, and what it is *told* about it is whatever
`adi-mono facts llm help` prints — `adi-tools` captures that and folds it into the agent's system
prompt on every turn. So the help is the interface: an agent that uses the tool wrongly is a help
that is wrong, not an agent that is careless.

```
$ adi-mono facts llm help
```

It is capped at 3,000 characters, which is the real constraint — every sentence displaces
something else the agent was going to be told, and the *tail* is what gets cut. So it carries
instructions and no rationale: what a fact sentence looks like (with two bad examples, which teach
faster than rules), **search before you write**, a three-command session showing that nothing
lands until `commit`, one line per verdict, `--from`, the difference between `--author` and
`--creator`, and one rule in capitals — **never guess a verdict**, with `tx abort` named as what
to do instead. An agent that reads `coexist` as "dismiss" will use it for everything, so that
line says it is a confirmation.

The ordering is the part that was learned rather than designed. `search` sits **before** the `add`
session, because an agent that asks what the base already knows will not stage what it already
holds — and that is worth more than any wording further down. Everything below it fits in
whatever budget is left, which is why `tx show` is not in the help at all: `resolve` reprints the
remaining pairs, so an agent finds it without being told.

Two things to know when changing it. The capture is **cached for an hour** and keyed on the shim
script's mtime — and the shim is a stable one-liner over `adi-mono`, so *rebuilding `adi-mono`
does not invalidate it*. Clear `~/.adi/mono/tools/.help/sys-facts` or wait out the TTL, or a test
run is graded on the old text. And `adi-mono` on an agent's PATH is the **release** binary, so a
debug build never reaches one.

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

**A two-tier floor** — a high threshold at insert time for a fast answer, a low one in a
background sweep for completeness. Moot now that there is no threshold at all, and it was already
weak: the fast pass caught a minority (17 of 48 actionable pairs at 0.75), so its value would have
been "don't let an agent write something obviously contradictory unnoticed", not completeness.

(**Top-K neighbours** was the other entry here, described as "the one that will have to be built".
It has been — see [neighbour selection](#neighbour-selection-top-k-and-the-floor-that-used-to-be-here).)

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
