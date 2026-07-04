# ADI.app (macOS menu-bar)

A menu-bar app that supervises **local ADI services** as per-user launchd
LaunchAgents — enable/disable each, run service-specific actions, and see live
status at a glance. **DNS** is the first built-in service; the registry is built so
"some other" services and generic daemons slot in as data, not new UI.

> Separate from the production `adi` platform. This app never touches the `adi`
> daemon, its `~/Library/Application Support/adi` directory, or its ports. It keeps
> its own namespace (`~/Library/Application Support/adi-menubar`, launchd labels
> `family.adi.app.*`).

## Build

```bash
apps/macos/build.sh
```

Produces `apps/macos/build/ADI.app` and `apps/macos/build/ADI.dmg`. The script builds
the release Rust binaries, compiles the Swift sources with `swiftc` (no Xcode
project), assembles the `.app`, code-signs it, and packages a DMG with `hdiutil`.
Requirements: Xcode command-line toolchain, `cargo`.

Signing is **ad-hoc** by default (fine for local use). Set `SIGN_ID` to a
"Developer ID Application" identity to sign for distribution (hardened runtime +
secure timestamp).

## Release (signed + notarized)

For a DMG that opens on any Mac with no Gatekeeper warning:

```bash
set -a; source ~/peronal-projects/VTT/.env; set +a   # TEAM_ID, AC_USER, AC_PASS
apps/macos/release.sh
```

`release.sh` finds the Developer ID cert for `TEAM_ID` in the keychain, runs
`build.sh` with hardened-runtime signing (nested `adi-dns` + the app), signs the
DMG, submits it to Apple's notary service (`notarytool --wait`), and staples the
ticket. Credentials are read from the environment and never stored in the repo. If
`AC_USER`/`AC_PASS` are unset, the DMG is signed but left un-notarized.

Verify a finished DMG:

```bash
spctl -a -t open --context context:primary-signature -v build/ADI.dmg  # -> accepted / Notarized Developer ID
```

## Use

Open the DMG, drag **ADI** to Applications, launch it. A stacked-squares icon
appears in the menu bar (no Dock icon — it's an `LSUIElement` agent). Each managed
service shows a status line and controls:

- **DNS** — *Enable/Disable* installs the `family.adi.app.dns` LaunchAgent
  (`launchctl bootstrap`, runs now + at login, auto-restart via `KeepAlive`) and
  routes `.adi` to it. *Install/Remove .adi route* manages just `/etc/resolver/adi`
  (one admin-password prompt). Status shows `Running · 127.0.0.1:10053` / `Stopped`.

## Architecture

```
Sources/
  ADIApp.swift         @main — MenuBarExtra, icon reflects "any service running"
  MenuContent.swift    renders one block per service (generic; no per-service code)
  AppModel.swift       the service registry + 2s status refresh + action dispatch
  ManagedService.swift protocol every service conforms to + rendered row types
  Launchd.swift        the generic engine: write plist, bootstrap/bootout, status
  DNSService.swift      the DNS service (adi-dns config + .adi route)
  Status.swift         Codable mirror of adi-dns's status.json
```

Every service is supervised identically — only the program and its files differ —
so `Launchd` is the single place that talks to `launchctl`, and the menu renders
the registry generically. Live status comes from the JSON status file each service
writes (`adi-dns` writes the bound port there); the app polls it and probes the PID
with `kill(pid, 0)`.

### Adding a service

Conform a type to `ManagedService` and add it to `AppModel.services`:

```swift
struct MyService: ManagedService {
    let id = "family.adi.app.myservice"     // launchd label
    let name = "My Service"
    var statusPath: String { AppPaths.support + "/myservice/status.json" }
    var logPath: String { NSHomeDirectory() + "/Library/Logs/adi-myservice.log" }
    func program() -> [String] { [binaryPath, configPath] }  // write config, return argv
    // optional: extraActions, onEnable/onDisable, detail(_:)
}
```

No menu code changes — it appears with its own status line and enable/disable. A
generic "add any daemon" service (pick a binary + args) fits the same protocol.

### Why per-user LaunchAgents (not SMAppService LaunchDaemons)

Services here bind unprivileged ports, so they need no root to run, and a per-user
LaunchAgent works with **ad-hoc signing** today. An `SMAppService` LaunchDaemon
would require a Developer ID certificate + the app in `/Applications`. Any
privileged step (e.g. writing `/etc/resolver/adi`) is a single admin prompt.

## Known limitations (v1)

- **arm64 only** — the build targets the host arch. Universal: build the Rust
  binaries for both `aarch64`/`x86_64-apple-darwin`, `lipo`, and add both Swift
  `-target`s. (`release.sh` inherits this until the build goes universal.)
- **Enable/Disable is the on/off toggle** (bootstrap/bootout); no separate paused
  state yet.
- **DNS is the only service so far** — the registry is ready for more.
