# Hive services

A hive service is a long-running process a project declares in its `.adi/hive.yaml`: a proxied
host, ports, and a runner. The supervisor keeps it alive (restart, backoff, hot-reload) and the
front door proxies `<host>.adi` to its loopback port.

## Runners (pick one)
- `runner.script` — a shell command run via `sh -c`.
- `runner.docker` — a container: `image`, plus `ports` (each host port key → container port),
  `volumes`, `environment`, `pull`, `command`, and raw `args`.

## Do it
- List: `GET /api/hive`. Panel: `/settings/hive`. Global services live in
  `~/.adi/mono/hive/hive.yaml`; a project's live in its `.adi/hive.yaml`.
- Create: `POST /api/hive/create` (pass a `docker` block for a container) — or edit the
  `hive.yaml` directly; the supervisor re-reads it.
- Start / stop: `POST /api/hive/start` · `/stop`.

## Ports & DNS — hard rules
- Never pick raw ports. Let the ports manager lease them (`GET /api/ports`, panel
  `/settings/ports-manager`); the supervisor wires the leased port to the host.
- **Never touch ADI DNS**: do not stop, kill, or restart the `adi.hive` service, and never bind
  the `15353` port range. When you need a scratch port, pick a clearly free high one.
