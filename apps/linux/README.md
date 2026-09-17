ADI for Linux
=============

The ADI platform (DNS + front door + control panel + mesh), statically linked against musl so
one tarball runs on any x86-64 distro.

This installs a full ADI machine on **this** one: a control panel, at
`http://127.0.0.1:<port>` from the moment `install.sh` finishes, with nothing to configure
first. Pairing it into a fleet is optional, not the point of installing it — do it now with
`--pair <token>` (or at the interactive prompt), or later with `adi-mono mesh join <token>`.

Paired or not, the machine listens on nothing but loopback by default — no `:22` exposure, no
`:80`, no `:443` — everything a fleet peer reaches is through the mesh, at
`<service>.<node>.n.adi`, and everything you reach standing at this machine is through the
loopback ports above.

There is no GUI and no launcher. The control panel is the same web UI `adi-app` serves
everywhere; open it in a browser on this machine at the loopback address `install.sh` prints,
or — once paired — reach it from elsewhere in the fleet at `app.<node>.n.adi`.


What's in this folder
---------------------

    bin/adi-mono   The CLI and the brain — every command (`adi-mono up`, `status`, `mesh join`, ...).
    bin/adi-dns    The .adi / .test split-DNS resolver.
    bin/adi-hive   The front-door reverse proxy that serves *.adi hosts.
    bin/adi-app    The web control panel, and the host of the in-process mesh daemon.
    bin/adi-mesh   The standalone mesh CLI — `id`, `ticket`, `forward` (the daemon itself runs
                   inside adi-app, so this is for inspection and port forwards).

    install.sh     Install and start; pair too, if you ask it to. The only thing you have to run.
    VERSION        The workspace version this package was built from.


Quick start
-----------

    tar -xzf adi-linux-x64.tar.gz
    cd adi-linux-x64
    ./install.sh

That is the whole install: no token, no fleet, nothing to answer. Run it with no arguments on
a terminal and it asks a couple of questions (where to put things, whether to fetch bun); pipe
it into a non-interactive session (`ssh node ./install.sh`) and it takes the defaults for all of
them instead. Either way it finishes by printing the address of the control panel it just
started — `install.sh` is the only thing you have to run.

It needs no root, and it opens no inbound port — not even temporarily: the control panel binds
loopback only, and pairing (if you ask for it) is a dial-out, never the reverse.

`install.sh` copies the binaries to `~/.local/adi/bin` — and tries to link `adi-mono` into
`/usr/local/bin` too, so it is on `PATH` immediately rather than only in your next shell; where
that needs a password it can't ask for, it prints the one line to run instead — enables
lingering, and runs `adi-mono up`. Pass `--prefix DIR` to install elsewhere.

To join a fleet — reach this machine, and open its dashboards, from another one you already run
ADI on — mint an invite token there and either pass it here:

    ./install.sh --pair '<invite-token>'

or pair later, any time, with `adi-mono mesh join <invite-token>`.

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


Why there is a DNS resolver at all, and the `.adi` domain (optional, needs root)
--------------------------------------------------------------------------------

`adi-dns` exists because `http://app.adi` is friendlier than `http://127.0.0.1:8090`, and that is
its whole job: it resolves `.adi` names to the loopback front door, on `127.0.0.1:10053`. It is
enabled by default and needs no root — but on its own, resolving a name that nothing has told
your machine to ask it about does nothing, which is the routing step below.

The panel already works at `http://127.0.0.1:<port>` (`install.sh` prints the port) with none of
this. `.adi` names — here, on this machine, `http://app.adi` — are one step further and are
**never turned on automatically**, on purpose: it means binding `:80`, a port your machine may
already have something on (nginx, say), and a routine `adi-mono up` must never silently fight it
for that port. You turn it on by name, once:

    adi-mono dns install-route

This does two separate things, and reports each:

* the `.adi` **route** — a systemd-resolved drop-in at
  `/etc/systemd/resolved.conf.d/adi-dns-adi.conf` (`Domains=~adi`, so only `.adi` queries leave
  the rest of the machine's DNS alone; a machine on hand-written `/etc/resolv.conf` or dnsmasq
  needs the equivalent for its own resolver instead);
* the front door's one **capability** — `cap_net_bind_service` on `adi-hive`, the smallest grant
  that lets it bind `:80`/`:443` as your own unprivileged user (`sudo sysctl
  net.ipv4.ip_unprivileged_port_start=80` is the machine-wide alternative, if you'd rather lower
  the floor than grant one binary a capability — a bigger change, so it is not what `install-route`
  reaches for on its own).

It uses `sudo -n`, so it fails immediately rather than hanging an unattended `ssh node adi-mono
up` on a password prompt; on refusal it prints the exact commands to run by hand. **It also
checks first whether something else already answers on `:80`**, names it, and refuses rather
than start a front door that can only crash-loop against it — free the port (or point the other
service at the panel yourself), then run `install-route` again.

Re-run it after upgrading the binaries — a file capability does not survive the file being
replaced.

None of this is needed to *reach* a paired node from elsewhere in the fleet: mesh access goes
through `<service>.<node>.n.adi`, resolved on the *viewer's* machine, never through this one's
own front door. It only affects browsing `.adi` names while logged into this machine itself.


Requirements & notes
--------------------

* x86-64 Linux with systemd (the supervisor is `systemd --user`; logind provides lingering).
* No glibc requirement — the binaries are static musl. No packages to install.
* Everything runs as an unprivileged user. The only steps in this document that need root are
  optional: the firewall, the `.adi` DNS drop-in, and the port-80 capability.
* Some features that shell out to Unix tools (project hooks, dashboard runners, `lsof`/`docker`
  port helpers) expect a normal POSIX userland; a stripped container image may not have it.
* Upgrades: unpack the new tarball and re-run `./install.sh` (no token needed — it only pairs
  when asked to, and a machine that is already paired stays paired). The binaries are replaced
  and the units re-written.
