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
    ├── adi-core/         # the platform command surface (Adi/Dns: enable, disable, status…)
    ├── adi-cli/          # the `adi-mono` binary — a thin argv adapter over adi-core
    └── adi-dns/          # the local DNS resolver (split-DNS overrides + forwarding)
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

## Common commands

```bash
cargo build              # build the whole workspace
cargo test               # test everything
cargo fmt                # format
cargo clippy --workspace # lint
cargo run -p <crate>     # run a specific binary crate
```
