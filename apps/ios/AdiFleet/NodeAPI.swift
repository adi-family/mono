import Foundation

/// A node's control panel API, spoken natively.
///
/// The transport is the one a dashboard's web view already uses: `Mesh.open(node, "app")` binds a
/// loopback port inside this app that carries each request to the node's panel over the mesh, and
/// the node's password from the Keychain answers its Basic gate. So this is a plain HTTP client
/// with nothing mesh-shaped in it — and the panel sees the phone exactly as it sees the web view.
///
/// The chat API is JSON over HTTP with **polling**. There is no token stream to subscribe to: an
/// answer in flight is a turn with `pending: true` whose text grows between two reads.
struct NodeAPI: Sendable {
    let node: String

    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    func get<T: Decodable>(_ path: String, as type: T.Type = T.self) async throws -> T {
        try await send(path, body: nil)
    }

    func post<T: Decodable>(_ path: String, _ body: [String: Any], as type: T.Type = T.self) async throws -> T {
        try await send(path, body: try JSONSerialization.data(withJSONObject: body))
    }

    /// The address of a page on this node's panel, for handing to a web view.
    func port() async throws -> UInt16 {
        try await Mesh.shared.open(node: node, service: "app")
    }

    private func send<T: Decodable>(_ path: String, body: Data?) async throws -> T {
        var request = try await request(path)
        request.httpMethod = body == nil ? "GET" : "POST"
        request.httpBody = body
        // The panel refuses a POST without a JSON content type (415): that rule is its defence
        // against a cross-site form post, and it applies to every client alike.
        if body != nil { request.setValue("application/json", forHTTPHeaderField: "Content-Type") }

        let (data, response) = try await Self.session.data(for: request)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        let refusal = (try? JSONDecoder().decode(Refusal.self, from: data))?.sentence
        guard (200..<300).contains(status) else {
            if status == 401 {
                throw Failure(message:
                    "\(node) refused this phone's password — open its control panel to sign in again.")
            }
            throw Failure(message: refusal ?? Self.reason(in: data).map { "\(node): \($0)" }
                ?? "\(node) answered \(status)")
        }
        // The panel reports some failures as `{"ok": false}` inside a 200.
        if let refusal { throw Failure(message: refusal) }
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw Failure(message: "\(node) answered something this app cannot read (\(path))")
        }
    }

    /// A request for `path` on this node's panel, addressed and carrying the node's password.
    private func request(_ path: String) async throws -> URLRequest {
        #if DEBUG
        if let panel = DevPanel.current, panel.node.petname == node {
            guard let url = URL(string: panel.base + path) else { throw Failure(message: "\(path) is not a path") }
            return URLRequest(url: url)
        }
        #endif
        guard let credential = Keychain.credential(for: node) else {
            throw Failure(message:
                "No password for \(node) on this phone yet — open its control panel once and sign in.")
        }
        let port = try await port()
        guard let url = URL(string: "http://127.0.0.1:\(port)\(path)") else {
            throw Failure(message: "\(path) is not a path")
        }
        var request = URLRequest(url: url)
        let token = Data("\(credential.username):\(credential.password)".utf8).base64EncodedString()
        request.setValue("Basic \(token)", forHTTPHeaderField: "Authorization")
        return request
    }

    /// What a refusal that is not the panel's JSON says, in a sentence.
    ///
    /// The loopback listener answers for the mesh when a node cannot be reached or will not serve
    /// the request, and it does so with a page meant for a browser. Its text is the only place the
    /// reason is written down, and "answered 502" tells the reader nothing they can act on.
    private static func reason(in data: Data) -> String? {
        guard var text = String(data: data.prefix(4096), encoding: .utf8) else { return nil }
        for pattern in ["(?is)<(style|script|head)[^>]*>.*?</\\1>", "<[^>]+>", "\\s+"] {
            text = text.replacingOccurrences(of: pattern, with: " ", options: .regularExpression)
        }
        text = text.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty else { return nil }
        return text.count > 200 ? String(text.prefix(200)) + "…" : text
    }

    private struct Refusal: Decodable {
        let ok: Bool?
        let error: String?
        var sentence: String? { ok == false ? (error ?? "the node refused without saying why") : nil }
    }

    /// Its own session: no cookies or cache to share with the web views, and a timeout long enough
    /// for a panel listing over a relayed connection — `GET /api/agents` takes seconds on a machine
    /// with many agents even locally.
    private static let session: URLSession = {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = 45
        configuration.waitsForConnectivity = false
        return URLSession(configuration: configuration)
    }()
}

