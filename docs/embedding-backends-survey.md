# Embedding backends — survey

Survey only. Nothing in the tree changed except this file. Every claim below is cited
`file:line` against the checkout at `workspaces/main`, read on 2026-09-12.

## Surprises

1. **A model swap does not trigger re-embedding in the indexer**, unlike every other store in
   the tree. `process_file` skips a file entirely — before it ever looks at the embedder — when
   the file's own content hash is unchanged
   (`crates/adi-indexer/src/indexer/mod.rs:541-551`), and the only thing that forces a full
   `rebuild` is `PIPELINE_VERSION` (`crates/adi-indexer/src/indexer/mod.rs:61-63`), never
   `Status.embedding_model`. So swapping the candle model without bumping `PIPELINE_VERSION`
   leaves every *unchanged* file's vectors in the usearch ANN index exactly as they were,
   silently mixed with freshly-embedded ones for touched files, forever, until someone forces a
   rebuild by hand. `adi-knowledge`'s per-note `is_stale(model)` check
   (`crates/adi-knowledge/src/note.rs:115-118`) has no equivalent gate at the file-walk level in
   the indexer — only inside the *cache lookup* that a walk has to reach first
   (`crates/adi-indexer/src/cache.rs:89-101`).

2. **`adi-knowledge`'s own search does not filter by model at query time**, despite the crate's
   module docs stating flatly that "a vector whose model no longer matches is treated as
   absent" (`crates/adi-knowledge/src/embed.rs:14`). `KnowledgeStore::search` embeds the query
   and calls `backend.search_vectors(&vector, limit)`
   (`crates/adi-knowledge/src/lib.rs:646-663`), and the SQLite backend's `search_vectors` reads
   every row in the `vectors` table and scores it with no model or staleness check at all
   (`crates/adi-knowledge/src/backend/sqlite.rs:252-277`). The "treated as absent" promise is
   kept only by `reembed` proactively overwriting stale rows
   (`crates/adi-knowledge/src/lib.rs:698-714`); a partial/failed reembed (network drop mid-sweep)
   leaves same-dimension, wrong-model vectors that rank in search results as if current.
   `cosine()` only guards against a *dimension* mismatch, returning `0.0`
   (`crates/adi-knowledge/src/backend/mod.rs:220-223`) — it says nothing about whether two
   same-width vectors came from the same model, which they provably do not, always, across
   `adi-knowledge` (jina, 768-wide) and `adi-facts` (nomic, 768-wide) — see #3.

