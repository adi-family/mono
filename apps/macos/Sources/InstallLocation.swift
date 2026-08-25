import AppKit
import Foundation

/// Where this bundle is running from, and moving it somewhere services can point at.
///
/// The companion to `adi-core`'s `install` module, which refuses to install services from a path
/// that will not survive — a mounted disk image, or Gatekeeper's randomised copy.
///
/// The core decides *whether* the app is misplaced and reports it in `setup.location_durable`;
/// this type only supplies the wording and does the moving. Deliberately not a second opinion:
/// one definition of "durable", in Rust, and the window reads it from the same status the rest
/// of the UI is built from.
///
/// Copying is deliberate rather than moving. A translocated bundle is readable but its *real*
/// location is not knowable without `SecTranslocate…`, and on a disk image the original must
/// stay put; copying works from both, and the source disappears on its own when the image is
/// ejected or the download is deleted.
enum InstallLocation {
    case durable
    /// A mounted volume — the downloaded disk image, most of the time.
    case volume
    /// macOS is running a randomised read-only copy because the bundle is quarantined and has
    /// not been moved. The path differs on every launch.
    case translocated

    static func current() -> InstallLocation {
        let path = Bundle.main.bundleURL.path
        if path.contains("/AppTranslocation/") { return .translocated }
        if path.hasPrefix("/Volumes/") { return .volume }
        return .durable
    }

    /// Why the app is asking, in the user's terms — no paths, no jargon.
    var reason: String {
        switch self {
        case .durable:
            return ""
        case .volume:
            return """
                ADI is running from the disk image you downloaded. Its services would stop \
                working as soon as you eject it, so they have not been set up yet.
                """
        case .translocated:
            return """
                macOS is running ADI from a temporary copy, which it does until the app is moved. \
                That copy is different every time, so ADI's services have not been set up yet.
                """
        }
    }

    // MARK: moving

    enum MoveError: LocalizedError {
        case noWritableDestination
        case failed(String)

        var errorDescription: String? {
            switch self {
            case .noWritableDestination:
                return "Neither Applications folder could be written to."
            case let .failed(why):
                return why
            }
        }
    }

    /// Copy this bundle into Applications, launch the copy, and quit.
    ///
    /// Falls back to `~/Applications` when `/Applications` is not writable, which is the case
    /// for a standard (non-admin) account. Never returns on success — the process is replaced by
    /// the copy.
    @MainActor
    static func moveToApplications() throws {
        let source = Bundle.main.bundleURL
        let name = source.lastPathComponent
        let manager = FileManager.default

        var lastFailure: Error?
        for directory in destinations() {
            let destination = directory.appendingPathComponent(name)
            do {
                try? manager.createDirectory(at: directory, withIntermediateDirectories: true)
                // An older copy sitting there is the normal case on a re-install. To the trash
                // rather than deleted outright: it is someone's application, and a failed move
                // that also destroyed the previous version would be unforgivable.
                if manager.fileExists(atPath: destination.path) {
                    try manager.trashItem(at: destination, resultingItemURL: nil)
                }
                try manager.copyItem(at: source, to: destination)
                clearQuarantine(destination)
                relaunch(destination)
                return
            } catch {
                lastFailure = error
            }
        }
        throw lastFailure.map { MoveError.failed($0.localizedDescription) }
            ?? MoveError.noWritableDestination
    }

    private static func destinations() -> [URL] {
        var urls = [URL(fileURLWithPath: "/Applications")]
        if let home = FileManager.default.urls(for: .applicationDirectory, in: .userDomainMask).first {
            urls.append(home)
        }
        return urls
    }

    /// Drop the quarantine flag from the copy.
    ///
    /// Without this the copy is quarantined too, so macOS translocates *it* on first launch and
    /// the app comes straight back to this same prompt — having moved itself correctly.
    private static func clearQuarantine(_ url: URL) {
        let xattr = Process()
        xattr.executableURL = URL(fileURLWithPath: "/usr/bin/xattr")
        xattr.arguments = ["-d", "-r", "com.apple.quarantine", url.path]
        // Best effort: it fails harmlessly when the attribute was never set.
        try? xattr.run()
        xattr.waitUntilExit()
    }

    @MainActor
    private static func relaunch(_ url: URL) {
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.createsNewApplicationInstance = true
        NSWorkspace.shared.openApplication(at: url, configuration: configuration) { _, _ in
            Task { @MainActor in NSApp.terminate(nil) }
        }
    }
}