#if DEBUG
/// A control panel reached directly, with no pairing: `ADI_DEV_PANEL=<name>=<base url>`.
///
/// Debug builds only, and only for the simulator, which shares its Mac's loopback — so
/// `mac=http://127.0.0.1:8000` puts the machine the app was built on in the fleet as `mac`. It
/// exists because the chat screens cannot be worked on, or photographed by the UI tests, without a
/// panel to read, and a real pairing is a single-use invite spent against a real machine. It stands
/// in for the transport only: everything above `NodeAPI.request` runs exactly as it does paired.
struct DevPanel {
    let node: Node
    let base: String

    /// Where each of the panel's apps answers on the Mac, by service label. A web view cannot be
    /// sent to `<label>.adi` — App Transport Security permits plain HTTP to an address, not to a
    /// name — so an app is opened on its own port instead.
    @MainActor static var ports: [String: Int] = [:]

    /// The panel's dashboards, in the shape the mesh catalog gives for a paired node.
    @MainActor
    func dashboards() async -> [NodeDashboard] {
        struct Reply: Decodable {
            let dashboards: [Row]
            struct Row: Decodable {
                let id: String
                let name: String?
                let description: String?
                let icon: String?
                let host: String?
                let frontend_port: Int?
                let frontend_running: Bool?
                let archived_at: UInt64?
            }
        }
        guard let reply: Reply = try? await NodeAPI(node: node.petname).get("/api/dashboards") else { return [] }
        return reply.dashboards.filter { $0.archived_at == nil }.map { row in
            let service = row.host.flatMap { $0.hasSuffix(".adi") ? String($0.dropLast(4)) : nil }
            if let service, let port = row.frontend_port { Self.ports[service] = port }
            return NodeDashboard(
                id: row.id, name: row.name ?? row.id, description: row.description, icon: row.icon,
                service: service, running: row.frontend_running ?? false, allowed: true)
        }
    }

    /// Start `service` if it is stopped, and wait until it answers.
    ///
    /// A paired node does this by itself: its mesh gateway is demand-aware and wakes a service the
    /// way the front door does. Opening an app on its bare port goes round both, so a stopped app
    /// was simply a dead port and an empty page. Asking the Mac's front door for the app by name is
    /// the same wake a browser on that Mac causes; it answers 503 while the service starts, so it
    /// is asked until it stops saying that.
    ///
    /// The front door is asked by address with a `Host` header because App Transport Security lets
    /// plain HTTP reach an address and not a name. `127.0.0.53` is where `*.adi` resolves on a
    /// machine running adi; `ADI_DEV_FRONTDOOR` names another.
    func wake(_ service: String) async {
        let door = ProcessInfo.processInfo.environment["ADI_DEV_FRONTDOOR"] ?? "127.0.0.53"
        guard service != "app", let url = URL(string: "http://\(door)/") else { return }
        var request = URLRequest(url: url, timeoutInterval: 20)
        request.setValue("\(service).adi", forHTTPHeaderField: "Host")
        for _ in 0..<20 {
            if let (_, response) = try? await URLSession.shared.data(for: request),
               (response as? HTTPURLResponse)?.statusCode != 503
            {
                return
            }
            try? await Task.sleep(for: .milliseconds(1500))
        }
    }

