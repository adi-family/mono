# adi-config

The **configurator** for the adi platform — a **pure library** (no CLI, no daemon)
that owns *where* settings live and gives every subsystem a scoped place to keep them.

Before this crate, each crate re-derived `$HOME` / `$ADI_DIR` / `mono` on its own
(`adi-core/paths.rs`, `adi-ports-manager`, `adi-hive`). That layout knowledge now lives
here, once.

## The model

The store is **one directory** — the `mono` dir. Modules are directories inside it.

```
Config (store)  ──►  ~/.adi/mono                (one dir; honors $ADI_DIR, default .adi)
  └─ module("hive")  ──►  ~/.adi/mono/hive       (a settings *directory*)
       ├─ file::<T>("settings.toml")             typed TOML config: load / default / create / save
       └─ raw_path / read_raw / write_raw        raw file storage (the module owns the format)
```

- **A module is always a directory.** `store.module("hive")` is a handle to
  `~/.adi/mono/hive`; nothing is created on disk until a write happens.
- **Typed config files** are TOML. `module.file::<Settings>("settings.toml")` gives a
  `ConfigFile<Settings>` with `load`, `load_or_default`, `load_or_create` (materializes
  the default on first run), and atomic `save`.
- **Raw files** are for modules that own their own on-disk format — JSON, YAML, a log,
  a socket path. The store only decides *where* they go.

```rust
use adi_config::Config;

let store = Config::open();            // ~/.adi/mono  (Config::with_root(..) in tests)
let hive  = store.module("hive");

// Typed TOML, created from Default on first run:
let settings: HiveSettings = hive.file("settings.toml").load_or_create()?;

// Or a raw file whose format the module owns:
hive.write_raw("hive.yaml", bytes)?;
let path = hive.raw_path("hive.yaml");
```

## Who uses it

| Crate               | What it keeps                                      | How |
| ------------------- | -------------------------------------------------- | --- |
| `adi-ports-manager` | `ports/registry.json` (the static-lease ledger)    | raw file (owns the JSON) |
| `adi-hive`          | `hive/hive.yaml` (the proxy + runner config)        | raw file (owns the YAML) |
| `adi-core`          | `dns/` (daemon config + status) and `paths::support_dir()` | `module("dns")` + layout delegation |

Only `serde` + `toml` are pulled in — the typed-config format. Raw storage and the path
layout are plain `std`.
