# adi-dns: scoping the local DNS resolver

Goal (from requirements): a **local-machine DNS resolver** that does **split-DNS
overrides *and* upstream forwarding**, works on **macOS, Linux, and Windows**, and
runs under a process supervisor. This document scopes two ways to get there —
a **config-only** setup and a **Rust build on hickory-dns** — and recommends one.

Both prototypes live in this crate; both were built and the Rust one was verified
end-to-end (transcript at the bottom).

---

## 1. The landscape (do we even need to build?)

"Custom DNS on the local machine" is really **three independent layers**:

1. **The resolver process** — answers queries on `127.0.0.1:53`.
2. **OS integration** — how the machine decides to send queries there (per-OS).
3. **The supervisor** — keeps the process alive.

Layers 2 and 3 are the same regardless of which resolver you pick. Only layer 1
is a build-vs-reuse decision, and several mature open-source resolvers already
cover "split-DNS + forwarding + all three OSes":

| Resolver | Lang | Win? | Notes |
|---|---|---|---|
| **CoreDNS** | Go | ✅ | Single static binary, `Corefile`, forward + template plugins. Cleanest config-only pick. |
| **smartdns-rs** | Rust | ✅ | dnsmasq-like local forwarder, DoT/DoH/DoQ. Rust-aligned. |
| **hickory-dns** | Rust | ✅ | Library *and* binary. What you build on if you build. |
| dnsmasq | C | ⚠️ weak | The classic dev split-DNS tool, but Unix-centric — **fails the Windows requirement**. |
| Unbound | C | ✅ | Recursive resolver + `local-data` overrides. |
| Blocky / AdGuard Home | Go | ✅ | Forwarding + blocklists; AdGuard is a full appliance (overkill here). |

Because **Windows is required**, dnsmasq (the usual local-dev answer) is out as the
primary. That narrows config-only to **CoreDNS** (chosen for Prototype A) or
smartdns-rs.

---

## 2. Prototype A — config-only (CoreDNS)

Files: `deploy/coredns/Corefile`, `deploy/supervisor/adi-dns.conf` (program
`adi-dns-coredns`), `deploy/windows/adi-dns.winsw.xml`.

CoreDNS is one static binary on all three OSes. The whole resolver is a `Corefile`:

```
adi:53 {
    template IN A    { answer "{{ .Name }} 60 IN A 127.0.0.1" }
    template IN AAAA { answer "{{ .Name }} 60 IN AAAA ::1" }
}
.:53 {
    cache 30
    forward . 1.1.1.1 8.8.8.8
}
```

- **Pro:** zero custom code; mature, battle-tested; rich plugin ecosystem
  (hosts, rewrite, blocklists, DoH/DoT) if scope grows.
- **Pro:** foreground by default → drops straight into any supervisor.
- **Con:** a Go binary to vendor/track, outside the Rust monorepo's build & CI.
- **Con:** custom resolution logic (dynamic records from adi-family's own service
  registry, per-request decisions) means writing CoreDNS plugins in Go — at which
  point "config-only" is no longer true.

**Best when:** the need stays "static dev domains + forward the rest."

## 3. Prototype B — Rust on hickory-dns (this crate)

Files: `src/main.rs`, `src/config.rs`, `adi-dns.toml`; supervised by
`deploy/supervisor/adi-dns.conf` (program `adi-dns`) or the WinSW wrapper.

~300 lines: a custom `RequestHandler` that checks override zones first (longest
suffix wins) then forwards via `hickory-resolver`, with a clean SIGTERM/SIGINT
shutdown. It compiles on the workspace toolchain (Rust 1.92, edition 2024) and
passes `clippy` with the repo's `pedantic` lints at **zero warnings**.

- **Pro:** lives in the monorepo — one `cargo build`, one CI, one language,
  shared crates (`adi-core`).
- **Pro:** full control. Dynamic records, service discovery against adi-family's
  own registry, custom policies are ordinary Rust, not a plugin SPI.
- **Pro:** pure Rust, memory-safe, single self-contained binary per OS.
- **Con:** it's our code to maintain — edge cases (EDNS, DNSSEC, more record
  types, negative caching) are on us. The prototype handles A/AAAA + forwarding;
  it is a skeleton, not a full recursive resolver.

**Best when:** the resolver needs adi-family-specific behavior, or "everything in
the monorepo, in Rust" is a goal in itself.

## 4. Side-by-side

| | A: CoreDNS (config-only) | B: hickory-dns (Rust) |
|---|---|---|
| Custom code | none (Corefile) | ~300 lines Rust |
| In the Rust monorepo / CI | ✗ (Go binary) | ✓ |
| Cross-platform 1 binary | ✓ | ✓ |
| Foreground / supervisor-ready | ✓ | ✓ |
| Split-DNS + forward + cache | ✓ | ✓ (cache via resolver) |
| Custom/dynamic resolution | Go plugin needed | native Rust |
| DoH/DoT, blocklists today | ✓ built-in | would add (hickory supports it) |
| Maturity | very high | our code is new |
| Time to first working setup | minutes | already done here |

---

## 5. Two integration modes (applies to BOTH prototypes)

