# adi-family — project instructions

please prefer working on the main unless asked to checkout. we value speed over stability now.

## ⚠️ ADI DNS — DO NOT DISTURB (highest priority)

The machine runs **ADI DNS** as the `adi.hive` service under the `adi daemon`
supervisor, listening on **`127.0.0.1:15353`** (UDP/TCP). It serves the `.test`
split-DNS zone and forwards everything else, and it also supervises other live
services (`app`, `webhook`, `webhook-tunnel`). It is critical infrastructure.

Two hard rules — never violate them:

1. **NEVER stop, kill, or restart ADI DNS (`adi.hive`) — ever.** Do not
   `kill`/`pkill`/`SIGKILL` it, do not run `adi daemon stop`,
   `stop-service`/`restart-service adi.hive`, `adi daemon stop`, or anything that
   interrupts it. There is **no need** to stop the current DNS for any task in
   this repo; stopping it causes real issues. If a task seems to require touching
   it, STOP and ask the user first.

2. **Never collide with its ports, so you never need to touch it.** Before
   binding any local UDP/TCP port (e.g. when testing `adi-dns` or any server),
   check first with `lsof -nP -iUDP:<port>` / `-iTCP:<port>` and pick a clearly
   free port. **Avoid `15353` and the surrounding range** — that range is used by
   `adi daemon` services. For ad-hoc local DNS testing use a high, unused port
   such as `45353`.

## Deploying: restart, don't just warn

When a change needs a service restart to take effect (e.g. after swapping a new
`adi-app` binary into `/Applications/ADI.app/Contents/Resources/` for `app.adi`),
**restart that service yourself** so the deploy actually lands. Do **not** stop at
warning that the running/deployed copy is stale — finish the job.

- **Be surgical.** Restart only the target service's process. For `app.adi`, kill
  just the app-service `adi-app` (`pkill -9 -f 'ADI.app/Contents/Resources/adi-app$'`
  — the `$` anchor matches `adi-app` only, never the `adi-hive` front door); its
  supervisor's `restart: on-failure` respawns it on the new binary.
- **Use `sudo` when the service runs as root** (the front door + `app` bind
  privileged ports, so they run as root). If passwordless `sudo` isn't available
  and you can't run it non-interactively, immediately hand the user the exact
  `! sudo …` restart command to run — still drive the restart to completion; don't
  leave it at "the current one is bad".
- **The one exception is ADI DNS (`adi.hive`)** — never restart it (see the hard
  rule above). Everything here is about the `app`/`webhook`/front-door services.
