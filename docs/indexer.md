# The code indexer

`adi-mono indexer` builds a searchable index of a project's source, and answers questions about
it. `crates/adi-indexer` is the library; `crates/adi-cli/src/indexer.rs` is the argv adapter.

## What it does

One tree-sitter parse per file feeds three things:

- **symbols** — every function, type, and method with its location and doc comment, in SQLite
  with an FTS5 index over names and docs;
- **references** — who calls whom, which `graph.rs` turns into callers, callees, transitive
  reachability, cycles, and entry points, and `analyzer/` turns into a dead-code report;
- **embeddings** — a 768-dimension vector per symbol from `jina-embeddings-v2-base-code`,
  stored in a usearch index, so a search matches meaning rather than spelling.

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

`search` ranks by meaning; `symbols` and `files` are full-text over names and paths; `tree`
prints the symbol tree; `status` says what is indexed; `languages` says what this binary can
parse. Every subcommand takes `--path` (default: the current directory) and `--json`.

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

## Languages

Grammars are **linked into the binary**, one cargo feature each (`lang-rust`, `lang-typescript`,
…, `all-languages` for the lot, all on by default):

rust · typescript · javascript · python · go · java · c · c++ · c# · php · ruby · lua · swift

A language with a grammar but no dedicated analyzer still indexes — `parser/treesitter/
analyzers/generic.rs` reads the node kinds that recur across languages. `adi-mono indexer
languages` prints what the running binary actually carries.

## Semantic search is a feature

The `candle` feature (on by default) pulls in candle + tokenizers + hf-hub and the embedding
model, which is most of what the indexer costs `adi-mono`: the release binary went from 21 MB to
49 MB when this group landed. On macOS the same build also gets Metal; elsewhere
`Device::new_metal` fails at run time and embedding falls back to the CPU.

Building `adi-indexer` with `--no-default-features` drops all of it. Indexing, symbol/file
search, the call graph, and dead-code analysis all still work; `indexer search` then reports
that this build has no embedder instead of pretending to rank by meaning.

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
