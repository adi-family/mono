# App Store Connect — the fields to fill in

Paste-ready. This is only the values; the reasoning behind each one is in `APPSTORE.md`, and the
things that are still blocked are marked **BLOCKER**.

Work through it in App Store Connect's own order. Everything below is for **adi Fleet**,
`family.adi.fleet`, version **0.1.0**, build **3** (uploaded 2026-08-31, delivery 409d6e71).

---

## 1 · App Information  (left sidebar → General → App Information)

| Field | Value |
|---|---|
| Name | `adi Fleet` |
| Subtitle | `Your machines, on your phone` |
| Privacy Policy URL | **BLOCKER — nothing exists yet. `withadi.dev/privacy` is a 404.** |
| Category — Primary | **Developer Tools** |
| Category — Secondary | **Utilities** |
| Content Rights | Does **not** contain, show, or access third-party content |
| Age Rating | **4+** — answer "None" to every question in the questionnaire |

---

## 2 · Pricing and Availability

| Field | Value |
|---|---|
| Price | **Free** |
| Availability | All countries and regions |

---

## 3 · Version 0.1.0  (the "iOS App 0.1.0" page)

### Screenshots

Drag these in, in order. Both sets are required because the app ships for iPhone and iPad.

- **iPhone 6.5"** → `apps/ios/shots/iphone/store/` — `01-fleet.png`, `02-dashboard.png`,
  `03-invite.png`, `04-empty.png`  (**1284×2778**, which is what this listing asks for)
- **iPad 13"** → `apps/ios/shots/ipad/store/` — same four names  (2064×2752)

### Promotional Text  *(optional, editable without a review)*

```
Pair a machine by scanning the code it draws, and its dashboards are on your phone — reached by key, with no port open on either side.
```

### Description

```
adi Fleet puts the machines you run in your pocket.

Pair a Mac, a Linux box or a server once, and the dashboards and services it hosts show up here — reached directly by cryptographic key, over an encrypted connection, with no port open on either end and no account anywhere.

Pairing is a scan
Run one command on the machine, or open its Fleet page, and point your phone at the code it draws. There is no sign-up, no cloud account and nothing to type: a pairing token is nine hundred characters, which is exactly why it is a QR.

How it works
Your phone holds its own identity, generated on the device and kept in the Keychain. A machine authorises that key once, during pairing. From then on the phone dials the machine over an encrypted peer-to-peer connection — directly when the network allows it, and through a relay when it does not. The relay never sees inside the connection.

What it does
• See every machine you have paired, and what each one is running
• Open a service's own web interface, full screen
• Put a single dashboard on your Home Screen as a shortcut
• Pair by scanning a code, or by handing an invite to the machine

What it does not do
adi Fleet is a viewer. It does not run services, host anything, or open a port on your phone. It has no account system and no server holding your data — there is nothing to sign up for, because there is nothing to sign up to.

adi Fleet is the companion to ADI, which runs on your machines. You need at least one machine running ADI for this app to have anything to show.
```

### Keywords  (100 max — this is 87)

```
mesh,remote,dashboard,homelab,server,devops,self-hosted,ssh,monitor,localhost,tunnel,vpn
```

### URLs

| Field | Value |
|---|---|
| Support URL | `https://github.com/adi-family/mono/issues` |
| Marketing URL | `https://withadi.dev` |

### Build

Select **0.1.0 (3)**.

**Not build 2.** It was uploaded earlier the same day and predates the QR scanner: its binary has
no `ScanView`, its `Info.plist` has no `NSCameraUsageDescription`, and its pairing sheet still
reads "Enter an invite". It also lacks the fix for the grant race, so a reviewer tapping a
dashboard would get "The node refused this service" — which is the rejection this whole demo-node
exercise exists to avoid. Build 3 has all three.

### What's New

Leave empty — it is the first release.

### Copyright

```
2026 With ADI, Inc. · Ihor Herasymovych
```

Both names, which is what you asked for, and the field is free text so it takes them. But read the
next paragraph before assuming that makes the company the publisher.

**The seller name is not this field.** What the App Store shows as the seller comes from the
Apple Developer Program enrolment, and this one is enrolled as an individual: the distribution
certificate reads `Apple Distribution: IHOR HERASYMOVYCH (752556J5V6)`. So the listing will say
**IHOR HERASYMOVYCH** whatever the Copyright line says.

If **With ADI, Inc.** should be the publisher on the store page, that is an account change, not a
metadata change — the enrolment has to be converted to an Organization, which needs a D-U-N-S
number for the entity. Worth deciding now rather than after the first release, because the seller
name is what customers and any future acquirer see.

The company is real and the details are on file (`projects/adi/business/`): **With ADI, Inc.**,
Delaware C corporation, incorporated 20 August 2026, file # 10742375, via Stripe Atlas.

---

## 4 · App Review Information

| Field | Value |
|---|---|
| Sign-in required | **No** |
| Contact | your name, phone, email |

### Notes  — paste this whole block

