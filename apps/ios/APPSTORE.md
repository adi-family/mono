# adi Fleet — App Store submission

What App Store Connect asks for, answered in one place, plus the state of each answer. Drafted
2026-08-31 against `MARKETING_VERSION 0.1.0` / `CURRENT_PROJECT_VERSION 1`.

Copy marked **draft** has not been reviewed by anyone but its author. Fields marked **BLOCKER**
cannot be submitted around — App Store Connect will not accept the version without them.

---

## 0. Decisions taken, and what changed on 2026-08-31

The operator settled the four open questions:

| | |
|---|---|
| Reviewer access | **Keep a real demo node online** and put a live invite in the review notes |
| Privacy policy | **The operator writes it.** Nothing here drafts one |
| How far to go unattended | **Upload, do not submit.** Pressing Submit for Review is the operator's |
| macOS | **Stays a notarized DMG.** ADI.app is not an App Store app and is not becoming one |

Choosing "a live invite in the review notes" turned out to need a capability the app did not have,
so it was built:

- **The phone can now spend an invite**, not only mint one — `adi_mesh_join`, `Viewer::join`, and a
  second mode in the pairing sheet. `docs/fleet.md` §8 already said the handshake was symmetric and
  the direction a deployment choice; only the viewer half of it existed. Without this a reviewer
  could not pair at all, because the old direction ends in "run `adi-mono mesh join` on your Mac".
- Verified against a live peer, not by inspection:
  `cargo test -p adi-mesh-ffi --test join_over_live_endpoint -- --ignored`.
- **Build 0.1.0 (2) is uploaded** to App Store Connect (delivery `2d81a2b2-…`, 2026-08-31).

And the question §1 could not answer from the checkout is now answered, by a 409 from App Store
Connect: **the app record exists and build 1 was uploaded on 2026-08-05.** Build numbers are not
reusable, so the next upload is 3.

## 1. Where this actually stands

Already done, and worth not re-deriving:

| | |
|---|---|
| App ID `family.adi.fleet` | registered on the developer portal |
| App Store provisioning profile | `iOS Team Store Provisioning Profile: family.adi.fleet`, minted 2026-08-05, valid to 2027-08-05 |
| Distribution certificate | `Apple Distribution: IHOR HERASYMOVYCH (752556J5V6)`, in the login keychain |
| Export compliance | declared in `Info.plist` (`ITSAppUsesNonExemptEncryption = false`) — not asked again per upload |
| iPad orientations | all four declared; validation error 90474 already hit and fixed |
| App icon | 1024×1024, no alpha, regenerated from the current Trefoil on 2026-08-31 |
| Toolchain | Xcode 26.6, iOS 26.5 SDK **and** simulator runtime (`actool` needs the runtime even for a device build) |

The `Info.plist` comment about error 90474 says a build was validated against App Store Connect at
least once, around 2026-08-05. Whether an app **record** exists in App Store Connect — and whether
a build was ever accepted there — is not knowable from this checkout. Look before uploading.

Still open: everything in §2 and §3 below.

---

## 2. Blockers

### 2.1 There is no privacy policy — BLOCKER

Every app needs a Privacy Policy URL. It is a required field on the version, not a guideline.

`withadi.dev` serves none: `/privacy`, `/privacy-policy`, `/legal/privacy`, `/terms` and `/legal`
all 404 (checked 2026-08-31; the root is 200, so this is absence, not an outage). The repo's only
legal document is `legal/terms-of-use.gen.md`, and its own header says **"Draft. Not reviewed by a
lawyer, and not in force."**

There is no way to submit without this. It needs a real page, published, at a stable URL.

### 2.2 Guideline 2.1 — the mechanism now exists, the demo node does not yet

A reviewer gets an iPhone and nothing else. On first launch `ContentView` shows the `empty` state —
**No nodes yet** — and before 2026-08-31 the only way past it was to run `adi-mono mesh join` on a
Mac the reviewer does not have. That is now fixed in the app: **Pair a node → Enter an invite**
takes a token the machine minted, so a reviewer can pair with a machine they will never touch.

