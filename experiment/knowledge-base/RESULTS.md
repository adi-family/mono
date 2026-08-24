# Experiment 1 — results

Run 1, 2026-08-22. Everything in `run-1/`: prompts, scripts, raw output.

- **Extractor:** `claude -p` (Claude Code 2.1.239), model `sonnet`, one call per note, custom
  system prompt (`run-1/extract_prompt.txt`), no tools, no session persistence.
- **Scorer:** a second `claude -p` pass matching extracted claims against `golden.json` by
  meaning, per note.
- **Embeddings:** local `mxbai-embed-large` via ollama, 1024-dim, cosine on L2-normalised
  vectors.
- **Two conditions:** `solo` (note alone) and `ctx` (note plus the two previous notes,
  marked as reference-only).
- **Cost:** $1.45 (solo) + $1.61 (ctx) for 30 notes each; ~3 min wall-clock at 6 concurrent.

## 1. Extraction — it works (this section is superseded by §8)

| | claims | recall vs golden | polarity errors | negation leaked into value/predicate |
|---|---|---|---|---|
| solo | 163 | **28/33 = 85%** | 0 | 4 / 163 |
| ctx  | 156 | 26/33 = 79% | 0 | **0 / 156** |

**Polarity survives.** This was the design's single point of failure — a missed negation
turns a contradiction into a silent co-existence — and there were **zero polarity errors in
59 matched claims across both conditions**. Lifting negation out of the text into a field
is a thing a small model can actually do: 22 of 163 claims came back negative-polarity, with
the value stated positively as instructed.

**Adding conversational context did not help — it hurt.** Recall fell 85% → 79%. Prior
notes pulled the extractor toward summarising the thread instead of the note in front of it,
and it split compound claims more aggressively. The context condition was built specifically
to fix the anaphora problem below, and it did not fix it.

### What was missed, and why it matters

Three of the five `solo` misses are granularity, not loss — the extractor split one
reference claim into two or more atoms (`forks allowed, license stays ours` became two
claims; the market list became five). A knowledge base does not need 1:1, so these are
scoring artefacts, arguably improvements.

Two are real, and one of them is the important one:

- **`aud-09` — the rejected audience. Missed in BOTH conditions.** This is the flagship case:
  note 42 proposes resellers as an audience, note 44 rejects them. Note 44 never says
  "resellers" — it says "хороший кейс… не хотелось бы превращаться в ETN". The extractor
  produced `product identity | becoming like ETN | -`, which is true but unaddressed: it
  will never be found by anyone looking up the audience. **Even with note 42 sitting in the
  context window, the referent was not resolved.**
