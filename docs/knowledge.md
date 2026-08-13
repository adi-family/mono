# The knowledge base

`adi-mono knowledge` keeps text notes and answers questions with them. `crates/adi-knowledge` is
the library; `crates/adi-cli/src/knowledge.rs` is the argv adapter.

A **knowledge** is one note — a title, a body of any length, tags, and where it came from. It
lives in a **base**, a named collection at one of three isolation levels. Every note is embedded,
so a base is searched by what a note means rather than which words it happens to use.

```
$ adi-mono knowledge base new global/runbooks --description "how this machine is operated"
Created global/runbooks (global isolation, sqlite provider).

$ adi-mono knowledge add global/runbooks -t "Restarting the control panel" \
    -b "launchctl kickstart -k gui/\$(id -u)/family.adi.app.control-panel" --tag ops
Added restarting-the-control-panel to global/runbooks.

$ adi-mono knowledge search "how do I bring the panel back up"
0.681  global/runbooks    restarting-the-control-panel   Restarting the control panel
```

The body comes from stdin when `--body` is omitted, so a long note needn't fit in a shell
argument: `cat postmortem.md | adi-mono knowledge add global/runbooks -t "The March outage"`.

## The three levels

A base id is `<scope>/<name>`, and a bare scope means that scope's `default` base.

| id | who reads it | who writes it |
| --- | --- | --- |
| `global/<name>` | everyone | everyone |
| `project:<id>/<name>` | whoever is working in that project | same |
| `agent:<name>/<base>` | **every agent** | that agent alone |

The last row is the interesting one, and it is deliberate. An agent that worked out how this
deployment actually behaves has learned something the next agent needs; what the agent level
protects is the *authorship* of a memory, not its secrecy. So `agent:reviewer/memory` is
readable by `solver` and writable only by `reviewer`.

An agent does not have to name itself: `adi-agents` exports `ADI_AGENT` and `ADI_PROJECT` into
every run, and the CLI falls back to them — so `adi-knowledge search "…"` from inside a run
already applies that agent's isolation. The person at the terminal has neither variable, is the
owner of the store, and reaches everything. To see what an agent would see, say so:

```
$ adi-mono knowledge --as-agent solver --as-project acme bases
global/runbooks                    global   sqlite   how this machine is operated
project:acme/notes                 project  sqlite
agent:reviewer/memory              agent    sqlite
agent:solver/memory                agent    sqlite
```

`project:other/notes` is not in that list, and no error was raised about it: "what is there" is
a different question from "let me into this one", and a listing that failed on the first base
belonging to somebody else would be useless to every agent. Asking for it directly *is* refused:

```
$ adi-mono knowledge --as-agent solver --as-project acme get project:other/notes something
error: agent solver may not read project:other/notes
```

**These levels are not a sandbox.** They organize knowledge and decide what a run reaches by
default. Anything that can run `adi-mono` can also pass a different `--as-agent`, or read the
files directly. What isolation buys is that one agent's memory cannot be *rewritten* by another
— not that it can be kept from them. Secrets belong in `adi-mono secrets`, which encrypts them.

## Staying embedded

Every note carries a `content_hash` over exactly the text that gets embedded — title, tags, body
— and its vectors record the hash **they** were made from. The two disagreeing is the definition
of stale. That makes "re-embedded whenever they change" a property of the data rather than a
promise about call sites:

- writing a note whose text has moved clears its vectors *in the same transaction*, so no row
  can outlive the truth of its vectors;
- the write path embeds again immediately, so an edit is searchable by the time it returns;
- an edit that leaves the embedded text alone — a new `--source` — keeps its vectors, because
  they are still accurate;
- changing the **model** invalidates every vector in the store at once, because the model name
  is compared too.

`reembed` is the sweeper for whatever was written while the model was unavailable, and for the
day the model changes:

```
$ adi-mono knowledge base status global/runbooks
global/runbooks
  level:     global
  provider:  sqlite
  notes:     412
  embedded:  0 (412 stale)
  model:     jinaai/jina-embeddings-v2-base-code

$ adi-mono knowledge reembed global/runbooks
global/runbooks: embedded 412 of 412 note(s) into 1174 chunk(s); 0 already current.
```

A note that could not be embedded is still **stored**, still full-text searchable, and says so
rather than failing the write — losing the note because a model download failed would be the
worse outcome. What must not happen is silence, so the reason travels with the result and `list`
marks the note with a `*`.

### Notes of any length

The embedder reads 512 tokens. A note longer than that is **chunked** — ~1400 characters per
window with 200 characters of overlap, cut on a paragraph or word boundary when one is near —
and each chunk carries the note's title and tags, so a chunk taken from the middle still says
what it is about. A note is ranked at its best chunk and reported once, which is what stops one
thorough note from filling a whole result page. Without this, "notes of any length" would mean
"the first 512 tokens of a note of any length".

