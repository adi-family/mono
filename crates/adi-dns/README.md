# adi-dns

The adi-family local DNS resolver: **split-DNS overrides + upstream forwarding**,
running as a single **foreground** process so a supervisor owns its lifecycle.

- Answers a local dev zone (e.g. `*.adi`) from config, no `/etc/hosts` edits.
- Forwards everything else to upstream resolvers, with caching.
- Cross-platform: macOS, Linux, Windows.
- Built on [hickory-dns](https://github.com/hickory-dns/hickory-dns) (pure Rust).

> This crate is a **scoping prototype**. It exists alongside a **config-only**
> alternative (CoreDNS) so the two approaches can be compared before committing.
> See [`COMPARISON.md`](./COMPARISON.md) for the full write-up and recommendation.

## Build & run

```bash
cargo build -p adi-dns --release

# Runs on an UNPRIVILEGED port by default (10053) — no root needed:
RUST_LOG=info ./target/release/adi-dns crates/adi-dns/adi-dns.toml
```

The resolver itself needs no privileges — it binds a high port. Root/admin is
only needed to *install the OS route* (`manage_os_routing`), which is a one-time,
install-time action (or handled by a privileged supervisor like `adi daemon`).

## Configuration (`adi-dns.toml`)

```toml
domain          = "adi"                 # the TLD we own; only .adi is ever routed to us
bind_addr       = "127.0.0.1"
preferred_port  = 10053                 # unprivileged; falls back if busy
fallback_ports  = [10153, 24053]
upstreams       = ["1.1.1.1:53", "8.8.8.8:53"]
manage_os_routing = false               # self-register the .adi route (needs admin)

[[overrides]]
suffix  = "adi"                         # matches adi. and *.adi.
address = "127.0.0.1"
```

**Ports.** It binds `preferred_port`, or the first free `fallback_ports` entry if
busy, so a taken port on any machine never blocks startup. On **Windows** the OS
route (NRPT) can't target a custom port, so there it always binds `:53` (as a
service) regardless of the above.

**Self-registration.** With `manage_os_routing = true`, adi-dns writes the OS route
for `.domain` at the port it actually bound (macOS `/etc/resolver`, Linux
systemd-resolved, Windows NRPT) at startup and removes it at shutdown — so any
machine that starts it "just works". If it lacks the rights, it logs the manual
command and keeps serving. It only ever touches `.domain`; other resolvers (e.g.
ADI DNS on `.test`) are never affected.

**Overrides.** Longest-suffix wins: with both `adi` and `svc.adi` configured,
`x.svc.adi` resolves via `svc.adi`. If `overrides` is omitted it defaults to
`domain -> 127.0.0.1`. Querying the wrong address family (e.g. `AAAA` on an IPv4
override) returns `NOERROR` with no records — the standard "no data" response.

## Deploying

Two orthogonal layers — see the scripts under [`deploy/`](./deploy):

| Layer | macOS | Linux | Windows |
|---|---|---|---|
| **Supervise the process** | supervisord / launchd | supervisord / systemd | WinSW / NSSM (no supervisord) |
| **Route DNS to it** | `/etc/resolver/adi` | systemd-resolved split DNS | NRPT rule |

```
deploy/
  coredns/Corefile              # config-only equivalent (Prototype A)
  supervisor/adi-dns.conf       # supervisord program (Linux/macOS)
  windows/adi-dns.winsw.xml     # Windows service wrapper
  os-integration/
    macos-install.sh            # /etc/resolver/adi
    linux-install.sh            # systemd-resolved ~adi split DNS
    windows-install.ps1         # NRPT rule for .adi
```

**Split-DNS vs primary-resolver mode.** The `os-integration` scripts default to
*split-DNS* (only `.adi` goes to the resolver). To also exercise **forwarding**,
run adi-dns as your *primary* resolver — set `127.0.0.1` as the sole DNS server —
so all queries flow through it and non-`.adi` names are forwarded upstream.
Details in [`COMPARISON.md`](./COMPARISON.md).

## Verified behavior

Exercised end-to-end with `dig` (see `COMPARISON.md` for the transcript):
override A/AAAA, longest-suffix match, wrong-family `NOERROR`/no-data, upstream
forwarding, `NXDOMAIN` passthrough, and TCP queries.
