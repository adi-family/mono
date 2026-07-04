# adi-family — project instructions

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
