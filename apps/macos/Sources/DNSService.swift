import Foundation

/// The `adi-dns` split-DNS resolver, packaged as an ADI service.
///
/// Two pieces, split by privilege so the common on/off toggle never needs a
/// password:
///   * **Resolver** — the bundled `adi-dns` on an unprivileged port
///     (`127.0.0.1:10053`), run as a per-user LaunchAgent. It answers `.adi` with
///     `127.0.0.53` (a dedicated loopback address, clear of anything on
///     `127.0.0.1`). Enable/Disable toggles just this — no admin prompt.
///   * **Landing** — a second, *landing-only* `adi-dns` run as a **root**
///     LaunchDaemon that binds `127.0.0.53:80` (aliasing `lo0` itself) and serves
///     the built-in "not found" page. So `http://anything.adi/` shows a real page
///     instead of falling through to whatever else is on `127.0.0.1:80`.
///
/// The route (`/etc/resolver/adi`) and the landing daemon are the only privileged
/// bits; they're installed together in one admin action and left in place, so the
/// day-to-day toggle stays prompt-free. Files live under the app's own support
/// dir, separate from the production `adi` platform.
struct DNSService: ManagedService {
    let id = "family.adi.app.dns"
    let name = "DNS"

    private let domain = "adi"
    private let port = 10053

    /// Dedicated loopback address the domain resolves to, and where the landing
    /// server binds `:80`. Kept off `127.0.0.1` so it never collides with anything
    /// else serving there.
    private let landingAddr = "127.0.0.53"
    private let landingPort = 80
    /// launchd label of the root landing daemon (distinct from the resolver agent).
    private let landingLabel = "family.adi.app.dns-landing"

    // MARK: paths (namespaced under the app's own support dir)
    private var serviceDir: String { AppPaths.support + "/dns" }
    var statusPath: String { serviceDir + "/status.json" }
    var logPath: String { NSHomeDirectory() + "/Library/Logs/adi-dns.log" }
    private var configPath: String { serviceDir + "/adi-dns.toml" }
    private var stagePath: String { serviceDir + "/resolver-\(domain)" }
    private var resolverFile: String { "/etc/resolver/\(domain)" }

    // Landing daemon: config + a staged plist that the admin step copies into
    // /Library/LaunchDaemons (root-owned), plus its runtime log.
    private var landingConfigPath: String { serviceDir + "/adi-dns-landing.toml" }
    private var landingPlistStage: String { serviceDir + "/\(landingLabel).plist" }
    private var landingDaemonPlist: String { "/Library/LaunchDaemons/\(landingLabel).plist" }
    private var landingLog: String { "/Library/Logs/adi-dns-landing.log" }

    private var binaryPath: String {
        Bundle.main.resourceURL?.appendingPathComponent("adi-dns").path
            ?? Bundle.main.bundlePath + "/Contents/Resources/adi-dns"
    }

    // MARK: ManagedService

    func program() -> [String] {
        try? FileManager.default.createDirectory(atPath: serviceDir, withIntermediateDirectories: true)
        writeConfig()
        return [binaryPath, configPath]
    }

    // Toggling the resolver must not re-prompt for admin every time. The route and
    // the landing daemon are root-owned and persist across reboots, so they're
    // installed once (a single admin prompt) and left in place — bootstrapping the
    // unprivileged resolver afterward is silent. Removing them is an explicit user
    // action (see `extraActions`), so Disable leaves them untouched.
    func onEnable() { if !routeInstalled { installRoute() } }
    func onDisable() {}

    func detail(_ status: DaemonStatus?) -> String {
        guard let status else { return "" }
        return "Running · 127.0.0.1:\(status.port)"
    }

    var extraActions: [ServiceAction] {
        [
            ServiceAction(
                id: "route",
                title: {
                    self.routeInstalled
                        ? "Remove .\(self.domain) route + page"
                        : "Install .\(self.domain) route + page…"
                },
                isVisible: { true },
                perform: { self.routeInstalled ? self.removeRoute() : self.installRoute() }
            )
        ]
    }

