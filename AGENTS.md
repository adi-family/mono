# adi-family — project instructions

please prefer working on the main unless asked to checkout. we value speed over stability now.

**Managed services belong in Hive.** Use Hive whenever a component needs process
supervision. Declare an internal domain for its local API and let Hive allocate the
first available managed port; pass the injected `$PORT` to the service. Callers use
the internal domain instead of hard-coded ports or separate port-discovery files.

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
