# Embedding backends — spec

One trait, many ways to make a vector. Read this before touching `crates/adi-embeddings` or
any of the three crates that embed text today (`adi-indexer`, `adi-knowledge`, `adi-facts`).

Status: **phase A built 2026-09-12, phase B built 2026-09-13, phase C built 2026-09-13, the
on-demand test built 2026-09-17 — the feature is complete.** Phase A is the spec, the registry
crate, and the four runtimes (`docs/embedding-backends-survey.md` is the research this reconciles
to). Phase B is the two things it deferred: the three consumers actually resolving through the
registry, and the two staleness holes the survey flagged (Surprises #1 and #2) that made a
consumer resolving *differently* from one process to the next dangerous. Phase C is the operator
surface: a CLI, an API, and a panel page — mirroring the LLM backend registry's surface,
deliberately narrower. The on-demand test is the one action that surface was still missing: a way
to find out whether a backend actually works, right now, without embedding real data through it
first.

**What the on-demand test added:**

- `adi_embeddings::{test_backend, test_manifest}` (`crates/adi-embeddings/src/ondemand.rs`): embed
  one short, fixed string through the backend's own runtime and report success, the vector's width,
  and how long it took. `test_manifest` takes a manifest directly rather than an id, so a **draft**
  — a form an operator is still typing into, never saved — can be tested exactly as it stands;
  `test_backend` is the thin id-based wrapper over it, for a saved row. Neither touches a hold,
  because there is no hold here to touch (see "Why this is narrower than the LLM design").
- **The width is the test.** Every vector a base holds is recorded against the backend's declared
  `dimensions`, so a runtime that quietly returns a different width is reported as a **failure**,
  not a success at the wrong number — a mismatch here would otherwise poison every future search
  against whatever base trusted it.
- **CLI**: `adi-mono embeddings test <id> [--json]`.
- **API**: `POST /api/embeddings/backends/test`, accepting either a saved backend's `id` or a
  `draft` (the same shape `save` takes) — `draft` wins when both arrive. Always a `200`: the
  verdict (`ok`/`failed`), a human-readable message and the elapsed milliseconds are the payload,
  not the status code, because a failed test is a successful answer to "does this work."
- **Panel**: a **Test** button beside **Save** in the editor, sending the form as currently edited
  (a draft, whether or not it has ever been saved) and showing the verdict inline once it lands —
  green for an answer (with the width and the latency), plain text carrying the runtime's own error
  otherwise. Shared with the LLM backends editor's identical button through one
  `crate::ui::test_verdict_view`, since both answer with the same wire shape.

**What phase C changed:**

- `adi-embeddings` gained two small, purely computed additions the surface needed and the crate is
  the honest place to answer from: `Runtime::available()` (cheap and local — whether *this binary*
  can build a runtime, `false` only for `candle` without the feature) and `Runtime::fixed_model()`
  (the model+width `candle`/`hash` always produce, `None` for `ollama`/`openai`, which take both
  from the manifest). `CANDLE_FIXED_MODEL` is now a named constant instead of a literal duplicated
  in `seed.rs`, and a feature-gated test pins it against the real embedder's constants so the two
  cannot silently drift apart. A new `ensure_seeded(&Config)` exposes the seeding step on its own —
  the "one-time migration command that seeds a store explicitly" this doc used to list as
  deferred — without paying `resolve`'s cost of going on to *build* an embedder afterwards.
- **CLI**: `adi-mono embeddings backends|show|save|delete|settings|seed`, reached through
  `adi_core::embeddings` (a whole-module re-export beside `adi_core::llm`) rather than importing
  `adi-embeddings` directly, mirroring `llm.rs`. `settings` doubles as the "what does each consumer
  currently resolve to" answer an operator actually asks for, and `--assign consumer=backend`
  changes one row while leaving the others untouched. No `Holds`/`Release`/`Probe`/`Migrate` —
  nothing here is rate-limited the way a chat subscription is.
- **API**: `GET /api/embeddings/backends` (listed in `SHARED_GETS`, so concurrent pollers share one
  read), `POST /api/embeddings/backends/save|delete`, `POST /api/embeddings/settings`. All four
  live in `adi-webapp-api` and run on the blocking pool like every other handler there — never an
  async worker, per `adi-app/src/main.rs`'s own note on why `/api/knowledge/search` is the same
  way. Delete is refused while a consumer is still assigned to a backend (409); a settings save is
  refused if it names a backend that is not here (400) — the same "catch it at save time" instinct
  `EmbeddingBackends::save` already applies to a fallback list.
- **Not in the live channel** (`adi-app/src/live.rs`'s `watchable`): the LLM backend registry is
  watched because the prober and other machines' runs move it without anyone touching the panel;
  nothing here has an equivalent mover, so the page fetches once on open — the same pattern
  `/api/knowledge` already uses, and for the same reason.
- **Panel**: a page at `/settings/embedding-backends`, modelled on the LLM backends page and much
  smaller — no warnings view, no rules view, no probe view. Page-local state
  (`EmbeddingsConsole`), like the Knowledge page's console, since nothing here rides the shell's 4s
  poll. Shows every backend with a live "available in this binary" status, a whole-object editor
  whose fields depend on the runtime (`candle`/`hash` ask for nothing but the runtime itself;
  `ollama` asks for a host, model and width; `openai` adds a base URL and the API key's environment
  variable name — never the key), and a per-consumer assignment panel showing what `indexer`,
  `knowledge` and `facts` each resolve to right now and whether that assignment would actually
  work.
- **Left undone, deliberately**: the panel's wasm build was verified with `cargo check --target
  wasm32-unknown-unknown` and native `cargo check`/`clippy`/tests only — nobody loaded it in a
  browser this pass, so "it renders correctly" is not a claim this note makes.

**What phase B changed:**

- `adi-knowledge`'s `KnowledgeStore::with_config` and `adi-facts`'s `FactStore::with_config` now
  build their `EmbedderSlot` by calling `adi_embeddings::resolve(&config, CONSUMER_*)` instead of
  a hardcoded `CandleEmbedder`/`OllamaEmbedder::new()`. Laziness and the cached failure are
  unchanged — resolution still does not load a model until something calls `.get()`.
- `adi-indexer` itself was **not** touched: it cannot depend on the registry without a cycle
  (`adi-embeddings` depends on it). Instead its one production caller, `adi-cli`'s `indexer`
  command group, now resolves `CONSUMER_INDEXER` and calls `Indexer::open_with_embedder` instead
  of the feature-gated `Indexer::open`, falling back to `NoEmbedder` if the resolved backend
  cannot be built (a `candle` assignment in a build without the `candle` feature) — the same
  degradation `Indexer::open` gave by hand before.
- `adi-knowledge`'s own `candle` cargo feature now also turns on `adi-embeddings/candle`
  (`adi-knowledge/Cargo.toml`) — without that, a normal full-featured build would have the
  registry report `candle` as unavailable even though `adi-indexer/candle` is compiled in,
  because `adi-embeddings` is taken with `default-features = false` everywhere it is a
  dependency.
- **Surprise #2 (adi-knowledge search ranked wrong-model vectors as current):** `Backend::search_vectors`
  now takes the searching embedder's `model` and every implementation (SQLite, in-memory) filters
  on it — a row made by another model does not rank, full stop, closing the gap between the
  crate's own docs and what `search` actually did.
- **Surprise #1 (a model swap did not force re-indexing):** `index_project` now also forces the
  full-rebuild path (`rebuild = true`, the same one `PIPELINE_VERSION` forces) when the stored
  `embedding_model` differs from the handed-in embedder's — chosen over failing outright because
  the existing rebuild machinery already does exactly the right thing (every file reprocessed,
  `cache.get` already keyed on model) and an operator gets a working index back with no manual
  step. `adi-facts`'s own near/search paths were checked too: `vector_of` already re-embeds on a
  cached-vector model mismatch per node, so it was never exposed to this hole.
- `adi-embeddings`'s own tests remain on the `hash` runtime throughout — no test here loads a
  real candle model or talks to a live ollama.

**Everything phase A and B deferred is now built.** See "What phase C changed," above, and "Out of
scope for this phase" at the bottom for what stays declined outright.

## The problem

Every embedder in this tree is built a different way, and none of the four ways can be changed
without a rebuild:

- `adi-indexer` hardcodes jina-embeddings-v2-base-code as Rust `const`s, gated by the `candle`
  cargo feature.
- `adi-knowledge` borrows that embedder when `candle` is on, and falls back to a word-overlap
  stub when it is off — also a compile-time choice.
- `adi-facts` embeds with a *different* model, nomic-embed-text, over a local ollama, moved only
  by the `ADI_FACTS_OLLAMA` / `ADI_FACTS_EMBED` environment variables read once at process start.
- `EmbeddingConfig` (`crates/adi-indexer/src/embed/config.rs`) already has the shape of a
  config-driven multi-provider setting — `provider`, `model`, `dimensions`, `batch_size`,
  `api_key`, `api_base` — and is never read by anything that builds an embedder. It is a design
  nobody wired up, not a design nobody wanted.

The consequence: there is no CLI or API surface today that lets an operator choose or configure
any of this, no way to point a knowledge base at a hosted model without a rebuild, and no place
that can answer "what embedders does this binary have, and which one is each store using" in one
list.

## The principle

**A consumer names an assignment. An assignment names a backend. A backend is one complete way
to turn text into a vector** — a runtime, a model, and the dials it runs with, under a name a
human chose.

This is the LLM backend design (`docs/llm-backends.md`), carried over for the same reason it was
built there: a login-plus-configuration is one flat thing, not a preset layered on a record. It
stops at that one borrowed idea, though — an embedding backend has no chat to fail over *within*,
so most of the LLM design's other half (holds, a prober, ask-on-switch, full-transcript replay)
has nothing to attach to here. See "Why this is narrower than the LLM design" below.

**Two backends are interchangeable only when they would write the same vector space.** Every
stored vector already records the model that made it, in every store in this tree
(`adi-knowledge`'s `embedding.model`, `adi-facts`'s vector rows, the indexer's file cache) — this
design does not get to relax that invariant, it has to keep it honest through a layer that did
not exist before. A registry that let two different models stand in for each other silently
would be strictly worse than the hardcoded status quo, which at least never mixes them within one
store.

## One object, one list

```
CONSUMER    indexer · knowledge · facts                        which store is asking?
   │
ASSIGNMENT  knowledge -> "candle"       settings.toml, one row per consumer, the only
            facts     -> "ollama"       place a consumer's choice of backend is recorded
            indexer   -> "candle"
   │
BACKEND     runtime · model · dimensions · dials · fallbacks   one flat object
              candle = in-process    · jina-embeddings-v2-base-code · 768
              ollama = local http    · nomic-embed-text             · 768 · fallback: (none)
```

**There is no fourth layer.** A consumer's assignment names exactly one backend; the backend's
own `fallbacks` list is the only failover, and it is validated, not free-form (see below).

## The backend

One file per backend under `embeddings/backends/<id>.toml` in the mono store
(`adi_config::Module`), mirroring `llm/backends/<id>.toml` exactly: **the id is the filename,
and there is no `id` field to drift from it.**

```toml
# embeddings/backends/ollama.toml
label = "Local ollama — nomic-embed-text"

runtime     = "ollama"                 # candle | ollama | openai | hash
model       = "nomic-embed-text"
dimensions  = 768

base_url    = "http://127.0.0.1:11434"
# api_key_env = "OPENAI_API_KEY"       # openai only — the env var the key is read from

fallbacks = ["ollama-hosted"]          # ids of other backends, tried in order if this one fails
```

- **`runtime`**, not `provider`. The survey found "provider" already meaning three different
  things in this codebase before this design added a fourth: `adi-knowledge`'s `Provider` trait
  is a *storage* backend, `EmbeddingConfig.provider` is dead code naming an embedding backend,
  and an LLM backend's `provider` is the wire name of a hosted chat API. `runtime` names the
  concern precisely — which of the four ways in this crate builds the `Embedder` — without
  colliding with any of them.
- **`model`** and **`dimensions`** are both required and both plain fields, not inferred: they
  are what the same-model failover check compares, and inferring them from the runtime would mean
  trusting a network call or a loaded model just to validate a save.
- **`base_url`** — the ollama host, or the OpenAI-compatible endpoint's base. Unused by `candle`
  and `hash`.
- **`api_key_env`** — the environment variable the key is read from, for `openai`. Mirrors
  `LlmBackendManifest::api_key_env` exactly: a name, not a secret, read at call time. No new
  dependency on `adi-secrets` for this — the platform's own precedent for a hosted API key is
  already "an env var name on the backend," not a secret-store integration, and matching it keeps
  one convention across LLM and embedding backends rather than two.
- **`fallbacks`** — an ordered list of other backend ids, tried in turn if this one's runtime
  fails to build or fails to answer. Lives on the backend, not on a consumer's assignment,
  because a fallback is a property of "how reliable is this way of embedding," which every
  consumer that names this backend should inherit alike.

## The four runtimes

| runtime  | what it is | model | notes |
|----------|-----------|-------|-------|
| `candle` | in-process, `adi_indexer::embed::CandleEmbedder` | jina-embeddings-v2-base-code (fixed) | only exists in a binary built with the `candle` cargo feature; **listed as unavailable, not fatal, when it is not** |
| `ollama` | local HTTP, one request per text | whatever `model` says | preserves `adi-facts`'s one-request-per-text behaviour deliberately — see below |
| `openai` | any OpenAI-compatible `/v1/embeddings` endpoint | whatever `model` says | one batched request per call, `Authorization: Bearer` from `api_key_env` |
| `hash`   | deterministic word-overlap, no model, no network | `hash-bow-256` (fixed) | the existing stub, nameable so tests and `--no-default-features` builds can ask for it explicitly instead of falling into it by default |

**A backend this binary cannot build is listed, not hidden and not fatal.** A build with the
`candle` feature off still lists a `candle` backend if one is on disk; asking the registry to
resolve it returns `EmbedError::Unavailable` with a message naming the missing feature, the same
shape `adi_indexer::embed::NoEmbedder` already answers with. This is the one behaviour the survey
flagged as non-negotiable (§7): the cargo feature is a compile-time fork, and a registry that
pretended every listed provider always works would lie to exactly the build that turned one off
on purpose.

**`ollama` keeps `adi-facts`'s one-request-per-text calibration.** `OllamaEmbedder::embed` still
sends `texts.iter().map(embed_one).collect()` rather than batching through `/api/embed` — the
recall thresholds in `adi-facts`'s own docs were measured through the single-request endpoint,
and batching differently would be a second, unmeasured configuration wearing the same model name.

**`openai` batches.** There is no prior calibration to preserve here — this runtime does not
exist in the tree today — so it takes the more efficient shape: one `/v1/embeddings` call with
every text in `input`, ordered by the response's own `index` rather than trusted to arrive in
request order.

## The assignment

`embeddings/settings.toml` holds one thing: which backend each consumer uses.

```toml
[assignments]
indexer   = "candle"
knowledge = "candle"
facts     = "ollama"
```

A consumer names itself with a short fixed string (`indexer`, `knowledge`, `facts` today); an
unassigned consumer is a resolution error, never a silent default to whatever backend happens to
exist. There is no `default_backend` and no inheritance between assignments — each consumer's
row is independent, exactly as the LLM design has no `default_backend` because the default is
just the first row of a list. Here there is no list to be first in; there is exactly one
assignment per consumer, and changing it is the whole of "point this store at a different model."

## Resolution

```
resolve(consumer):
    seed_if_never_seeded()                         # see Seeding, below
    id := settings.assignments[consumer]           # error if the consumer has no row
    backend := store.get(id)                       # error if the id names nothing
    chain := [backend] + backend.fallbacks
               .filter(|f| f.model == backend.model && f.dimensions == backend.dimensions)
    return FailoverEmbedder(chain.map(build))       # build() may itself fail per-runtime;
                                                     # a failed member is skipped, not fatal,
                                                     # unless every member fails
```

The chain is filtered by model and dimensions **again** at resolve time, not only at save time —
a fallback that matched when it was named may have been edited since, and a stale mismatch must
be dropped rather than trusted. Dropping it silently is correct here in a way it would not be for
the primary: the primary backend is what the consumer explicitly asked for, and its own mismatch
would be a config error worth surfacing, but a *fallback* going stale is exactly the kind of
drift the validation exists to make survivable — the assignment still resolves, just without the
now-untrustworthy fallback behind it.

The returned `Embedder` tries each surviving chain member in order on every `embed()` call and
returns the first success — so a runtime failure (an unreachable ollama, a network blip against a
hosted endpoint) fails over per call, not only at start-up. A chain of one (the common case, no
fallback configured) costs nothing extra: it is the primary, tried once.

## The same-model failover rule, and why it is narrower than the LLM's

The LLM design fails over between backends naming **different** models and even different
vendors — Anthropic to GLM — because a chat conversation survives that: `docs/llm-backends.md`'s
whole "handoff" section exists to replay the transcript into whatever answers next. An embedding
has no equivalent replay. A vector already written under one model does not become a vector from
another model by asking nicely; it is simply wrong from that moment on, silently, for every
future comparison against it. Surprise #3 in the survey is exactly this failure mode already
latent in the tree — two different 768-dimensional models sitting one string-equality check away
from being treated as the same space.

So embedding failover is deliberately the one case where **cross-backend rerouting is refused,
not merely discouraged**: `EmbeddingBackends::save` and `resolve` both reject a fallback whose
`model` or `dimensions` differ from the backend that names it. The case this narrow rule serves
is real and worth building for — a local `ollama` running `nomic-embed-text` falling over to a
hosted copy of the exact same model when the local server is down — and it is the *only* case a
fallback list is for. Everything else is explicit assignment: wanting a different model for a
consumer means editing that consumer's row in `settings.toml`, not adding it as a fallback.

## Seeding

**Upgrading must change no behaviour.** On the first call to `resolve` against a store that has
never been seeded (`embeddings/settings.toml` does not yet exist — its presence is the one-shot
marker, exactly as a migration's version stamp is, not "the store currently has zero backends"),
the registry materializes the backends and assignments that reproduce today's hardcoded behaviour
exactly:

- a `candle` backend named `candle`, `model = "jinaai/jina-embeddings-v2-base-code"`,
  `dimensions = 768` — matching `adi_indexer::embed::candle`'s `MODEL_ID`/`DIMENSIONS` constants,
  which this file duplicates rather than imports (see "Known issues" below for why coupling to
  them any tighter is not worth it yet);
- an `ollama` backend named `ollama`, `model` and `base_url` read from `ADI_FACTS_EMBED` and
  `ADI_FACTS_OLLAMA` **at seed time**, falling back to `nomic-embed-text` and
  `http://127.0.0.1:11434` exactly as `adi_facts::embed::OllamaEmbedder::new()` does today;
- assignments pointing `indexer` and `knowledge` at `candle`, and `facts` at `ollama` — the exact
  pairing every consumer already has hardcoded.

Seeding runs once. A machine whose `ADI_FACTS_OLLAMA` changes after that first resolve does not
move the `ollama` backend's `base_url` — exactly the LLM design's own migration is a one-time
lift out of ambient configuration, not an ongoing mirror of it. An operator who wants the backend
to point somewhere else edits the backend, the same way they would edit any other.

## The on-demand test

Unlike the LLM design, there is no background sweep here to be distinct from — nothing in this
registry is rate-limited the way a chat subscription is, so nothing needed a prober in the first
place. The on-demand test is simply the first thing that ever asks a backend anything, other than
a real caller storing real vectors: a human pressing **Test**, or running `adi-mono embeddings test
<id>`.

```
test(manifest):
    embedder := build(manifest)              # the same construction resolve() uses, no fallback
    vector   := embedder.embed([TEST_TEXT])  # one short, fixed string
    if manifest.dimensions != 0 && vector.width != manifest.dimensions:
        fail("returned a Nw vector, but this backend declares M")
    else:
        ok(vector.width)
```

Taking a manifest rather than an id is what lets a **draft** be tested — the whole point of a
**Test** button beside a form that has not been saved yet. `EmbeddingBackends::build`'s body moved
out to a free `build_embedder(&EmbeddingBackendManifest)` for exactly this: building an embedder
has never needed the store's `Config`, only the manifest, and a draft has no id to open a
`Config`-backed store with.

An undeclared width (`dimensions == 0`, the state of a form nobody has finished filling in) is not
checked against — there is nothing yet to disagree with.

## Known issues

**The candle attention-mask bug is out of scope for this phase**, and the code is deliberately
untouched. `JinaBertModel::forward` (`crates/adi-indexer/src/embed/candle.rs:258-266`) is never
handed the attention mask, so softmax runs over padded positions and a text's vector depends on
what else shared its batch — up to six points of cosine drift on a ranking scale whose useful
range is a few tenths, pinned by an `#[ignore]`d test that is never run in CI
(`candle.rs:474-513`). A registry sitting on top of this runtime inherits the bug unchanged: a
`candle` backend's vectors are not reproducible across re-indexes with different batch
composition today, registry or not. Fixing it changes every vector already stored and needs its
own re-embed and its own measurement — tracked separately, not folded into this work.

## Decisions taken 2026-09-12

The operator's calls, and the ones made here to fill in what they left to the implementation:

1. **A new `adi-embeddings` crate**, sitting where the survey's §6 recommended a registry crate —
   depending on `adi-indexer` (the `Embedder` trait, `EmbedError`, and, feature-gated,
   `CandleEmbedder`) and `adi-config` (the `Module` store pattern), depended on by `adi-knowledge`
   and `adi-facts` today and by `adi-cli`, `adi-webapp-api` and `adi-app` once phase B wires them
   in. Not inside `adi-agents`: that crate sits *above* `adi-knowledge` in the dependency graph
   (`adi-agents` depends on `adi-knowledge`, not the reverse), so a registry living there could
   never be reached by the two crates that most need it without a cycle.
2. **`embeddings/backends/<id>.toml` plus `embeddings/settings.toml`**, mirroring `llm/`
   exactly, id-as-filename included.
3. **The field is `runtime`, not `provider`** — see "The backend," above.
4. **Failover only between a backend and a fallback declaring the same model and dimensions**,
   refused at both save time and resolve time. See "The same-model failover rule," above.
5. **Staleness is keyed on the model name, never on width.** Two 768-dimensional models are not
   the same model (survey Surprise #3); nothing in this design compares vectors, or lets a
   fallback stand in for a primary, on width alone.
6. **A backend this binary cannot build is listed as unavailable, not fatal.** A `--no-default-
   features` build (or one with `adi-embeddings`'s own `candle` feature off) still lists a
   `candle` backend on disk and reports why resolving it fails, rather than hiding the entry or
   erroring the whole store.
7. **`HashEmbedder` moves down from `adi-knowledge` and `OllamaEmbedder`'s HTTP core moves down
   from `adi-facts`**, with re-exports kept at both old paths. A registry that cannot list every
   runtime in one place is not a registry (survey §6) — but `adi-facts`'s public
   `OllamaEmbedder::new()`/`::at()` and `adi-knowledge`'s public `HashEmbedder` keep working
   unchanged for every existing caller and test.
8. **The candle attention-mask bug is out of scope.** See "Known issues," above.
9. **`api_key_env`, not a secrets-store lookup**, for the `openai` runtime's key. Matches
   `LlmBackendManifest::api_key_env` exactly, and avoids a new dependency edge from
   `adi-embeddings` onto `adi-secrets` for a pattern the platform already has a convention for.
10. **`hash` and `candle` backends must declare the exact model and width their runtime actually
    produces**, checked at save time wherever this crate can know the answer without a network
    call (always for `hash`; only in a build with the `candle` feature for `candle`, since the
    constant lives behind that feature). Neither runtime takes its model from configuration — a
    manifest that claimed otherwise would be lying about what a stored vector actually is, the
    exact failure mode "the principle" exists to close off.

## Out of scope

**Everything the survey's §4 recommended not copying from the LLM design, for the same reasons it
gave:** holds and a prober (nothing here is rate-limited the way a chat subscription is — `hash`
and `candle` are local and free, and `ollama`/`openai` failing over on the same request is already
handled by the chain itself, not by a background sweep) · `classify`/`failover`'s
quota/rate/auth/transient taxonomy (an embedding call either works or it doesn't; there is no
"ask the human" step to gate) · `ask_on_switch` (there is no live conversation to protect from a
switch happening under it). Phase C's surface followed this all the way through: no
`Holds`/`Release`/`Probe`/`Migrate` verb, no hold column, no probe view.

**Built in phase C, smaller than the LLM surface it mirrors, on purpose:**

- No rename-with-repoint. `LlmBackends::save` accepts a `rename_from` because an LLM backend can
  be listed by many agents' rows, each a separate file a rename would otherwise strand. An embedding
  backend has at most three possible namers — the fixed `indexer`/`knowledge`/`facts` rows in one
  `embeddings/settings.toml` — so a rename is delete-the-old, save-the-new, then repoint the (at
  most three) assignments by hand through `embeddings settings --assign`. Adding rename support
  would be solving a fan-out problem that does not exist here.
- No dedicated seed API endpoint. `ensure_seeded` is CLI-only (`adi-mono embeddings seed`),
  mirroring `llm migrate`'s own CLI-only status — a one-time operational action, not a page an
  operator watches.
- The panel's assignment editor never calls `resolve`; it shows what the assignments file says and
  whether the named backend is here and available (`Runtime::available`), never whether it would
  actually answer a text — that would mean building it, which for `candle` is a model load neither
  a page load nor a row edit should pay for.

**Declined outright:** cross-model failover (the entire point of the narrower rule above) ·
inheritance between backends (declined for the same reason the LLM design declined it —
configuration, not programming) · a `batch_size` dial resurrected from the dead `EmbeddingConfig`
(nothing in any of the four runtimes reads one yet; adding the field back without a reader would
repeat survey Surprise #5, not fix it).