**The demo node exists.** Built 2026-08-31 by `scripts/deploy-demo-node.sh`:

| | |
|---|---|
| Instance | `adi-demo`, `europe-southwest1-a`, `e2-small`, 20GB, Debian 12 — project `mono-504617` |
| Runs | ADI 1.0.1 from the released `adi-linux-x64.tar.gz`, all services up under `systemd --user` |
| Paired with this Mac | as `adi-demo`; its panel opens at `http://app.adi-demo.n.adi/` (verified 200) |
| Review invites | four minted, 14 days each, expiring **2026-09-14** — in `~/adi-demo-review-invites.txt` |

Re-mint at any time (they are cheap, and they expire):

```bash
gcloud compute ssh adi-demo --zone=europe-southwest1-a --project=mono-504617 \
  --command 'sudo -u adi -i bash -c "export PATH=/home/adi/.local/adi/bin:\$PATH; adi-mono mesh invite --ttl 20160 --no-qr --json"'
```

Put **all four** in App Review Information → Notes. An invite is one machine *once*, whatever its
TTL, so a reviewer who retries — or a second reviewer on an appeal — needs the next one.

### What the reviewer sees on it

Two things, and both are reachable from a phone that has only just paired:

- **`app`** — the ADI control panel itself, granted by default at pairing (`join.rs`,
  `DEFAULT_SERVICE`).
- **Hello Reviewer** (`hello-reviewer.adi`) — a one-panel dashboard reading *"Hello reviewer,
  server time is 2026-08-31 06:55:18 UTC"*, ticking every second against the machine's clock. It
  exists to make the demo obviously **live** and obviously **remote**: the time is the node's, not
  the phone's, so it cannot be faked by a page the phone rendered on its own.

**A fresh pairing does not grant the dashboard, and does not need to.** The phone gets `http:app`
only; the dashboard is a separate service label. Tapping its row makes the app call
`adi_mesh_allow`, which posts `/api/fleet/grants/add` to the node's *own panel* with the credential
pairing already gave it — so the phone grants itself, with no operator involved.

That path was verified rather than assumed on 2026-08-31: this Mac's `http:hello-reviewer` grant
was revoked (dashboard → 502), re-added through `/api/fleet/grants/add` exactly as the app does it,
and the dashboard answered 200 again. So the reviewer's sequence really is pair → tap → read.

The residual risk is unchanged and worth restating: if the box is asleep or broken when the
reviewer looks, this fails, and a second rejection reads worse than the first. `adi-demo` is a
normal GCE instance with no autohealing — nothing restarts it if it is stopped.

The residual risk is honest and worth stating: if that machine is asleep when the reviewer looks,
this fails, and a second rejection reads worse than the first. A demo/offline mode with a fake
fleet is the version of this that cannot go offline, and is worth building if 2.1 comes back.

Suggested review note:

> adi Fleet shows dashboards running on machines you own; it needs one paired machine to show
> anything. We keep one online for review — no account or sign-in is required.
>
> 1. Tap **Pair a node** (＋, top right), then the **Enter an invite** tab.
> 2. Paste this token and tap **Pair**: `adi-invite:…`
> 3. The machine ("adi-demo") appears in the list. Tap **Hello Reviewer** under it.
> 4. It opens a page reading "Hello reviewer, server time is …", ticking once a second. That
>    clock is the remote machine's, served over an encrypted peer-to-peer connection — no port is
>    open on either device.
>
> The machine's own control panel is listed alongside it and opens the same way.
>
> If a token is refused as already used, please use the next one:
> `adi-invite:…` / `adi-invite:…` / `adi-invite:…`

### 2.3 Screenshots — done

Both required sets exist, at exactly the sizes App Store Connect accepts, RGB with no alpha:

| | |
|---|---|
| iPhone 6.9" | `apps/ios/shots/iphone/store/01..04.png` — 1320×2868 |
| iPad 13" | `apps/ios/shots/ipad/store/01..04.png` — 2064×2752 |

