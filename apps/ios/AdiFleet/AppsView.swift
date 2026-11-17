import SwiftUI

/// Every app on every paired machine, as a grid of tiles — the control panel's own home screen.
///
/// An app is a dashboard a machine serves. A tile is its icon (the one its manifest names, else
/// its initials), its name, and the machine it lives on; a tap opens it full-screen on its own
/// origin, asking the machine to share it first when this phone holds no grant for it yet.
struct AppsView: View {
    let model: FleetModel
    /// Start a new chat — the home screen's other action, which lives on the next tab.
    let newChat: () -> Void

    @State private var path = NavigationPath()
    @State private var query = ""
    @State private var pairing = false

    var body: some View {
        NavigationStack(path: $path) {
            Group {
                if model.nodes.isEmpty {
                    unpaired
                } else {
                    grid
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(ADI.bg)
            .navigationTitle("Apps")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .primaryAction) {
                    Button(action: newChat) {
                        Label {
                            Text("New chat")
                        } icon: {
                            LucideIcon(icon: .plus)
                        }
                    }
                    .disabled(model.nodes.isEmpty)
                }
            }
            .appDestination(model)
            .sheet(isPresented: $pairing) { PairView(model: model) }
        }
    }

    private struct Tile: Identifiable {
        let node: Node
        let app: NodeDashboard
        var id: String { "\(node.petname)/\(app.id)" }
    }

    private var tiles: [Tile] {
        model.nodes.flatMap { node in
            (model.dashboards[node.petname] ?? []).map { Tile(node: node, app: $0) }
        }
    }

    private var shown: [Tile] {
        let needle = query.trimmingCharacters(in: .whitespaces)
        guard !needle.isEmpty else { return tiles }
        return tiles.filter {
            $0.app.name.localizedCaseInsensitiveContains(needle)
                || ($0.app.description ?? "").localizedCaseInsensitiveContains(needle)
                || $0.node.petname.localizedCaseInsensitiveContains(needle)
        }
    }

    private var unpaired: some View {
        VStack(spacing: 12) {
            LucideIcon(icon: .layoutGrid, size: .xl)
                .foregroundStyle(ADI.ink3)
            Text("No nodes yet")
                .font(ADI.TextStyle.section)
                .foregroundStyle(ADI.ink)
            Text("Pair a machine and its apps show up here — reached by key over the mesh, with no port open on either side.")
                .font(ADI.TextStyle.small)
                .foregroundStyle(ADI.ink2)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 320)
            Button("Pair a node") { pairing = true }
                .buttonStyle(.adi(.accent))
                .disabled(!model.ready)
                .padding(.top, 8)
        }
        .padding(32)
    }

    private var grid: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HStack(spacing: 8) {
                    Text("All apps")
                        .font(ADI.sans(14, .semibold))
                        .foregroundStyle(ADI.ink)
                    Text("\(tiles.count)")
                        .font(ADI.TextStyle.label.monospacedDigit())
                        .foregroundStyle(ADI.ink2)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(ADI.chip, in: RoundedRectangle(cornerRadius: ADI.Radius.sm))
                }
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 104), spacing: 8, alignment: .top)], spacing: 8) {
                    ForEach(shown) { tile in
                        if let service = tile.app.service {
                            Button {
                                path.append(AppRoute(node: tile.node.petname, service: service))
                            } label: {
                                AppTile(tile.app, on: tile.node, showsNode: model.nodes.count > 1)
                            }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("app")
                        } else {
                            // No `<label>.adi` host of its own: it exists on the machine and there
                            // is no address the mesh could carry a request to.
                            AppTile(tile.app, on: tile.node, showsNode: model.nodes.count > 1)
                                .opacity(0.5)
                        }
                    }
                }
                if let status {
                    Text(status)
                        .font(ADI.TextStyle.small)
                        .foregroundStyle(ADI.ink3)
                }
            }
            .padding(16)
        }
        .searchable(text: $query, prompt: "Search apps")
        .refreshable { await model.refresh() }
    }

    /// The one line under the grid, for when the grid alone does not explain itself.
    private var status: String? {
        let needle = query.trimmingCharacters(in: .whitespaces)
        if tiles.isEmpty, !model.listing.isEmpty { return "Loading apps…" }
        if !needle.isEmpty, shown.isEmpty { return "No apps found for “\(needle)”." }
        if tiles.isEmpty {
            return model.listingFailure.isEmpty
                ? "No apps on your machines yet."
                : "Some apps could not be loaded. The event log on the Fleet tab says why."
        }
        return nil
    }
}

