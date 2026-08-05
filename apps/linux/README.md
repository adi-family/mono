ADI for Linux — a fleet node
============================

The ADI platform (DNS + front door + control panel + mesh), statically linked against musl so
one tarball runs on any x86-64 distro.

This package builds a **node** in the sense of `docs/fleet.md`: a full adi machine whose only
network-facing action is an *outbound* QUIC session to an iroh relay. It listens on nothing but
loopback — no `:22` exposure, no `:80`, no `:443` — and you reach it from your own machine
through the mesh, at `<service>.<node>.n.adi`.

There is no GUI and no launcher. The control panel is the same web UI `adi-app` serves
everywhere; on a node you open it from *your* browser, through the mesh, at
`app.<node>.n.adi`.


What's in this folder
---------------------

    bin/adi-mono   The CLI and the brain — every command (`adi-mono up`, `status`, `mesh join`, ...).
    bin/adi-dns    The .adi / .test split-DNS resolver.
    bin/adi-hive   The front-door reverse proxy that serves *.adi hosts.
    bin/adi-app    The web control panel, and the host of the in-process mesh daemon.
    bin/adi-mesh   The standalone mesh CLI — `id`, `ticket`, `forward` (the daemon itself runs
                   inside adi-app, so this is for inspection and port forwards).

    install.sh     Install, start, and pair. The only thing you have to run.
    VERSION        The workspace version this package was built from.


Quick start
-----------

On the machine you are pairing *from*, mint an invite token. Then, on the node:

    tar -xzf adi-linux-x64.tar.gz
    cd adi-linux-x64
    ./install.sh '<invite-token>'

That is the whole install. It needs no root, and it opens no inbound port — not even
temporarily — because pairing is a dial-out: the node contacts the relay, never the reverse.

`install.sh` copies the binaries to `~/.local/adi/bin`, enables lingering, runs `adi-mono up`,
and then `adi-mono mesh join <token>`. Pass `--prefix DIR` to install elsewhere, or `--no-pair`
to bring the services up now and pair later.

### bun — fetched, not bundled