3. **The two real embedding models in the tree happen to share a width (768) by coincidence**,
   which is exactly the case the dimension check in #2 cannot catch. `adi-knowledge`'s default
   is jina-embeddings-v2-base-code at 768 dims (`crates/adi-indexer/src/embed/candle.rs:19`);
   `adi-facts`'s is nomic-embed-text, also declared 768
   (`crates/adi-facts/src/embed.rs:47`). They are different vector spaces and the crate's own
   docs say so explicitly (`crates/adi-knowledge/src/embed.rs:9-14`) — but nothing structural
   stops a future registry entry from pointing a knowledge base at "nomic" and a facts base at
   "jina" and getting plausible-looking, wrong answers, because the only thing standing between
   "different model" and "silently comparable" is a string equality check that a caller has to
   remember to make (and, per #2, `search` doesn't make it).

4. **The candle embedder has a known, documented, *unfixed* correctness bug**: attention runs
   over padded positions because `JinaBertModel::forward` is never handed the attention mask
   (`crates/adi-indexer/src/embed/candle.rs:440-452`). A symbol's vector — and therefore its
   search rank — depends on which other symbols happened to share its batch. The comment
   estimates six points of cosine drift on a ranking scale whose useful range is a few tenths,
   pinned by an `#[ignore]`d test (`crates/adi-indexer/src/embed/candle.rs:474-513`) that is
   never run in CI. Any embedding-backend design that treats "candle" as an interchangeable
   provider needs to know its current implementation is not reproducible across re-indexes with
   different batch composition — this is not a hypothetical remote-vs-local wrinkle, it is a bug
   in the one local provider that exists today.

5. **`EmbeddingConfig` — the one piece of code in the tree that already looks exactly like a
   config-driven multi-provider embedding setting (`provider`, `model`, `dimensions`,
   `batch_size`, `api_key`, `api_base`) — is dead weight.** It is defined in
   `crates/adi-indexer/src/embed/config.rs:15-28`, folded into `IndexerConfig`
   (`crates/adi-indexer/src/config.rs:9-14`), and has a `merge` method with its own tests — but
   `CandleEmbedder::new()` takes no arguments at all (`crates/adi-indexer/src/embed/candle.rs:289`)
   and reads a hardcoded `MODEL_ID`/`DIMENSIONS`
   (`crates/adi-indexer/src/embed/candle.rs:18-19`). Nothing in the tree ever constructs a
   `CandleEmbedder` from an `EmbeddingConfig`. A backend registry is not a green field here; it
   is filling in a shape that already exists on paper and was never wired to the one embedder
   that runs.

## 1. Every construction point of an embedder

| Site | Builds | Configured today by | Called by |
|---|---|---|---|
| `adi_indexer::embed::default_embedder` — wait, no such function; see below | — | — | — |
| `adi_indexer::embed::CandleEmbedder::new()` (`crates/adi-indexer/src/embed/candle.rs:288-352`) | jina-embeddings-v2-base-code, downloaded via `hf_hub` on first use, on Metal/CUDA/CPU by `select_device` (`candle.rs:357-365`) | Nothing — `MODEL_ID`/`DIMENSIONS`/`MAX_TOKENS` are `const`s (`candle.rs:18,19,27`); gated only by the `candle` cargo feature | `adi_knowledge::embed::default_embedder` (`crates/adi-knowledge/src/embed.rs:36-47`, `#[cfg(feature = "candle")]`); the indexer's own `Indexer::open`-style entry (see `crates/adi-indexer/src/indexer/mod.rs` — embedder is passed in as `&Arc<dyn Embedder>` to `process_file`, built by whoever calls the indexing entry point, e.g. `adi-cli`'s `indexer index`) |
| `adi_knowledge::embed::default_embedder()` (`crates/adi-knowledge/src/embed.rs:36-47`) | `CandleEmbedder` with `candle` feature, else `HashEmbedder` | The `candle` cargo feature on `adi-knowledge` (`crates/adi-knowledge/Cargo.toml:17-18`, on by default) | `EmbedderSlot::default()` (`embed.rs:65-70`), which is what `KnowledgeStore` falls back to when nothing is injected |
| `adi_knowledge::embed::EmbedderSlot` (`crates/adi-knowledge/src/embed.rs:58-133`) | Lazily builds and caches whatever `build` closure it is given (default or injected), and caches a load *failure* too | Constructed either `injected(Arc<dyn Embedder>)` or `lazily(build_fn)` | `KnowledgeStore` (holds one as `self.embedder`, `crates/adi-knowledge/src/lib.rs` — used at `search`, `add`/`update`, `reembed`, `base_status`) |
| `adi_knowledge::HashEmbedder` (`crates/adi-knowledge/src/embed.rs:145-193`) | Deterministic bag-of-words, 256-dim, no model, no network | Compiled in whenever `candle` is off, or injected explicitly in tests | `default_embedder()` fallback (`embed.rs:44-46`); test fixtures across `adi-knowledge` and `adi-webapp-api` (`crates/adi-webapp-api/src/handlers/knowledge.rs:441-450`) |
| `adi_facts::OllamaEmbedder::new()` / `::at(host, model)` (`crates/adi-facts/src/embed.rs:62-79`) | `nomic-embed-text` over a local ollama's `/api/embeddings`, one HTTP request per text | `ADI_FACTS_OLLAMA` (host) and `ADI_FACTS_EMBED` (model) env vars (`embed.rs:37-40`, via `crate::ollama::env_or`) | `default_embedder()` inside `adi-facts` (`crates/adi-facts/src/lib.rs:1032-1033`), fed into its own `EmbedderSlot::lazily` (`lib.rs:260`) |
| `adi_indexer::embed::NoEmbedder` (`crates/adi-indexer/src/embed/mod.rs:37-57`) | Nothing — errors on every call | Compiled in whenever `candle` is off in `adi-indexer` itself | Whatever constructs an indexer without the `candle` feature; search degrades to FTS |

