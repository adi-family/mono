# The code indexer

`adi-mono indexer` builds a searchable index of a project's source, and answers questions about
it. `crates/adi-indexer` is the library; `crates/adi-cli/src/indexer.rs` is the argv adapter.

## What it does

One tree-sitter parse per file feeds four things:

- **symbols** — every function, type, and method with its location and doc comment, in SQLite
  with an FTS5 index over names and docs;
- **references** — who calls whom, which `graph.rs` turns into callers, callees, transitive
  reachability, cycles, and entry points, and `analyzer/` turns into a dead-code report;
- **embeddings** — a 768-dimension vector per symbol from `jina-embeddings-v2-base-code`,
  stored in a usearch index, so a search matches meaning rather than spelling;
- **structural fingerprints** — the shape of each symbol's syntax with the names taken out, so
  copy-paste is recognisable as copy-paste.

```
$ adi-mono indexer index                      # parse, store, embed (only what changed)
Indexed 466 / 466 files, 7245 symbols

$ adi-mono indexer search "restart a launchd service" --limit 3
3 result(s):
  0.623  function restart_onto (crates/adi-core/src/update.rs:192)
        fn restart_onto(app: &Path)
  0.423  method Supervisor::reconcile (crates/adi-hive/src/runner.rs:62)
  0.419  method Adi::ensure_enabled (crates/adi-core/src/commands.rs:118)
```

`search` ranks by meaning; `symbols` and `files` are full-text over names and paths; `similar`
finds code like a given symbol; `clones` reports duplication; `tree` prints the symbol tree;
`status` says what is indexed; `languages` says what this binary can parse. Every subcommand
takes `--path` (default: the current directory) and `--json`.

## Finding code like other code

Two questions that sound alike and are answered by different machinery.

**What means something like this** — `similar` puts a symbol's own embedding back in as the
query, so it costs one index lookup and no embedding:

```
$ adi-mono indexer similar restart_onto --limit 3
```

**What looks like this** — `--structural` compares syntax shape instead, which finds the copy
whose names, types and literals all changed:

```
$ adi-mono indexer similar restart_onto --structural --distance 6
```

**What is duplicated anywhere** — `clones` groups symbols that share a shape. `--distance 0`
(the default) reports exact matches; a non-zero distance also groups the copies that have since
drifted:

```
$ adi-mono indexer clones --min-nodes 40
$ adi-mono indexer clones --min-nodes 40 --distance 8    # include drifted copies
```

`--min-nodes` is not decoration. Every codebase has thousands of three-node accessors that share
a shape and mean nothing by it, and a report without a floor is all of them.

Containers — modules, classes, traits — are left out unless you pass `--all-kinds`. A container's
shape is its children's concatenated, so it repeats what they already said, and being the largest
symbol in any index it says it first: with them in, this repository's near-duplicate report was
eighteen straight rows of `module tests`, which is true and useless.

### How the fingerprint works

Walking a symbol's subtree and keeping only the *kinds* of its named nodes discards every
identifier and literal on the way — `fn a(x: u32)` and `fn b(y: u64)` reduce to the same
sequence. Renaming a copy therefore cannot hide it. Comments are skipped, so the same code with
and without one is the same shape. From that sequence come two numbers:

| | what it answers | how |
| --- | --- | --- |
| `structure_hash` | same shape or not | SHA-256 of the kind sequence, truncated to 64 bits |
| `structure_simhash` | *how close* two shapes are | simhash over 3-gram shingles; `structure::hamming` counts the differing bits |

Distance is in bits out of 64: 0 is identical and unrelated code sits near 32, so single digits
are the useful range. Note that distance is only meaningful at size — a four-line function's
fingerprint moves a long way on any edit at all, which is the other reason `--min-nodes`
exists.

The limits worth knowing: this finds code with the same *syntax*, so it will not recognise two
functions that reach the same result by different constructs (that is what `similar` without
`--structural` is for), and `--distance` clustering is greedy — a symbol close to two groups
lands in whichever came first, and is reported once.

