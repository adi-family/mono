import SwiftUI

/// The fleet: every node this phone has paired with, and a way to add one.
///
/// One list on the page surface, grouped by node with tone and hairlines rather than boxes
/// (`design/DESIGN.md` §2.5): a node is a header row — its name, and its key in mono — and the
/// rows under it are what it runs. The one filled orange on the screen is the pairing action.
struct ContentView: View {
    @State private var model = FleetModel()
    @State private var pairing = false
    /// The pushed screens, owned here so a Home Screen shortcut can push one.
    @State private var path: [Route] = []
    /// A link that arrived before the fleet was loaded. A shortcut tapped from a cold start beats
    /// `model.start()` to it every time, and dropping it then would make the icon work only when
    /// the app happened to be warm — the one case its owner will never be testing.
    @State private var pending: (node: String, service: String)?
    @State private var sharing: ShareTarget?

    var body: some View {
        NavigationStack(path: $path) {
            Group {
                if model.nodes.isEmpty {
                    empty
                } else {
                    list
                }
            }
            .background(ADI.bg)
            .navigationTitle("Fleet")
            .toolbar {
                ToolbarItem(placement: .primaryAction) {
                    Button {
                        pairing = true
                    } label: {
                        Label {
                            Text("Pair a node")
                        } icon: {
                            LucideIcon(icon: .plus)
                        }
                    }
                    .disabled(!model.ready)
                }
            }
            .refreshable { await model.refresh() }
            .sheet(isPresented: $pairing) { PairView(model: model) }
            .sheet(item: $sharing) { target in
                HomeScreenLinkSheet(node: target.node, service: target.service, title: target.title)
            }
            .alert(
                "Something went wrong",
                isPresented: .init(get: { model.failure != nil }, set: { if !$0 { model.failure = nil } })
            ) {
                Button("OK", role: .cancel) { model.failure = nil }
            } message: {
                Text(model.failure ?? "")
            }
        }
        .task { await model.start() }
        .onOpenURL { url in
            guard let link = HomeScreenLink.parse(url) else { return }
            pending = link
            open(link)
        }
        // The fleet arriving is the other half of a cold start: the link was filed above because
        // there was no node to match it to, and this is the moment there is one.
        .onChange(of: model.nodes) { _, _ in
            if let link = pending { open(link) }
        }
    }

    /// Push the screen a link names, once the node it names is known here. A link to a node this
    /// phone is not paired with is simply left pending: the fleet is refreshed on foreground, and a
    /// shortcut kept from an unpaired machine starting to work again after re-pairing is kinder
    /// than an alert about a name the person did not type.
    private func open(_ link: (node: String, service: String)) {
        guard let node = model.nodes.first(where: { $0.petname == link.node }) else { return }
        pending = nil
        path = [Route(node: node, service: link.service, share: !granted(link.service, on: node))]
    }

    /// Whether the node has already shared this label with us — the same question the row asks, so
    /// a shortcut to a dashboard that was never granted still asks for it instead of 502ing.
    private func granted(_ service: String, on node: Node) -> Bool {
        if node.any_service || node.services.contains(service) { return true }
        return (model.dashboards[node.petname] ?? [])
            .contains { $0.service == service && $0.allowed }
    }

