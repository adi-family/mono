# How the tests are run, and how "better" is decided

Broad strokes. The point of writing this down is that most of the wrong turns in this
experiment were method mistakes, not design mistakes.

## Ground rules

1. **The thing being tested never sees the fixture.** The extractor gets a note and a prompt.
   It never sees what a human recorded from that note.
2. **One variable per run.** Change the prompt, or the context window, or the embedder — never
   two. (Violated once by a bug; it invalidated a whole comparison. See `RESULTS.md` §5.)
3. **What the machine must find is scored. What a human decides is never scored.** The fixture
   keeps them in separate columns on purpose.
4. **A noisy measurement is repeated.** A single LLM-judged score is worth about ±5 points.
5. **The fixture is not exhaustive.** It lists pairs a reviewer would want; it does not claim
   every other pair is junk. So recall is trustworthy and precision is not — unlabelled pairs
   near the top of the queue are usually real, just unlabelled.

## The pipeline

```
CORPUS
    notes ← real dictated human turns, verbatim, harness envelopes stripped
    # never cleaned up, never paraphrased: filler and profanity are the input

FIXTURE  (hand-written once, by a human reading the corpus)
    facts     ← [ {id, fact sentence, source note} ]          # what a careful human would record
    pairs     ← [ {a, b, verdict} ]                           # which facts a reviewer must see together
    gaps      ← [ {a, b, why} ]                               # pairs known to be unfindable; not scored
    # `verdict` (merge/coexist/supersede/review) is recorded but NEVER scored:
    # the design forbids the machine from deciding it

EXTRACT(condition)
    for each note:
        candidates[note] ← claude -p (system prompt = condition.prompt,
                                      input = note [+ condition.context notes])
    # one call per note, no tools, no session state, parallel

SCORE_EXTRACTION(condition)                     # → recall
    for each note:
        ask a second claude -p:
            "here are the reference facts and the candidate facts for this note.
             for each reference, is any candidate the same fact by meaning?"
    recall ← matched references / all references
    repeat 3-4 times, report mean AND range        # the judge disagrees with itself

EMBED_AND_RANK(embedder)                        # → queue
    vectors ← embedder(fact) for every fact in the fixture
    queue   ← all C(n,2) pairs, sorted by cosine, descending
    # no threshold anywhere: a reviewer works a queue from the top, not a gate

SCORE_PAIRING(embedder)
    recall@K   ← labelled pairs inside the top K, for K = 20/30/50/80/120
    worst_rank ← position of the last labelled pair        # "all N within top-X"
    separation ← mean cosine of labelled pairs − mean cosine of all the rest
```

## How better/worse is decided

Three numbers, in order of how much they are trusted:

- **`separation`** — how far the model pushes related pairs above unrelated ones. Trusted
  most: it is an average over all 528 pairs, so it barely moves when one label is arguable.
  It is also what explains the other two.
- **`worst_rank`** — how deep the queue must be worked before nothing related is left below.
  This is the operational number: it is the size of the reviewer's day.
- **`recall@K`** — the shape of the top of the queue. Noisiest of the three, because moving one
  pair across the K boundary changes it by a whole point.

A run wins when it improves these **without** costing recall on the extraction side, and
without introducing a reversal (a fact recorded as the opposite of what the note said).
Reversals are treated as disqualifying, not as a percentage.

Anything that moves a number by less than the measured noise is reported as "inside the noise"
and does not count as a win. This is how "context makes extraction worse" got withdrawn.

## What this method is bad at

- **33 facts and 14 labelled pairs is small.** Differences of a handful of pairs are not
  conclusive, and every table here carries that caveat.
- **Precision is unmeasurable** with a non-exhaustive fixture. Only recall and separation mean
  anything.
- **One corpus, one speaker, one language.** Nothing here has been tested on anyone else's
  notes.
- **The judge is an LLM.** It is the noisiest component in the harness and it is measuring the
  thing that matters most.