- **`aud-03` — "solo developers" as an audience.** Buried mid-sentence in the 2.5 KB
  four-topic note 28, in a subordinate clause ("откуда фаундер получает информацию или
  соло-разраб").

**Finding: extraction is solid on what a note *says* and unreliable on what a note *refers
to*.** Anaphora is the failure mode, and naive context does not fix it.

## 2. Pair detection — the gate misses pairs that a reviewer would want

33 claims produce 528 possible pairs. Gate them on subject and predicate proximity:

| threshold | labelled pairs found | pairs queued | noise per real pair |
|---|---|---|---|
| 0.60 | 12/15 | 65 | 4.4 |
| 0.70 | 10/15 | 44 | 3.4 |
| 0.80 | 8/15 | 36 | 3.5 |

**It never reaches full recall.** Three labelled pairs are invisible at any threshold,
because the operator framed the same real-world subject two different ways:

- `secondary market: India, post-CIS [+]` vs `supported region: CIS [-]` — s=0.66, p=0.46.
  The same decision, reversed, six notes apart, and the gate cannot see it.
- `repository layout: open core + enterprise separate [+]` vs `risk: an enterprise can write
  its own wrapper [+]` — p=0.40. A stated risk against a stated decision.
- `Mesh excluded feature: user management` vs `Mesh integration: Tailscale` — p=0.51.

Embedding the address as one string (`subject — predicate`) instead of two fields trades
differently: 12/15 at threshold 0.75 with 70 queued — the same recall as two fields at 0.60,
at the same cost. Neither is clearly better; both plateau at 12–13 of 15.

## 3. Value similarity does not label contradictions — and it is not supposed to

The embedder's only job is to raise near pairs. Whether a pair is a contradiction or an
addition is a *verdict*, and verdicts belong to the reviewer. So the numbers below are not
a failure; they are what correct behaviour looks like.

| what the reviewer later decided | n | min | median | max |
|---|---|---|---|---|
| contradiction | 6 | 0.520 | 0.752 | 1.000 |
| co-existence / merge | 9 | 0.470 | 0.758 | 0.758 |

The distributions overlap because they should:

- `people who need delegation [+]` vs `people with enough budget to hire [-]` → 0.752
- `founders who cannot afford to hire [+]` vs `people who need delegation [+]` → 0.758

Both pairs are about the same thing. Both belong in front of a human. The embedder put both
there. Nothing is wrong.

**What this does invalidate is the table in pillar 5 of `DESIGN.md`.** That table promised a
mechanical label — `same value + opposite polarity → contradiction`. On real notes that
label fires only when the value string is literally identical (`China [-]` vs `China [+]`,
`resellers [+]` vs `resellers [-]`, both at cosine 1.000) and stays silent on every
contradiction phrased in different words. A label that is right twice out of six is worse
than no label, because it implies the four silent ones are safe.

So the column comes out. The machine produces **candidate pairs and nothing else**; the
reviewer supplies the verdict. Polarity stays on the claim — it is what lets the reviewer
see the disagreement at a glance, and it is what makes an identical-value clash jump the
queue — but it stops being the trigger for a mechanical verdict.

The consequence to accept: the confirmation queue cannot be pre-sorted by urgency, beyond
floating exact-value clashes to the top.

## 4. Confirmation load — quantified, and this is the real problem

Pairs a human must resolve, as claims arrive one at a time:

| gate | threshold | total for 33 claims | per claim | worst single claim |
|---|---|---|---|---|
| two fields | 0.60 | 65 | 2.0 | 8 |
| two fields | 0.70 | 44 | 1.3 | 7 |
| two fields | 0.80 | 36 | 1.1 | 7 |
| one string | 0.75 | 70 | 2.1 | 8 |

The corpus yields ~5.2 claims per dictated note. At the threshold that gives usable recall
(2-field @0.60), that is **~10 confirmations per note the operator dictates** — and one
conversation of 30 notes produced 65 of them.

And the per-claim figure is not stable: it is the count of near neighbours, which grows with
the base. Nothing here caps it.

## 5. Run 2 — the self-containment rule, and how noisy this measurement actually is

Two things were tested after run 1: whether telling the extractor to resolve referents fixes
the anaphora problem, and — because the answer looked surprising — how much of run 1's
numbers were signal at all.

**The judge is noisy.** Scoring the same extraction repeatedly gives a different number each
time. Four runs per condition, identical inputs:

| condition | recall across repeats | mean | polarity errors |
|---|---|---|---|
| `solo` — note alone | 88, 94, 97, 88 | **92%** | 0 in all runs |
| `ctx` — note + 2 prior notes | 70, 88, 82, 88 | 82% | 0 in all runs |
| `solo2` — note alone + self-containment rule | 88, 88, 82, 88 | 87% | 0, 1, 1 |

A single score carries ±5 points, and `ctx` swung 18. **Run 1's headline that "context made
extraction worse, 85% vs 79%" was inside the noise, and is withdrawn.** On four repeats
`ctx` does average lower, but the ranges overlap and 33 reference claims cannot settle it.
Anything measured here in one shot is worth about ±5 points, and that includes §1.

**A bug invalidates part of this comparison.** The runner only skipped the context block
when the condition was literally named `solo`; the `solo2` condition fell through to the
context branch. So `solo2` differed from `solo` in *two* ways — the prompt rule and the two
previous notes — not one. In particular the "invented referent" below was not invented: the
model had note 42 in its window and read темщики from it. That finding is withdrawn.

What survives the bug: `solo2` still scored no better than `solo` while producing 22 fewer
claims, and it is still the only condition that ever produced a polarity error.

**The self-containment rule did not work.** Adding it to the prompt:

- cut extraction from 163 claims to 141, losing note 42 entirely — the base no longer knows
  resellers were ever considered as an audience;
- produced the subject `reselling agents' output (темщики model)` on note 44, which does not
  contain that word — but see the bug above: the model had note 42 in context, so this was
  resolution, not invention;
- did not reduce pointer words in the output — 8%, against 9% without the rule;
- is the only condition that ever produced a polarity error, in 2 of 3 repeats.

Self-containment is still the right property for a claim. A prompt rule is not how to get it.

## 6. Two reference labels were wrong, and the fixture was corrected

Run 1 counted two misses that were the fixture's fault, not the extractor's. Both labels read
a note through its predecessors, which the self-containment rule forbids:

- `aud-09` recorded note 44 as *"target audience = resellers, polarity -"*. Note 44 names only
  ETN. Corrected to `association to avoid = ETN, polarity -` — which is what the extractor
  produced all along.
- `mkt-07` recorded note 80's "China we can" as a go-to-market claim. Note 80 is about
  regulatory coverage. Corrected to `legal coverage / supported region`.

The reference was changed because a design rule the operator stated after the fixture was
written contradicts it — not to flatter the extractor. The two cross-note links those labels
were smuggling in moved to `accepted_gaps` in `golden.json`, where they belong: the base will
not find them, and a verifier or a human marks them later.

All numbers in §5 are against the corrected fixture. §1's numbers are the originals, kept for
the record.

## 7. Run 3 — the gate was wrong, and fixing it recovered the silent losses

The operator's correction: score all three fields into one strength, do not AND two of them.
Measured over the same 528 pairs, as recall@K — how many of the 14 related pairs land in the
top K of the reviewer's queue:

| ranking | @30 | @50 | @80 | @120 | all 14 within |
|---|---|---|---|---|---|
| old: AND(subject, predicate), value unused | 6 | 9 | 10 | 10 | **top 497 of 528** |
| max of the three | 4 | 6 | 12 | 12 | top 468 |
| mean of the three | 7 | 10 | 12 | 12 | top 241 |
| value only | 9 | 10 | 11 | 13 | top 226 |
| **address × value, polarity-boosted** | 10 | 10 | 12 | 13 | **top 139** |

The old gate never really surfaced everything: "all related pairs are in the top 497 of 528"
means the queue is the whole cartesian product.

The case that motivated this: `go-to-market / market / China [-]` against
`legal coverage / supported region / China [+]` — the reversal six notes apart. Subject
similarity 0.657, predicate 0.482, **value 1.000**. The two-field gate discarded it. With
value in the index it ranks **6th of 528**.

One more thing the numbers show: the pairs at the top that are *not* in the fixture are
mostly not noise. `USA [+] / China [-]`, `founders who cannot afford [+] / micromanagers [-]`,
`supported region China [+] / CIS [-]` — all pairs a reviewer would want. The fixture's 14
labels are not exhaustive, so every precision figure reported earlier against it understates
the ranking's quality.

## 8. Run 4 — no polarity, no fields. A node is author, creator, and one sentence.

The operator's instruction: drop the polarity field and the subject/predicate/value split
entirely. Negation lives in the sentence, the way a person says it. Test on the bare sentence.

`golden-flat.json` is the same 33 facts and the same 14 relations as `golden.json`, rewritten
as plain sentences — so the two representations are measured on identical pairs.

### Extraction

| condition | repeats | mean | reversed facts | facts extracted |
|---|---|---|---|---|
| flat, note alone | 88, 88, 85 | 87% | 0 | 108 (3.6/note) |
| **flat, note + 2 previous notes** | 91, 94, 94 | **93%** | 0 | 114 (3.8/note) |
| decomposed, note alone (§5) | 88, 94, 97, 88 | 92% | 0 | 163 (5.4/note) |

Flat with context is the best result measured on this corpus, and it needs no fields at all.
Zero reversed facts across six runs: dropping the polarity field did not cost the thing the
polarity field was introduced to protect.

**Context helps here, clearly** — the opposite of what run 1 suggested, and run 1's comparison
was the one carrying the bug. The reason is visible in the misses: notes 48 and 50 are "Да,
лицензия на компанию" and "Давай проприетарная с открытым кодом да". Alone they record
nothing. After the question they answer, they are unambiguous.

### Pair ranking

Cosine on the bare sentence, no fields, no weights, no polarity boost:

| representation + embedder | @30 | @50 | @80 | @120 | all 14 within |
|---|---|---|---|---|---|
| flat sentence, `mxbai-embed-large` | 7 | 8 | 10 | 11 | top 465 of 528 |
| **flat sentence, `nomic-embed-text`** | 9 | 11 | 12 | 13 | **top 125** |
| flat sentence, Google `embeddinggemma`, no prefix | 8 | 10 | 10 | 11 | top 271 |
| flat sentence, Google `embeddinggemma`, `task: sentence similarity` | 6 | 7 | 8 | 11 | top 423 |
| flat sentence, Google `embeddinggemma`, `title: none \| text:` | 9 | 10 | 10 | 12 | top 166 |
| decomposed, address × value, polarity-boosted (§7) | 10 | 10 | 12 | 13 | top 139 |

Google's `embeddinggemma` (308M, Gemma 3 derived, 768-dim, run locally through ollama) lands
in the middle: better than `mxbai`, worse than `nomic` on this fixture. The Gemini API models
were not tested — no key on this machine.

Its **prompt prefix changes the result more than the model choice does**. EmbeddingGemma is
trained with task prefixes, and picking the wrong one costs more than switching models: the
document prefix (`title: none | text:`) needs the top 166, the bare sentence 271, and the
prefix the docs recommend for symmetric similarity (`task: sentence similarity | query:`) is
the worst of the three at 423. Whatever the intended semantics, on this task the document
prefix wins. It also ranks the China reversal 17th — the best placement any model gave it.

A caveat that applies to this whole table: 528 pairs and 14 labels is a small sample, and the
gap between `nomic` at 125 and `embeddinggemma` at 166 is a handful of pairs.

**The decomposition bought nothing.** A plain sentence with the right embedder matches the
three-field weighted index, and beats the two-field gate that started all this by a factor of
four.

**The embedder matters far more than the schema.** Same sentences, same pairs: 125 through
465 depending only on which model — and, for a prefix-trained model, on which prefix. That is
a bigger effect than every representation change tested, and it was invisible until the
representation got simple enough to isolate it.

### Paid embedders — Google Vertex AI (run 5)

Run through Application Default Credentials, project `mono-504617`, `us-central1`. Same 33
facts, same 14 labelled pairs, same measure.

| embedder | @30 | @50 | @80 | @120 | all 14 within | China pair |
|---|---|---|---|---|---|---|
| **`nomic-embed-text` — free, local** | 9 | 11 | 12 | 13 | **top 125** | 30 |
| Google `embeddinggemma` — free, local, doc prefix | 9 | 10 | 10 | 12 | top 166 | 17 |
| `text-multilingual-embedding-002` — paid | 6 | 8 | 10 | 11 | top 207 | 61 |
| `gemini-embedding-001` 3072d, `RETRIEVAL_DOCUMENT` — paid | 5 | 10 | 12 | 12 | top 212 | 48 |
| `text-embedding-005` — paid | 7 | 8 | 10 | 11 | top 251 | 107 |
| `gemini-embedding-001` 3072d, `SEMANTIC_SIMILARITY` — paid | 6 | 9 | 9 | 11 | top 336 | 48 |
| `gemini-embedding-001` 768d, `SEMANTIC_SIMILARITY` — paid | 6 | 6 | 9 | 11 | top 323 | 60 |
| `gemini-embedding-001` 3072d, `CLUSTERING` — paid | 7 | 7 | 9 | 12 | top 388 | 56 |
| `mxbai-embed-large` — free, local | 7 | 8 | 10 | 11 | top 465 | 27 |

**Every paid model lost to the free local one.** Not by a little: the best paid configuration
needs the top 207 of 528 where `nomic-embed-text` needs 125.

Two things are worth separating here, because "the paid model is worse" is not the useful part.

**The task type moves the result more than the model does.** `gemini-embedding-001` ranges
from top-212 to top-388 across its own task types, on identical input. And on both Google
models tested, the type whose *name* matches this task — `SEMANTIC_SIMILARITY`, sentence
similarity — is beaten by the document/retrieval type. Whatever the intended semantics, on
symmetric fact-vs-fact comparison the retrieval framing wins.

**Truncating to 768 dimensions did not help or hurt** (323 vs 336, inside the noise). The
3072-dimension advantage is not what this task is short of.

### Why the flagship loses: it compresses the range

The metric that explains the table is not any single rank, it is how far apart the model puts
related and unrelated pairs. Mean cosine of the 14 labelled pairs, minus the mean of the other
514:

| embedder | min cosine | median | max | range | related − unrelated |
|---|---|---|---|---|---|
| `nomic-embed-text` | 0.299 | 0.456 | 0.836 | 0.536 | **+0.195** |
| `mxbai-embed-large` | 0.224 | 0.452 | 0.779 | 0.554 | +0.157 |
| `text-multilingual-embedding-002` | 0.409 | 0.578 | 0.848 | 0.439 | +0.131 |
| `gemini-embedding-001` `RETRIEVAL_DOCUMENT` | 0.645 | 0.763 | 0.919 | 0.274 | +0.079 |
| `gemini-embedding-001` `SEMANTIC_SIMILARITY` | 0.671 | 0.760 | 0.909 | 0.238 | +0.062 |

`gemini-embedding-001` squeezes all 528 pairs into a 0.24-wide band starting at 0.67 — it
says everything is fairly similar to everything. `nomic` spreads the same pairs across 0.54,
starting at 0.30. Ranking is the only thing this design asks an embedder to do, and a model
that answers "0.85" to every question cannot rank.

This is not a claim that `gemini-embedding-001` is a worse model. It is a claim that on
**short, single-sentence, symmetric fact-vs-fact comparison** its calibration is wrong for
the job, and that paying more bought nothing here.

Caveat, again: 528 pairs and 14 labels. The gap between 125 and 207 is a handful of pairs, and
the separation column is the part of this table that would survive a bigger fixture.

### One thing to fix

46–49% of extracted facts open with a template subject — "The operator…", "The company…",
"The product…". Half of every sentence is identical boilerplate, which pushes every pairwise
cosine up and compresses the range the ranking has to work with. Worth stripping at extraction.

## 9. Run 6 — every pair in the live base, classified locally

Not the fixture this time: the actual base the pipeline produces. 114 facts extracted from the
30 notes, embedded with `nomic-embed-text`, **all 6441 pairs** classified — no top-N sampling,
because the open question was whether the ranking was only ever examined at the top.

Classifier: `qwen3.6` running locally through ollama, 60 pairs per call, 53 minutes, zero cost.
It also got right two of the three false positives the cloud classifier produced on the top-120
sample earlier (China/EU as "one replaced the other"; delegate/compete as "conflicting desires").

### Findings collapse with rank, but the tail is not empty

| queue band | controversy | duplicate | narrows | hits per 100 examined |
|---|---|---|---|---|
| 1–120 | 16 | 9 | 16 | **34.2** |
| 121–500 | 19 | 0 | 25 | 11.6 |
| 501–1000 | 8 | 2 | 3 | 2.6 |
| 1001–2000 | 9 | 0 | 2 | 1.1 |
| 2001–4000 | 11 | 0 | 0 | 0.5 |
| 4001–6441 | 3 | 0 | 1 | 0.2 |

124 of 6441 pairs (1.9%) were marked actionable; 66 of those were controversies.

### But the deep findings are false, and they announce themselves

Reading all 54 cross-note controversies: roughly **13 are genuine, and every one of them ranks
above 750**. Below about rank 750 the classifier's `why` routinely describes facts that are not
in the pair it was given — at rank 4279 it paired "the plan includes launching a website" with
"we can support China" and explained it as "supporting all non-sanctioned countries conflicts
with supporting China". At low similarity the model reconstructs a plausible pair instead of
reading the one in front of it.

**That mismatch is a free false-positive detector**: check whether the stated reason mentions
the facts actually supplied. No extra model call needed.

### Where the cut-off is

| verdict | n | min | median | max |
|---|---|---|---|---|
| duplicate | 11 | 0.617 | 0.821 | 0.898 |
| narrows | 47 | 0.444 | 0.721 | 0.898 |
| controversy | 66 | 0.394 | 0.672 | 0.886 |
| independent | 6306 | 0.245 | 0.488 | 0.898 |
| **genuine findings (13, read by hand)** | | **0.631** | **0.683** | **0.816** |

| floor | pairs above | % of base | genuine findings lost |
|---|---|---|---|
| 0.75 | 89 | 1.4% | 12 of 13 |
| 0.70 | 233 | 3.6% | 10 of 13 |
| 0.65 | 496 | 7.7% | 2 of 13 |
| **0.60** | **955** | **14.8%** | **0** |
| 0.50 | 2925 | 45.4% | 0 |

**0.60 keeps everything and discards 85% of the work.** 0.65 halves the queue again at the cost
of 2 findings of 13.

The medians separate the classes, but they mislead about the band that matters. **Above 0.80
the base holds 10 controversies and 8 duplicates** — more contradictions than merges, at the very
top. The ceilings are indistinguishable: 0.898 for a duplicate, 0.886 for a controversy, 0.898
for an `independent` pair. Duplicates skew high only because there are 11 of them in 6441 pairs.

The case that settles it — rank 6 overall, cosine 0.886, both facts from note 80:

```
The company supports all countries except the CIS.
Within the CIS, the company supports Ukraine.
```

Merging on similarity would erase the Ukraine carve-out. And the classifier itself mislabels
here: the second-ranked pair in the whole base (0.898), "China is one of the main target markets"
against "China is a great market", came back `independent`.

**Nothing may be auto-merged at any cosine.** The floor bounds the search; it decides nothing.

### One accepted gap turned out not to be a gap

`golden.json` recorded the pair "post-CIS is a market worth exploring" (note 74) against "we
support everything except the CIS" (note 80) as unfindable — the structured representation put
them under different subjects. On flat facts it surfaces at **rank 220 of 6441**. The gap was an
artefact of the subject/predicate schema, not a property of the corpus.

## 10. Phrasing moves the cosine as much as the relation does

Building the compose flow surfaced this by accident. The pair that motivated the whole feature
did not surface at all: hand-written as "We support all countries." against "We do not support
the CIS.", it scores **0.583** — below the 0.60 floor.

The same relation, phrased four ways:

| cosine | phrasing |
|---|---|
| 0.583 | "We support all countries." / "We do not support the CIS." |
| 0.695 | "The company supports all countries." / "The company does not support the CIS." |
| 0.856 | "We support all countries except the CIS." / "In the CIS we support Ukraine." |
| 0.886 | "The company supports all countries except the CIS." / "Within the CIS, the company supports Ukraine." |

**Adding the shared subject "The company" is worth 0.11 on its own.** Terse facts score lower on
an identical relation, because there is less shared surface for the embedder to work with.

This corrects a recommendation made in §8. There, 46–49% of extracted facts were observed to
open with a template subject — "The operator…", "The company…" — and the note said that was
boilerplate inflating every cosine and worth stripping at extraction. **Stripping it would have
made the base worse.** The shared subject is not noise; on short facts it is a large part of what
carries the relation across.

The floor is therefore calibrated to three things, not one: the embedder, the corpus, and **how
verbosely the extractor writes**. Change the extraction prompt and the floor needs re-measuring
just as surely as if the embedder changed.

Reassuringly, this is not a systematic hole in the live base: only 6% of `narrows` and 0% of
`duplicate` verdicts fall below 0.60 there, because real extracted facts are wordier than
hand-written test sentences. But it means every threshold in this document was measured on
sentences one particular prompt produced.

## What this says about the design

Holding up:

- **The flat node.** author + creator + one sentence reaches 93% extraction and top-125 pair
  ranking — equal to or better than every structured version tried, with none of the machinery.
- **Extraction from raw dictated speech.** 93% against a hand-labelled reference, at ~$0.05 per
  note, from unedited speech with filler and swearing left in.
- **Previous notes as read-only context.** +6 points, and it fixes the class of note that is
  meaningless alone.
- **"Machine surfaces, reviewer decides."** Every attempt to move a decision into the machine —
  a mechanical contradiction label, a prompt rule that resolves referents — failed on contact
  with the corpus.

Dropped, and rightly:

- **Polarity as a field.** Zero reversed facts without it. It solved a problem the plain
  sentence does not have.
- **subject / predicate / value.** Bought nothing on either axis.
- **Thresholded gates.** Replaced by a ranked queue.

Settled by decision rather than evidence:

- **Confirmation load** goes to a verifier agent. Deferred: nothing caps its growth.
- **Gaps are accepted.** The base is eventually consistent, not complete.

Still open:

- **The embedder.** The largest measured effect in the whole experiment, and only two models
  were tried.
- **Template subjects.** ~48% of facts open with identical boilerplate, inflating every cosine.
- **Measurement.** ±5 points of judge noise on 33 reference facts. A bigger fixture, or majority
  voting over repeats, before any further tuning is believed.
