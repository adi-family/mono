# `facts` — the CLI, and the decision behind it

## The extraction question: who turns prose into facts?

Two options were on the table. The caller — an LLM in a live conversation — either emits
finished facts, or pastes the paragraph and lets our own extractor do it in the background,
eventually with a model we train ourselves.

**Decision: the caller emits facts by default. `--text` exists, and is the fallback.**

The reason is the one thing the experiment could not fix by any other means. Extraction from raw
dictated notes reaches ~93% (`RESULTS.md` §8), and every remaining failure is **anaphora** — a
fact whose referent lives in a different note. Note 44 says "I would not want us to turn into
ETN"; what was being rejected was named in note 42. A background extractor never has that.

We measured the fix: handing the extractor the two previous notes lifted recall from 87% to 93%.
**A caller in a live conversation has strictly more context than that** — it has the whole
thread, it knows what "that direction" meant, and it is the only party that ever will. Moving
extraction to the background throws away the only information that solves the problem.

The counter-argument is real and should be stated: emitting a list of facts costs the caller
more output tokens than pasting a paragraph, and structured generation is where hallucination
creeps in. That is exactly what `--text` is for — bulk import, a human's raw dictation, anything
where the caller has no context worth preserving anyway. It is a fallback, not the default.

**The source text is stored either way.** Extraction will get better; re-projecting facts from
the original is only possible if the original survived. This is cheap and there is no argument
against it.

**A side effect worth naming:** every resolved transaction is a labelled training example —
these facts, this pair, this verdict, decided by this confirmer. The system produces its own
training set as a by-product of being used. That is what makes "train our own extractor later"
a plan rather than a hope.

## Why a transaction and not a refusal

Inserting is expensive: embed every new fact, compare it against the base, classify what comes
back. Measured on the live base, at the 0.60 floor that is ~2 pairs per fact needing a look.
Throwing that away because one fact is contentious — "иди нахуй, try again" — burns the whole
cost and gives the caller nothing to act on.

So an insert opens a **transaction**. It returns an id, the facts staged under it, and the
pairs that need a decision. The caller works the list and commits. Nothing is visible to the
rest of the base until it does.

Two things this buys that a refusal does not:

- The caller can resolve **per pair**, and drop a single fact rather than losing the other
  nineteen.
- The expensive work — embeddings, neighbour search, classification — is computed once and
  survives across the caller's decisions.

## The commands

```
facts add [--author WHO] [--creator WHO] [--text] [--note-id ID]
```

Reads stdin. **One fact per line** by default — this is the bulk path, a caller can write
fifty at once. With `--text`, stdin is one raw note and the extractor runs first.

`--author` is whose meaning it is (a human, usually). `--creator` defaults to the calling
agent's identity. Both are recorded on every node; neither is inferred.

It opens a transaction and **prints what needs deciding straight away**. There is no envelope to
parse and no second call to discover the work — a caller that has to run `tx show` to find out
what just happened is a caller spending a round trip on nothing. Output is plain text
everywhere; the reader is a language model and JSON only costs it tokens.

```
facts tx show <tx>
```

The same view again, for when the caller comes back to it later:

```
tx_7f3a91 — 12 staged, 3 pending

[p1] 0.886  CONTROVERSY
  new  n#4  We support all countries except the CIS.
  base f091 Within the CIS, we support Ukraine.
  why       one excludes the CIS, the other carves Ukraine out of it

[p2] 0.821  DUPLICATE
  new  n#7  China is one of our main target markets.
  base f044 China is one of the operator's main markets.

[p3] 0.712  CONTROVERSY
  new  n#9  We can support China.
  base f038 We are not sure we can enter the China market.
  why       one asserts capability, the other doubts it
```

```
facts tx resolve <tx> <pair> --verdict coexist|merge|supersede|drop [--keep ID] [--fact TEXT]
facts tx commit <tx>
facts tx abort <tx>
```

`resolve` writes the verdict, who confirmed it and when into the pair's row — that record is the
audit trail, and it is written whatever the verdict. What each verdict then does to the facts:

- `coexist` — nothing. Both land. **Confirmed, not assumed**: this is a decision, not a skip.
- `drop` — the new fact never lands. The base was already right.
- `supersede --keep <id>` — at commit the winner's sentence is written into the **old node in
  place** and its version bumped, so everything derived from it goes stale. No second row.
- `merge --fact "..."` — the same operation, with the sentence supplied instead of chosen.

**`merge` and `supersede` are one mechanism.** Both retire the losing fact; only where the
winning sentence comes from differs — `merge` takes it from `--fact`, `supersede` from whichever
side won. `merge` is for a duplicate, and the sentence it takes is the one that says what both
sides said. `supersede` is for a controversy, where one side simply wins.

Three bugs lived here, all of them silent, and all of them found by running the tool rather than
reading it:

- `merge` rewrote only the incoming fact and left the base one alone, so committing produced the
  merged sentence **and** the original it was meant to replace.