## Where things live

| what | where |
| --- | --- |
| a project's index | `<project>/.adi/tree/index.sqlite` + `.adi/tree/embeddings/` |
| project settings | `<project>/.adi/config.toml` |
| user settings | `~/.adi/mono/indexer/config.toml` |
| parse + embedding cache | `~/.adi/mono/indexer/cache/` |

The cache is content-addressed by SHA-256 of the file bytes, and shared machine-wide: the same
file across ten worktrees is parsed and embedded once. Embeddings in it carry the model name, so
changing models invalidates exactly the vectors it should and keeps the parse results.

The index is derived state and is gitignored — rebuild it with `indexer index`.

A run also takes out what it no longer finds. Incremental indexing compares the content hash of
each file it walks, and a file deleted since the last run is never walked — so nothing in that
comparison is in a position to notice it left, and without a separate pass its symbols answer
searches and pair with live code in duplication reports forever. The walk is that pass: whatever
the index holds and the walk did not reach — deleted, newly ignored, grown past the size limit —
goes, along with its symbols, the references at either end of them, its full-text rows and its
embeddings. Scope is the walk's root, which is the project the index belongs to.

Absence only counts where the walk could actually look. A directory it failed to read returns no
files, which is indistinguishable from a directory whose files were all deleted — so anything
under a path the walk reported an error for is left alone, and a walk whose error named no path
at all prunes nothing. A prune that removes more than half the index is logged at `warn`: a
branch switch legitimately looks like that, and so does an ignore rule that matched more than its
author meant it to.

### References are stored twice, resolved and not

`symbol_refs` holds edges between symbol ids. `pending_refs` holds what the parser actually saw —
a symbol, a name it mentions, and where — and the resolved table is derived from it on every run.

The second table exists because a resolved edge cannot outlive its target. Reprocessing a file
deletes its symbols and inserts new ones, and `AUTOINCREMENT` never reuses an id, so every edge
pointing *into* that file dies with the old ids. Rebuilding those edges needs the parse of the
*referring* file — which an incremental run does not have, having skipped that file precisely
because it had not changed. The graph therefore lost edges on every run and only a full rebuild
restored them: measured on this tree, changing one file cost 982 edges, and an index grown across
a working session had lost 41% of the graph.

Keeping the unresolved form makes the graph a function of the symbol table as it stands rather
than of which files a run happened to touch. Resolution reads every `pending_refs` row and
rewrites `symbol_refs` whole; on this tree that is 92k rows in and 213k edges out, and it is why a
run resolves the same graph whether one file changed or all of them did.

The cost is disk: roughly 80% on top of the index, for rows that duplicate what the source already
says. A code index that quietly forgets edges is worse.

### Two version numbers, and why both exist

Indexing is incremental twice over, and both layers key on file content — which means a change
to *this crate* is invisible to them. The file did not change, so the cache serves what it has
and the index skips the file entirely. Each layer therefore carries a version to compare
against, and bumping it is what turns a change here into a rebuild:

- `cache::SCHEMA_VERSION` — what a cache entry holds. A mismatch drops the entry, so the file is
  reparsed and re-embedded rather than served with a fingerprint field that did not exist when
  it was written, or a vector built from a different text.
- `indexer::PIPELINE_VERSION` — what the pipeline stores. Recorded in the index's `status`
  table; a mismatch makes the next run reprocess every file once, after which incremental runs
  resume.

Neither is cosmetic: without them the symptom is not an error but quietly worse answers. Bump
the one that applies whenever you change what gets stored or how the embedded text is built.

## Languages

Grammars are **linked into the binary**, one cargo feature each (`lang-rust`, `lang-typescript`,
…, `all-languages` for the lot, all on by default):

rust · typescript · javascript · python · go · java · c · c++ · c# · php · ruby · lua · swift

