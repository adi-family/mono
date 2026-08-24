# Market scan — has anyone built this already?

Scanned 2026-08-23. Question: does an existing system do what `DESIGN.md` describes — a base of
plain human-authored facts, where every relation is confirmed rather than inferred, and where
AI-derived artifacts go stale mechanically when the facts under them change.

Short answer: **the parts exist, the combination does not.** Two of our four pillars are
solved elsewhere and better; two appear to be genuinely unoccupied.

## The field

Agent-memory is a crowded 2026 category — Mem0, Zep/Graphiti, Letta, Cognee, Supermemory,
Hindsight, Memvid. Everyone stores facts and retrieves them. The differences that matter to us
are what happens when two facts disagree, and who decides.

| system | contradiction handling | who decides |
|---|---|---|
| **Zep / Graphiti** | bi-temporal validity windows; a superseded fact is invalidated, not deleted, and history survives | **automatic** — "automatic fact invalidation with temporal history preserved" |
| **Mem0** | both facts kept; recency and relevance decide which surfaces. Dedup is LLM-assisted at write time, then exact MD5 only, so a reworded fact stays a separate row | mostly automatic; maintainers state classifying conflict types "still needs a human" and their tooling only identifies candidates |
| **Letta (MemGPT)** | the agent rewrites its own memory block, so contradiction handling is whatever the prompt produces | the agent |
| **Cognee** | ontology-driven: relations violating the schema's cardinality are rejected or merged at ingest | the schema |
| **Supermemory** | validity intervals per fact, retrieval scored by recency as well as similarity | automatic |

## Where we are behind

**Temporal validity is a solved problem and we do not have it.** Graphiti's bi-temporal model —
when a fact became true, and when it stopped — is more careful than our "edit in place, history
is the user's backup problem". That decision was made deliberately (`DESIGN.md`), but the
alternative is real, shipped, and open source.

**Cognee's ontology check is the thing we deleted.** Its cardinality rules are exactly the
arity idea that got dropped in favour of "everything is a set". Worth knowing that someone
shipped it and it works for them — the difference is that their facts arrive typed, and ours
arrive as dictated speech.

## Where the field is behind us

**Nobody makes confirmation first-class.** Every system above resolves contradictions
automatically, or leaves it to a prompt. Ours refuses: detection is ranked, every verdict —
including "these two can coexist" — is confirmed and carries the confirmer's identity and
version. Mem0's own maintainers say the classification "still needs a human"; nobody has built
the queue that human would work.

**Nobody separates author from creator.** Graphiti has provenance in the sense of "which
episode produced this", which is *creator*. The distinction that matters to us — whose meaning
this is versus who typed it — appears nowhere in the memory-layer field. It is what makes
"human said it" and "an agent inferred it" different kinds of record rather than different
confidence scores.

**Staleness propagation to derived artifacts is close to unclaimed — but not entirely.** The
one real prior art found is **EA-Graph** (arXiv 2608.04278, 2026), a verification memory for
coding agents: claims are anchored to a content hash over a sub-path, an upstream edit changes
the hash, and the claim is mechanically marked stale — no model in the loop, exactly our design.
The differences: it anchors to *code spans*, its subject is "is this verification still valid",
and there is no human confirmation anywhere. Nobody found doing this for prose artifacts —
landing page copy going stale because the audience note under it changed.

The commercial "stale content" tools (Brandlight and similar) are SEO freshness monitors: they
watch citation drop-off and page age. They do not know what a page was derived from.

## The honest read

Our two genuinely unoccupied ideas are **confirmation as a first-class, identity-carrying
record** and **mechanical staleness propagation from facts to prose artifacts**. Both are
design positions rather than technology — nothing here needs a model nobody else has.

Everything else we have is a simpler version of something shipped. The flat fact, the ranked
queue, the local embedder — those are choices, and this experiment measured them, but they are
not a moat.

Worth deciding before building: whether to adopt Graphiti's bi-temporal model rather than our
edit-in-place, since it is the one place where a shipped system is clearly more careful than
the design.

## Sources

- Zep vs Mem0 comparison — https://atlan.com/know/zep-vs-mem0/
- Graphiti — https://github.com/getzep/graphiti
- Mem0 dedup/contradiction discussion #4787 — https://github.com/mem0ai/mem0/discussions/4787
- Letta / Cognee / Mem0 / Zep comparison — https://theaiengineer.substack.com/p/cognee-vs-zep-vs-mem0-vs-letta
- Agent memory systems and knowledge graphs — https://codepointer.substack.com/p/agent-memory-systems-and-knowledge
- EA-Graph, artifact-anchored verification memory under upstream drift — https://arxiv.org/html/2608.04278
- Temporal knowledge graphs for agent memory — https://supermemory.ai/blog/temporal-knowledge-graphs-agent-memory
- Truth maintenance / belief revision overview — https://cse.buffalo.edu/~shapiro/Papers/br-overview.pdf
