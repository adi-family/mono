# Experiment 1 — does claim extraction survive real dictated speech?

## Why this one first

The design (see `DESIGN.md`) rests on one assumption that could kill it: that an agent can
read a human's raw, dictated, multi-topic paragraph and pull out addressable claims —
`(subject, predicate, value, polarity)` — well enough that contradictions between them are
findable later. Everything else in the design is arithmetic. This part is not.

So it is tested on the worst realistic input available, not on clean prose.

## The corpus

`corpus.md` — 30 turns of the operator's own dictated Russian, verbatim, from the
`adi-business` session `1787359989639-0002` (15.2 KB). Filler, self-interruption, swearing
and mid-sentence reversals left in. This is the actual input the knowledge base would face.

It was chosen because it contains, unprompted, every phenomenon the design claims to
handle:

- **A rejected audience.** Turn 42 proposes resellers as an audience; turn 44 rejects them.
  This is the "the audience is *not* machine operators" case, occurring naturally — after
  it, the base must know resellers were considered and ruled out, not merely be silent.
- **A reversal across turns.** Turn 74: "not sure we can enter China, too much regulation."
  Turn 80: "China we can." Six turns apart. A working base must catch this.
- **A contradiction inside one turn.** Turn 74 calls China a core market *and* doubts entry,
  four sentences apart.
- **Two subjects that look like one.** Base Mesh gets no user management; enterprise Mesh
  gets global Mesh control. Same word, two subjects — the Airbnb buyers/sellers shape, real.
- **A boundary defined by a negative.** Turn 28: the audience is people who cannot afford to
  hire, and explicitly *not* people who can. The positive and the negative are one
  definition, not a conflict.
- **A 2.5 KB turn covering four topics at once** (turn 28) — the stress case for extraction.

## The fixture

`golden.json` — 33 claims and 15 relations, hand-labelled from the corpus. Two columns,
deliberately separate:

- `machine` — what the mechanical pass (embedding proximity + polarity) must compute alone.
  Only two values exist: `same-value-opposite-polarity` and `different-value`.
- `verdict` — what a human answers at the confirmation prompt: `merge`, `coexist`,
  `supersede`, `review`. **Not derivable, and never to be guessed by a scorer.**

Current spread: 6 mechanical contradictions, 9 co-existence candidates; verdicts 7 coexist,
3 merge, 3 supersede, 2 review. 11 subjects, 20 predicates, 5 negative-polarity claims.

The split is the point. A scorer that predicts `verdict` is measuring the wrong thing — the
design says a machine may not decide that. What is being measured is whether the machine
surfaces the right *pairs*, cheaply, and never misses a real contradiction.

## What to measure

1. **Extraction recall** — of the 33 hand-labelled claims, how many does an automatic
   extractor find? Which topics does it drop from the 2.5 KB multi-topic turn?
2. **Extraction precision** — how much noise does it invent that a human would not record?
3. **Polarity accuracy** — the single point of failure. A missed negation turns a
   contradiction into a silent co-existence, and the base quietly holds both.
4. **Pair recall at threshold** — with subject/predicate embeddings, does every one of the
   15 labelled pairs actually come up as near? What threshold, and how many false pairs
   does that threshold drag into the confirmation queue per 33 claims?
5. **Confirmation load** — pairs queued per claim added. If this is not roughly flat, the
   design does not survive contact with a real note-taking day.

## Not yet done

Extraction has not been run. The fixture is the baseline; the extractor is the next step.
