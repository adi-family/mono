# adi-app

The **adi app** — the real thing served at `app.adi`. One small Rust process with two
roles:

- `GET /` (and any non-`/api` path) → the **control-panel SPA**, a single embedded
  HTML file (no build step, no runtime asset paths).
- `/api/*` → the **Rust backend**: a JSON API over the live
  [`adi-ports-manager`](../adi-ports-manager) port registry.

adi-hive fronts it: `app.adi → 127.0.0.1:<port>`, and adi-hive's runner launches and
supervises this process, injecting `$PORT`. So the browser hits `app.adi`, the SPA
loads, and its `fetch('/api/...')` calls land on this same process through the proxy.

## API

| Method | Path                      | Does                                                        |
| ------ | ------------------------- | ----------------------------------------------------------- |
| GET    | `/api/health`             | `{ ok, service, version, uptime_secs }`                     |
| GET    | `/api/ports`              | `{ range, reserved[], leases[] }` — the allocator's state   |
| POST   | `/api/ports/reserve`      | body `{ service, key }` → reserves a static port, returns `{ port }` |
| POST   | `/api/ports/release`      | body `{ service, key }` → releases it, returns `{ freed }`  |

The reserve/release endpoints mutate the real registry at
`~/.adi/mono/ports/registry.json` (honoring `$ADI_DIR`), so the panel is operating
actual platform state, not a mock.

## Run

```sh
adi-app            # listens on 127.0.0.1:$PORT (else :8090)
adi-app 8091       # explicit port
adi-app 127.0.0.1:8091
```

Under adi-hive it needs no arguments — the runner injects `$PORT`. See
[`hive.example.yaml`](./hive.example.yaml) for the `app.adi` wiring.

## Design

Hand-rolled HTTP/1.1 (`src/http.rs`), the same dependency-light approach as adi-hive's
proxy — no web framework. Every response is `Connection: close`, so each request is its
own connection and there's no keep-alive framing to track. The API handlers
(`src/api.rs`) are pure `(status, json)` functions over an `adi_ports_manager::Ports`,
so they're unit-tested without a socket. The SPA (`web/index.html`) is `include_str!`'d
into the binary.
