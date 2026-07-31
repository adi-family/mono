Test node — a Linux node in a container
=======================================

A throwaway Linux machine that runs the real node stack (`docs/fleet.md` §6) with **real
systemd**, so the parts that cannot be unit-tested from macOS get exercised: the `systemd --user`
units, lingering, `setcap` on the front door, the `systemd-resolved` drop-in, and a full pairing
over the mesh.

    docker build --platform linux/amd64 -t adi-node-test .
    bash apps/linux/build.sh          # the package the node installs
    ./run.sh                          # create, install, pair — prints the hostnames to open

`run.sh` leaves a node paired with this machine and reachable at `app.<node>.n.adi`, with a known
password so a second person can open it. From ~25 seconds, cold.


The three docker flags, and why each one matters
------------------------------------------------

Every one of these was learned by getting a wrong answer first.

* **`--cap-add SYS_ADMIN`, not `--privileged`.** Privileged grants every capability, so the front
  door binds `:80` whether or not `setcap` was applied — the capability path silently untested.
* **`--sysctl net.ipv4.ip_unprivileged_port_start=1024`.** Docker's default is `0`, which makes
  *every* port unprivileged inside the container. Without restoring 1024, `getcap` comes back
  empty while `:80` is happily bound and nothing about privileged ports is being tested at all.
* **`--cgroupns=host` + `/sys/fs/cgroup`.** systemd needs its own cgroup view to run as PID 1.

The image also installs **`libpam-systemd`** and **`dbus-user-session`**. Without pam_systemd,
logind never creates `$XDG_RUNTIME_DIR`, `loginctl enable-linger` does not take, and every
`systemctl --user` fails with *"Failed to connect to bus"* — which reads exactly like a bug in
the software under test rather than a missing package.

`docker exec -u adi` gives no login session, so any manual exec needs
`export XDG_RUNTIME_DIR=/run/user/$(id -u)` first.


What this cannot tell you
-------------------------

Real network conditions. The container reaches the relay through the host's connection, so
latency, loss, reconnect and NAT traversal are all easier here than on a machine behind CGNAT.
The connection pool's reconnect-and-backoff path is the one thing still worth confirming on a
real remote box.
