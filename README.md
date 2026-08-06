# adi-family

A Rust monorepo. All crates live under [`crates/`](crates/) and share one
[Cargo workspace](Cargo.toml).

## Layout

```
.
├── Cargo.toml            # workspace root: members, shared deps, lints, profiles
├── rust-toolchain.toml   # pinned toolchain + components
├── rustfmt.toml          # formatting config
└── crates/
    ├── adi-core/          # the platform command surface (Adi/Dns: enable, disable, status…)
    ├── adi-cli/           # the `adi-mono` binary — a thin argv adapter over adi-core
    ├── adi-dns/           # the local DNS resolver (split-DNS overrides + forwarding)
    ├── adi-hive/          # reverse proxy / .adi front door + runs & supervises service runners
    ├── adi-agents/        # agent definitions stored under ~/.adi/mono/agents
    ├── adi-tasks/         # task tree stored under ~/.adi/mono/tasks
    ├── adi-indexer/       # the code index: tree-sitter symbols, call graph, semantic search (docs/indexer.md)
    ├── adi-mesh/          # the fleet over iroh: remote nodes at *.n.adi, raw port forwards (docs/fleet.md)
    ├── adi-ports-manager/ # port allocator: collision-free static + dynamic ports (library)
    ├── adi-update/        # auto-update engine: one artifact per platform, every binary in it (docs/adi-update.md)
    ├── adi-app/           # the adi app served at app.adi: control-panel SPA + Rust /api backend
    └── adi-structs/       # repo tool: writes each crate's structs.gen.md (never shipped)
```

Frontends (e.g. the macOS menu-bar app in [`apps/`](apps/)) own no control logic —
they trigger `adi-core` commands by running `adi-mono` and render its JSON status.

## Adding a crate

```bash
cargo new --lib crates/my-crate     # library
cargo new crates/my-app             # binary
```

New crates are picked up automatically by the `crates/*` glob in the workspace
`members`. Have each crate inherit shared metadata and lints:

```toml
[package]
name = "my-crate"
version.workspace = true
edition.workspace = true

[lints]
workspace = true
```

Declare shared dependency versions once in the root `[workspace.dependencies]`
and reference them per crate with `some-dep = { workspace = true }`.

## Type pages (`structs.gen.md`)

Every crate carries a generated `structs.gen.md` at its root: each struct, enum and
type alias it declares, reduced to its shape — derives, serde attributes, fields,
variants — with the prose stripped. It is the map of what a subsystem moves around,
readable on one page instead of chased across forty files, and a change to it shows up
as a diff in review.

```bash
scripts/install-hooks.sh              # once per clone — see below
cargo run -p adi-structs              # regenerate every page
cargo run -p adi-structs -- adi-agents  # just one crate
cargo run -p adi-structs -- --check   # fail if any page is stale (CI)
```

The generator parses the source with `syn` and never builds it, so it also works on a
crate that does not currently compile.

## Git hooks

The repo's hooks live in [`.githooks/`](.githooks/); `.git/hooks` is per-clone and
never travels with a checkout, so point git at them once:

```bash
scripts/install-hooks.sh
```

`pre-commit` refreshes the `structs.gen.md` pages and stages the ones that moved,
whenever a commit touches `crates/*/src/**.rs` or a crate manifest. Skip it for one
commit with `git commit --no-verify`.

## Common commands

```bash
cargo build              # build the whole workspace
cargo test               # test everything
cargo fmt                # format
cargo clippy --workspace # lint
cargo run -p <crate>     # run a specific binary crate
```

## License

Business Source License 1.1 — see [LICENSE](LICENSE). Free for personal,
educational, research, and small-business use; larger commercial use needs a
separate license from the Licensor (https://the-ihor.com).
