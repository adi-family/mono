# Auto-update — how a machine gets the next version

Every installed copy of adi polls one small JSON file and, when it names a version newer than
the one installed, downloads that release, verifies it, swaps it in, restarts the stack, and
rolls back if the stack does not come back. Publishing a release is the whole act of shipping:
no machine needs to be touched.

There is exactly one artifact per platform and it contains *every* binary — the notarized
`ADI.app` bundle on macOS, the node package on Linux and Windows. A binary added in a future
release therefore ships with no updater change at all, which is the property the whole design
is built around.

## 1. Cutting a release

**The git tag is the source of truth for the version.**

```
# 1. rename `## Unreleased` in CHANGELOG.md to `## 0.3.0 — 2026-08-21`, and open a fresh one
# 2. commit that
git tag v0.3.0 && git push --tags
```

That is the entire release procedure. `.github/workflows/release.yml` picks the tag up and
builds all three platforms, then writes and publishes the manifest.

Step 1 is not optional and not a convention — the workflow's *first* job runs
`scripts/changelog.sh` against the tag's version and **refuses the release** if the file has
no section for it. That check is deliberately in front of the builds rather than beside the
publish: the notes are what every machine is shown before it takes the release (§5), and
discovering they were never written after three platforms have spent half an hour compiling
is discovering it too late. Forgetting to rename `## Unreleased` looks exactly the same from
there, and is caught the same way.

`scripts/version.sh` is what resolves the number, and every build path calls it:

| order | source | when |
|-------|--------|------|
| 1 | `$ADI_VERSION` | CI, set from the pushed tag |
| 2 | the tag on `HEAD` | a local release build |
| 3 | the nearest tag | a dev build between releases |
| 4 | the workspace version | no tags, or no git |

The resolved number must parse as `major[.minor[.patch]]`; anything else is rejected on the
spot, because [`Version::is_newer`](../crates/adi-update/src/version.rs) fails closed on a
version it cannot read and a machine stamped `0.2.0-rc1` would quietly stop updating forever.

A dev build between releases reports the *previous released* version deliberately. Comparison
is strict, so such a build is never "older" than the release it came after — nothing overwrites
a work-in-progress bundle — while the next real release still lands normally.

The packaging scripts export `ADI_VERSION` before invoking cargo, so the same number is
compiled into the binaries (`adi_update::BUILT_VERSION`, `adi-mono --version`, adi-app's
`/api/health`), stamped into `Info.plist`, written to the node's `VERSION` file, and published
in the manifest. They cannot drift.

## 2. The manifest

`manifest.json`, attached to each release and written by `scripts/manifest.sh`:

```json
{
  "version": "0.2.0",
  "pub_date": "2026-08-01T10:56:25Z",
  "notes": "ADI v0.2.0",
  "dmg": { "url": "…/ADI.dmg", "sha256": "…", "size": 104857600 },
  "artifacts": {
    "macos":          { "url": "…/ADI.dmg",              "sha256": "…", "size": 104857600 },
    "linux-x86_64":   { "url": "…/adi-linux-x64.tar.gz", "sha256": "…", "size": 41943040 },
    "windows-x86_64": { "url": "…/ADI-windows-x64.zip",  "sha256": "…", "size": 44040192 }
  }
}
```

`notes` is one section of [`CHANGELOG.md`](../CHANGELOG.md), lifted out by
`scripts/changelog.sh` — markdown, and the release's own case for being taken. It is the only
field here written for a person rather than for the updater.

Clients look up `artifacts[host_platform()]` — `macos` for the universal bundle (one DMG covers
both arches, so the key carries no architecture), `<os>-<arch>` everywhere else.

The top-level `dmg` field is **legacy and must keep being published**: clients released before
per-platform artifacts existed require it, and dropping it would strand every Mac still running
one of those builds. `scripts/manifest.sh` mirrors the `macos` artifact into it automatically.

A release that publishes no artifact for a platform is not an available update *for that
platform* — the check reports it as such instead of offering an update that would fail to
download on every scheduled run.

Clients poll `https://github.com/adi-family/mono/releases/latest/download/manifest.json` by
default. Point `manifest_url` in `~/.adi/mono/update/config.toml` at any static host to change
channels; `auth_header` is there for a private one.

