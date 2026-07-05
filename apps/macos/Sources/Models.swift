import Foundation

/// Codable mirror of the JSON `adi-mono status --json` emits (see
/// `crates/adi-core/src/commands.rs` + `service.rs`). The app renders these directly;
/// it derives no state of its own, so new services/actions appear with no UI changes.
struct Report: Codable {
    let anyRunning: Bool
    let services: [ServiceReport]

    enum CodingKeys: String, CodingKey {
        case anyRunning = "any_running"
        case services
    }

    static let empty = Report(anyRunning: false, services: [])
}

struct ServiceReport: Codable, Identifiable {
    let id: String
    let name: String
    let enabled: Bool
    let running: Bool
    let detail: String
    let actions: [ActionReport]
}

/// One button. `args` is the argv to pass back to `adi-mono` to perform it, so the
/// menu stays generic — it never encodes what an action does.
struct ActionReport: Codable, Identifiable {
    let id: String
    let title: String
    let args: [String]
}
