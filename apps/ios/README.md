# adi Fleet — the mesh on a phone

A phone joins the fleet as a **viewer**: it holds an Ed25519 identity, is authorized by key like any
other peer, and reaches a node's services over the same `adi/mesh/http/1` that a Mac uses. What it
never does is host. It runs no hive, no DNS, no services, and answers exactly one ALPN — the
[pairing](../../docs/fleet.md) one, so a node can dial back to be enrolled. Everything else it
initiates.

That asymmetry is the whole reason this needs no entitlement, no VPN profile, no paid developer
membership and no open port. It is also why the app is a viewer and not a node: a node is
`adi-mono` under a supervisor, and iOS has neither.

```
WKWebView → 127.0.0.1:<port>  (a listener inside this app)
          → [iroh, ALPN adi/mesh/http/1]
          → the node's mesh gateway
          → the node's local service
```

## Build and run

```bash
./build.sh              # simulator: builds the core, generates the project, installs, launches
./build.sh device       # device build (open the project once to pick a signing team)
./build.sh core         # just the Rust staticlibs, both targets
./build.sh project      # just regenerate AdiFleet.xcodeproj from project.yml
```

`project.yml` is the source of truth for the Xcode project; `AdiFleet.xcodeproj` is generated from
it with [xcodegen](https://github.com/yonaskolb/XcodeGen) and committed so that opening the app needs
only Xcode. A setting changed in Xcode's UI is lost at the next `build.sh project`.

Signing uses the team in `project.yml` — the paid **IHOR HERASYMOVYCH** team, `752556J5V6`.
Override it with `DEVELOPMENT_TEAM=XXXXXXXXXX ./build.sh device`.

The app declares no capability, so a free **Personal Team** signs it just as well — but the two
differ in one way worth planning around. A free team's provisioning profile lasts **seven days**,
after which the build stops launching and has to be reinstalled; a paid one is minted for a **year**
(check with `security cms -D -i .build-device/Products/AdiFleet.app/embedded.mobileprovision |
plutil -p -`, which is also how to tell which you actually got — `codesign -dv` reports the team
identifier, and the certificate's own name can name a different one).

### Neither build uses `-destination`, on purpose

This Xcode ships the iOS SDK without the iOS **platform** component, so it reports every real device
as ineligible ("iOS 26.5 is not installed") and enumerates no simulator destinations at all. Both
failures happen during destination resolution, before anything is compiled, and both are avoided by
naming the SDK instead: `-scheme … -sdk iphonesimulator` for the simulator, `-target … -sdk
iphoneos` for a device. The products are identical, and `simctl` / `devicectl` install them by
identifier without Xcode's device machinery. The alternative — a multi-gigabyte `xcodebuild
-downloadPlatform iOS` — buys nothing here.

## Pairing

The handshake is symmetric and which side dials is a deployment choice (`docs/fleet.md` §8), so the
sheet offers both directions. They end in the same registry record, and a node paired either way is
indistinguishable afterwards.

**Invite a machine** — the phone mints, the machine spends. The default, and the right way round
when you are sitting at the machine:

1. Tap **Pair a node**. The phone mints a single-use invite once its relay session is up.
2. Copy or AirDrop the command it shows, and run it on the node:
   `adi-mono mesh join adi-invite:…`
3. The node dials back. The phone files it under a petname and the row appears.

**Enter an invite** — the machine mints, the phone spends. The right way round when whoever holds
the phone is not at the machine, or has no terminal to paste into at all:

1. Run `adi-mono mesh invite` on the machine (`--ttl <minutes>` if it will not be spent right away).
2. Tap **Pair a node → Enter an invite**, paste the token, tap **Pair**.
3. The phone dials out to the machine. Nothing opens a port, on either side.

It is a paste field and not a camera: the token arrives by whatever channel is already open, and a
camera permission bought to read a code off a screen you could paste from would be a permission
bought for nothing. The QR stays on the *minting* screen, where the token is on a terminal and the
phone is the thing holding a camera.

**The password is never typed**, whichever direction it went. The minting side mints it (§8): in
the first direction that is this app, and in the second it is the machine, which hands it over once
in the join reply. Either way it goes straight into the Keychain — `ThisDeviceOnly`, so it cannot
follow an iCloud account onto a device that was never paired. Only a rotated password brings up a
prompt.

> An invite is **one machine, once**, whatever its TTL. Two people spending the same token is not a
> thing that works, and the second one is told the nonce is spent — which matters when handing an
> invite to someone you cannot talk to, like App Review. Mint one each.

## Dashboards

A node's dashboards are listed under it by name, and a tap opens one full-screen on its own origin
— the same page, the same `/api`, as on the machine itself (`docs/fleet.md` §4).

Two things had to be true for that, and neither is a protocol change:

**The list comes from the node's control panel.** `adi/mesh/http/1` deliberately cannot answer
"what do you serve?" — the node refuses an unauthorized peer *before* it looks at its route table,
so that nobody can enumerate a machine's services by watching two refusals differ. But `app` is a
service like any other: it is what the default grant names, it is behind the node's password, and it
already publishes `GET /api/dashboards`. A phone holding that password is entitled to the answer, so
it just asks (`viewer::catalog`).

**A dashboard nobody has named is not shared yet.** §8 makes the default grant `http:app` and
nothing else, on purpose — pairing a phone must not hand it every page on the machine. So a freshly
paired node lists its dashboards with *Not shared with this phone yet*, and the tap asks for the
grant (`POST /api/fleet/grants/add`) before binding the port. That asks through the panel the phone
is already inside, with the same password, so it is a reach and not an escalation: `http:app` can
already create dashboards and run tasks. What the grant buys is the browser reaching the page
directly, on its own origin, instead of driving it through the panel.

The grant is mirrored into the phone's own registry afterwards, so its node list agrees with what
the node will actually serve — the same thing pairing does for the grants it writes. The mirror is
never consulted when opening: the node decides every request on its own copy (§5).

## Two decisions worth knowing

**A port per service, not one gateway.** On a Mac the front door owns `*.n.adi` and hands the
gateway a `Host` to parse. Nothing on an unjailbroken iPhone can make `n.adi` resolve without a
Network Extension, so there is no hostname to route on. Instead each `(node, service)` gets its own
loopback listener and the target is fixed when the listener is bound. That preserves what §4 exists
to protect — a service is one origin — so relative URLs, cookies, `localStorage` and WebSocket
upgrades all behave normally. The ports are recorded and re-used across launches
(`viewer::ports`), because an origin that moved would drop the page's stored state every time.

The node therefore sees `Host: 127.0.0.1:<port>`. That is correct rather than a compromise: §3
forbids rewriting `Host` so that a node's absolute redirects land back on the origin the browser is
actually on — and here that origin *is* the loopback port. Routing on the far side never reads
`Host`; it uses the service label in the frame.

**Everything but a dashboard is named, not discovered.** The protocol still has no "list your
services" call, and dashboards are only listed because the control panel knows them by name (above).
For the rest, the app shows what the node's grants name: `http:app` lists one service; `http:*` lists
what it can and offers a field to name another. Nothing here guesses.

## What is not built

- **Real `*.n.adi` names in Safari.** That needs an `NEPacketTunnelProvider` to own DNS and TCP
  system-wide, which needs the Network Extension entitlement and a paid membership. The Rust core
  is already the right shape for it — the same viewer, with a packet source instead of a listener.
- **Serving from the phone.** A viewer hosts nothing, deliberately.
- **Background operation.** iOS freezes the process; the app retires its connection pool on the way
  back to the foreground and re-dials. A push-driven wake would be a separate design.

## Testing against a real node

```bash
bash apps/linux/test-node/run.sh adi-phone-test     # a throwaway node, ~25 s
docker exec -u adi adi-phone-test sh -c \
    'export PATH=$HOME/.local/adi/bin:$PATH; adi-mono mesh join <the app's invite>'
```

The node ends up with two viewers — the Mac from `run.sh` and the phone — which is the intended
shape: a grant is per peer, so the Mac reaching it proves the node is healthy and any phone-side
failure is the phone's.

Never point this at the demo node: re-pairing rotates a password that may be on screen.
