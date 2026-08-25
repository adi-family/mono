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
apps/macos/build.sh                 # ADI.app        — the real install
apps/macos/build.sh --flavor dev    # ADI Dev.app    — installable beside it
```

Produces `apps/macos/build/<name>.app` and `apps/macos/build/<name>.dmg`. The script builds
the release Rust binaries, compiles the Swift sources with `swiftc` (no Xcode
project), assembles the `.app`, code-signs it, and packages the DMG via
`dmg/make-dmg.sh` (see **Disk image** below).
Requirements: Xcode command-line toolchain, `cargo`.

Signing is **ad-hoc** by default (fine for local use). Set `SIGN_ID` to a
"Developer ID Application" identity to sign for distribution (hardened runtime +
secure timestamp).

## Flavours: running a build beside the real install

`--flavor dev` builds **ADI Dev.app**, a complete second install that shares nothing with the
real one: it serves `.adi-dev`, keeps its store in `~/.adi-dev`, supervises under
`family.adi-dev.app.*`, resolves on 10063 and fronts on `127.0.0.54:80`. Both can be installed,
enabled and running at once — which is the point, since the alternative is testing a change by
overwriting the copy that is currently serving.

The identity is one type, `adi_config::Flavor` (`crates/adi-config/src/flavor.rs`). Ask any
build what it is:

```bash
adi-mono flavor                    # the release install
adi-mono --flavor dev flavor       # what a dev build would touch
adi-mono --flavor dev status       # ...and what it currently has running
```

`--flavor` is global, so every subcommand takes it. Two named flavours exist — `release` and
`dev`, guaranteed disjoint by a test — and **any other id also works with no code**, deriving a
whole identity from its own name: `--flavor staging` gives `.adi-staging`, `~/.adi-staging`,
`family.adi-staging.app.*` and ports of its own. Every field is separately overridable
(`ADI_DOMAIN`, `ADI_DIR`, `ADI_RESOLVER_PORT`, `ADI_FRONTDOOR_ADDR`, `ADI_SUPERVISOR_PORT`,
`ADI_APP_NAME`, `ADI_BUNDLE_ID`, `ADI_LABEL_PREFIX`, `ADI_AUTO_UPDATE`), and an explicit
variable always beats the preset.

Three things are worth knowing:

- **The bundle carries its own flavour**, in `Info.plist`'s `ADIFlavor`. Both apps ship the same
  `adi-mono`, so a CLI that read the flavour from the environment would give *both* the release
  install — and the dev app would enable, disable and reconfigure the real one. `Core.swift`
  reads the key and pins every CLI it launches.
- **The identity is exported, not re-derived.** `Flavor::env()` goes into every service
  definition and every spawned runner, so a service launchd starts at login resolves the
  identity its installer had rather than re-deriving today's presets.
- **Only `release` auto-updates.** A dev build that ran the updater would pull the released
  bundle over its own. `adi-core` does not register the updater outside the release flavour,
  and `adi-update`'s default target is the flavour's own bundle.

A dev install still needs its own privileged step — its own `/etc/resolver/adi-dev` and its own
root front-door daemon on `127.0.0.54` — so `dns install-route` prompts once, exactly as the
real one did. Nothing it does touches `/etc/resolver/adi`, `family.adi.app.*` or the running
resolver.

To undo one completely: disable it (`adi-mono --flavor dev disable`), remove its route
(`dns remove-route`), and `rm -rf ~/.adi-dev`.

## Disk image

`dmg/make-dmg.sh <ADI.app> <out.dmg> [flavor]` packages the install window: the app, the
`/Applications` symlink, the background picture, the committed Finder layout and the
volume icon. `build.sh` and `release.sh` both call it, so the design cannot drift between
a local build and a notarized one.

```
dmg/background.html            the art; rendered headless, this is what to edit
dmg/background-<id>.tiff       committed, 1x + 2x in one file so Retina is sharp
dmg/layout.applescript         the Finder view settings (window size, icon positions, bars off)
dmg/layout-<id>.DS_Store       committed, baked from the above by driving Finder once
dmg/check-contrast.py          the readability guardrail
dmg/make-assets.sh             regenerates both, per flavour
```

The layout is baked once and committed rather than applied at build time, because
applying it means driving Finder over AppleScript — slow, and needing an automation
permission the CI runner does not have.

Three things about the design are load-bearing and easy to break:

- **The cards exist for contrast, not decoration.** Finder writes each icon label onto the
  background — black under Light appearance, white under Dark — and a disk image cannot
  override either. Only luminance 0.175–0.183 clears 4.5:1 against both; `#6E7684` is
  0.178. `make-assets.sh` renders the exact glyphs Finder will draw and refuses to ship art
  whose worst pixel under the text misses. It is not a formality: a draft that measured
  4.60:1 as a flat colour really shipped 3.42:1 once a texture went over it.
- **Nothing load-bearing goes below y=340.** The Finder path bar follows a *global* user
  preference; when on it takes ~28pt off the icon view and clips the background with it.
  `layout.applescript` turns it off per-window, but a viewer can switch it back on.
- **Names and coordinates are paired across files.** `layout.DS_Store` stores positions
  against the item names `ADI.app` and `Applications`, and the background against the path
  `.background/background.tiff`. Rename any of them, or move an icon without moving its
  card in `background.html`, and the window opens wrong with no error anywhere.

Assets are **per flavour**, and the layout has to be as well as the art: Finder keys an icon
position on the item's *name*, so the release layout would leave `ADI Dev.app`'s icon unplaced.
`make-assets.sh` asks `adi-mono flavor` for the name and passes it into the art, so a disk image
cannot announce a name its bundle does not ship under.

To change the art: edit `dmg/background.html`, then regenerate every flavour —

```bash
for f in release dev; do apps/macos/dmg/make-assets.sh --flavor "$f"; done
```

adding `--bake-layout` if any geometry moved.

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
    src/dns.rs           the DNS service (adi-dns config + .adi route + adi-hive front-door daemon)
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
4XX page (`crates/adi-hive/src/notfound.rs`). `ADILogo.swift` (in-window) and
`icon-gen.swift` (app icon) both draw it from the page SVG's 200×200 coordinates, so
they stay identical to the web logo; keep the coordinates in sync between the two.

**App icon** — `icon-gen.swift` draws the master PNG, and `build.sh --regen-icon` runs it
through `sips` + `iconutil` to rebuild `ADI.icns`. `build.sh` copies `ADI.icns` into the
bundle (Info.plist `CFBundleIconFile = ADI`).

The app polls `adi-mono status --json` (which reports each service's
`enabled`/`running`/`detail`) to drive the power button's on/off state and the status
word; the button toggles the whole platform (`adi-mono enable` / `disable`). `adi-mono`,
`adi-dns`, and `adi-hive` are bundled side by side in `Contents/Resources/` (adi-mono
resolves adi-dns and adi-hive as siblings; the `.adi` route + adi-hive front door are
the privileged bits installed once).

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