    /// The address a service of this panel is opened at.
    @MainActor
    func address(of service: String, path: String) -> URL? {
        if service == "app" { return URL(string: base + path) }
        return Self.ports[service].flatMap { URL(string: "http://127.0.0.1:\($0)\(path)") }
    }

    static let current: DevPanel? = {
        guard let spec = ProcessInfo.processInfo.environment["ADI_DEV_PANEL"],
              let cut = spec.firstIndex(of: "=")
        else { return nil }
        let name = String(spec[..<cut])
        var base = String(spec[spec.index(after: cut)...])
        while base.hasSuffix("/") { base.removeLast() }
        guard !name.isEmpty, URL(string: base)?.host != nil else { return nil }
        return DevPanel(
            node: Node(
                petname: name, key: "dev-panel", nickname: name, paired_at: 0,
                pending_nickname: nil, services: [], any_service: false),
            base: base)
    }()
}
#endif

// MARK: - Chat calls

extension NodeAPI {
    /// How many turns one read asks for. Earlier ones are fetched by asking for more.
    static let page = 20

    /// Every agent and its conversations, newest activity first once flattened.
    func chats(filter: ChatFilter) async throws -> AllRuns {
        try await get("/api/agents/runs/all?hidden=false&limit=100" + (filter == .all ? "" : "&filter=\(filter.rawValue)"))
    }

    /// The node's agents, with the star its operator gave each — what a new chat is picked from.
    func agents() async throws -> [AgentInfo] {
        let reply: Agents = try await get("/api/agents")
        return reply.agents
    }

    func peek(agent: String, run: String, limit: Int) async throws -> Peek {
        try await post("/api/agents/run/peek", view(agent, run, limit))
    }

    /// Start a conversation. The run id comes back empty for an agent that only runs in a terminal.
    func start(agent: String, message: String) async throws -> Started {
        try await post("/api/agents/run", ["name": agent, "message": message, "launched_by": "human"])
    }

    func reply(agent: String, run: String, message: String, limit: Int) async throws -> Peek {
        var body = view(agent, run, limit)
        body["message"] = message
        body["mode"] = "regular"
        return try await post("/api/agents/run/reply", body)
    }

    /// Answer the agent's pending question: one reply string per question it asked.
    func answer(agent: String, run: String, ask: String, replies: [String], limit: Int) async throws -> Peek {
        var body = view(agent, run, limit)
        body["ask"] = ask
        body["replies"] = replies
        return try await post("/api/agents/run/answer", body)
    }

    func unqueue(agent: String, run: String, index: Int, limit: Int) async throws -> Peek {
        var body = view(agent, run, limit)
        body["index"] = index
        return try await post("/api/agents/run/unqueue", body)
    }

    /// The tool calls a folded `calls` step stands for.
    func steps(agent: String, run: String, turn: Int, from: Int, to: Int) async throws -> [Step] {
        let reply: Steps = try await post(
            "/api/agents/run/steps",
            ["name": agent, "run_id": run, "turn": turn, "from": from, "to": to])
        return reply.steps
    }

    func stop(agent: String, run: String) async throws {
        let _: Ignored = try await post("/api/agents/run/stop", ["name": agent, "run_id": run])
    }

    func rename(agent: String, run: String, title: String) async throws {
        let _: Ignored = try await post(
            "/api/agents/run/rename", ["name": agent, "run_id": run, "title": title])
    }

    func hide(agent: String, run: String) async throws {
        let _: Ignored = try await post(
            "/api/agents/run/hide", ["name": agent, "run_id": run, "hidden": true])
    }

    func star(agent: String, run: String, starred: Bool) async throws {
        let _: Ignored = try await post(
            "/api/agents/run/star", ["name": agent, "run_id": run, "starred": starred])
    }