The requirement pairs split-DNS **and** forwarding. Which OS-integration you use
decides whether forwarding is even exercised:

- **Split-DNS mode** (`deploy/os-integration/*` defaults): the OS routes *only*
  `.adi` to the resolver; all other names use the system's normal DNS. The
  resolver's forwarding path is never hit. Great for "just give me dev domains."
- **Primary-resolver mode**: set `127.0.0.1` as the machine's sole DNS server. All
  queries flow through the resolver; `.adi` is overridden, everything else is
  forwarded upstream. **This is the mode that uses both features together.**

Per-OS mechanics:

| | Split-DNS (`.adi` only) | Primary (all queries) |
|---|---|---|
| macOS | `/etc/resolver/adi` → 127.0.0.1 | `networksetup -setdnsservers <svc> 127.0.0.1` |
| Linux | systemd-resolved `Domains=~adi` | `DNS=127.0.0.1` + `Domains=~.`, or resolv.conf |
| Windows | NRPT rule for `.adi` | adapter DNS = 127.0.0.1 |

macOS is the nicest: `/etc/resolver/<tld>` is true per-domain split-DNS with no
global change. Linux uses systemd-resolved routing domains. Windows has no
`/etc/resolver` analog, so NRPT is the per-domain path.

## 6. Supervisor notes

adi-dns/CoreDNS both run in the **foreground**, so any supervisor works:

- **supervisord** — Linux/macOS only (**not Windows**); needs foreground
  programs (satisfied) and root or `CAP_NET_BIND_SERVICE` for `:53`.
- **launchd** (macOS), **systemd** (Linux), **WinSW/NSSM** (Windows) — native
  per-OS options.

> **Discovered during testing:** this machine already runs a supervisor named
> `adi` — `/Users/mgorunuch/.local/bin/adi daemon run-service …` managing
> `adi.hive`, `adi.cocoon`, `adi.workforce`. It **auto-restarts** its services
> (it respawned one within seconds during a port test). If that's the
> adi-family service manager, `adi-dns` most naturally becomes **another service
> under `adi daemon`** rather than under a separate supervisor — and the `.adi`
> split-DNS zone could resolve `hive.adi`, `cocoon.adi`, `workforce.adi`, … to
> those local services. That turns this from "dev domains" into **service
> discovery for adi-family**, which strongly favors Prototype B (custom logic
> reading the same service registry). Worth confirming.

---

## 7. Recommendation

**Build Prototype B (hickory-dns), in this monorepo** — provided the resolver is
meant to know about adi-family's own services (which the `adi daemon` discovery
suggests). It keeps everything in one Rust build/CI, and the custom-resolution
ceiling is the whole point: static overrides today, dynamic service discovery
tomorrow, without a Go plugin boundary. The skeleton already works.

Use **Prototype A (CoreDNS)** only if the scope truly stays "static dev domains +
plain forwarding" forever — then it's less code to own.

Either way, ship the **`deploy/` layer as-is**: it's resolver-agnostic and is the
part that actually makes the OS use your resolver on each platform.

Concrete next steps if we go with B:
1. Confirm the `.adi` → adi-family-services intent, and whether adi-dns should run
   under `adi daemon`.
2. Add dynamic overrides sourced from the service registry (hot-reload on change).
3. Fill in resolver hardening: negative caching, EDNS passthrough, more record
   types, metrics/health endpoint.

---

## 8. Verification transcript (Prototype B)

Run on `127.0.0.1:45353` (port 53 needs root; 5353/15353 were already taken on
this machine — see the supervisor note). Config: overrides `adi → 127.0.0.1` and
`v6.adi → ::1`, upstreams Cloudflare + Google.

```
T1  api.adi         A      -> 127.0.0.1          (override)
T2  deep.sub.adi    A      -> 127.0.0.1          (wildcard suffix match)
T3  host.v6.adi     AAAA   -> ::1                (longest-suffix wins: v6.adi over adi)
T4  api.adi         AAAA   -> NOERROR, 0 answers (wrong family = no-data, not error)
T5  example.com     A      -> 104.20.23.154 …    (forwarded upstream)
T6  <random>.example A     -> NXDOMAIN           (forwarded negative)
T7  api.adi         A/tcp  -> 127.0.0.1          (TCP listener works)
```

All expected. Response flags are correct too: overrides answer with `AA`
(authoritative) set; forwarded answers with `RA` (recursion available).

---

## Sources

- CoreDNS — <https://coredns.io/> · plugins: `forward`, `template`, `cache`
- hickory-dns (ex trust-dns) — <https://github.com/hickory-dns/hickory-dns>
- smartdns-rs — <https://github.com/mokeyish/smartdns-rs>
- dnsmasq on macOS `/etc/resolver` — <https://gist.github.com/ogrrd/5831371>
- systemd-resolved split DNS — <https://fedoramagazine.org/systemd-resolved-introduction-to-split-dns/>
- Windows NRPT — `Add-DnsClientNrptRule` (PowerShell DnsClient module)
- Acrylic DNS Proxy (Windows) — <https://mayakron.altervista.org/support/acrylic/Home.htm>
- supervisord — <https://supervisord.org/configuration.html> · WinSW — <https://github.com/winsw/winsw>
