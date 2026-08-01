import Foundation

/// The Rust core, as Swift sees it.
///
/// Every C function returns one JSON reply that this type owns the decoding of — and, importantly,
/// the freeing of. `call` is the only place a `char *` from Rust is touched, so there is exactly
/// one `adi_mesh_free` in the app and no way to forget it.
///
/// The whole surface is synchronous on the Rust side and none of it blocks for long (the slowest,
/// `open`, binds a socket), so the actor hops off the main thread rather than introducing a
/// callback protocol for calls that finish in microseconds.
actor Mesh {
    static let shared = Mesh()

    /// What Rust answers with. A failure is a sentence meant to be shown, not a code to switch on.
    struct MeshError: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    private var started = false

    /// Start the mesh, returning this device's key. Idempotent — the Rust side returns the running
    /// endpoint's key rather than binding a second one.
    @discardableResult
    func start() throws -> String {
        let store = try Self.storeDirectory()
        let value = try call { adi_mesh_start(store.path) }
        started = true
        return try value.string("key")
    }

    /// Where the mesh store lives — the directory the core treats as `$HOME`, so its files land in
    /// `…/Application Support/.adi/mono`.
    ///
    /// **Not the container root.** `NSHomeDirectory()` is writable in the simulator, which is an
    /// ordinary directory on a Mac, and is *not* writable on a device — where it fails at the first
    /// write with `Operation not permitted`. Application Support is the documented home for data
    /// the app owns and the user does not manage.
    ///
    /// It is excluded from backup deliberately. The identity key is *this device's* name on the
    /// mesh, and a backup restored onto a second phone would put two endpoints behind one key,
    /// racing each other for the same relay session. A restored device getting a fresh identity and
    /// pairing again is the honest outcome — and its passwords would not have come along either,
    /// since the Keychain items are `ThisDeviceOnly`.
    private static func storeDirectory() throws -> URL {
        var url = try FileManager.default.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        )
        var excluded = URLResourceValues()
        excluded.isExcludedFromBackup = true
        try? url.setResourceValues(excluded)
        return url
    }

    /// This device's key, and whether a node could reach it yet.
    func status() throws -> (key: String, ready: Bool) {
        let value = try call { adi_mesh_status() }
        return (try value.string("key"), (value["ready"] as? Bool) ?? false)
    }

    /// A single-use invite for a node to spend with `adi-mono mesh join <token>`.
    func invite() throws -> String {
        try call { adi_mesh_invite() }.string("token")
    }

    /// Every paired node.
    func nodes() throws -> [Node] {
        let value = try callRaw { adi_mesh_nodes() }
        return try JSONDecoder().decode([Node].self, from: value)
    }

    /// The loopback port serving `service` on `node`, binding a listener on first ask.
    func open(node: String, service: String) throws -> UInt16 {
        let value = try call { adi_mesh_open(node, service) }
        guard let port = value["port"] as? Int, let port = UInt16(exactly: port) else {
            throw MeshError(message: "the mesh returned no usable port")
        }
        return port
    }

    /// Pairings completed since the last call. Each arrives exactly once and carries a password
    /// that must go straight to the Keychain.
    func takePairings() throws -> [Pairing] {
        let value = try callRaw { adi_mesh_take_pairings() }
        return try JSONDecoder().decode([Pairing].self, from: value)
    }

    /// Unpair a node locally.
    @discardableResult
    func forget(node: String) throws -> Bool {
        (try call { adi_mesh_forget(node) })["removed"] as? Bool ?? false
    }

    /// Retire pooled connections after the app was suspended. Never throws — it is called from a
    /// lifecycle hook, where there is nothing useful to do with a failure and nothing to show.
    func resume() {
        guard started else { return }
        _ = try? call { adi_mesh_resume() }
    }

    // MARK: - The one place a Rust pointer is handled

    /// Run a C call, decode its reply as a JSON object, and free the string exactly once.
    private func call(_ body: () -> UnsafeMutablePointer<CChar>?) throws -> [String: Any] {
        let data = try callRaw(body)
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw MeshError(message: "the mesh returned something that is not an object")
        }
        return object
    }

    /// The shared half: call, check `ok`, and hand back the `value` as raw JSON so a caller can
    /// either decode it into a type or read a field out of it.
    private func callRaw(_ body: () -> UnsafeMutablePointer<CChar>?) throws -> Data {
        guard let raw = body() else {
            throw MeshError(message: "the mesh returned nothing at all")
        }
        defer { adi_mesh_free(raw) }

        let json = String(cString: raw)
        guard
            let data = json.data(using: .utf8),
            let reply = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            throw MeshError(message: "the mesh returned malformed JSON")
        }
        if reply["ok"] as? Bool != true {
            throw MeshError(message: reply["error"] as? String ?? "the mesh failed without saying why")
        }
        guard let value = reply["value"] else {
            throw MeshError(message: "the mesh reported success with no value")
        }
        return try JSONSerialization.data(withJSONObject: value)
    }
}

private extension [String: Any] {
    /// Read a string field, failing with the field's name rather than a nil deep in a view.
    func string(_ key: String) throws -> String {
        guard let value = self[key] as? String else {
            throw Mesh.MeshError(message: "the mesh reply is missing \(key)")
        }
        return value
    }
}

// MARK: - The shapes Rust sends

/// One paired node. Mirrors `viewer::NodeInfo`.
struct Node: Codable, Identifiable, Hashable {
    let petname: String
    let key: String
    let nickname: String
    let paired_at: UInt64
    let pending_nickname: String?
    /// The service labels the grants name.
    let services: [String]
    /// True when `http:*` was granted, so `services` is a starting point and not the whole list.
    let any_service: Bool

    var id: String { petname }

    /// The key, shortened for a screen that is 390 points wide. The full value stays available —
    /// this is presentation, and §2 is clear that the key is what an operator acts on.
    var shortKey: String {
        key.count > 16 ? "\(key.prefix(8))…\(key.suffix(6))" : key
    }

    /// A node that renamed itself after pairing (§2 rule 4): reported, never applied.
    var renamedItself: Bool {
        guard let pending = pending_nickname else { return false }
        return pending != nickname
    }
}

/// A pairing that just completed. Mirrors `viewer::Paired`.
struct Pairing: Codable {
    let petname: String
    let username: String
    let password: String
}