Four panels, in order: the fleet, a dashboard open, the pairing QR, and the empty state carrying
the security claim. Regenerate with `apps/ios/shots.sh` (captures) then `apps/ios/frames.py`
(composition); both are repeatable and neither needs a human to tap anything.

**A real bug came out of doing this, and it is not fixed:** on iPad the pairing sheet **clips its
own buttons**. `Copy command` and `Share` are cut in half below the fold of a form sheet that does
not grow to its content — visible in the raw capture at `shots/ipad/02-invite.png`. The sheet is a
`ScrollView`, so they can be scrolled to and the flow is not dead, but on the iPad the first thing
a reviewer sees on the pairing screen is a severed control. Worth fixing before submitting iPad.

<details><summary>What this used to say (the two things that made it hard)</summary>

### The old blocker

None, in any size. Because the app targets iPhone **and** iPad (`TARGETED_DEVICE_FAMILY "1,2"`),
both sets are required:

- iPhone 6.9" — 1290×2796 or 1320×2868
- iPad 13" — 2064×2752 or 2048×2732

Two things are in the way, and only one of them is work.

**The content.** A screenshot of this app is a screenshot of a *populated fleet*. Pairing the
simulator with this laptop populates it with the real one — `nosh`, `bugbounty`, `adi-gtm`,
`nakit-yok` — and those are project names that would then be on a public store page forever. What
gets photographed is a disclosure decision, so it needs the demo node from §2.2 with a curated set
of dashboards on it, not this machine.

**The mechanism.** `simctl` can boot, install and screenshot, but it cannot tap, so nothing can
navigate to the screen being photographed. Two ways round it:

- **A UI test target.** What `fastlane snapshot` does: an XCUITest drives the app and calls
  `XCUIScreen.main.screenshot()` on each screen. Repeatable, survives a redesign, and is the only
  option that scales to a second language. Needs a `AdiFleetUITests` target in `project.yml`.
- **Grant Terminal/osascript accessibility access** in System Settings → Privacy & Security →
  Accessibility, and drive Simulator with System Events. Ten minutes, no code, but it is a manual
  ritual repeated every time a screenshot changes. `osascript` is currently denied (`-1719`).

Two simulators are already created for this, and the sizes are confirmed rather than assumed —
`simctl io … screenshot` on the iPhone returned exactly 1320×2868:

```bash
xcrun simctl list devices | grep adi-shot     # adi-shot-iphone (17 Pro Max), adi-shot-ipad (13" M4)
```

Delete them with `xcrun simctl delete adi-shot-iphone adi-shot-ipad` if this approach is dropped.

**It went the UI-test way**, and the harness is `AdiFleetUITests/ScreenshotTests.swift`.

</details>

---

## 3. The fields, drafted

### Identity

| Field | Value |
|---|---|
| Name | `adi Fleet` |
| Subtitle (30) | `Your machines, on your phone` (28) — **draft** |
| Bundle ID | `family.adi.fleet` |
| Primary category | **Developer Tools** |
| Secondary category | **Utilities** |
| Age rating | 4+ — no user-generated content, no web browsing of arbitrary URLs (the web view only ever loads a service on a node you paired with) |
| Price | Free |
| Copyright | `2026 Ihor Herasymovych` — **confirm the legal name to use** |

On the category: Developer Tools is right for what it is — a viewer for services running on
machines you administer. Productivity was the other candidate and is a worse fit; nothing here
organises the user's own work.

### URLs

| Field | Value |
|---|---|
| Support URL | `https://github.com/adi-family/mono/issues` — **or** a real support page on `withadi.dev` |
| Marketing URL | `https://withadi.dev` |
| Privacy Policy URL | **MISSING — see §2.1** |

Apple accepts a GitHub issues page as a support URL. It is honest for a developer tool, but a
`withadi.dev/support` page is the stronger answer if the privacy page is being written anyway.

### Keywords (100 characters max) — draft

```
mesh,remote,dashboard,homelab,server,devops,self-hosted,ssh,monitor,localhost,tunnel,vpn
```