A dashboard is a pair of [bun](https://bun.sh) servers. `install.sh` downloads bun into
`$PREFIX/bin` for you, pinned to a known version and **verified against a pinned SHA-256** before
it is made executable. Pass `--no-bun` to skip it.

It is fetched at install time rather than shipped inside this tarball on purpose. bun itself is
MIT, but it statically links JavaScriptCore (LGPL-2) and tinycc (LGPL-2.1); redistributing the
binary in our package would carry their relink obligation with it. Downloading from oven-sh means
the operator gets it upstream and unmodified — exactly as bun's own installer would — while we
still pin the version a node runs.

Two details that matter on a real node:

* It lands in `$PREFIX/bin`, **not** `~/.bun/bin` where bun's own installer puts it. A
  `systemd --user` unit inherits the manager's bare PATH, so a bun in `~/.bun/bin` is invisible to
  the dashboards supervisor. The units adi-core writes carry a PATH covering both.
* The x64 build needs AVX2; the installer reads `/proc/cpuinfo` and takes the `-baseline` build on
  a CPU without it, because the wrong one dies with "Illegal instruction" at first run rather than
  failing visibly at install.
* bun ships only as a `.zip`, and a minimal cloud image often has no `unzip` — which is exactly
  where bun's own installer stops. The installer falls back to `python3`'s stdlib zip reader.

If bun cannot be fetched (no outbound access to GitHub, say), the install still succeeds and the
node is still a working node: only dashboards need it. The dashboards supervisor then starts and
has nothing to run — a dashboard would be listed, have a hostname, and answer nothing — so
`install.sh` says so at the end rather than leaving that to be discovered.


How supervision works
---------------------

Each service is a **`systemd --user` unit** written by `adi-mono` into
`~/.config/systemd/user/` — the Linux counterpart of the macOS LaunchAgents and the Windows
scheduled tasks:

    family.adi.app.dns.service             the resolver
    family.adi.app.control-panel.service   adi-app
    family.adi.app.updater.service         the auto-updater, driven by a .timer beside it

They run as *you*, with no root anywhere. `Restart=always` with a short backoff and no start
rate limit is the `KeepAlive` equivalent: a service that crash-loops keeps being retried instead
of being given up on. Service output goes to files under `~/.adi/mono/logs/`, the same place
every other platform puts it, so the control panel's log view works unchanged.

    adi-mono up        Start everything (idempotent; safe to re-run).
    adi-mono status    Each service: enabled / running / detail.
    adi-mono disable   Stop and remove the units.

    systemctl --user list-units 'family.adi.*'
    systemctl --user status family.adi.app.control-panel.service
    tail -f ~/.adi/mono/logs/adi-app.log

### Lingering is not optional

A `systemd --user` manager is normally created at login and destroyed at the last logout — and
it takes every service with it. A node is headless: you install over ssh and disconnect. So the
installer runs

    loginctl enable-linger $USER

which is also re-attempted on every `adi-mono up`. The stock polkit rule lets an active session
enable lingering for its own user without a password; if it is refused (a non-interactive
session, say), the installer prints the fallback and you run it once:

    sudo loginctl enable-linger <user>

Without it the node works perfectly until you log out, and then stops. It is the single most
important line in this file.


Networking: nothing listens off loopback
----------------------------------------

Every adi service on a node binds `127.0.0.1`. Reachability comes from the mesh session the
node opens *outward*. **No inbound port is required at any point, including during install.**

The intended firewall is therefore the strictest one that still works — drop all inbound,
allow loopback and replies, and let outbound out:

    # nftables
    nft add table inet filter
    nft 'add chain inet filter input { type filter hook input priority 0; policy drop; }'
    nft 'add rule inet filter input iif lo accept'
    nft 'add rule inet filter input ct state established,related accept'

    # or iptables (mirror every line with ip6tables)
    iptables -P INPUT DROP
    iptables -A INPUT -i lo -j ACCEPT
    iptables -A INPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT

Outbound needs UDP/443 to reach an iroh relay, with TCP/443 as the fallback path when UDP is
blocked. Nothing else. A node behind NAT, on a residential line, or on a cloud instance with an
empty inbound security group is the normal case, not a special one.

### Then take ssh off the network too

Once the node is paired you no longer need inbound ssh either. Set

    # /etc/ssh/sshd_config
    ListenAddress 127.0.0.1

and reach it over the mesh instead: grant your peer `tcp:127.0.0.1:22` on the node, forward a
local port on your own machine, and ssh to that.

    # on your machine
    adi-mesh forward 2222 <node-ticket> 22
    ssh -p 2222 user@127.0.0.1

Do this in the right order — pair first, verify the mesh path works, *then* move sshd — and
keep an out-of-band console (cloud serial console, KVM, physical access) available. A firewall
change that also removes your only way back in is the classic way to lose a box.


The `.adi` domain on a node (optional, needs root)
--------------------------------------------------

The installer does not touch system DNS, and a node does not need it. You reach the node's
services from *your* machine, where your own front door resolves `<service>.<node>.n.adi`; the
mesh gateway on the node then resolves the service label against the node's own `hive.yaml`
route table and connects to that service's loopback port. Nothing on the node has to resolve a
name for any of that to work — which is why there is no root step in the install.

If you want the friendly names *on the node itself* (you ssh in and `curl http://app.adi/`),
that is one root-owned file. `adi-dns`'s Linux routing is a systemd-resolved drop-in:

    # /etc/systemd/resolved.conf.d/adi-dns-adi.conf
    [Resolve]
    DNS=127.0.0.1:10053
    Domains=~adi

    sudo systemctl restart systemd-resolved

`Domains=~adi` is a *routing-only* domain: only `.adi` queries go to the local resolver, so this
cannot disturb the rest of the machine's DNS. It requires systemd-resolved; a machine using a
hand-written `/etc/resolv.conf` or dnsmasq needs the equivalent for its own resolver.

Two further caveats, both only relevant if you want `.adi` names locally:

* The front door (`adi-hive`) serves those names on port 80, which is privileged. Either grant
  the capability once — `sudo setcap 'cap_net_bind_service=+ep' ~/.local/adi/bin/adi-hive` (redo
  it after an upgrade; capabilities do not survive a file replacement) — or lower the floor with
  `sudo sysctl net.ipv4.ip_unprivileged_port_start=80`.
* `adi-mono up` supervises the front door on Linux as a `systemd --user` unit beside the
  resolver — deliberately not a root system daemon, so nothing in `~/.adi/mono` ends up
  root-owned. It is only enabled once the binary can actually bind: `adi-hive` exits when no
  address bound, and `Restart=always` would turn that into a permanent crash loop, so `up`
  probes first and prints the `setcap` line instead of starting a unit that cannot work.
* `adi-mono dns install-route` performs the two privileged steps and reports each: the
  `systemd-resolved` drop-in and the capability grant. It uses `sudo -n`, so it fails
  immediately rather than hanging an unattended `ssh node adi-mono up` on a password prompt; on
  refusal it prints the exact commands to run by hand.
* Re-run `dns install-route` after upgrading the binaries — a file capability does not survive
  the file being replaced.
* None of this is needed to *reach* the node. Mesh access does not go through the node's front
  door, so a node is fully usable with no route and no capability; this only affects browsing
  `.adi` names while logged into the node itself.


Requirements & notes
--------------------

* x86-64 Linux with systemd (the supervisor is `systemd --user`; logind provides lingering).
* No glibc requirement — the binaries are static musl. No packages to install.
* Everything runs as an unprivileged user. The only steps in this document that need root are
  optional: the firewall, the `.adi` DNS drop-in, and the port-80 capability.
* Some features that shell out to Unix tools (project hooks, dashboard runners, `lsof`/`docker`
  port helpers) expect a normal POSIX userland; a stripped container image may not have it.
* Upgrades: unpack the new tarball and re-run `./install.sh --no-pair`. The binaries are
  replaced and the units re-written; pairing is already recorded and is not repeated.
