# Fleet demo — runbook

The story: a Linux machine with **no inbound ports at all** appears in your browser under real
hostnames, behind a password. See `docs/fleet.md` for the design.

## What you need in front of you

- A paired node. `adi-mono mesh fleet` names it; `apps/linux/test-node/run.sh` makes one in ~25 s
  if you want a throwaway.
- Its password. Pairing prints it once; `adi-mono mesh passwd <peer> --password <pw>` on the node
  sets a new one you can type.
- Open `http://app.<node>.n.adi/` — the node's own control panel — and, once you make one there,
  `http://<dashboard>.<node>.n.adi/`. Both work over `https://` as long as the node was paired
  before the last front-door start (see the traps below).
- The panel's **Fleet** page is at `/extended/settings/fleet`. Local hosts (`app.adi`, `api.adi`,
  and your own dashboards) are untouched by any of this.

## The three beats

1. **No service is exposed.** On the node, every HTTP listener is on `127.0.0.1` or `127.0.0.53`:
   the front door, the control panel, the resolver, the mesh gateway, the dashboard's two bun
   servers. Nothing answers from outside the machine.
   ```
   docker exec adi-demo ss -tlnp
   ```
   Be precise if asked, because two non-loopback sockets do exist and someone will spot them:
   a **UDP** port with no service behind it — iroh's QUIC endpoint, which is *how* the outbound
   session receives its replies and accepts nothing without a handshake against this node's key —
   and `:5355`, which is systemd-resolved's LLMNR, Debian's default and not ours. The claim to
   make is "no inbound service, and no port to forward", not "no open socket".
2. **It is still a normal machine to you.** Open `app.<node>.n.adi` — the browser asks for the
   password once, then it is that machine's control panel. Create a dashboard from it.
3. **The dashboard is one origin.** It appears at `<name>.<node>.n.adi`, and its backend answers
   at `/api` on the same host — no port, no address in the page, so the same dashboard works
   under its local name, its fleet name, and a real domain later.

## Bringing up a fresh node live (≈25 s)

```
bash apps/linux/test-node/run.sh adi-demo2
```
It prints the hostname and password. To do it by hand, the same three steps:
```
adi-mono mesh invite                 # on this machine
./install.sh '<token>'               # on the node — dials out, nothing opened
adi-mono mesh passwd <viewer> --password <pw>   # on the node, for a password you can type
```

## Don't do these during the demo

- **Do not restart the front door.** `*.adi` goes down for ~40 s while it re-leases ports and
  re-mints the certificate. Anything that replaces `target/release/adi-hive` triggers it, because
  the daemon watches its own binary — so no `cargo build -p adi-hive`.
- **Do not re-pair a node you are already showing.** Pairing rotates the password, and the one on
  screen stops working.
- Adding or removing a node changes the certificate's name list, which only takes effect at the
  next front-door start. `http://` is unaffected; `https://` to a *newly* paired node warns until
  then.

## If something is wrong

| Symptom | Meaning |
| --- | --- |
| `node not paired` page | the petname is not in the local registry — `adi-mono mesh fleet` |
| `no such service` | the node has no such host — check it on the node: `curl http://<name>.adi/` |
| `mesh gateway unavailable` | the control panel is down (the gateway runs inside it) |
| `401` | wrong password; set a new one on the node with `adi-mono mesh passwd` |
| everything `000` | the front door is restarting — wait ~40 s |