87 characters. Do not repeat words already in the name or subtitle — Apple indexes those anyway.

### Description — draft

> adi Fleet puts the machines you run in your pocket.
>
> Pair a Mac, a Linux box or a server once, and the dashboards and services it hosts show up here —
> reached directly by cryptographic key, over an encrypted connection, with no port open on either
> end and no account anywhere.
>
> **How it works**
>
> Your phone holds its own identity, generated on the device and kept in the Keychain. A machine
> authorises that key once, during pairing. From then on the phone dials the machine over an
> encrypted peer-to-peer connection — directly when the network allows it, and through a relay when
> it does not. The relay never sees inside the connection.
>
> **What it does**
>
> • See every machine you have paired, and what each one is running
> • Open a service's own web interface, full screen
> • Put a single dashboard on your Home Screen as a shortcut
> • Pair by scanning a code, or by AirDropping the invite to the machine
>
> **What it does not do**
>
> adi Fleet is a viewer. It does not run services, host anything, or open a port on your phone. It
> has no account system and no server holding your data — there is nothing to sign up for, because
> there is nothing to sign up to.
>
> adi Fleet is the companion to ADI, which runs on your machines. You need at least one machine
> running ADI for this app to have anything to show.

That last paragraph is deliberate and should survive editing: it is the honest statement of the
prerequisite, and burying it is how an app gets rejected under 2.1 *and* one-starred by people who
downloaded it expecting it to do something on its own.

### What's New (for 0.1.0)

First release — omit, or `First release.`

---

## 4. App Privacy — answer honestly, and check the relay before you do

The easy half:

- **No account system, no analytics SDK, no third-party SDKs, no advertising.**
- The device identity (an Ed25519 keypair) is generated on the device and stored in the Keychain as
  `family.adi.fleet.node` with `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly` — it never syncs
  to iCloud and never leaves the phone.
- Petnames and the fleet list are local.

The half that needs a decision: **we operate the relays ourselves** —
`<region>.mono-relay.withadi.dev`, deployed by `scripts/deploy-relay.sh` onto GCE. When no direct
path exists, our infrastructure carries the traffic. It cannot read it (QUIC with TLS 1.3 between
authenticated peers, end to end), but it necessarily sees IP addresses and connection metadata.

Whether that is "Data Not Collected" turns on **what the relay logs and for how long** — which is a
question about the deployed `iroh-relay` configuration, not about this app. Answer it from the
running boxes before ticking the box. Getting this wrong is a metadata misrepresentation, which is
a worse category of problem than a rejected build.

---

## 5. Uploading — done once, and how to do it again

The scheme already archives Release (`project.yml`), which is deliberate — archiving Debug ships
assertions and no optimisation, and it *installs and runs*, which is what makes that mistake
survive review.

```bash
cd apps/ios
./build.sh core                       # both staticlibs, release
./build.sh project                    # regenerate the xcodeproj from project.yml

xcodebuild -project AdiFleet.xcodeproj -scheme AdiFleet \
    -configuration Release -sdk iphoneos \
    -archivePath build/AdiFleet.xcarchive archive

xcodebuild -exportArchive -archivePath build/AdiFleet.xcarchive \
    -exportPath build/export -exportOptionsPlist ExportOptions.plist

xcrun altool --validate-app -f build/export/AdiFleet.ipa -t ios \
    -u "$AC_USER" -p "$AC_PASS"      # validate first, always
xcrun altool --upload-app  -f build/export/AdiFleet.ipa -t ios \
    -u "$AC_USER" -p "$AC_PASS"
```

`ExportOptions.plist` does not exist yet — it needs `method: app-store-connect` and
`teamID: 752556J5V6`.

Credentials: `AC_USER` / `AC_PASS` (an **app-specific** password) live in `apps/macos/.env`, which
is gitignored and shared with the Mac app's notarization step. There is no App Store Connect API
key on this machine — everything here goes through altool with the Apple ID.

**Validate before uploading, every time.** A validation failure costs nothing; a rejected upload
burns a build number, and build numbers are not reusable.
