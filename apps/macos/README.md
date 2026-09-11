# ADI.app (macOS)

A standard windowed app that controls **local ADI services**. The window is a small dark
panel drawn to [`design/DESIGN.md`](../../design/DESIGN.md): the mark and wordmark, what ADI
is doing as a dot and a word, the services switch, and one filled button into the control
panel. **DNS** is the first built-in service.

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
  override either. Only luminance 0.175–0.183 clears 4.5:1 against both; the cards are
  `#79756F`, the one grey in the token family's range that does, pinned by that constraint
  rather than picked. `make-assets.sh` renders the exact glyphs Finder will draw and refuses
  to ship art whose worst pixel under the text misses. It is not a formality: a draft that
  measured 4.60:1 as a flat colour really shipped 3.42:1 once a texture went over it. The
  art itself is flat and on the design tokens — a monochrome mark, no masthead blur, no
  gradient — and the guardrail stays green.
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

It also takes `build` or `notarize` to run one half. That is for CI, which wants the
compile and the wait in Apple's queue timed as two steps rather than one opaque
half-hour; locally, run it with no argument and it does both.

Verify a finished DMG:

```bash
spctl -a -t open --context context:primary-signature -v build/ADI.dmg  # -> accepted / Notarized Developer ID
```

## Use

Open the DMG, drag **ADI** to Applications, launch it. The window walks through moving the
app and the two permissions, then shows the **ADI services** switch — flip it to turn all
services On/Off (`adi-mono enable` / `disable`) — and **Open control panel**. Quit from the
app menu (⌘Q).

Turning **On** installs the `family.adi.app.dns` LaunchAgent (`launchctl bootstrap`,
runs now + at login, auto-restart via `KeepAlive`) and, on first enable, the `.adi`
route (`/etc/resolver/adi` + the landing daemon — one admin-password prompt). The status
line reads `Running` / `Starting…` / `Off` beside a green, orange or grey dot.

### The control panel, in the app

*Open control panel* opens a second window of ADI's own — `http://app.<domain>` in a `WKWebView`
(`Sources/PanelWindow.swift`, `Sources/WebPanel.swift`) — rather than handing the URL to whatever
browser the Mac opens `.adi` links with. Same page, same origin, same WebSocket; what it buys is
that the panel is *in the app*: one icon in the Dock, one window that is where you left it, and
not a tab lost in a window of thirty.

The window wears an **ordinary title bar** — the traffic lights and the name of the page on screen,
on a line of their own — and the shell's 48px bar (§7) sits under it, carrying the tabs and an
arrow that hands the current page to the default browser. Tabs *in* the title bar is what a browser
does; this is one window onto one machine, and the row reads better as a band of its own.

**The title bar wears the app's surface and stays AppKit's** (`WindowChrome`). Both halves matter:
the band is `--bg-side` rather than the system grey that leaves a seam across the top, *and*
double-click to zoom or minimise, dragging, and the window menu all work — because they are the
real title bar rather than an imitation of one. Measured: 38,41,44 → 16,16,16, and the strip
hit-tests as `NSThemeFrame`.

