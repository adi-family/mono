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
`~/projects/nakit-yok/.adi/hive.yaml`). adi-hive reads two slices and ignores the rest:

- **Proxy:** `proxy.bind` (addresses to listen on) and, per service, `proxy.host` +
  its HTTP port (`rollout.recreate.ports.http`) → one rule `Host: <host> →
  127.0.0.1:<http port>`.
- **Run:** per service, a `runner` — either a `runner.script` (`run` + optional
  `working_dir`) or a `runner.docker` container (see below) — plus `environment.static`
  and `restart`: what to launch and how to keep it alive.

Everything else (healthcheck, hooks, depends_on, defaults, observability, …) is
accepted-but-ignored. A service without a `proxy:` block is simply not routed; one
whose `runner` has neither a `script` nor a `docker` block is simply not launched. See
[`hive.yaml`](./hive.yaml) for a worked example.

## Running services

For each service that declares a `script` runner, adi-hive launches the command and
supervises it, so the port it proxies to is actually serving — no manual `bun run dev`:

- Runs `run` via `sh -c` in `working_dir` (relative to the hive.yaml's directory).
- Injects the ports as env: `PORT` = the service's http/sole port, plus a
  `PORT_<KEY>` for every named port. `{{runtime.port.<key>}}` placeholders in `run`
  and env values are expanded too. The service's `environment.static` is merged last,
  so an explicit value wins.
- On exit it relaunches per `restart` (`always` | `on-failure` (default) | `no`) with
  an exponential backoff.
- Each runner runs in its own process group, so on SIGTERM adi-hive tears down the
  whole tree (the shell, the dev server, and anything it forked) — no orphans holding
  a port.

## Docker runners

A service can run as a **container** instead of a host process — an "irregular Docker
Compose": one container, declared with familiar compose-ish keys, but supervised by
adi-hive (restart/backoff, hot-reload, SIGTERM teardown) rather than by `docker compose`.

```yaml
services:
  web:
    proxy: { host: web.adi }
    rollout: { recreate: { ports: { http: 8080 } } }   # host port — leased by adi-hive
    restart: always
    environment: { static: { LOG_LEVEL: info } }
    runner:
      docker:
        image: nginx:1.27
        ports: { http: 80 }          # host port key → container port
        volumes: ['./site:/usr/share/nginx/html:ro']
        environment: { LOG_LEVEL: debug }   # overrides environment.static
        pull: missing                # always | missing | never
        args: ['--memory=512m']      # raw `docker run` flags — the escape hatch
        command: ['nginx', '-g', 'daemon off;']   # overrides the image CMD
```

It compiles to a single foreground command the ordinary supervisor drives:

```sh
docker rm -f adi-web >/dev/null 2>&1; exec docker run --rm --name adi-web …
```

- **Host ports stay adi-hive's job.** `rollout.recreate.ports` are the leased host ports
  (auto-leased for a proxied service, exactly as for a script). `docker.ports` maps each
  host **port key** to the container port it forwards to, published on loopback
  (`-p 127.0.0.1:<host>:<container>`) — so the container is reachable only through the
  front door. The container also gets `PORT` / `PORT_<KEY>` (the *container* ports), so a
  `$PORT`-aware image works either way.
- **Lifecycle is the same as a script.** `docker run` runs in the foreground (no `-d`) and
  `exec`s so it *is* the supervised process: adi-hive's SIGTERM reaches it and it forwards
  to the container; `--rm` cleans up on exit. The leading `docker rm -f` clears a container
  a prior hard-kill may have orphaned, so a relaunch never trips a name clash. A changed
  `docker:` block hot-reloads like any other runner.
- **Bind mounts** use compose `host:container[:mode]` syntax; a relative host path resolves
  against the hive.yaml's directory, an absolute path and a named volume pass through.
- **`args`** is a raw passthrough for anything not modelled first-class (`--network host`,
  `-w /app`, `--user 1000`, `--gpus all`, …).
- The container name defaults to `adi-<service>` (unsafe characters mapped to `-`);
  override with `docker.name`.

Caveat: if a container ignores SIGTERM past the shutdown grace period, adi-hive `SIGKILL`s
the `docker run` process group but the daemon-managed container may linger — the next
launch's `docker rm -f` reclaims the name.

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
