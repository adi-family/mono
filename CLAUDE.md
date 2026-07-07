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

## Deploying `app.adi`: restart, don't just warn

When a change needs a service restart to take effect, **land the deploy yourself** —
don't stop at warning that the running copy is stale.

**How `app.adi` is wired (know this before touching it):**

- The control panel is `adi-app`, run by an **unprivileged per-user `LaunchAgent`**
  — label `family.adi.app.control-panel`, plist at
  `~/Library/LaunchAgents/family.adi.app.control-panel.plist` (user-owned, editable,
  `KeepAlive` + `RunAtLoad`). It runs `adi-app 8000` **as the user** on
  `127.0.0.1:8000`. Restarting it needs **no sudo**.
- A separate **root** front door (`adi-hive`, from `hive-frontdoor.yaml`) binds `:80`
  and *proxies* `app.adi` → `127.0.0.1:8000`. Never confuse the two; never restart
  the front door for an app deploy.

**Build:** `scripts/build-app.sh` → `trunk` builds the Leptos UI, then
`cargo build --release -p adi-app` embeds `dist/` → `target/release/adi-app`.

**⚠️ You (probably) cannot write into `/Applications/ADI.app`.** It's a signed,
notarized bundle, so macOS **App Management** protection blocks modifying
`…/Contents/Resources/adi-app` — you get `Operation not permitted` **even under
`sudo`** (it's a TCC check on the *terminal*, which root doesn't override). A bundle
swap only works if the user first grants their terminal *App Management* (System
Settings → Privacy & Security → App Management), then re-runs the `! sudo cp … && sudo
mv …` swap. Offer that, but don't assume it.

**Working local deploy (no sudo, no bundle write) — repoint the LaunchAgent at the
fresh binary:**

1. Back up the plist:
   `cp ~/Library/LaunchAgents/family.adi.app.control-panel.plist <scratch>/cp.plist.bak`
2. Edit `ProgramArguments[0]` in that plist from the bundle path to
   `/Users/<you>/adi-family/target/release/adi-app` (keep the `8000` arg).
3. Reload: `launchctl bootout gui/$(id -u)/family.adi.app.control-panel 2>/dev/null;
   launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/family.adi.app.control-panel.plist`
   — `bootout` also kills the old process; `RunAtLoad` starts the new one.
4. **Verify through the front door**, not just the port:
   `curl -s http://app.adi/api/health` and confirm a *new* endpoint your change added
   returns `200` (e.g. `curl -o /dev/null -w '%{http_code}' http://app.adi/api/<new>`).

Caveat: this runs the **repo dev binary**, not the bundle. It survives reboots and
app relaunches (`adi up` won't rewrite the plist while the service is loaded), but an
explicit `adi enable` / disable→enable (or `cargo clean` removing the path) reverts to
the old bundle binary. To revert deliberately: restore the backed-up plist + reload.

**Surgical restart pattern.** To kill only the app-service so launchd respawns it:
`pkill -9 -f 'Resources/adi-app '` (or `'target/release/adi-app 8000'` after a
repoint). Use a pattern that includes the trailing arg — the old
`'…/adi-app$'` anchor **never matches**, because the live command ends in ` 8000`.
`pgrep -af '<pattern>'` first to confirm it hits exactly the app-service and never
`adi-hive`.

**The one exception is ADI DNS (`adi.hive`)** — never restart it (see the hard rule
above). Everything here is about the `app` / front-door services.
