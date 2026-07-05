import Foundation

/// The `adi-dns` resolver as an ADI service, split by privilege so the on/off
/// toggle never needs a password:
///   * **Resolver** — bundled `adi-dns` on an unprivileged port, per-user
///     LaunchAgent. Answers `.adi` with `127.0.0.53`. Enable/Disable toggles this.
///   * **Landing** — a second, landing-only `adi-dns` as a **root** LaunchDaemon
///     binding `127.0.0.53:80` (it aliases `lo0`) to serve the "not found" page.
///
/// The route and landing daemon are the only privileged bits — installed together
/// in one admin action and left in place, so the day-to-day toggle stays prompt-free.
struct DNSService: ManagedService {
    let id = "family.adi.app.dns"
    let name = "DNS"

    private let domain = "adi"
    private let port = 10053

    /// Kept off `127.0.0.1` so `:80` never collides with anything else serving there.
    private let landingAddr = "127.0.0.53"
    private let landingPort = 80
    private let landingLabel = "family.adi.app.dns-landing"

    // MARK: paths
    private var serviceDir: String { AppPaths.support + "/dns" }
    var statusPath: String { serviceDir + "/status.json" }
    var logPath: String { NSHomeDirectory() + "/Library/Logs/adi-dns.log" }
    private var configPath: String { serviceDir + "/adi-dns.toml" }
    private var stagePath: String { serviceDir + "/resolver-\(domain)" }
    private var resolverFile: String { "/etc/resolver/\(domain)" }

    // Landing daemon: config + a staged plist the admin step copies into
    // /Library/LaunchDaemons, plus its runtime log.
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

    // Route + landing daemon are installed once (one admin prompt) and left in
    // place, so toggling the resolver never re-prompts. Disable leaves them; removal
    // is an explicit action (see `extraActions`).
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

    // Both bits must be present; if either is missing the action reads "Install…"
    // and re-runs the idempotent admin step rather than stranding a half state.
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

        # Route .\(domain) to the landing address so http://<name>.\(domain)/ hits our page.
        [[overrides]]
        suffix = "\(domain)"
        address = "\(landingAddr)"
        """
        try? cfg.write(toFile: configPath, atomically: true, encoding: .utf8)
    }

    /// Stage the landing daemon's config + plist (unprivileged); the admin step
    /// copies the plist into /Library/LaunchDaemons and bootstraps it.
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
    /// landing daemon in a single admin prompt.
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

    /// Tear down both privileged bits, best-effort (incl. the `lo0` alias).
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