    // MARK: DNS-specific

    // "Installed" means the whole `.adi` front is in place — both the resolver
    // route and the landing daemon. If either is missing (e.g. after an upgrade
    // that added the daemon), the action reads "Install…" and re-runs the single
    // idempotent admin step rather than stranding a half-configured state.
    private var routeInstalled: Bool {
        FileManager.default.fileExists(atPath: resolverFile)
            && FileManager.default.fileExists(atPath: landingDaemonPlist)
    }

    private func writeConfig() {
        let cfg = """
        # Written by ADI.app — edits are overwritten when the app rewrites it.
        domain = "\(domain)"
        bind_addr = "127.0.0.1"
        preferred_port = \(port)
        fallback_ports = []
        upstreams = ["1.1.1.1:53", "8.8.8.8:53"]
        manage_os_routing = false
        status_file = "\(statusPath)"

        # Answer .\(domain) with the dedicated loopback address the landing server
        # owns, so a bare http://<name>.\(domain)/ reaches our page — not whatever
        # else is on 127.0.0.1:80.
        [[overrides]]
        suffix = "\(domain)"
        address = "\(landingAddr)"
        """
        try? cfg.write(toFile: configPath, atomically: true, encoding: .utf8)
    }

    /// Stage the landing daemon's config and plist (unprivileged writes). The admin
    /// step below copies the plist into /Library/LaunchDaemons and bootstraps it.
    private func writeLandingArtifacts() {
        try? FileManager.default.createDirectory(atPath: serviceDir, withIntermediateDirectories: true)
        let cfg = """
        # Written by ADI.app — landing-only adi-dns serving the .\(domain) page.
        domain = "\(domain)"
        serve_dns = false

        [landing]
        enabled = true
        bind = "\(landingAddr):\(landingPort)"
        """
        try? cfg.write(toFile: landingConfigPath, atomically: true, encoding: .utf8)

        let plist = Launchd.plistXML(
            label: landingLabel,
            program: [binaryPath, landingConfigPath],
            log: landingLog,
            env: ["RUST_LOG": "info"]
        )
        try? plist.write(toFile: landingPlistStage, atomically: true, encoding: .utf8)
    }

    /// The one privileged step: install the `/etc/resolver` route AND the root
    /// landing daemon in a single admin prompt. The daemon aliases `lo0` and binds
    /// `\(landingAddr):\(landingPort)` itself when launchd starts it.
    private func installRoute() {
        try? FileManager.default.createDirectory(atPath: serviceDir, withIntermediateDirectories: true)
        try? "nameserver 127.0.0.1\nport \(port)\n".write(toFile: stagePath, atomically: true, encoding: .utf8)
        writeLandingArtifacts()
        Launchd.runAdmin(
            "mkdir -p /etc/resolver"
                + " && cp '\(stagePath)' '\(resolverFile)'"
                + " && chmod 644 '\(resolverFile)'"
                + " && cp '\(landingPlistStage)' '\(landingDaemonPlist)'"
                + " && chown root:wheel '\(landingDaemonPlist)'"
                + " && chmod 644 '\(landingDaemonPlist)'"
                + " && (launchctl bootout system/\(landingLabel) 2>/dev/null || true)"
                + " && launchctl bootstrap system '\(landingDaemonPlist)'"
                + " && launchctl enable system/\(landingLabel)"
                + " && dscacheutil -flushcache"
                + " && killall -HUP mDNSResponder"
        )
    }

    /// Tear down both privileged bits (best-effort, so one missing piece doesn't
    /// block the rest), including the `lo0` alias the daemon added.
    private func removeRoute() {
        Launchd.runAdmin(
            "(launchctl bootout system/\(landingLabel) 2>/dev/null || true)"
                + " ; rm -f '\(landingDaemonPlist)'"
                + " ; rm -f '\(resolverFile)'"
                + " ; (ifconfig lo0 -alias \(landingAddr) 2>/dev/null || true)"
                + " ; dscacheutil -flushcache"
                + " ; killall -HUP mDNSResponder"
        )
    }
}
