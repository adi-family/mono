# adi-ports-manager

Allocate and track TCP ports for adi services — a **pure library** (no CLI, no
daemon). Other crates (`adi-core`, `adi-hive`, …) link it to get collision-free
ports instead of hand-picking numbers in `hive.yaml`.

Every port it hands out avoids three things:

1. **Reserved bands** — privileged ports (`0..=1023`) and the `adi daemon`
   supervisor band (`15000..=15999`, around ADI DNS on `127.0.0.1:15353`, which must
   never be disturbed — see the repo `CLAUDE.md`).
2. **Ports already live on the machine** — probed by trying to `bind` them on
   loopback.
3. **Ports it has already promised** — tracked static leases.

## Two allocation modes

| Mode        | API                        | Persisted? | Use when                                            |
| ----------- | -------------------------- | ---------- | --------------------------------------------------- |
| **static**  | `reserve(service, key)`    | yes        | the port must be stable across restarts (so config, resolvers, docs can name it) |
| **dynamic** | `allocate_dynamic()`       | no         | any free port will do; ephemeral / throwaway        |

`reserve` is idempotent: the same `(service, key)` always returns the same port. The
read-modify-write is serialized by a cross-process lock file, so concurrent callers
never race onto the same port.

```rust
use adi_ports_manager::Ports;

let ports = Ports::new();
let http    = ports.reserve("frontend", "http")?; // stable across restarts
let scratch = ports.allocate_dynamic()?;          // ephemeral, not persisted
ports.release("frontend", "http")?;               // free the lease
# Ok::<(), adi_ports_manager::Error>(())
```

## State

Static leases persist as a small JSON array at
`~/.adi/mono/ports/registry.json`. The path comes from the shared `adi-config` store
(the `ports` module, honoring `$ADI_DIR`); this crate owns the JSON format and keeps it
there as a raw file. Dynamic allocations are computed on the fly and never recorded.

The `(service, key)` keys mirror `hive.yaml`'s `rollout.recreate.ports` slots
(`http`, `db`, …), so a future integration can populate those ports from here.

## Configuration

`Ports::new()` uses the defaults (8000s range, the two reserved bands, the standard
registry path). Build a `Config` by hand to widen the range, add reserved bands, or
point the registry elsewhere — `Ports::with_config(config)`.