    /// `fold` collapses each run of tool calls into one `calls` step, which is what a phone wants
    /// to draw anyway and keeps a poll small.
    private func view(_ agent: String, _ run: String, _ limit: Int) -> [String: Any] {
        ["name": agent, "run_id": run, "limit": limit, "fold": true]
    }
}

// MARK: - The shapes the panel sends
//
// Decoded leniently: the panel omits most fields when they are empty, and a client that insisted
// on any of them would stop reading chats the day one is added or dropped.

private extension KeyedDecodingContainer {
    func value<T: Decodable>(_ key: Key, or fallback: T) -> T {
        ((try? decodeIfPresent(T.self, forKey: key)) ?? nil) ?? fallback
    }

    func optional<T: Decodable>(_ key: Key) -> T? {
        (try? decodeIfPresent(T.self, forKey: key)) ?? nil
    }
}

struct Ignored: Decodable {}

private struct Agents: Decodable {
    let agents: [AgentInfo]

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        agents = c.value(.agents, or: [])
    }

    enum CodingKeys: String, CodingKey { case agents }
}

struct AgentInfo: Decodable {
    let name: String
    let starred: Bool
    /// False for one that cannot be launched as it stands — no backend it can run on.
    let runnable: Bool
    /// True for an agent that lives in a terminal rather than in a transcript.
    let interactive: Bool

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        starred = c.value(.starred, or: false)
        runnable = c.value(.runnable, or: true)
        interactive = (c.optional(.caps) as Caps?)?.interactive ?? false
    }

    private struct Caps: Decodable {
        let interactive: Bool?
    }

    enum CodingKeys: String, CodingKey { case name, starred, runnable, caps }
}

struct AllRuns: Decodable {
    let agents: [AgentRuns]

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        agents = c.value(.agents, or: [])
    }

    enum CodingKeys: String, CodingKey { case agents }
}

struct AgentRuns: Decodable {
    let name: String
    /// True for an agent that lives in a terminal rather than in a transcript.
    let interactive: Bool
    let runs: [RunInfo]

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        interactive = c.value(.interactive, or: false)
        runs = c.value(.runs, or: [])
    }

    enum CodingKeys: String, CodingKey { case name, interactive, runs }
}

struct RunInfo: Decodable {
    let runID: String
    /// Unix milliseconds.
    let startedAt: UInt64
    let lastActivity: UInt64
    let message: String
    let title: String?
    let running: Bool
    let starred: Bool
    let asking: Bool
    /// It has left itself a wake — a timer, an event — and is waiting on the world for it.
    let awaiting: Bool

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        awaiting = !((c.optional(.awaits) as [Ignored]?) ?? []).isEmpty
        runID = try c.decode(String.self, forKey: .run_id)
        startedAt = c.value(.started_at, or: 0)
        lastActivity = c.value(.last_activity, or: 0)
        message = c.value(.message, or: "")
        title = c.optional(.title)
        running = c.value(.running, or: false)
        starred = c.value(.starred, or: false)
        asking = (c.optional(.pending_question) as Ask?) != nil
    }

    enum CodingKeys: String, CodingKey {
        case run_id, started_at, last_activity, message, title, running, starred, pending_question, awaits
    }
}

struct Started: Decodable {
    let runID: String

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        runID = c.value(.run_id, or: "")
    }

    enum CodingKeys: String, CodingKey { case run_id }
}

/// One read of a conversation.
struct Peek: Decodable {
    let running: Bool
    let ask: Ask?
    let turns: [Turn]
    let totalTurns: Int

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        running = c.value(.running, or: false)
        ask = c.optional(.pending_question)
        turns = c.value(.turns, or: [])
        totalTurns = c.value(.total_turns, or: 0)
    }

    enum CodingKeys: String, CodingKey { case running, pending_question, turns, total_turns }
}

