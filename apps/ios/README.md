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

Signing uses the free **Personal Team** in `project.yml`; override it with
`DEVELOPMENT_TEAM=XXXXXXXXXX ./build.sh device`. The app declares no capability, so there is nothing
a paid membership would unlock — the only cost of the free tier is that a build stops launching
after seven days and has to be reinstalled.

### Neither build uses `-destination`, on purpose

This Xcode ships the iOS SDK without the iOS **platform** component, so it reports every real device
as ineligible ("iOS 26.5 is not installed") and enumerates no simulator destinations at all. Both
failures happen during destination resolution, before anything is compiled, and both are avoided by
naming the SDK instead: `-scheme … -sdk iphonesimulator` for the simulator, `-target … -sdk
iphoneos` for a device. The products are identical, and `simctl` / `devicectl` install them by
identifier without Xcode's device machinery. The alternative — a multi-gigabyte `xcodebuild
-downloadPlatform iOS` — buys nothing here.

## Pairing

Pairing is pull-only (`docs/fleet.md` §8), so the phone mints and the node spends:

1. Tap **Pair a node**. The phone mints a single-use invite once its relay session is up.
2. Copy or AirDrop the command it shows, and run it on the node:
   `adi-mono mesh join adi-invite:…`
3. The node dials back. The phone files it under a petname and the row appears.

**The password is never typed.** The viewer mints it (§8) and this is the viewer, so the app puts it
straight into the Keychain — `ThisDeviceOnly`, so it cannot follow an iCloud account onto a device
that was never paired. Only a rotated password brings up a prompt.

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

**Services are named, not discovered.** The protocol has no "list your services" call, so the app
shows what the node's grants name. `http:app` lists one service; `http:*` lists what it can and
offers a field to name another. Nothing here guesses.

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