- When both sides of a pair were in the same incoming batch, `merge` and `supersede` did nothing
  at all — neither row was retired and both landed.
- A typo in `--keep` was read as "the base side won", so the incoming fact was discarded without
  a word. `--keep f_typo` now fails and names the two ids that are actually valid.

None of the three raised an error. That is the pattern worth remembering about this tool: its
failure mode is not a crash, it is a base that quietly holds the wrong thing.

`commit` fails while anything is pending, and says what. `abort` discards the whole
transaction.

```
facts stale        # what is out of date, and because of what
facts refresh <id>          # a derived node was regenerated; re-stamp its sources
facts near <id> [--top N]   # the queue around one fact, for a verifier agent to work
```

## Two things the implementation settled

**`supersede` rewrites the losing fact in place; it does not delete it.** The obvious
implementation — drop the old row — cascades its edges away, and everything derived from it
becomes *orphaned* rather than *stale*. That is the one outcome this whole design exists to
prevent. So superseding rewrites the node's text and bumps its `version`, which is exactly the
signal the staleness graph is watching for.

Worked through end to end: a base fact at v1, "We are not sure we can enter the China market",
with a derived plan built on it, "Market entry plan: skip China for now". A new fact arrives,
"We can support China after all", the caller resolves the pair as `supersede`, and after commit
the base fact is v2 with the new text, no duplicate row exists, and `facts stale` reports the plan
as out of date with the reversed fact named as the cause. Nothing had to be remembered by hand.

The same run surfaced something not designed for: the new fact was compared against the
**derived** plan too, and the pair "we can support China" against "skip China for now" came back
as a controversy. Derived artifacts are nodes, so they get checked like everything else.

**Open: `independent` pairs are currently assumed**Open: `independent` pairs are currently assumed, not confirmed.** `DESIGN.md` says
co-existence is a decision — "we know these two are both true" — and the implementation does not
ask about it: pairs classified `independent` never reach the queue. The tension is real and
costs money either way. Confirming everything above the floor is roughly 2 pairs per fact
(`RESULTS.md` §4), most of them independent; confirming nothing means the base quietly assumes
compatibility it was never told about. The measured pair that makes this concrete is "we support
all countries except the CIS" against "within the CIS we support Ukraine" — the classifier calls
it independent, and nobody ever confirms that the carve-out is intended.

Needs a decision before this ships.

## References from outside: `facts get`

Fact ids get written down where the base cannot see them — a marker in source, the way a TODO
is:

```
// FACT: adi-family#f_1a02c93d661_0@1
```

The surprising part is what does **not** go wrong. Because `merge` and `supersede` rewrite the
winner in place, a committed id is never destroyed and an outside reference never dangles. What
changes underneath it is the **meaning** — the same id can end up saying the opposite of what it
said when someone wrote that marker. A dangling pointer announces itself; a pointer whose target
quietly changed does not.

So every change is logged: both texts, the verdict that caused it, and who confirmed that
verdict. `facts get` replays it.

```
$ facts get f_1a02c93d661_0@1
f_1a02c93d661_0  v2  [fact]
  The company was reincorporated in Nevada.
  said by igor, written by agent:chat@1

  STALE REFERENCE — written against v1, the fact is now v2.

what changed since:
  v2   supersede   by igor
        was: The company was incorporated in Delaware.
        now: The company was reincorporated in Nevada.
```

The `@1` is the version the reference was written against, and it is what makes the marker
self-checking: the tool reports the drift instead of a reader having to notice it. Without a
version, `facts get` prints the whole history and says plainly that the id still resolves but no
longer means what it did.

**One case does destroy a record: two facts merged inside the same batch.** The loser never
reaches the base, so it never gets an id to reference. It is logged against the survivor as
`absorbed`, with its text, so the decision is still findable — searching the log for the wording
someone remembers will find the id that swallowed it.

The log is append-only. It is not the version history that `DESIGN.md` declines to keep: it
records **decisions**, not revisions, and it exists to keep outside references honest rather than
to let anyone browse the past.

## Rules the interface enforces

**Nothing merges automatically, at any similarity.** Above 0.80 the live base holds more
controversies than duplicates, and merging rank 6 — "we support all countries except the CIS"
with "within the CIS we support Ukraine" — would silently delete the Ukraine carve-out
(`RESULTS.md` §9). A high score raises a pair; it never resolves one.

**The floor is 0.60 and it belongs to the embedder.** Below it, nothing on the measured corpus
was ever worth looking at. Change the embedder and the number is meaningless until re-measured.

**Truncation is always reported.** If the pending list is capped, the response says how many
pairs were dropped and at what strength. A silent cut reads as "nothing else to see", which is
the one lie this interface must never tell.

**A verdict records who made it.** Human id, or agent id and version. `coexist` decided by
`agent:verifier@3` and `coexist` decided by a person are different records.
