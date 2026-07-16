# adi-update — auto-updating the whole platform

One update covers **everything**: the release artifact is the entire notarized
`ADI.app` bundle (as a DMG), so every bundled binary — `adi-mono`, `adi-app`,
`adi-dns`, `adi-hive`, the menu-bar `ADI`, and **any binary a future release adds** —
ships in a single atomic swap. The updater never needs to know what's inside.

## How an update flows

```
publisher (your machine)                     every installed machine
────────────────────────                     ──────────────────────────────
apps/macos/publish.sh                        family.adi.app.updater (launchd,
  └─ release.sh: build universal,              at login + every 6h) runs:
     sign, notarize, staple → ADI.dmg          adi-mono update run --quiet
  └─ manifest.json (version/url/sha256)          1. GET manifest.json
  └─ gh release create vX.Y.Z                    2. version newer? else stop
       ADI.dmg + manifest.json                   3. download DMG → verify sha256
                                                 4. mount → verify codesign
                                                    + Team ID 752556J5V6
                                                 5. swap /Applications/ADI.app
                                                    (old bundle kept as backup)
                                                 6. restart services
```

## Client pieces

- **`crates/adi-update`** — the engine: manifest fetch, download, checksum,
  signature/Team-ID verification, atomic bundle swap with rollback. Shells out to
  the macOS toolchain (`curl`, `shasum`, `hdiutil`, `codesign`, `plutil`); no new
  dependencies.
- **`adi-mono update`** — `check`, `run [--force] [--no-restart] [--quiet]`,
  `status`, `enable`, `disable`.
- **`Updater` service** (`crates/adi-core/src/update.rs`) — a *periodic* LaunchAgent
  (`family.adi.app.updater`, `StartInterval`, no `KeepAlive`) managed by the usual
  `adi-mono up`/`enable`/`disable`, so it appears as an "Updates" row in the menu-bar
  app automatically.
- Settings: `~/.adi/mono/update/config.toml` — `manifest_url` (default: the latest
  GitHub release of `mgorunuch/adi-family`), `check_interval_hours` (default 6),
  optional `auth_header` for private static hosting.
- State: `~/.adi/mono/update/state.json` (what `update status` and the GUI row show);
  previous installs in `~/.adi/mono/update/backups` (last 2 kept).

## Restarting services after a swap

The swap replaces binaries on disk; running processes keep the old code until
restarted. `update run` (without `--no-restart`):

1. `launchctl kickstart -k` the per-user agents (`family.adi.app.dns`,
   `family.adi.app.control-panel`) — no password needed in the gui domain.
2. The **root front door** (`family.adi.app.dns-landing`, adi-hive on `:80`) cannot
   be kickstarted without admin rights, so it restarts *itself*: with
   `ADI_WATCH_SELF=1` (set in its plist) adi-hive polls its own binary's inode and
   exits cleanly once the bundle swap replaces it — launchd's `KeepAlive` respawns
   the new build. No prompt, no interruption beyond the respawn.
3. Relaunch the menu-bar app if it was running (plain SIGTERM + `open`, never
   Apple Events).
4. Run the **new** bundle's `adi-mono up` — the idempotent reconcile. This is what
   enables services a newer release introduces, so future additions need zero
   updater changes.

Existing installs get the `ADI_WATCH_SELF` plist once via the usual front-door
migration in `Dns::on_enable` (one admin prompt on the next `up`/`enable`).

## Publishing a release

```bash
# 1. bump the workspace version (the single source of truth)
#    Cargo.toml → [workspace.package] version = "0.2.0"
#    (build.sh stamps it into the bundle's Info.plist automatically)

# 2. build + notarize + write manifest + upload to GitHub Releases
apps/macos/publish.sh --notes "what changed"

# variants
apps/macos/publish.sh --skip-build   # reuse build/ADI.dmg
apps/macos/publish.sh --no-upload    # just write build/manifest.json for another host
ADI_UPDATE_REPO=me/elsewhere apps/macos/publish.sh
```

Clients poll `https://github.com/<repo>/releases/latest/download/manifest.json`, so
publishing the release is the whole rollout. Any static host works instead — serve
`manifest.json` + the DMG and point `manifest_url` at it.

## Security

An update is installed only if **all** of these hold:

- the DMG's sha256 matches the manifest;
- `codesign --verify --deep --strict` passes on the bundle inside;
- the signing `TeamIdentifier` equals `752556J5V6` (override: `ADI_UPDATE_TEAM_ID`).

So a compromised manifest or host can, at worst, offer a genuine older/newer ADI
build — never foreign code. A failed swap rolls the previous bundle back in place.

Test/dev escape hatches (never set in production): `ADI_UPDATE_APP` (target bundle
path), `ADI_UPDATE_INSECURE_SKIP_CODESIGN=1` (fixture bundles in tests).