```
adi Fleet shows dashboards running on machines you own; it needs one paired machine to show
anything. We keep one online for review. No account or sign-in is required.

1. Tap "Pair a node" (the + button, top right). The sheet opens on the "Scan a code" tab.
2. There is no machine in front of you to scan, so open "Paste the token instead" and paste
   the token below, then tap "Pair".
3. The machine ("adi-demo") appears in the list. Tap "Demo Shop" under it.
4. It opens a storefront dashboard - orders today, revenue, the last twelve hours, and the most
   recent orders with their fulfilment state. Every figure is generated on the remote machine
   and changes as it serves them; nothing is stored on the phone.

There is a second dashboard, "Hello Reviewer", which shows the remote machine's clock ticking
once a second if you would like a simpler demonstration that the data is live and remote. The
machine's own control panel is listed alongside both and opens the same way.

Opening a dashboard for the first time asks the machine to share it, so there is a few-second
wait before the page appears. That is the machine authorising this device, not a hang.

Each token below is one single long line, and each can be spent once. Please copy a whole line.
If the first is refused as already used, use the second.

1.
adi-invite:7b2276223a312c22656e64706f696e74223a226164696d6573683a376232323639363432323361323233323335363633363337333933353636363133363332363333323633333933353632333133313331363633363634333533333331333036363337363436313339333733313335333333303634363136333335333233313631333436363631333833393331363433323633363333313632333036363632333036363636363336333232326332323631363436343732373332323361356237623232353236353663363137393232336132323638373437343730373333613266326636643631363432653664366636653666326437323635366336313739326537373639373436383631363436393265363436353736326632323764326337623232343937303232336132323331333032653332333033343265333032653333336133333338333633313337323237643263376232323439373032323361323233333334326533313337333532653331333932653331333333313361333333383336333133373232376435643764222c226e6f6e6365223a226636373039323733336438373436353036393631313932613832643465393333222c2265787069726573223a313739313034343332327d

2.
adi-invite:7b2276223a312c22656e64706f696e74223a226164696d6573683a376232323639363432323361323233323335363633363337333933353636363133363332363333323633333933353632333133313331363633363634333533333331333036363337363436313339333733313335333333303634363136333335333233313631333436363631333833393331363433323633363333313632333036363632333036363636363336333232326332323631363436343732373332323361356237623232353236353663363137393232336132323638373437343730373333613266326636643631363432653664366636653666326437323635366336313739326537373639373436383631363436393265363436353736326632323764326337623232343937303232336132323331333032653332333033343265333032653333336133333338333633313337323237643263376232323439373032323361323233333334326533313337333532653331333932653331333333313361333333383336333133373232376435643764222c226e6f6e6365223a223133656134313935393233633932383534633637363431313137336632356237222c2265787069726573223a313739313034343332327d
```

**These expire 2026-10-03.** Re-mint with:

```bash
gcloud compute ssh adi-demo --zone=europe-southwest1-a --project=mono-504617 \
  --command 'sudo -u adi -i bash -c "export PATH=/home/adi/.local/adi/bin:\$PATH; adi-mono mesh invite --ttl 20160 --no-qr --json"'
```

---

## 5 · App Privacy  (left sidebar → App Privacy)

Answer: **"No, we do not collect data from this app."**

What that rests on, so it can be defended if asked:

- No account system, no analytics SDK, no third-party SDKs, no advertising.
- The device identity (an Ed25519 keypair) is generated on the device and stored in the Keychain
  as `family.adi.fleet.node`, `ThisDeviceOnly` — it never syncs to iCloud and never leaves the phone.
- Petnames and the fleet list are local files in the app container.

**One thing to check before you tick it.** We run the relays ourselves
(`<region>.mono-relay.withadi.dev`). When no direct path exists, our infrastructure carries the
traffic. It cannot read it — QUIC with TLS 1.3 between authenticated keys, end to end — but it
necessarily sees IP addresses. Whether that is still "Data Not Collected" turns on **what the
deployed `iroh-relay` logs and for how long**. Read that off the running boxes rather than
assuming. Getting it wrong is a metadata misrepresentation, which is worse than a rejected build.

---

## 6 · Already done — do not redo

| | |
|---|---|
| Export compliance | Declared in `Info.plist` (`ITSAppUsesNonExemptEncryption = false`). No upload will ask. |
| Build upload | 0.1.0 (3) is uploaded — the first build with the scanner and the grant fix. Builds 1 and 2 are superseded; numbers are not reusable, so the next one is 4. |
| App icon | 1024×1024, no alpha, in the build. |
| iPad orientations | All four declared — validation error 90474 is already fixed. |

---

## What still blocks Submit for Review

1. **The Privacy Policy URL.** Required field, nothing exists. This is the only hard stop.
2. **The demo node must stay up** through review. `adi-demo` is a plain GCE instance with no
   autohealing — nothing restarts it if it stops.
3. **iPad has a real layout bug**: the pairing sheet clips its own buttons ("Copy command" and
   "Share" are cut in half below the fold). Scrollable, so not fatal, but it is the first thing an
   iPad reviewer sees on that screen. Worth fixing before submitting the iPad build.