The rule underneath it, which cost four wrong turns: **colour the band, never cover it.** A view
over the title bar claims every click in it, and `allowsHitTesting(false)` does not give them back
(the hit is SwiftUI's hosting view, not the title bar). So:

- `.windowStyle(.hiddenTitleBar)` is taken for *one* of its two effects — it makes SwiftUI want the
  bar transparent, which is what lets the window's own background show. Setting
  `titlebarAppearsTransparent` by hand on an ordinary window is silently reverted (measured true at
  t+3s, false at t+6s: a working setting that looks broken).
- Its other effect, `fullSizeContentView`, is the half that hurts, so `WindowChrome` removes it and
  keeps it out — on `NSWindow.didUpdateNotification`, because SwiftUI re-asserts window
  configuration long after the view is built, and both a bounded timer and `updateNSView` lose that
  race.
- `window.backgroundColor` holds once set. It was never the problem, though it looked like it while
  the transparency underneath kept flipping back.

The cost is the window's title, which `.hiddenTitleBar` also hides and which cannot be restored
(`titleVisibility = .visible` is re-hidden from `makeNSView`, from `updateNSView`, and deferred a
turn of the loop past either). The tab strip under the band names every open page anyway.
`WindowChrome` also sets `isMovableByWindowBackground`, so the window can be dragged by its tab bar
the way any window with a strip of tabs can.

The window is dark because the whole app is (`preferredColorScheme(.dark)`, and `Tokens.swift`
restates only the dark half of `design/tokens.css`). Following the system appearance is task
**ADI-55**, which lists what has to move — including this band, which the app paints itself and
which would otherwise stay black in a light window.

There is **no address bar** — nothing is typed into this window, because everything it can reach is
a click away on the page it opens on — and **no back, forward or reload button**: three glyphs
spent on what this window almost never needs, and the web view keeps all three on its right-click
menu for the times it does.

**The first tab is the brand.** It wears the mark and the wordmark (§10's top-bar treatment at the
tab-sized 16) instead of whatever the panel currently calls itself, because it is not one page
among the open ones — it is the thing they were opened from, and it says so by being the only tab
that never changes. Pressing it does what pressing a tab does: shows that tab.

The page **stops drawing its own** mark while the app is the window around it, so the brand is not
on screen twice a few pixels apart. Two halves:

- The app **tells the page it is here**, with a `WKUserScript` setting `window.__adiNative` at
  document start — not a User-Agent suffix, which every host this window opens would then see.
- The page reads it (`adi-webapp`'s `native::in_app`) and `launcher::brand` and `launcher::floating`
  draw nothing. Verified as an A/B on one URL: Chrome renders two triggers, the app renders none.

**What that costs, deliberately:** those two were also the palette's only *pointer* triggers, so
inside the app ⌘K is the keyboard's alone. The menu itself is untouched and the page still listens
for the keystroke; what the app takes over is the mark, not the menu. If a click-target is wanted
back, the cheapest honest answer is to let `launcher::floating` draw again — a corner mark
duplicates nothing in the bar — rather than to put a second brand in the tab strip.

**The first tab is the app** (`Sources/PanelTabs.swift`). It is created with the window, it cannot
be closed, and nothing can navigate it off `app.<domain>`: a link that would opens a tab instead.
That is enforced against *every* main-frame navigation and not just clicks — a `window.location`
or a redirect is a navigation too, and an invariant that holds for one kind and not the others is
not an invariant. So there is always somewhere to go back to, which is what makes it safe to let a
dashboard take the window over.

Dashboards and hive services open beside it, because the panel links to every one of them with
`target="_blank"` (and `window.open` from the launcher) — so `createWebViewWith` is the hook, and
what it makes is a tab rather than a window. ⌘-click opens one anywhere; ⌘1…⌘9 select; the ⨯ on a
tab — a **wheel-click** on it, or ⌘W — closes it and selection falls to the tab on its left. A host
that is already open is brought forward rather than opened twice: pressing a dashboard's link again
means *show me it*, not *give me another copy*. Each tab keeps its own web view, so its history and
scroll position survive being switched away from.

**Right-click a tab for the three things there is no room for on a chip**: *Rename…*, *Pin Tab* (or
*Unpin Tab*) and, behind a divider, *Close Tab*. The app's own tab has no menu at all — it cannot be
closed, the brand is not renamed, and it is already where pinning would put a tab — so it offers
nothing rather than three dead items. The menu is title case rather than the app's sentence case
(§8): it opens beside the menu bar's own *Close Tab*, and a menu that disagreed with the menu bar
about capitalisation would read as the odd one out rather than as the house style.

**Pinning keeps a tab.** A pinned one moves up behind the app's and stays there, which is most of
what it is for: the strip is numbered by position, so a tab kept on purpose is one whose ⌘-number
stops moving every time a dashboard opens and closes beside it. It wears a pin where its ⨯ was and
declines the two gestures that close a tab *under the pointer* — the ⨯ and the wheel-click — while
⌘W and its own menu still close it, because both of those say what they are about to do. Unpinning
leaves it at the head of the ordinary tabs rather than throwing it back where it came from.

**Renaming happens in the chip.** *Rename…* turns the tab into a box holding the name it is already
wearing, selected, so the usual gesture is to type over it: ↩ keeps it, ⎋ leaves the tab as it was,
and clicking anywhere else keeps what was typed — a box left open on a tab nobody is looking at is
worse than either answer. Emptying it puts the page's own title back, which is the only undo a
rename needs. The name is the window's and is written nowhere, and ⇧⌘T brings a closed tab back
with its name and its pin as well as its address.

Two things about the wheel-click are load-bearing. **SwiftUI has no middle button** — its buttons
and tap gestures are the left one's alone — so the chip carries a transparent `NSView` that claims
*only* middle-button events: its `hitTest` answers with itself while the event under dispatch is a
middle click and with nil otherwise, so every ordinary click falls straight through to the SwiftUI
button underneath. Measured both ways round on a real bundle: with a button-2 event in dispatch the
window's hit test returned the catcher, and a synthesized left click on the same pixels still
selected the tab. And the press has to both start and end on the chip, so a wheel-press dragged off
it is a change of mind rather than a closed tab.

**The ⨯ needs a shape of its own, and it is not the glyph.** A `LucideIcon` is a stroked `Shape`,
and SwiftUI hit-tests a stroked shape **on its stroke** — so a close button whose whole label was
the icon had a target of two ~0.9pt diagonals, and every other pixel of it fell through to the tab
underneath: on an unchosen tab that selected it, and on the tab already in front it did nothing at
all, which is what "I can't close the current tab" turned out to mean. Measured on a real bundle,
clicking a grid across the glyph's 14pt box: **4 of 49 points closed the tab, and all four lay on a
diagonal.** The fix is `.padding(4)` then `.contentShape(Rectangle())` inside the button, which
makes the target 22pt and — because the 4 comes back off the button's own trailing padding — leaves
the glyph on exactly the pixels it was on before. The same grid after: the middle of the glyph's top
edge, a point the old build never once closed, closed 11 times in 12.

Driving any of this from a probe has traps of its own. A synthesized `.otherMouseDown` built
with `NSEvent.mouseEvent(with:)` carries **button 0**, and one built from a `CGEvent` carries button
2 but **no window** — so neither on its own reaches a view, and `CGEvent.post` needs an Accessibility
grant the bundle will not have. What works is posting the CGEvent-derived event through the app's
own queue, which is what makes `NSApp.currentEvent` right for the hit test, and then handing the
down/up pair to the view that hit test names. A posted `.rightMouseDown` opens no context menu at
all; `NSView.menu(for:)` returns the real one, and `NSMenu.performActionForItem(at:)` chooses from
it — `popUpContextMenu` only blocks on a tracking loop that synthesized keystrokes do not drive.
And for pictures: `screencapture` and `CGWindowListCreateImage` both want Screen Recording, while a
view drawing itself with `cacheDisplay(in:to:)` wants nothing and does capture the web view's pixels.

A **left** click is no easier. Posted as a `.leftMouseDown` and a `.leftMouseUp` in the same turn of
the loop, the up is lost and the button fires on the *next* pair instead — which reads as a control
that works one trial late. Post the up a beat later, and then expect it to land about three times in
four: measure a point by repeating it rather than by trying it once, and take a click that lands on
an *unrelated* control as the check that the run is worth reading at all. A run whose window came up
`key=false` lands nothing whatsoever, so log that too — otherwise a probe that was never in front
reports every target as dead.

**The keyboard is on the menu bar** (`Sources/PanelCommands.swift`), and what is on it are the
habits a browser already taught: ⌘R reload and ⇧⌘R reload ignoring the cache; ⌘W close the tab and
⇧⌘T put it back; ⌘[ and ⌘] back and forward; ⌃⇥ and ⌃⇧⇥ along the tabs; ⌘+, ⌘− and ⌘0 for the size
of the page; ⌘F to find in it, ⌘G and ⇧⌘G for the next match and the last. Menu items rather than
bare shortcuts, because an item *says what it does* — a keystroke on no menu is one somebody has to
be told about, and one System Settings cannot rebind. ⌘1…⌘9 stay what they were, shortcuts on the
tab buttons themselves, which is why they appear nowhere on the menu bar. *File* names what ⌘W will
actually do: **Close Tab** on a dashboard, **Close Window** on the app's own tab, which is never
closed — which is what ⌘W on a browser's last tab has always meant. Every item resolves what it acts
on at the moment it is pressed, so none of them can act on a tab that has since gone.

Three of the habits were deliberately left off. ⌘← and ⌘→ are *not* bound to back and forward: they
are start-of-line and end-of-line in the find bar's box, and a shortcut that navigates the window
while you are editing text is a shortcut that eats your cursor. There is no ⇧⌘W (close window), no
⌘P, and no Esc to stop a load — nobody asked for them, and each one is a keystroke spent. And there
is **no "3 of 17"** in the find bar: the public API is `WKWebView.find(_:configuration:)`, whose
`WKFindResult` carries `matchFound` and nothing else, and a count computed separately in JavaScript
disagrees with WebKit's own highlighting on shadow DOM, on hidden text and on a word split across
nodes — often enough that a wrong number is worse than none. The bar answers what it can answer
honestly: found, or *No matches*.

Measurements chose the rest of the design. On the two menu items themselves:

- **Nothing was taken from anyone.** An app built of `Window` scenes rather than a `WindowGroup`
  gets no File menu and no *Close* at all until a command asks for one, so ⌘W was an unbound
  keystroke that did nothing.
- **Asking conjures SwiftUI's own *Close* and *Close All*, and those close the window** — on the
  panel, every open tab at once. Two items then hold ⌘W and AppKit gives it to one of them: from
  `CommandGroup(after: .newItem)` it sat on ours while SwiftUI's *Close* was disabled and moved to
  *Close* the moment the panel window made it valid, so the keystroke would close tabs until it
  silently began closing the window. `CommandGroup(replacing: .saveItem)` takes their slot instead
  of competing for it, and leaves exactly one ⌘W on the whole menu bar — verified by walking it.
  The cost is *Close All* (⌥⌘W), which in an app of two windows was never worth a keystroke.
- **`.focusedSceneValue` does not work here**, though it is the documented way to let a menu act on
  the window holding the keyboard. With the panel key and its scene in front the value arrived at
  the `Commands` body as nil — *Reload* stayed grey, ⌘W kept its fallback meaning — and taking the
  web view out of the window did not change it. SwiftUI publishes a focused value for a scene whose
  content holds *focus*, and nothing in a window that is one web view under a tab strip ever takes
  it. So the two items read a shared `FrontPanel` instead, told by `controlActiveState` whether the
  panel has the keyboard.
- **The web view eats neither keystroke.** Driven through `NSApp.sendEvent` against a real bundle,
  ⌘W closed the tab (two down to one, selection falling back to the app's own) and ⌘R reloaded the
  page that was left.

And on the rest of the set, each of which was driven the same way — a real bundle, real synthesized
keystrokes, the whole battery green before it was written down:

- **⌃⇥ is the one keystroke the web view *does* eat.** WebKit claims the Tab key as a key equivalent
  of its own, and a view's key equivalent is offered before the menu bar's, so *Show Next Tab* never
  saw it while a page had the keyboard. Measured, with the web view first responder: ⌃⇥ moved
  nothing, the very same event handed straight to `NSApp.mainMenu` switched tabs, and a ⌃-letter item
  on that menu fired normally — so what is taken is the key, not the modifier. `PageView`
  (`Sources/WebPanel.swift`) is a `WKWebView` that declines exactly this one and passes everything
  else through. Tab on its own, which is how a form is actually walked, is not a key equivalent and
  never comes near it.
- **⌘F opened a bar that was never on screen.** `PanelWindow` observes `PanelTabs`, which publishes
  when tabs are opened, closed or switched — and not when the tab in front changes something about
  itself. `if tabs.selected.finding` in the window's own body was therefore a read nothing had
  subscribed to. It looked like a focus bug for two runs; what settled it was walking the window's
  view tree and finding **no text field in it at all**. The fix is a `Selected` view holding the
  front tab as an `@ObservedObject`. The window's *title* had the same fault and hid it better — it
  was right whenever anything else had forced a redraw.
- **`@FocusState` cannot take the keyboard back from AppKit.** At the moment ⌘F is pressed the first
  responder is the tab's `WKWebView`; setting `.focused($focused)` from `onAppear` moves focus within
  SwiftUI's own focus system and does not touch that. Measured: the responder was still the web view
  a full second later. So the box is an `NSTextField` (`FindField`) that asks for the job itself, and
  its field editor is also where ⎋ and ↩/⇧↩ are caught — the only place they can be caught while the
  box has the keyboard. With it, the responder is the field editor 150 ms after ⌘F, and typed
  keystrokes land in the query.
- **⌘= reaches the ⌘+ item.** *Zoom In* carries ⇧⌘=, which is what ⌘+ is; AppKit matches the
  unshifted key too, so the hand that presses ⌘= without the shift zooms anyway and no second
  binding is needed.
- **`CGEvent.postToPid` is no use for this.** From an unsigned bundle it delivers nothing at all —
  not even ⌘j to a menu item wired to a counter. `NSApp.sendEvent` does reach a non-⌘ key equivalent
  with the web view first responder (measured against a plain-`j` and a ⌃-`j` control item), so it is
  the one to drive a test with.

Two traps for whoever tests this next. SwiftUI keeps those items **lazily**: read from outside, a
title or an enabled flag is whatever it was when somebody last looked, and `NSMenu.update()` alone
does not always freshen it — the delegate's `menuNeedsUpdate:` does, which AppKit sends when a menu
is opened and on the key-equivalent path. Two runs looked like the item was stuck on *Close Tab*
when it was only the reader that was stale. And a **disabled item does not fire its key equivalent**,
while an item is a SwiftUI view that re-renders a turn of the loop after the model it reads — so a
⌘] sent in the same millisecond as `canGoForward` became true arrives at an item AppKit still
believes is dead. Both showed up as the feature being broken and were the test being faster than a
hand: ⌘] and ⇧⌘T each passed once given ~0.8 s to settle.

It is a viewer, not a general browser and not a second control panel. There is no address bar and no
new-tab button — there would be nowhere for either to go — and **a clicked link that leaves this
machine opens in the default browser**: an issue tracker or a docs site is not what this window is
for. Only a *click* leaves, though: a redirect, a form post or a subframe is the page doing its
job (`WebPanel.isLocal` decides, against the flavour's own zone plus loopback, so a dev build
stays inside `.adi-dev`).

Three things about it are load-bearing and easy to undo by accident:

- **App Transport Security has to be told about the zone.** `Info.plist`'s
  `NSAppTransportSecurity` excepts `<domain>` (plus `NSAllowsLocalNetworking` for the
  `127.0.0.1:<port>` a dashboard is reached on before it has a host), and `build.sh` restamps
  that exception per flavour beside `ADIDomain`. There is no HTTPS to move to: the front door
  serves names that resolve only on this Mac, for which no CA issues anything. Without the
  exception every load fails with a policy error and the window is simply blank.
- **The JavaScript panels are implemented** (`WKUIDelegate`). The webapp guards destructive
  actions with `confirm()`, and WebKit answers a delegate that does not implement it with
  *false* — so *Delete* would silently do nothing, with no error anywhere to explain it.
- **The web view is owned by `WebPanel`, not built by the representable.** A representable that
  made its own would hand SwiftUI a fresh, blank page on every body rebuild, losing the history,
  the scroll position and anything half-typed.
- **The tab's title is observed, not sampled.** A page sets `document.title` when it likes, which
  for a dashboard can be after the load finished, so reading it in the navigation callbacks catches
  it only sometimes — the same tab came up *ADI — agents on your own machine* one run and
  *landing.adi* the next. KVO on `\.title`, and the host only as the fallback before there is one.

Right-click → *Inspect Element* works (macOS 13.3+): this is a window onto your own machine, and
whoever opens it is likely the person changing what it shows.

### The two controls at the foot of the window

Below a rule, on **every** step including the two setup ones, sit the two things that have to
work when nothing else does (`Sources/Maintenance.swift`).

**Update.** The installed version, what the release channel says about it, and one button —
*Check for updates*, becoming *Update to X* when there is one — the window's one orange
button while it is, at which point *Open control panel* steps back to an ink fill. It runs the bundled CLI
(`adi-mono update check` / `update run`) and reaches GitHub over the system's own DNS.

> **It is in the app, and not only on the control panel, on purpose.** The panel is `app.adi`,
> so every route to it goes through a name *this install* serves — and a broken `.adi` route is
> precisely the fault someone needs the fix for. An update offered only on a page you cannot
> load is an update that is not offered. `adi-mono` sits in `Contents/Resources` and needs
> nothing of ADI's own to be working.

A successful install swaps the bundle and then terminates and reopens the app, so the window
disappears and comes back as the new version; the row says so before it happens. The row is
hidden on a non-release flavour (there is one release channel and its artifact is `ADI.app` —
a dev build updating itself would swap the released bundle over its own), and on the *move me*
step (an update installs into `/Applications` wherever the running copy is, so from a disk image
it would update a different bundle and leave this window untouched).

**Something not working?** Marked with a ladybug, and carrying two buttons.

*Create report* runs `adi-mono diagnose`, writes an archive of everything that could explain a
fault, and reveals it in Finder (it becomes *Show in Finder* afterwards). It also says what the
collector already thinks is wrong, so a fixable problem need not be sent anywhere. See
**Diagnostic reports** below.

*Open an issue ↗* opens a **pre-filled** GitHub issue at `ADIIssuesURL` (`Info.plist`, canonical
value = the workspace `Cargo.toml`'s `repository`): a blank space to write in, then the ADI
version and flavour, the macOS version and the architecture slice actually running, the three
setup gates, every service's state, and the findings from the last report by name. Those are the
facts that decide a bug report and the ones nobody thinks to include — each one otherwise costs a
round trip.

The one thing the URL cannot carry is the archive: GitHub takes an attachment only from a drop
onto the form, so the draft ends by asking for it by filename. Both buttons sit under the text
rather than beside it — side by side on the title's line they do not fit 340pt, and shortening
either costs the word that says what it does.

> Two things about the URL are easy to break. `URLComponents` leaves a literal `+` alone in a
> query and the form reads it back as a *space*, so a version like `1.2.0+dev` arrives mangled —
> hence the post-encoding fixup to `%2B`, which is the only point where a real plus and an
> encoded space can still be told apart. And the body is capped (6000 chars): a prefill past
> what GitHub or the browser will accept presents as a button that opens a blank page.

## Diagnostic reports

`adi-mono diagnose` collects one archive of this machine's ADI state. It is what the window's
report button runs, and it can be run in a terminal on a Mac whose app will not open at all:

```bash
/Applications/ADI.app/Contents/Resources/adi-mono diagnose
adi-mono diagnose --out ~/Desktop            # somewhere else
adi-mono diagnose --json                     # what the app reads
```

It lands in `~/.adi/mono/reports/adi-report-<version>-<stamp>.zip` — the store rather than the
Desktop, because writing to `~/Desktop` raises a macOS file-access consent prompt, and a prompt
is one more thing to fail on a machine somebody is already reporting as broken.

Inside: `summary.txt` (versions, the three setup gates, every service, and the collector's own
reading of what looks wrong — read this first), `status.json`, `install.txt` (bundle path,
quarantine, code signature, Gatekeeper, architectures), `services.txt` (`launchctl print` per
job), `network.txt` (the resolver file, `scutil --dns`, a `dig` against the resolver, listening
sockets, and HTTP probes of the panel and the front door), `update.txt`, `store.txt`,
`environment.txt`, `logs/` (the last 256 KB of every `adi*.log`, including the root front
door's), `crash-reports/`, and `unified-log.txt`.

Two properties are load-bearing and worth not breaking:

- **It reads and changes nothing.** No service is started, stopped or reconfigured, so it is
  safe on a machine mid-incident — including one whose DNS is down, which is the case it exists
  for.
- **Secrets never enter it.** `secrets/`, the database, agent transcripts and the front door's
  TLS keys are listed in `store.txt` and never opened; only a fixed whitelist of config files is
  copied verbatim, and every line written is passed through a redactor first, so a key whose
  name says it carries a credential ships as `«redacted»`. The marker list deliberately spells
  out `authorization` rather than `auth`, because `codesign` reports the signing chain as
  `Authority=…` and that is the one line saying whether the bundle is genuine.

It replaces the pair of hand-written scripts that used to be airdropped to whoever was stuck
(`build/adi-diagnose.sh`, `build/adi-services-diagnose.sh`, both untracked). Those still have
one use this does not: a Mac where the app will not launch at all can still be asked to run the
CLI, but only the scripts do a foreground launch of the app binary to show why it dies.

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
  ADIApp.swift         @main — the dark Window (content-sized) + the panel Window
  ContentView.swift    the window: header, the step the install is on, the footer
  PanelWindow.swift    the panel's window: the 48px bar, the tab strip, the page
  PanelTabs.swift      the tabs — the rule that the first one is always the app, and pinning
  WebPanel.swift       one tab's WKWebView: navigation, what leaves, what opens a tab, JS panels
  StatusLine.swift     the status dot + word, and the services switch
  DashboardButton.swift  the one filled button: Open control panel
  Maintenance.swift    the footer: the update row and the report-a-problem row
  Tokens.swift         design/tokens.css as Swift: colours, radii, spacing, type roles
  Controls.swift       the button style, dot, hairline, chip, field and code block
  Lucide.swift         GENERATED — the Lucide icons the app uses, drawn as SwiftUI paths
  ADILogo.swift        the mark, monochrome, drawn from Trefoil
  Trefoil.swift        the mark's geometry — the one Swift definition
  AppModel.swift       holds the last report + 2s refresh + isOn/busy + toggle
  Core.swift           the only bridge to core: runs `adi-mono`, decodes its JSON
  Models.swift         Codable mirror of `adi-mono status/update/diagnose --json`
apps/macos/
  Resources/Fonts/     Geist and Geist Mono (variable TTF, OFL) — bundled by build.sh
  lucide-gen.py        writes Sources/Lucide.swift (and the iOS copy) from crates/adi-ui/icons
  icon-gen.swift       renders the app icon (the coloured build of the mark); see below
  ADI.icns             the built app icon (Info.plist CFBundleIconFile = ADI)
```

### The design system, natively

The window is drawn to `design/DESIGN.md`, and the values come from `design/tokens.css` —
restated in `Sources/Tokens.swift` because a native app cannot import a stylesheet. Change a
value there first, then here. `Tokens.swift`, `Controls.swift` and `Lucide.swift` are the same
files in `apps/ios/AdiFleet`, so a fix lands in both by copying.

- **Type** is Geist, with Geist Mono for machine strings only. Both ship as variable TTFs in
  `Resources/Fonts`; `build.sh` copies the directory into the bundle and `Info.plist`'s
  `ATSApplicationFontsPath` registers it at launch. If registration fails the roles in
  `Tokens.swift` fall back to the system face rather than to nothing.
- **Icons** are Lucide and nothing else — no SF Symbols. `lucide-gen.py` writes the glyphs the
  apps use into `Lucide.swift` as their SVG markup, and `LucideShape` turns that into a `Path`
  at draw time (stroke 1.5 on the 24 grid, round caps). Add an icon by putting its name in the
  script's list and running it; the script refuses a path its parser cannot draw.
- **One orange per screen.** The panel button is the ready step's orange, unless an update is
  waiting — then *Update to X* takes it and the panel button becomes an ink fill.
- **The mark** in the window is monochrome — `Trefoil` at 52 / 74 / 100% of the ink. The
  coloured build (grey, orange, ink) is the app icon's alone.

**App icon** — `icon-gen.swift` draws the master PNG, and `build.sh --regen-icon` runs it
through `sips` + `iconutil` to rebuild `ADI.icns`. `build.sh` copies `ADI.icns` into the
bundle (Info.plist `CFBundleIconFile = ADI`). The icon is the coloured, flat build of the mark
on a paper tile; `ADI.icns` was last generated before that change and is redrawn by the next
`--regen-icon`.

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
