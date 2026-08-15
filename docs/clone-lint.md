# `adi-clone-lint` — duplication as a rustc lint

A [dylint](https://github.com/trailofbits/dylint) library that reports duplicated logic in Rust
by comparing **AST paths** through rustc's HIR. It lives in `lints/adi-clone-lint`, outside the
workspace, and warns — it never denies.

```bash
cargo dylint --lib adi_clone_lint --workspace     # the whole tree
cargo dylint --lib adi_clone_lint -- -p adi-indexer
```

```
warning: duplicated logic: 12 lines, the same as `total` with the variables renamed
  --> src/main.rs:36:5
   |
36 | /     let mut acc = 0;
37 | |     for item in items {
...  |
47 | |     acc
   | |_______^
   |
note: first written here
  --> src/main.rs:21:5
   = note: variables renamed: `acc` stands where `sum` does, `items` stands where `rows` does
   = note: literals that differ: `100` becomes `200`
   = help: extract the shared part into a function, or say why the copies must stay apart
```

## What makes it different from `indexer clones`

The two answer the same question with different machinery, and neither replaces the other.

| | `adi-mono indexer clones` | `adi-clone-lint` |
| --- | --- | --- |
| sees | the whole tree at once | one crate at a time |
| languages | 13 | Rust |
| unit | a whole symbol | any run of statements |
| renaming | inferred from the text | **proved** from `Res::Local` |
| answers | which symbols share a shape | which lines duplicate which, and *what differs* |

Reach for the indexer to sweep the tree; reach for the lint when you want the exact extent of a
duplication and the list of what changed between the copies.

## AST paths

The indexer flattens a symbol's parse tree into a pre-order list of node kinds and hashes it.
That finds whole-symbol copies and nothing smaller, and any edit shifts the whole sequence.

This lint describes a stretch of code by the **set of walks between nearby leaves** of its tree —
up from one leaf to the lowest common ancestor and back down to another. `total += row.weight`
contributes, among others:

```
path ^ assign:+= v field v path
```

A fragment is then a *bag* of those paths, and two fragments are alike to the degree their bags
agree. Three things follow, and all three are why the representation was worth the trouble:

- **An edit stays local.** Inserting a statement perturbs only the paths that touch it, so
  similarity degrades in proportion to the edit rather than the sequence shifting wholesale.
- **Sibling order stops mattering**, because a bag has no order.
- **Partial overlap becomes a number.** `|A ∩ B| / |A|` is what "ten lines of this function also
  appear in that one" actually means, and a yes/no hash cannot express it.

## Two strengths of finding

**Exact** — the fragments are alpha-equivalent. Locals are renumbered by the order they are first
seen and the tree is hashed; equal hashes mean the bindings correspond one for one.

This is the part working on HIR buys. A source-level tool has to guess whether two occurrences of
`x` are the same variable; rustc has already resolved them, so each leaf carries the `HirId` of
the binding it refers to and "the same binding" is a fact rather than a spelling match. Because
the correspondence is an alignment, the diagnostic can name it: `sum` stands where `acc` stands,
`100` becomes `200`. Those literals are exactly the arguments a merged version would need.

**Near** — the path bags overlap by at least `min_similarity`. MinHash estimates the overlap in
32 words, and LSH banding proposes candidate pairs so the search is not all-pairs.

Findings are ranked and pruned so each duplicated region is reported once: an exact finding
replaces a near one that restates it, unless the near one is substantially larger, and any group
whose members all overlap a group already kept is dropped.

## Configuration

`dylint.toml` at the workspace root, under `[adi-clone-lint]`. Defaults shown.

| key | default | what it does |
| --- | --- | --- |
| `min_nodes` | 25 | floor on a fragment's size in HIR-derived nodes (~5 lines) |
| `min_lines` | 5 | floor on a fragment's height in source lines |
| `max_run` | 12 | longest run of consecutive statements treated as one fragment |
| `max_width` | 4 | how far apart two leaves may be to have a path between them |
| `max_length` | 12 | longest path, in labels passed through |
| `min_similarity` | 0.85 | overlap a near match must reach |
| `min_paths` | 20 | paths a fragment needs before its similarity score means anything |
| `report_near` | true | set false for exact findings only |

`min_nodes` is the one to reach for. It started at 40, which sounds conservative and quietly cost
the main case the lint is for: the shared part of two functions is smaller than either of them,
so a floor set for whole functions never sees it.

Suppress a finding the ordinary way — `#[allow(adi_duplicate_code)]` on the statement or the
enclosing item. Diagnostics are attached to the HIR node, so a local allow works.

`ADI_CLONE_LINT_DEBUG=1` prints every candidate fragment with its size and canonical hash, then
every group. A finding that never appears looks exactly like a finding that was never a
candidate, and this is what tells the two apart.

## Two limits worth knowing

**It is per crate.** rustc lints run inside one compilation, so a duplicate whose halves sit in
two crates is invisible here. That is what `adi-mono indexer clones` is for.

**A confirmed clone is not automatically a refactor.** Leptos `view!` functions are identical in
shape because that is what the macro expands to; `as_str` implementations over parallel enums are
the same `match` over different variants. Both are reported and both should stay. The lint finds
candidates; judgement is still yours.

## Working on it

The crate is pinned to `nightly-2025-12-04` with `rustc-dev` and `llvm-tools-preview`, and is its
own workspace root — a `rustc_private` crate must never be pulled into the tree's stable
workspace. `cargo test` runs unit tests over the grouping plus a `ui/` test whose expected output
is `ui/main.stderr`.

Four things that are easy to get wrong and cost real time:

- **`cargo dylint` only knows this library because the root `Cargo.toml` names it.** The tree is
  not this crate's workspace, so nothing links the two implicitly; `[workspace.metadata.dylint]`
  there points at `lints/adi-clone-lint`. Take that entry out and every command above fails with
  `Could not find `--lib adi_clone_lint``, which reads like a build failure and is not one — the
  alternative spelling, if you ever need it, is `DYLINT_LIBRARY_PATH` pointed at the built
  `target/debug`.
- **`Span::from_expansion` is not "came from a macro".** It is also true of desugarings, and HIR
  is what the compiler produces *after* desugaring, so every `for`, `?` and `.await` carries a
  desugaring context. Filtering on it skips most of the control flow in the crate. `tree::from_macro`
  walks the expansion chain and only rejects `ExpnKind::Macro`.
- **Desugared spans break span comparisons.** A `for` loop's span has a non-root `SyntaxContext`,
  and two spans with different contexts neither contain nor overlap each other — so a loop was
  never recognised as sitting inside its own block, and nested findings survived pruning.
  Fragment spans are normalised with `source_callsite()` before anything compares them.
- **`dylint_linting` names the pass struct for you.** `impl_late_lint!` expands to
  `impl_lint_pass!([<ADI_DUPLICATE_CODE:camel>] => ...)`, so the struct must be called
  `AdiDuplicateCode`, and it declares `extern crate rustc_lint` and `rustc_session` itself.
- **`dylint-link` must be the linker** (`.cargo/config.toml`), or the library builds and then
  cannot be loaded: dylint looks it up by a toolchain-suffixed filename that only that wrapper
  produces.
