import AppKit
import Darwin
import Foundation

/// Supervises a bundled binary as a per-user launchd LaunchAgent (`gui/$UID`,
/// `RunAtLoad` + `KeepAlive`). The one place that talks to `launchctl`.
enum Launchd {
    static var guiDomain: String { "gui/\(getuid())" }
    static func target(_ label: String) -> String { "\(guiDomain)/\(label)" }

    static var launchAgentsDir: String { NSHomeDirectory() + "/Library/LaunchAgents" }
    static func plistPath(_ label: String) -> String { launchAgentsDir + "/\(label).plist" }

    static func enable(label: String, program: [String], log: String, env: [String: String]) {
        writePlist(label: label, program: program, log: log, env: env)
        // Boot out any stale instance first so bootstrap can't fail on a dupe.
        _ = run(["/bin/launchctl", "bootout", target(label)])
        let r = run(["/bin/launchctl", "bootstrap", guiDomain, plistPath(label)])
        if r.status != 0 {
            NSLog("ADI: launchctl bootstrap %@ failed (%d): %@", label, r.status, r.output)
        }
        _ = run(["/bin/launchctl", "enable", target(label)])
    }

    static func disable(label: String) {
        _ = run(["/bin/launchctl", "bootout", target(label)])
        try? FileManager.default.removeItem(atPath: plistPath(label))
    }

    static func isLoaded(label: String) -> Bool {
        FileManager.default.fileExists(atPath: plistPath(label))
            && run(["/bin/launchctl", "print", target(label)]).status == 0
    }

    // MARK: - Plist

    /// Identical XML for a per-user LaunchAgent and a root LaunchDaemon — only the
    /// install location differs — so the privileged landing daemon reuses this.
    static func plistXML(label: String, program: [String], log: String, env: [String: String]) -> String {
        let argsXML = program
            .map { "        <string>\(xmlEscape($0))</string>" }
            .joined(separator: "\n")
        let envXML: String
        if env.isEmpty {
            envXML = ""
        } else {
            let entries = env
                .map { "        <key>\(xmlEscape($0.key))</key><string>\(xmlEscape($0.value))</string>" }
                .joined(separator: "\n")
            envXML = "    <key>EnvironmentVariables</key>\n    <dict>\n\(entries)\n    </dict>\n"
        }
        return """
        <?xml version="1.0" encoding="UTF-8"?>
        <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
        <plist version="1.0">
        <dict>
            <key>Label</key>
            <string>\(xmlEscape(label))</string>
            <key>ProgramArguments</key>
            <array>
        \(argsXML)
            </array>
        \(envXML)    <key>RunAtLoad</key>
            <true/>
            <key>KeepAlive</key>
            <true/>
            <key>ProcessType</key>
            <string>Background</string>
            <key>StandardOutPath</key>
            <string>\(xmlEscape(log))</string>
            <key>StandardErrorPath</key>
            <string>\(xmlEscape(log))</string>
        </dict>
        </plist>
        """
    }

    private static func writePlist(label: String, program: [String], log: String, env: [String: String]) {
        try? FileManager.default.createDirectory(atPath: launchAgentsDir, withIntermediateDirectories: true)
        try? plistXML(label: label, program: program, log: log, env: env)
            .write(toFile: plistPath(label), atomically: true, encoding: .utf8)
    }

    private static func xmlEscape(_ s: String) -> String {
        s.replacingOccurrences(of: "&", with: "&amp;")
            .replacingOccurrences(of: "<", with: "&lt;")
            .replacingOccurrences(of: ">", with: "&gt;")
    }

    // MARK: - Status file

    static func readStatus(_ path: String) -> DaemonStatus? {
        guard let data = FileManager.default.contents(atPath: path) else { return nil }
        return try? JSONDecoder().decode(DaemonStatus.self, from: data)
    }

    /// True if `pid` exists (signal-0 probe; EPERM still means alive).
    static func processAlive(_ pid: Int32) -> Bool {
        guard pid > 0 else { return false }
        if kill(pid, 0) == 0 { return true }
        return errno == EPERM
    }

    // MARK: - Processes

    /// argv[0] must be an absolute path. Returns status + combined stdout/stderr.
    @discardableResult
    static func run(_ argv: [String]) -> (status: Int32, output: String) {
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: argv[0])
        proc.arguments = Array(argv.dropFirst())
        let pipe = Pipe()
        proc.standardOutput = pipe
        proc.standardError = pipe
        do {
            try proc.run()
        } catch {
            return (-1, "failed to launch \(argv[0]): \(error)")
        }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        proc.waitUntilExit()
        return (proc.terminationStatus, String(data: data, encoding: .utf8) ?? "")
    }

    /// Run a shell command as root via a single Authorization prompt.
    static func runAdmin(_ shell: String) {
        let escaped = shell
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
        let script = "do shell script \"\(escaped)\" with administrator privileges"
        let r = run(["/usr/bin/osascript", "-e", script])
        if r.status != 0 {
            NSLog("ADI: admin action failed (%d): %@", r.status, r.output)
        }
    }
}