There is **no CLI or API surface today that lets an operator choose or configure any of
this.** Every knob above is either a Rust `const`, a cargo feature resolved at compile time, or
a bare environment variable read once at process start. Contrast with LLM backends, which are
entirely operator-editable at runtime through a store, an API and a CLI (§4–5).

## 2. The `Embedder` trait

Exact surface, `crates/adi-indexer/src/embed/mod.rs:21-31`:

```rust
pub trait Embedder: std::fmt::Debug + Send + Sync {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    fn dimensions(&self) -> u32;
    fn model_name(&self) -> &str;
}
```

Implementors found in the tree: `CandleEmbedder` (`adi-indexer`), `NoEmbedder`
(`adi-indexer`), `HashEmbedder` (`adi-knowledge`), `OllamaEmbedder` (`adi-facts`), plus
test-only stand-ins in `adi-indexer/src/indexer_tests.rs:36` and
`adi-indexer/src/indexer/mod.rs:927`.

What a remote/HTTP implementation would need that this does not offer, verified against the
one HTTP implementation that already exists (`OllamaEmbedder`):

- **No batching contract.** The signature takes `&[&str]` but nothing says whether an
  implementor may, should, or must send them together. `CandleEmbedder` batches (and has the
  padding bug in Surprise #4 as the cost of doing so); `OllamaEmbedder` explicitly does *not* —
  `embed` is `texts.iter().map(|t| self.embed_one(t)).collect()`, one HTTP round trip per text
  (`crates/adi-facts/src/embed.rs:125-127`, with the comment at `embed.rs:118-124` explaining
  why it stayed that way — matching what the calibration measurements were taken through). A
  registry manifest would want a `batch_size` dial; `EmbeddingConfig` already has one
  (`crates/adi-indexer/src/embed/config.rs:23`) that nothing reads (Surprise #5).
- **No async.** Every call site in the tree runs this on a blocking thread pool by design —
  `adi-app`'s dispatcher explicitly keeps `/api/knowledge/search` off the async workers because
  "a search may load the embedding model, which is seconds of CPU the async workers must not
  spend" (`crates/adi-app/src/main.rs:722-724`). `OllamaEmbedder` uses
  `reqwest::blocking::Client` (`crates/adi-facts/src/ollama.rs:73-76`), consistent with that
  convention. This is a constraint a remote backend must follow, not a hole the trait needs
  filled — but it does mean an embedding backend that wants real concurrency (many parallel
  HTTP calls to a hosted API) gets none for free; it would have to build its own thread pool
  inside one `embed()` call.
- **No per-item error reporting.** `embed` returns `Result<Vec<Vec<f32>>>` for the whole batch.
  `OllamaEmbedder::embed`'s `.collect()` over a `Result` iterator (`embed.rs:126`) means one bad
  text anywhere in a batch discards every vector already computed for the texts before it, and
  the caller cannot tell which item failed without re-deriving it from the error string.
- **No timeout, retry, or auth concept.** `Ollama::at` hardcodes a 900s timeout
  (`crates/adi-facts/src/ollama.rs:49-51`) with no per-call override; there is no header, token,
  or `api_key_env`-equivalent anywhere in the embedding stack. Every hosted embedding API worth
  naming (OpenAI, Voyage, Cohere) needs an `Authorization` header, and nothing here has ever
  had to send one.
- **No rate-limit/quota classification.** LLM backends have `LimitClass`, `Holds`, and a
  `Prober` (§4). Nothing analogous exists for embeddings: a 429 from a hosted embedding API
  today is just `EmbedError::Embedding(String)` (`crates/adi-indexer/src/embed/error.rs`, used
  throughout) — a flat string, not a class, with no store to record "this credential is
  spent" and nothing to retry it later.
- **`dimensions()` is asserted, not negotiated.** The usearch vector index is opened once at a
  hardcoded 768 (`crates/adi-indexer/src/search/usearch.rs:28-33`) and `add`/`search` reject a
  vector of the wrong width with an error (`usearch.rs:85-89, 114-117`) — safe, but a registry
  that lets an operator pick *any* backend for a given base would need to either rebuild the ANN
  index per width or refuse the pairing up front; nothing today does either because nothing
  today lets the width vary at runtime.

## 3. Model identity beside stored vectors, and what happens on mismatch

| Store | Where the model is recorded | What happens on mismatch |
|---|---|---|
| **`adi-indexer`'s file cache** (`crates/adi-indexer/src/cache.rs`) | `CachedFileData.embed_model` (`cache.rs:38`), one string per cached file, alongside `schema_version` | `GlobalCache::get` compares it against the *current* embedder's `model_name()` (`cache.rs:73,89`); a mismatch returns the parsed data with `embeddings: Vec::new()` (`cache.rs:92-101`) — but only for a file this run actually re-parses. See Surprise #1: an **unchanged file is never handed to `cache.get` at all**, because the walk's own file-hash check short-circuits first (`crates/adi-indexer/src/indexer/mod.rs:541-551`). So a global model swap re-embeds only files that also changed content; everything else keeps its old vectors in the ANN index indefinitely. |
| **`adi-indexer`'s SQLite status table** | One global `embedding_model` key (`crates/adi-indexer/src/storage/sqlite.rs:832,851-852`), written once per full index run (`indexer/mod.rs:157`) | Purely descriptive — a status line (`adi-mono indexer status`, presumably). Nothing reads it back to decide whether to rebuild; only `PIPELINE_VERSION` does (`indexer/mod.rs:61-63`). |
| **`adi-knowledge`** | `Knowledge.embedding.model` per note (`crates/adi-knowledge/src/note.rs:65`), stored as the `embed_model` column (`crates/adi-knowledge/src/backend/sqlite.rs:156`) | `Knowledge::is_stale(model)` compares both the model name and the content hash (`note.rs:115-118`); `base_status`, `list(stale_only)`, and `reembed` all check it (`crates/adi-knowledge/src/lib.rs:449-465, 619-625, 711`) and a mismatch is reported/re-embedded through those paths. **But `search`'s `search_vectors` does not check it at all** (Surprise #2) — a stale-model row still scores and ranks until something explicitly re-embeds it. |
| **`adi-facts`** | Not read during this survey in the same depth as knowledge, but the crate's own docs state the identical invariant ("storing the model's name beside every cached vector and treating a row from any other model as absent", `crates/adi-facts/src/embed.rs:26-29`) and the `vectors` table exists (`sqlite3 …/facts.db .tables` shows a `vectors` table). Not verified further whether its query path enforces the check any more strictly than `adi-knowledge`'s does — flagged as **unverified**, worth checking before relying on it. |

## 4. The LLM backend machinery, as a template

All in `crates/adi-agents/src/llm/`, doc'd in `docs/llm-backends.md` (status line in that file
says "not implemented" — **stale**; the code, tests, API, CLI and UI below are real and
exercised by passing tests, verified 2026-09-12).

- **`LlmBackendManifest`** (`crates/adi-agents/src/llm/backend.rs:193-241`) — one flat TOML per
  backend (`llm/backends/<id>.toml`, id = filename, no `id` field), holding `runtime`, `model`,
  `context_tokens`, the four credential fields (`settings`/`provider`/`base_url`/`api_key_env`),
  free-form `params` (dials), `limit_rules`, and an optional `probe`.
  **For embeddings:** the credential/dial split and the "id is the filename" convention both
  transfer cleanly. `context_tokens` does not — there is no embedding analogue of "this much
  history fits." `runtime` doesn't either in the same shape, since there's no equivalent runner
  registry; a `provider` enum (`candle` | `http` | `ollama`) would do the equivalent job.
- **`LlmBackends`** (`backend.rs:362-462`) — the on-disk store: `list`/`get`/`save`/`delete`
  over `adi_config::Module`, timestamps stamped by the store, name validated against the
  existing agent-name rule. **For embeddings:** directly reusable shape, different module path
  (e.g. `embed/backends/<id>.toml`).
- **`LlmSettings`** (`crates/adi-agents/src/llm/settings.rs`) — two global switches
  (`ask_on_switch`, `probe_every`) in `llm/settings.toml`. **For embeddings:** `ask_on_switch`
  has no equivalent (there is no failover to ask about, per below); a `probe_every`-shaped
  setting only matters if a probe-and-hold system is built at all, which is not obviously
  justified (see below).
- **`Holds`** (`crates/adi-agents/src/llm/holds.rs`) — a SQLite table keyed on
  `(credential, model)`, recording "spent until T", shared across every backend naming the same
  credential. **For embeddings:** only makes sense if a remote/hosted embedding backend is
  added that can be rate-limited or quota-capped the way an LLM subscription is — plausible for
  a hosted API, pointless for `candle` (local, no login) or `ollama` (local server, no quota in
  practice). Worth building only alongside an actual hosted-API backend, not speculatively.
- **`classify`/`failover`** (`classify.rs`, `failover.rs`) — reading an error against
  `limit_rules` to decide `quota`/`rate`/`auth`/`transient`/`unknown`, and what a run does about
  each. **For embeddings:** there is no "failover chain" concept today — a knowledge base or
  facts base names exactly one embedder, not an ordered list of fallbacks — so this whole layer
  has nothing to attach to unless the design also invents an ordered per-base backend list,
  which is a much bigger step than the registry itself.
- **`Prober`** (`crates/adi-agents/src/llm/prober.rs`) — background sweep that proactively
  checks a held backend so a real request never has to discover recovery. **For embeddings:**
  same conditional as `Holds` — only earns its keep next to a hosted API backend with real rate
  limits; wasted machinery next to `candle`/`ollama`.
- **`migrate`** (`crates/adi-agents/src/llm/migrate.rs`) — one-time lift of settings baked into
  another object (the agent) out into a backend. **For embeddings:** there is a real migration
  waiting here too — `ADI_FACTS_OLLAMA`/`ADI_FACTS_EMBED` env vars and the `candle` cargo
  feature are exactly the kind of ambient, undiscoverable configuration this pattern was built
  to retire.

**Net assessment:** the store/manifest/CLI/API/UI shape (registry proper) transfers well. The
hold/prober/classify/failover half of the LLM design exists to solve a problem — a live chat
mid-conversation discovering a subscription is out of budget — that has no embedding analogue
today, because nothing embeds inside a live conversation the way a chat turn does, and no
embedding backend in the tree is rate-limited. Copying that half now would be building for a
hosted-API backend that does not exist yet.

## 5. Surface area a parallel "embeddings" feature would touch

**API** (`crates/adi-webapp-api/src/`):
- `handlers/llm_backends.rs` (whole file, 621 lines) is the template: `llm_backends`,
  `save_llm_backend`, `delete_llm_backend`, `save_llm_settings`, `release_llm_hold`, plus the
  DTO mapping functions at the bottom (`backend_dto`, `limit_rule_dto`, `probe_dto`,
  `hold_dto`, `settings_dto`, and the `wire`/`from_wire` enum-serialization helpers,
  `llm_backends.rs:260-359`).
- `types.rs` — `LlmBackendDto`, `LlmBackendsDto`, `SaveLlmBackend`, `LlmBackendRef`,
  `LlmSettingsDto`, `SaveLlmSettings`, `LimitRuleDto`, `ProbeDto`, `HoldDto`, `ContextWarningDto`,
  `DanglingRowDto` (imported at `llm_backends.rs:22-25`) — an embeddings equivalent needs its
  own DTO set, smaller (no hold/probe/context-warning shapes unless §4's conditional machinery
  is built).
- `handlers/knowledge.rs` already has the *consumer* side wired for one call site:
  `knowledge()` reports `providers()` and `model_name_if_loaded()`
  (`crates/adi-webapp-api/src/handlers/knowledge.rs:47-58`), and `KnowledgeStoreError::Embed`
  maps to a 503 (`knowledge.rs:421-424`) — a registry would change what feeds that model name
  and that error, not the shape of either.

**Routing** (`crates/adi-app/src/main.rs`):
- The route table entries at `main.rs:806-813` (`/api/llm/backends[/save|/delete]`,
  `/api/llm/holds/release`, `/api/llm/settings`) are the exact pattern to mirror.
  `/api/llm/backends` is also listed in `SHARED_GETS` (`main.rs:569`) so concurrent pollers
  share one read — an embeddings list route should join that list too.
- `crates/adi-app/src/live.rs:97-101` — the websocket "watchable" GET table explicitly
  special-cases `/api/llm/backends` with a comment explaining *why* it needs live polling (the
  prober and other machines change it without the panel's own action). An embeddings backends
  route needs the same entry **only if** it can change from outside the panel (a probe/hold
  system would justify it; a passive registry might not need to poll at all, the way
  `/api/knowledge` isn't in this table at all today).

**CLI** (`crates/adi-cli/src/llm.rs`, 687 lines) — `LlmCommand` (`Backends`, `Show`, `Save`,
`Delete`, `Settings`, `Holds`, `Release`, `Migrate`, `Probe`) and `run_llm`'s dispatch. An
`embed` command group would mirror `Backends`/`Show`/`Save`/`Delete` directly; `Holds`/
`Release`/`Probe`/`Migrate` only if §4's conditional machinery is built. Note this file imports
from `adi_core::llm`, not `adi_agents::llm` directly (`llm.rs:13-16`) — `adi-core` re-exports it
(§6) — so a new registry reached through `adi-core` the same way would keep the CLI's import
style consistent.

**Webapp UI** (`crates/adi-webapp/src/pages/llm_backends.rs`, 1585 lines) — by far the largest
single piece. It is not just a CRUD form: `editor_view`/`runtime_sections`/`login_view`/
`model_view`/`dials_view` (`llm_backends.rs:425-805`) dynamically render login/dial fields by
reading an `AgentFormSpec` schema per runtime (`crates/adi-webapp-api/src/types.rs:1020-1028,
1100-1187`) — the same schema the agent-creation form uses, so a runtime's fields are declared
once. An embeddings editor could be **much smaller** if it does not need runtime-conditional
field sets: `candle` has no fields at all, `ollama`/`http` need host+model+maybe-a-header. It
does not need `warnings_view`'s context-shrink logic (`llm_backends.rs:344-424`, no context
window concept) or most of `rules_view`/`probe_view` (`989-1136`, only relevant under §4's
conditional).

## 6. Cargo dependency reality and where a registry crate could live

Direct `adi-*` dependency edges observed in each crate's `Cargo.toml`:

```
adi-indexer          (owns Embedder trait, CandleEmbedder, NoEmbedder)
  ← adi-knowledge     (adi-indexer, default-features = false; HashEmbedder, EmbedderSlot)
      ← adi-facts     (adi-knowledge + adi-indexer directly; OllamaEmbedder)
      ← adi-agents    (adi-knowledge; owns the llm/ module — LlmBackends etc.)
          ← adi-core  (adi-agents, adi-knowledge, adi-db, …; re-exports adi_agents::llm as adi_core::llm)
              ← adi-app   (adi-core, adi-knowledge, adi-agents, adi-webapp-api[server])
              ← adi-cli   (adi-core, adi-indexer, adi-knowledge, adi-facts, adi-marketplace)
adi-webapp-api        (adi-agents, adi-knowledge, … all `optional = true`, feature-gated)
  ← adi-webapp        (adi-webapp-api, adi-ui)
```

(`crates/adi-indexer/Cargo.toml`, `crates/adi-knowledge/Cargo.toml:12-13`,
`crates/adi-facts/Cargo.toml`, `crates/adi-agents/Cargo.toml`, `crates/adi-core/Cargo.toml`,
`crates/adi-app/Cargo.toml`, `crates/adi-cli/Cargo.toml`, `crates/adi-webapp-api/Cargo.toml`.)

**Recommendation: a new low-level crate (e.g. `adi-embed-backends`) sitting beside
`adi-knowledge` and `adi-facts` in the graph — depending on `adi-indexer` (for the `Embedder`
trait and `EmbedError`) and `adi-config` (for the `Module`/manifest-file store pattern), and
depended on by `adi-knowledge`, `adi-facts`, `adi-cli`, `adi-webapp-api`, and `adi-app`.**

Reasoning:
- It must depend on `adi-indexer`, not the reverse — `adi-indexer` is the lowest crate that
  defines the trait, and it must stay ignorant of any registry or store built on top of it
  (parsing/indexing is a lower concern than "which model should I use").
- It must **not** live inside `adi-agents` the way `adi-agents::llm` does, because `adi-agents`
  sits *above* `adi-knowledge` in the graph (`adi-agents` depends on `adi-knowledge`, not the
  other way around) — `adi-knowledge` and `adi-facts` could not reach an embedding registry
  placed inside `adi-agents` without a cycle. LLM backends live inside `adi-agents` specifically
  because they are inseparable from `Agents`/`AgentBackendEntry` (repointing an agent's rows on
  rename, `crates/adi-webapp-api/src/handlers/llm_backends.rs:244-258`) — embedding backends
  have no comparable owning object; they're consumed independently by three siblings that must
  not depend on each other.
- `HashEmbedder` (currently in `adi-knowledge`) and `OllamaEmbedder` (currently in `adi-facts`)
  would need to move down into this new crate (or into `adi-indexer` itself) for it to offer a
  complete catalog of providers — right now each lives in the one crate that happens to use it,
  which is fine only as long as nothing needs to list "every embedder available" in one place,
  which a registry by definition does.

## 7. What would make a naive port wrong

- **Dimension is not model identity** (Surprise #3) — a registry must key staleness/comparability
  on the model *name*, never on width alone; two 768-dim models are not the same model, and
  nothing downstream of `cosine()`'s length check would notice if a registry conflated them.
- **The `candle` cargo feature is a compile-time fork, not a runtime choice.** `adi-knowledge`'s
  `candle` feature is `on` by default (`crates/adi-knowledge/Cargo.toml:18`) and `adi-app`
  depends on `adi-knowledge` with default features (`crates/adi-app/Cargo.toml:44`), so today's
  binary always ships the ~300MB jina weights path. A registry that offers "candle" as one
  choice among several needs that choice to exist in binaries that were built with the feature
  off too — meaning the registry has to degrade gracefully (report the provider as
  unavailable, the way `NoEmbedder` already does, `crates/adi-indexer/src/embed/mod.rs:40-48`)
  rather than assume every provider it lists can actually be built.
- **First-run download is synchronous and blocking.** `CandleEmbedder::new()` calls
  `hf_hub::api::sync::Api` three times in the constructor (`crates/adi-indexer/src/embed/candle.rs:302-314`)
  with no timeout, no progress reporting, and no offline fallback — a first search on a machine
  with no network hangs (or fails slowly) inside whatever request triggered the lazy load. A
  registry-driven "try this backend" UX needs to surface that wait explicitly rather than let a
  save/probe action block silently the way today's first `search()` call does.
- **Every embedding call in the tree is synchronous and expected to run on a blocking pool**
  (§2) — `adi-app`'s router deliberately keeps knowledge routes off async workers
  (`crates/adi-app/src/main.rs:722-724`). A registry's "probe this backend" action (if built)
  must follow the same convention or it will stall the async runtime the first time somebody
  points it at a slow hosted API.
- **The candle embedder's batching bug (Surprise #4) makes "re-embed and compare" unsound as a
  migration check.** A naive "does the new backend agree with the old one" smoke test that
  re-embeds a handful of symbols and diffs cosine scores will show drift that has nothing to do
  with the backend change — it's baseline noise in the existing implementation.
- **Corpora observed on this machine are small and not representative of production scale**:
  `~/.adi/mono/knowledge/agents/adi-agent/memory/knowledge.db` has 35 notes / 43 vectors,
  `.../reviewer/memory/knowledge.db` has 1/1, and `~/.adi/mono/facts/global/default/facts.db`
  has 1 row in its `vectors` table (checked directly with `sqlite3 … "select count(*) from
  …"`, 2026-09-12). This is a development sandbox with no indexer cache present at all
  (`~/.adi/mono/indexer` does not exist here). **Re-embedding cost at production scale is not
  observable from this environment** — a real estimate needs to come from a machine that has
  actually run `adi-mono indexer index` against a real tree, not from this survey.
- **Terminology collision**: "provider" means three different things across the three crates
  that would feed a registry — `adi-knowledge`'s `Provider` trait is a *storage* backend
  (sqlite/memory, `crates/adi-knowledge/src/backend/mod.rs:126-172`), the indexer's
  `EmbeddingConfig.provider` is an *embedding* backend name (`crates/adi-indexer/src/embed/config.rs:17`,
  currently only ever `"candle"`), and an LLM backend's `provider` field is the wire name for a
  hosted API (`anthropic`/`zai`/…, `crates/adi-agents/src/llm/backend.rs:218-220`). A new
  embedding-backend registry needs its own distinct field name for "which embedding provider"
  to avoid a fourth, colliding meaning of the same word in the same codebase.
