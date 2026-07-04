import AppKit
import Darwin
import Foundation

/// Stateless helpers to supervise a bundled binary as a per-user launchd
/// **LaunchAgent**. Every ADI service is managed the same way — only the program
/// and its files differ — so this is the one place that talks to `launchctl`.
///
/// A service runs unprivileged in the user's GUI domain (`gui/$UID`), starts at
/// login and on demand, and auto-restarts via `KeepAlive`. Enabling writes
/// `~/Library/LaunchAgents/<label>.plist` with absolute paths; disabling boots it
/// out and removes the plist.
enum Launchd {
    static var guiDomain: String { "gui/\(getuid())" }
    static func target(_ label: String) -> String { "\(guiDomain)/\(label)" }

    static var launchAgentsDir: String { NSHomeDirectory() + "/Library/LaunchAgents" }
    static func plistPath(_ label: String) -> String { launchAgentsDir + "/\(label).plist" }

    /// Write the plist and (re-)bootstrap the job so it starts now and at login.
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

    /// Stop + unload the job and remove its plist.
    static func disable(label: String) {
        _ = run(["/bin/launchctl", "bootout", target(label)])
        try? FileManager.default.removeItem(atPath: plistPath(label))
    }

    /// True when the job is installed and known to launchd.
    static func isLoaded(label: String) -> Bool {
        FileManager.default.fileExists(atPath: plistPath(label))
            && run(["/bin/launchctl", "print", target(label)]).status == 0
    }

    // MARK: - Plist

    /// Build a launchd plist. The XML is identical for a per-user LaunchAgent and a
    /// root LaunchDaemon — only the install location and the domain it's
    /// bootstrapped into differ — so a privileged service (e.g. the `.adi` landing
    /// server) reuses this and stages the result for an admin `cp`.
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

    /// True if a process with `pid` currently exists (probe via signal 0).
    static func processAlive(_ pid: Int32) -> Bool {
        guard pid > 0 else { return false }
        if kill(pid, 0) == 0 { return true }
        return errno == EPERM
    }

    // MARK: - Processes

    /// Run `argv` (argv[0] is an absolute path), returning status + combined output.
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