    /// Nothing paired yet: the one icon size reserved for an empty state (§9), a section title,
    /// one sentence, and the one orange.
    private var empty: some View {
        VStack(spacing: 12) {
            LucideIcon(icon: .network, size: .xl)
                .foregroundStyle(ADI.ink3)
            Text("No nodes yet")
                .font(ADI.TextStyle.section)
                .foregroundStyle(ADI.ink)
            Text("Pair a machine and its services show up here — reached by key over the mesh, with no port open on either side.")
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
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var list: some View {
        List {
            ForEach(model.nodes) { node in
                // The node's own row, then its dashboards, its plain services, and what it has
                // to say for itself — a band of rows under one hairline-separated header rather
                // than a section in a box.
                NodeHeader(node: node, isNew: node.petname == model.justPaired)
                    .adiRow()
                    .padding(.top, 16)
                    .listRowSeparator(.hidden, edges: .top)
                    .swipeActions {
                        Button("Unpair", role: .destructive) {
                            Task { await model.forget(node) }
                        }
                    }

                ForEach(model.dashboards[node.petname] ?? []) { dashboard in
                    DashboardRow(node: node, dashboard: dashboard) { sharing = $0 }
                        .adiRow()
                }
                if model.listing.contains(node.petname) && model.dashboards[node.petname] == nil {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("Asking \(node.petname) what it has…")
                            .font(ADI.TextStyle.small)
                            .foregroundStyle(ADI.ink3)
                    }
                    .adiRow()
                }
                // A dashboard is a service, so anything already listed above would otherwise
                // appear twice — once by name and once by label.
                ForEach(services(of: node), id: \.self) { service in
                    NavigationLink(value: Route(node: node, service: service)) {
                        ServiceLabel(icon: icon(for: service), title: service, mono: true)
                    }
                    .adiRow()
                    .contextMenu {
                        Button {
                            sharing = ShareTarget(
                                node: node.petname, service: service, title: service)
                        } label: {
                            Text("Add to Home Screen…")
                        }
                    }
                }
                if node.any_service {
                    NavigationLink(value: Route(node: node, service: nil)) {
                        ServiceLabel(icon: .ellipsis, title: "Open another service…", ink: ADI.ink2)
                    }
                    .adiRow()
                }
                footer(for: node)
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .navigationDestination(for: Route.self) { route in
            if let service = route.service {
                ServiceView(node: route.node, service: service, share: route.share, model: model)
            } else {
                AnyServiceView(node: route.node, model: model)
            }
        }
    }

    /// A node's plain services, minus the ones already on screen as a dashboard by name.
    private func services(of node: Node) -> [String] {
        let named = Set((model.dashboards[node.petname] ?? []).compactMap(\.service))
        return node.services.filter { !named.contains($0) }
    }

    /// What a node has to say for itself under its rows: a nickname it has declared, and why
    /// its dashboards could not be listed.
    @ViewBuilder
    private func footer(for node: Node) -> some View {
        if node.renamedItself, let pending = node.pending_nickname {
            // §2 rule 4 of docs/fleet.md: a node that renames itself files a notice; it never
            // takes over the links this device already has.
            Note(icon: .info, text: "This node now calls itself “\(pending)”. Your name for it is unchanged.")
                .adiRow()
                .listRowSeparator(.hidden)
        }
        if let problem = model.listingFailure[node.petname] {
            Note(icon: .triangleAlert, text: problem)
                .adiRow()
                .listRowSeparator(.hidden)
        }
    }

    /// A guess at the right glyph. Cosmetic only — a service the app has never heard of gets the
    /// generic one and works exactly the same.
    private func icon(for service: String) -> Lucide {
        switch service {
        case "app": .layoutGrid
        case "api": .code
        default: .globe
        }
    }
}

/// The row a node's band opens with: its name, `new` while it is, and its key in mono.
private struct NodeHeader: View {
    let node: Node
    let isNew: Bool

    var body: some View {
        HStack(spacing: 8) {
            LucideIcon(icon: .monitor)
                .foregroundStyle(ADI.ink2)
            Text(node.petname)
                .font(ADI.sans(15, .medium))
                .foregroundStyle(ADI.ink)
            if isNew {
                Chip(text: "new")
            }
            Spacer(minLength: 8)
            Text(node.shortKey)
                .font(ADI.TextStyle.mono)
                .foregroundStyle(ADI.ink3)
        }
        .padding(.vertical, 4)
    }
}

/// A service or dashboard as a row: the glyph in the row's ink, the name, an optional second line.
private struct ServiceLabel: View {
    let icon: Lucide
    let title: String
    var subtitle: String? = nil
    /// Whether the title is a service *label* — a machine string — rather than a name.
    var mono = false
    var ink: Color = ADI.ink

    var body: some View {
        HStack(spacing: 10) {
            LucideIcon(icon: icon)
                .foregroundStyle(ink == ADI.ink ? ADI.ink2 : ink)
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(mono ? ADI.mono(14) : ADI.sans(15))
                    .foregroundStyle(ink)
                if let subtitle {
                    Text(subtitle)
                        .font(ADI.TextStyle.small)
                        .foregroundStyle(ADI.ink3)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
        .padding(.vertical, 4)
    }
}

/// One dashboard, as a row.
///
/// The row is a link whether or not the node has shared it: a dashboard this phone may not open
/// yet is one tap away from being one it may, and hiding that behind a separate control would make
/// the common case — your own machine, which you have the password for — the awkward one. What the
/// subtitle owes the reader is that the tap will ask.
private struct DashboardRow: View {
    let node: Node
    let dashboard: NodeDashboard
    let onShare: (ShareTarget) -> Void

    var body: some View {
        if let service = dashboard.service {
            NavigationLink(value: Route(node: node, service: service, share: !dashboard.allowed)) {
                ServiceLabel(icon: .layoutDashboard, title: dashboard.name, subtitle: subtitle)
            }
            .contextMenu {
                Button {
                    onShare(ShareTarget(
                        node: node.petname, service: service, title: dashboard.name))
                } label: {
                    Text("Add to Home Screen…")
                }
            }
        } else {
            // Nothing to route to: a dashboard with no `<label>.adi` host of its own is reachable
            // on the node and nowhere else (`docs/fleet.md` §4).
            ServiceLabel(icon: .layoutDashboard, title: dashboard.name, subtitle: subtitle, ink: ADI.ink3)
        }
    }

    /// The one thing worth saying about this dashboard, in the order it matters: it cannot be
    /// opened, it has not been shared, it is not running, or what it is for.
    private var subtitle: String? {
        if dashboard.service == nil {
            return "No address on \(node.petname) yet, so there is nothing to open"
        }
        if !dashboard.allowed {
            return "Not shared with this phone yet — tap to ask \(node.petname)"
        }
        if !dashboard.running {
            return "Not running on \(node.petname)"
        }
        return dashboard.description
    }
}

/// Where a tap goes.
///
/// `service == nil` means "let me name one", which is what a `http:*` grant allows and what the
/// protocol cannot enumerate for us. `share` means the node has not granted this label yet, so
/// opening it asks first.
private struct Route: Hashable {
    let node: Node
    let service: String?
    var share = false
}

/// What a "Add to Home Screen…" tap is about. `Identifiable` so one `.sheet(item:)` serves every
/// row — the alternative is a bool plus three more `@State`s that can disagree with each other.
struct ShareTarget: Identifiable {
    let node: String
    let service: String
    /// What the icon should be called: a dashboard's name, or the bare label for a plain service.
    let title: String

    var id: String { "\(node)/\(service)" }
}

/// For a node that granted `http:*`: ask which service, since nothing on the wire can list them.
private struct AnyServiceView: View {
    let node: Node
    let model: FleetModel
    @State private var service = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Service on \(node.petname)")
                .font(ADI.TextStyle.small)
                .foregroundStyle(ADI.ink2)
            TextField("service label", text: $service)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .font(ADI.mono(16))
                .foregroundStyle(ADI.ink)
                .adiField()
            Text("The label a service has on the node — what it answers to as <label>.adi there. This node granted every one of them, and the mesh has no way to list them, so it has to be named.")
                .font(ADI.TextStyle.small)
                .foregroundStyle(ADI.ink3)
                .fixedSize(horizontal: false, vertical: true)
            NavigationLink {
                ServiceView(node: node, service: service, model: model)
            } label: {
                Text("Open")
            }
            .buttonStyle(.adi(.accent, wide: true))
            .disabled(service.isEmpty)
            .padding(.top, 8)
            Spacer()
        }
        .padding(20)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(ADI.bg)
        .navigationTitle(node.petname)
        .navigationBarTitleDisplayMode(.inline)
    }
}
