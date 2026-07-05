# ADI.app (macOS)

A standard windowed app that controls **local ADI services**. The window is a
translucent (vibrancy) control panel: the ADI logo and one big **On/Off** power button
with a live status word under it. **DNS** is the first built-in service.

The app is a **thin trigger**: all control logic (config, launchd supervision, the
`.adi` route + admin prompt, status) lives in `adi-core` and is exposed as the bundled
**`adi-mono`** CLI. Every button runs `adi-mono <args>`, and the live view is the JSON
`adi-mono status --json` emits. (`adi-mono` is the current name; it will be renamed to
`adi`.)

> Runtime files live under `$HOME/<dir>/mono/`, where `<dir>` comes from the
> `ADI_DIR` env var (default `.adi`, the adi platform home) — so by default
> `~/.adi/mono/dns/`. The `mono` subdir keeps this app's files isolated from the
> platform's own (`hive`/`cocoon`/`workforce`). launchd labels are namespaced
> `family.adi.app.*`. This app never stops or restarts the production `adi` daemon
> or collides with its ports.
>
> A login-launched LaunchAgent only sees env vars set in the launchd session, so
> to override the directory use `launchctl setenv ADI_DIR <name>` (then relaunch),
> not a shell `export`.

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

Open the DMG, drag **ADI** to Applications, launch it. The translucent control window
opens with the ADI logo and one big **power button** — click it to turn all services
On/Off (`adi-mono enable` / `disable`). Quit from the app menu (⌘Q).

Turning **On** installs the `family.adi.app.dns` LaunchAgent (`launchctl bootstrap`,
runs now + at login, auto-restart via `KeepAlive`) and, on first enable, the `.adi`
route (`/etc/resolver/adi` + the landing daemon — one admin-password prompt). The status
word reads `Running` / `Starting…` / `Off`.

## Architecture

The control logic is in Rust (`adi-core`), triggered through the `adi-mono` CLI; the
Swift app only triggers it and renders status.

```
crates/
  adi-core/            the command surface: Adi { enable, disable, status } and
                       Adi::dns() -> Dns { enable, disable, install_route, … }
    src/commands.rs      the Adi facade + the JSON status Report
    src/service.rs       the Service trait + report row types
    src/dns.rs           the DNS service (adi-dns config + .adi route + landing daemon)
    src/launchd.rs       write plist, bootstrap/bootout, is-loaded (talks to launchctl)
    src/status.rs        read adi-dns's status.json + PID liveness (kill -0)
    src/paths.rs         $HOME/<ADI_DIR>/mono file locations
  adi-cli/             the `adi-mono` binary — a thin argv adapter over adi-core

apps/macos/Sources/
  ADIApp.swift         @main — a single translucent Window (content-sized)
  ContentView.swift    the window: logo + big power button + status word
  PowerButton.swift    the big circular On/Off toggle
  VisualEffectView.swift  NSVisualEffectView vibrancy + non-opaque window
  ADILogo.swift        the ADI mark (hexagon cage + orange core, from the .adi page)
  AppModel.swift       holds the last report + 2s refresh + isOn/busy + toggle
  Core.swift           the only bridge to core: runs `adi-mono`, decodes its JSON
  Models.swift         Codable mirror of `adi-mono status --json`
apps/macos/
  icon-gen.swift       renders the app icon (same design as ADILogo); see below
  ADI.icns             the built app icon (Info.plist CFBundleIconFile = ADI)
```

The logo is the **real ADI mark** — the hexagonal cage + orange core from the `.adi`
landing page (`crates/adi-dns/src/landing.rs`). `ADILogo.swift` (in-window) and
`icon-gen.swift` (app icon) both draw it from the landing SVG's 200×200 coordinates, so
they stay identical to the web logo; keep the coordinates in sync between the two.

**App icon** — `icon-gen.swift` draws the master PNG, and `build.sh --regen-icon` runs it
through `sips` + `iconutil` to rebuild `ADI.icns`. `build.sh` copies `ADI.icns` into the
bundle (Info.plist `CFBundleIconFile = ADI`).

The app polls `adi-mono status --json` (which reports each service's
`enabled`/`running`/`detail`) to drive the power button's on/off state and the status
word; the button toggles the whole platform (`adi-mono enable` / `disable`). `adi-mono`
and `adi-dns` are bundled side by side in `Contents/Resources/` (adi-mono resolves
adi-dns as a sibling).

### Adding a service

Implement the `Service` trait in `adi-core` and register it in `Adi::services()`:

```rust
struct MyService;
impl Service for MyService {
    fn id(&self) -> &'static str { "myservice" }        // CLI namespace + report id
    fn name(&self) -> &'static str { "My Service" }
    fn label(&self) -> String { "family.adi.app.myservice".into() }  // launchd label
    fn status_path(&self) -> PathBuf { paths::support_dir().join("myservice/status.json") }
    fn log_path(&self) -> PathBuf { paths::logs_dir().join("adi-myservice.log") }
    fn program(&self) -> Vec<String> { /* write config, return argv */ }
    // optional: extra_actions, on_enable/on_disable, detail
}
```

No Swift changes — it appears in the menu with its own status line and enable/disable,
and (if you add a CLI subcommand) any extra actions.

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