## Searching

`search` ranks by meaning; `--text` ranks by word (FTS5, no model, no network). Without
`--base`, both search every base the caller may read — which for an agent is global, its
project's, its own, and every other agent's:

```
$ adi-mono knowledge --as-agent solver search "why does the front door 502"
0.714  agent:reviewer/memory   frontdoor-502-is-a-stale-route   The front door 502s on a stale route
0.502  global/runbooks         restarting-the-control-panel     Restarting the control panel
```

The query is embedded once and put to every base, so searching four bases costs one embed.

## Pluggable storage

What holds a base's notes is a **provider**, named in the base's manifest. Scoping, access,
chunking, staleness, and search all sit above that line, so a new backend implements storage and
inherits the rest.

```
$ adi-mono knowledge providers
memory     In process, in a map, gone on exit — for tests and scratch bases.
sqlite     One SQLite file per base: FTS5 for words, stored f32 vectors for meaning.

$ adi-mono knowledge base new global/scratch --provider memory
$ adi-mono knowledge base new project:acme/big --provider hosted --set collection=acme --set region=eu
```

Implement `adi_knowledge::backend::Provider`, register it with `Providers::register`, and hand
the registry to the store with `KnowledgeStore::with_providers`. `--set key=value` reaches your
provider untouched in `BaseContext::settings`. A base naming a provider nothing is registered
under is refused at creation rather than written and discovered later.

**Why the built-in vector search is a scan.** The indexer keeps a usearch HNSW because it ranks
tens of thousands of symbols. A knowledge base holds notes somebody wrote on purpose, and at
that size an exact scan of stored f32 blobs wins on every axis that matters: exact (no recall
cliff), no second file to keep in step with the rows, and 10,000 chunks × 768 dimensions is
~30MB and a few milliseconds. A base that outgrows it wants a provider that speaks to something
built for it — which is what the trait is for.

## The embedder is the indexer's

`adi_knowledge::Embedder` *is* `adi_indexer::embed::Embedder` — the same trait, the same
jina-embeddings-v2-base-code on candle, the same weights cached in the same place. A parallel
trait would have made a note and a symbol incomparable by construction however identical the
model behind them.

It loads **lazily**: adding, listing, editing, deleting, and reading need no vectors, so nothing
pays the multi-second model load until something genuinely embeds. `adi-knowledge` takes
`adi-indexer` with `default-features = false` and re-enables `candle` through its own feature,
so a dependent that never embeds carries neither the model stack nor eleven tree-sitter
grammars — which is why `adi-app` links the same facade and ships none of it, while `adi-mono`
turns the feature back on for itself.

A build without `candle` falls back to a hashed bag-of-words embedder. It finds "restart the
panel" from "panel restart" and will never find it from "bring the control surface back up" —
it has no idea the two mean the same thing, which is the entire point of a real model. It is
there so tests can assert ranking without a 300MB download and so a lean build degrades to
something rather than to nothing.

## Agents

Two fields on an agent definition:

```toml
knowledge = ["global/runbooks", "project:acme/notes", "agent:reviewer/memory"]
memory = true
```

`knowledge` is a **wish list, not a grant**. It is resolved through the isolation levels at read
time (`adi_knowledge::resolve_agent_bases`), so naming another project's base drops it and
naming another agent's makes it read-only. An agent does not fail to start because somebody
deleted a base it was pointed at.

`memory` gives the agent `agent:<name>/memory` — the base it alone writes and every other agent
may read. Off by default: an agent that records what it learns is a different thing from one
that does not, and that should be a decision somebody made.

```
$ adi-mono agents save solver --backend harness:adi \
    --knowledge global/runbooks,agent:reviewer/memory --memory
```

Both are omit-to-keep on save, in the CLI and in the API: a save from a form that never rendered
them must not cut an agent off from what it knows.

Agents reach all of this through the `adi-knowledge` system tool (`sys-knowledge`), which
forwards to this CLI — so an agent runs `adi-knowledge search "…" --as-agent $ADI_AGENT` the
same way it runs `adi-tasks` or `adi-secrets`.

## On disk

```
~/.adi/mono/knowledge/
  global/<base>/base.toml           # provider, description, settings, timestamps
  global/<base>/knowledge.db        # the sqlite provider's file: notes, vectors, FTS5
  projects/<project-id>/<base>/…
  agents/<agent-name>/<base>/…
```

One file per base rather than a table in the shared `adi-db` store, for three reasons: the agent
level has no equivalent scope there, deleting a base becomes a directory removal instead of a
cascade, and one agent's memory never shares a write lock with every other agent's. The file
carries the same WAL + `busy_timeout` settings the rest of the platform uses, because agents,
their tools, and the control panel all reach it at once.