/// One app: a rounded square carrying its icon, its name, and where it runs.
private struct AppTile: View {
    let app: NodeDashboard
    let node: Node
    let showsNode: Bool

    init(_ app: NodeDashboard, on node: Node, showsNode: Bool) {
        self.app = app
        self.node = node
        self.showsNode = showsNode
    }

    var body: some View {
        VStack(spacing: 10) {
            Group {
                if let icon {
                    LucideIcon(icon: icon, size: .xl)
                } else {
                    Text(initials)
                        .font(ADI.sans(17, .semibold))
                }
            }
            .foregroundStyle(ADI.ink)
            .frame(width: 56, height: 56)
            .background(ADI.bgRaise, in: RoundedRectangle(cornerRadius: ADI.Radius.lg))
            .overlay(RoundedRectangle(cornerRadius: ADI.Radius.lg).stroke(ADI.line, lineWidth: 1))
            VStack(spacing: 2) {
                Text(app.name)
                    .font(ADI.sans(13.5, .medium))
                    .foregroundStyle(ADI.ink)
                    .multilineTextAlignment(.center)
                    .lineLimit(2)
                if showsNode {
                    Text(node.petname)
                        .lineLimit(1)
                }
                if let notice {
                    Text(notice)
                        .multilineTextAlignment(.center)
                }
            }
            .font(ADI.sans(11.5))
            .foregroundStyle(ADI.ink3)
        }
        .padding(.horizontal, 4)
        .padding(.vertical, 12)
        .frame(maxWidth: .infinity)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }

    private var notice: String? {
        if app.service == nil { return "No app address" }
        // Unless the node grants every service, in which case there is nothing to ask for.
        if !app.allowed, !node.any_service { return "Asks for access" }
        return nil
    }

    /// The icon the app's manifest names, else the one the panel gives its own stock apps.
    private var icon: Lucide? {
        if let named = app.icon.flatMap(Lucide.init(rawValue:)) { return named }
        return Self.stock[app.id]
    }

    /// The panel's own defaults (`apps/primary-ui/frontend/src/lib/home-apps.ts`), so a stock app
    /// wears the same glyph here as it does there.
    private static let stock: [String: Lucide] = [
        "adin": .bot, "agent-board": .messageSquare, "ask-the-founder": .messageCircleQuestion,
        "automate-this": .zap, "company-handbook": .bookOpen, "debt-ledger": .wallet,
        "defence-security-operation": .shieldCheck, "design-system": .blocks, "jev-lab": .flaskConical,
        "llm-gateway": .activity, "pulse": .heartPulse, "recorder": .video, "vite-starter": .braces,
    ]

    /// Two letters: the first of the first and last words, or the name's first two.
    private var initials: String {
        let words = app.name.split(whereSeparator: \.isWhitespace)
        let letters = words.count > 1
            ? "\(words.first?.first.map(String.init) ?? "")\(words.last?.first.map(String.init) ?? "")"
            : String(app.name.prefix(2))
        return letters.uppercased()
    }
}

extension View {
    /// Where an `AppRoute` goes: the service, full-screen, on the node it names — asking that node
    /// to share it first when this phone holds no grant for it.
    func appDestination(_ model: FleetModel) -> some View {
        navigationDestination(for: AppRoute.self) { route in
            if let node = model.nodes.first(where: { $0.petname == route.node }) {
                ServiceView(
                    node: node, service: route.service,
                    share: !model.granted(route.service, on: node),
                    path: route.path, model: model)
            }
        }
    }
}