A language with a grammar but no dedicated analyzer still indexes — `parser/treesitter/
analyzers/generic.rs` reads the node kinds that recur across languages. `adi-mono indexer
languages` prints what the running binary actually carries.

## What gets embedded

The text standing in for a symbol is:

```
kind name | signature | doc comment | body | kind name | signature | doc comment
```

Declaration first because truncation bites at the end — what a symbol *is* survives it, and
only the tail of what it does is lost. Declaration **again** at the end because the embedding is
a mean over token vectors, so a part's influence is just its share of the tokens, and a 60
character signature inside a 1200 character body is three percent of the answer.

That second copy is not a refinement, it is the difference between working and not. Embedding
bodies without it measurably traded one failure for another: on this repository "restart a
launchd service" had ranked `restart_onto` first at 0.623, and once the body diluted the
signature the function fell out of the top *fifteen*, replaced by generic service-handling code.
Repeating the declaration buys back roughly double its share for a few dozen tokens.

The body is capped twice. `MAX_BODY_BYTES` (1200) cuts the source slice, and the tokenizer
truncates at `MAX_TOKENS` (512) as the backstop that matters — the model reads 8192, but the
ALiBi bias is precomputed at its position limit and sliced to the batch's sequence length, so an
untruncated long symbol is a hard error that takes its whole batch's embeddings down with it.

Batches are bounded by **padded area** — length × width — not length. Attention costs the square
of the padded sequence length, and bounding length alone is not a weaker version of the same
rule: a batch of 32 symbols at the token limit put a multi-gigabyte command buffer on the GPU
and left the indexer sitting in `wait_until_completed` at 0% CPU indefinitely. Under the area
rule short symbols still batch wide and only long ones pack thin.

Including bodies is what makes `search` and `similar` rank on what code does. Before it, both
saw only the declaration surface — two functions with identical bodies and different names
looked unrelated, and two unrelated functions with similar names looked identical.

## Semantic search is a feature

The `candle` feature (on by default) pulls in candle + tokenizers + hf-hub and the embedding
model, which is most of what the indexer costs `adi-mono`: the release binary went from 21 MB to
49 MB when this group landed. On macOS the same build also gets Metal; elsewhere
`Device::new_metal` fails at run time and embedding falls back to the CPU.

Building `adi-indexer` with `--no-default-features` drops all of it. Indexing, symbol/file
search, the call graph, dead-code analysis, and everything structural — `clones`, and `similar
--structural` — all still work, because a fingerprint comes from the parse and not the model.
`indexer search` and a bare `indexer similar` then report that this build has no embedder
instead of pretending to rank by meaning.

The model is downloaded on first use into the standard Hugging Face cache
(`~/.cache/huggingface`), not into the mono store.

## How it got here

It was three cdylib plugins in the older `adi-family/cli` tree — `adi.indexer` plus eleven
`adi.lang.*` — discovered under a plugins directory at run time, dlopen'd, and called across a
v3 plugin ABI. The move dropped all of that machinery:

- language analyzers became modules that receive the tree the parser already built, instead of
  ABI calls that re-parsed the source themselves;
- the plugin manager became `lang::grammar` / `lang::analyzer`, two matches over cargo features;
- `lib-embed` became `embed/` with the one backend a shipped build used;
- `lib-migrations` became `migrations/runner.rs`, forward-only against SQLite;
- `lib-cli-common::AdiUserDirs` became `paths.rs` over the mono store, so `$ADI_DIR` is honored.

Two upstream behaviours survived the move and are worth knowing, each pinned by a test in
`parser/tests.rs`:

- the Rust analyzer never reads `pub` — every Rust symbol is stored with `Visibility::Unknown`,
  which is why the dead-code analysis's public-symbol filter finds nothing to keep in a Rust
  tree;
- an inline `mod foo { … }` is indexed as a module and its body is not descended into.