**A release must be marked latest** (`gh release create --latest`) or that default URL never
sees it.

## 3. What happens on a machine

The background agent (`family.adi.app.updater`, a periodic launchd / systemd / Task Scheduler
job) runs `adi-mono update run --quiet` at login and every `check_interval_hours` (6 by
default, clamped to at least 1 so a mis-edit cannot become a poll loop).

1. **Check.** Fetch the manifest; compare its version against what is *installed* — the
   bundle's `Info.plist`, or the `VERSION` file beside the node's binaries. Never against the
   running process: the CLI in your hands may be a repo dev build.
2. **Download + checksum.** Nothing is mounted, unpacked or executed until the bytes match the
   manifest's sha256.
3. **Authenticate.** On macOS the bundle must pass `codesign --verify --deep --strict` *and* be
   signed by team `752556J5V6`. On Linux and Windows there is no codesign; the trust anchor is
   the sha256 in a manifest fetched over TLS.
4. **Preflight.** Run the downloaded `adi-mono --version`. It must start, and it must report the
   version the manifest promised. The second half matters more than it looks: a release whose
   binaries were built without the tag would install, still read as older than the published
   version, and reinstall itself on every check forever.
5. **Swap.** macOS renames the old bundle aside and the new one in — two atomic metadata
   operations on the same volume. A node replaces its binaries one rename-aside at a time
   (renaming a *running* executable is allowed on both platforms; only deleting it is refused
   on Windows), then records the new number in `VERSION`.
6. **Restart.** Kickstart the per-user services, relaunch the menu-bar app on macOS, then run
   the **new** `adi-mono up` — which is what enables services a newer version introduces.
   The root front door is never restarted here: it watches its own binary (`ADI_WATCH_SELF`)
   and exits when the bundle changes, so its supervisor respawns it without an admin prompt.
7. **Health check.** Wait up to 90s for every service that was up *before* the update to be up
   again. Services the user had deliberately stopped are not expected back — otherwise a
   stopped stack would read as a failed update and roll back a perfectly good release.
8. **Roll back on failure.** Restore the backup, restart onto it, and record `rolled-back` with
   the reason in `state.json`.

Step 7 is what stands between a bad release and a bricked machine. Supervisors restart a
crashing binary forever, so without a verdict there a broken update would sit in a crash loop
with no DNS, no front door, and no way left to deliver the fix.

Two previous installs are kept under `~/.adi/mono/update/backups/` for manual recovery.

## 4. Commands

```
adi-mono update check              # compare installed vs published, install nothing
adi-mono update run                # install if newer, restart, roll back if it fails
adi-mono update run --force        # reinstall even when not newer
adi-mono update run --no-restart   # swap, but leave services on the old binaries
adi-mono update status             # the persisted last check/install record
adi-mono update enable | disable   # the periodic background agent
```

`--json` on `check`, `run` and `status` for a machine-readable form.

## 5. From the control panel

The same three things the CLI does, in the top bar of every adi screen — the chat at `/`, the
setup wizard, and the workbench at `/extended`. A quiet `v0.2.0` chip beside the other bar
controls, and, only while there is one to take, a green **Update to 0.3.0** button next to it.
Hover the chip for what it knows; click it to check now. Both live in
[`crates/adi-webapp/src/update.rs`](../crates/adi-webapp/src/update.rs).

**The button opens the changelog; it does not install.** Taking an update restarts everything
the machine is running, which is not a thing to do to someone who meant to click its
neighbour — and the release carries notes precisely so there is an answer to "what for?". So
it opens *What's new*: the section of `CHANGELOG.md` published with that release, rendered as
markdown, with what installing will do said beside the two ways out — **Install 0.3.0** and
**Not now**. A release cut without notes still offers the update, on the version number
alone.

The notes reach the machine through `state.json`: `Engine::check` records the manifest's
`notes` alongside the version, so the dialog opens on what the last check already fetched
rather than going back to the network to find out what it is offering.

