# adi-hive

The adi-family **reverse proxy** — an nginx-style, hostname-routed HTTP proxy. It
accepts HTTP on one or more local addresses and forwards each connection to a local
upstream chosen by the request's `Host` header.

It is built as a foreground daemon on the exact same pattern as
[`adi-dns`](../adi-dns): a supervisor owns its lifecycle, it reads its config, logs
to stdout/stderr, writes a JSON status file, and shuts down cleanly on SIGTERM.

## The single config

adi-hive reads **one** config: a `hive.yaml` at

```
~/.adi/mono/hive/hive.yaml        # honoring $ADI_DIR; the mono-app namespace
```

(or an explicit path passed as the first argument, for testing). If the file is
missing, adi-hive falls back to built-in defaults — bind `127.0.0.1:8080`, no routes —
so the daemon still runs.

The file is the nakit-yok **hive spec** format (see
`~/projects/nakit-yok/.adi/hive.yaml`). adi-hive parses only the proxy-relevant slice
and ignores the rest:

- `proxy.bind` — the addresses to listen on.
- for each service, `proxy.host` + its HTTP port (`rollout.recreate.ports.http`) —
  one routing rule `Host: <host> → 127.0.0.1:<http port>`.

Everything else in a hive.yaml (runner, healthcheck, environment, hooks, depends_on,
restart, defaults, observability, …) is accepted-but-ignored. A service without a
`proxy:` block is simply not routed. See [`hive.yaml`](./hive.yaml) for a worked
example.

## How it fits

adi-hive is the HTTP **front door** for the `.adi` zone:

1. **adi-dns** resolves `*.adi` to `127.0.0.53` (the front-door address).
2. **adi-hive** binds `127.0.0.53:80` and fans those hostnames out to per-service
   ports — `app.adi → 127.0.0.1:8010`, `api.adi → 127.0.0.1:8009`, …
3. A hostname that matches no service gets an animated `4XX` page (the same page
   adi-dns used to serve), so `http://anything.adi/` shows something real, not a bare
   connection error.

## Run

```sh
# reads ~/.adi/mono/hive/hive.yaml
adi-hive

# explicit config (testing)
adi-hive ./hive.yaml
```

The front-door `127.0.0.53:80` needs root — to bind `:80`, and on macOS to alias
`127.0.0.53` onto `lo0` first (adi-hive runs `ifconfig lo0 alias` for you). Each bind
is independent and non-fatal: a failure is logged and skipped, so an unprivileged run
still serves on the addresses it could bind (e.g. the `127.0.0.1:8080` dev fallback).
Run under a privileged supervisor to bind the front door.

## How it works

Hand-rolled L7 proxy, no HTTP framework. Per connection it reads the request head,
parses just the `Host` header to pick an upstream, forwards the original bytes
unchanged (upstream sees the real `Host`), then splices bytes both ways for the life
of the connection. A connection is pinned to one upstream (the first request's host),
which matches how browsers open a separate connection per hostname. A host that
matches no route gets the animated `404` page; a matched host whose upstream is
unreachable gets a small self-contained `502` — different failures, different pages.
