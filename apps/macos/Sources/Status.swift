import Foundation

/// Codable mirror of the JSON status file `adi-dns` writes on startup
/// (see `crates/adi-dns/src/status.rs`). The controller polls this to learn the
/// live state — most importantly the port the resolver actually bound, which it
/// chooses dynamically at runtime.
struct DaemonStatus: Codable {
    let pid: Int32
    let domain: String
    let boundAddr: String
    let port: Int
    let routeInstalled: Bool
    let startedAtUnix: Int
    let version: String

    enum CodingKeys: String, CodingKey {
        case pid
        case domain
        case port
        case version
        case boundAddr = "bound_addr"
        case routeInstalled = "route_installed"
        case startedAtUnix = "started_at_unix"
    }
}
