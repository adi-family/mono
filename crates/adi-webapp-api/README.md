# adi-webapp-api

The **contract** between the adi webapp and its host — the one place the `/api/*` shapes
are defined, so the client and server can't drift.

- [`types`](src/types.rs) — a plain serde struct per JSON payload (`Health`, `PortsState`,
  `Lease`, `LeaseRef`, `ReserveResponse`, …). No I/O, no platform deps, so it compiles for
  `wasm32-unknown-unknown`. The Leptos frontend ([`adi-webapp`](../adi-webapp)) depends on
  this crate for exactly these types.
- [`handlers`](src/handlers.rs) — the server backend over the live
  [`adi-ports-manager`](../adi-ports-manager) registry. Each handler is a pure
  `(status, json) ` function, unit-tested without a socket. Gated behind the **`server`**
  feature (it pulls in filesystem I/O and is native-only).

```
adi-webapp   ── depends on ──▶  adi-webapp-api            (types only)
adi-app      ── depends on ──▶  adi-webapp-api + "server" (types + handlers)
```

## Feature flags

| Feature  | Adds                                       | Who enables it |
| -------- | ------------------------------------------ | -------------- |
| *(none)* | `types` only — the wasm-safe wire structs  | adi-webapp     |
| `server` | `handlers` over `adi_ports_manager::Ports` | adi-app        |