/// One message. There is no message id on the wire: `seq` is its stable index in the run.
struct Turn: Decodable, Equatable, Identifiable {
    let role: String
    let text: String
    let seq: Int
    /// Unix milliseconds; 0 while the answer is still being written.
    let at: UInt64
    /// The answer in flight.
    let pending: Bool
    /// A message of ours waiting behind the answer in flight.
    let queued: Bool
    let attachments: Int
    let steps: [Step]
    let durationMs: UInt64?
    let costMicroUSD: UInt64?

    var id: Int { seq }
    var isUser: Bool { role == "user" }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        role = c.value(.role, or: "assistant")
        text = c.value(.text, or: "")
        seq = c.value(.seq, or: 0)
        at = c.value(.at, or: 0)
        pending = c.value(.pending, or: false)
        queued = c.value(.queued, or: false)
        attachments = (c.optional(.images) as [Ignored]?)?.count ?? 0
        steps = c.value(.steps, or: [])
        let metrics: Metrics? = c.optional(.metrics)
        durationMs = metrics?.duration_ms
        costMicroUSD = metrics?.cost_micro_usd
    }

    private struct Metrics: Decodable {
        let duration_ms: UInt64?
        let cost_micro_usd: UInt64?
    }

    enum CodingKeys: String, CodingKey {
        case role, text, seq, at, pending, queued, images, steps, metrics
    }
}

/// What happened inside one answer, in order.
enum Step: Decodable, Equatable {
    case message(String)
    case thinking(String)
    case tool(name: String, input: String, output: String, status: ToolStatus)
    /// A folded run of tool calls: steps `from..<to` of the turn, fetched on demand.
    case calls(from: Int, to: Int, count: Int, tools: [String], preview: String, status: ToolStatus)
    /// A kind this build has not heard of. Drawn as nothing rather than failing the whole read.
    case unknown

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let status = ToolStatus(rawValue: c.value(.status, or: "")) ?? .ok
        switch c.value(.kind, or: "") {
        case "message":
            self = .message(c.value(.text, or: ""))
        case "thinking":
            self = .thinking(c.value(.text, or: ""))
        case "tool":
            self = .tool(
                name: c.value(.name, or: ""), input: c.value(.input, or: ""),
                output: c.value(.output, or: ""), status: status)
        case "calls":
            self = .calls(
                from: c.value(.from, or: 0), to: c.value(.to, or: 0), count: c.value(.count, or: 0),
                tools: c.value(.tools, or: []), preview: c.value(.preview, or: ""), status: status)
        default:
            self = .unknown
        }
    }

    enum CodingKeys: String, CodingKey {
        case kind, text, name, input, output, status, from, to, count, tools, preview
    }
}

enum ToolStatus: String {
    case running, ok, error, unanswered
}

private struct Steps: Decodable {
    let steps: [Step]

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        steps = c.value(.steps, or: [])
    }

    enum CodingKeys: String, CodingKey { case steps }
}

/// A question the agent put to its person, which holds the run until somebody answers.
struct Ask: Decodable, Equatable {
    let id: String
    let note: String
    let headline: String
    let questions: [Question]

    struct Question: Decodable, Equatable {
        let header: String
        let question: String
        let options: [Choice]
        let multiSelect: Bool

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            header = c.value(.header, or: "")
            question = c.value(.question, or: "")
            options = c.value(.options, or: [])
            multiSelect = c.value(.multi_select, or: false)
        }

        enum CodingKeys: String, CodingKey { case header, question, options, multi_select }
    }

    struct Choice: Decodable, Equatable {
        let label: String
        let description: String

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            label = c.value(.label, or: "")
            description = c.value(.description, or: "")
        }

        enum CodingKeys: String, CodingKey { case label, description }
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = c.value(.id, or: "")
        note = c.value(.note, or: "")
        headline = c.value(.headline, or: "")
        questions = c.value(.questions, or: [])
    }

    enum CodingKeys: String, CodingKey { case id, note, headline, questions }
}