| endpoint | cost | what |
|----------|------|------|
| `GET /api/update` | two file reads | the chip's whole state — installed, last published seen, whether an install is running. Polled. |
| `POST /api/update/check` | one manifest fetch | §2's check, persisted. The page fires it on load *only* when the server reports `stale` — the record is older than `check_interval_hours` — so a machine with the background agent switched off still gets a truthful chip without every tab hitting GitHub on every load. |
| `POST /api/update/run` | spawns the CLI | §3, start to finish. |

**`run` does not install anything itself.** Step 5 of §3 swaps the binaries this very server is
running from and step 6 restarts it, so a handler that installed inline would be killed part-way
through its own reply. It spawns `adi-mono update run --quiet` — the bundled one, resolved beside
the running binary and never off `PATH` — in its **own process group**, and answers immediately.
The process group is load-bearing: the supervisor kills the app service's group on kickstart, and
an updater inside that group would die between the swap and the health check, leaving the machine
on unverified binaries with nothing left running to roll them back.

That leaves a gap `state.json` cannot describe — the minutes between the button and the verdict,
spanning the app's own death — so the endpoint writes `update/installing.json` (the child's pid
and start time) and `GET /api/update` reports `installing` from it. The CLI knows nothing about
that file; whoever reads it next collects it, once the pid is gone or half an hour has passed.
`update/run.log` holds that run's output.

None of this is per-platform. The panel asks the same three questions on a Mac, a Linux node and
a Windows node, and every difference — DMG or tarball, launchd or systemd or Task Scheduler — is
behind the CLI it hands the work to. A release publishing no artifact for the host reads as "not
an update for this machine" rather than as an offer that would fail to download.

## 6. Publishing by hand

`apps/macos/publish.sh` cuts a **macOS-only** release from a workstation with the signing
certificate and notarization credentials in `apps/macos/.env`. It refuses to publish when the
built bundle's version disagrees with the tree's tag.

That path leaves Linux and Windows on their old versions — it publishes no artifact for them,
which the client reports honestly rather than failing on. Use the workflow for a real release.

## 7. Release secrets

The macOS job is the only one that needs any:

| secret | what |
|--------|------|
| `MACOS_CERT_P12` | base64 of the "Developer ID Application" `.p12` |
| `MACOS_CERT_PASSWORD` | its export password |
| `APPLE_ID` | Apple ID used for notarization |
| `APPLE_APP_PASSWORD` | app-specific password for that Apple ID |
| `APPLE_TEAM_ID` | must match `adi_update::DEFAULT_TEAM_ID` |

The workflow checks all five are present before building, since an unsigned bundle is refused
by Gatekeeper on every other Mac and by the updater's own Team ID check.

## 8. Things that will bite

- **`crates/adi-app/build.rs` creates an empty `dist/` when the webapp has not been built.**
  That keeps a fresh checkout compiling, but at release time it means a packaging run that
  forgot `trunk build` ships a control panel with no UI — and nothing downstream notices.
  `scripts/require-webapp-dist.sh` is the guard; every packaging script calls it first.
- **macOS App Management blocks replacing `/Applications/ADI.app`** from an unentitled process,
  including under `sudo`. The updater agent has the entitlement; a terminal generally does not.
  The error names this when it happens.
- **The version comparison is numeric and strict.** `0.10.0` is newer than `0.9.9`; `0.2.0` is
  not newer than itself; anything unparseable is never newer, in either direction.
- **Never restart ADI DNS by hand** to force an update through — see the hard rule in
  `CLAUDE.md`. The updater kickstarts it as part of a swap, which is a different thing from an
  operator stopping it.

## 9. Not built yet

- **No staged rollout.** Every machine takes a release as soon as it appears; there is no
  canary ring and no way to pause a bad one except publishing another.
- **No signature on the node artifacts.** Linux and Windows trust the manifest's sha256 over
  TLS. Since the manifest and the artifact come from the same GitHub release, an attacker who
  can replace one can replace both — macOS's codesign check has no equivalent there yet.
  `Artifact` has room for a `sig` field when that changes.
- **No fleet view of versions.** §5's pill answers for *the machine whose panel you have open*
  — a node's own panel, read over the mesh, shows and updates that node. There is no one screen
  listing what every paired node is on, and no "update the fleet" action.
