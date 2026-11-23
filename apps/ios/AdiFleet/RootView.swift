import SwiftUI

/// The app's three places: the apps, which is where it opens, the chats, and the fleet both run on.
///
/// The first two are the control panel's own home — its grid of apps and its conversations — drawn
/// natively. The fleet tab is where machines are paired and where a service is opened by label.
struct RootView: View {
    enum Tab: Hashable {
        case apps, chats, fleet
    }

    @State private var model = FleetModel()
    @State private var chats = ChatStore()
    @State private var tab = Tab.apps
    @State private var startingChat = false
    /// A Home Screen shortcut waiting for the fleet to load. See `ContentView.open`.
    @State private var link: PendingLink?

    @Environment(\.scenePhase) private var scenePhase
    /// Watches for the phone changing network while the app is open — see `NetworkChangeWatcher`.
    @State private var network = NetworkChangeWatcher()
    /// When the app went to the background, until it is back. `nil` while it is in front — which
    /// is how a mere `inactive` (Control Centre pulled down, the app switcher glanced at) is told
    /// from having actually been away: nothing was frozen, so there is nothing to make good.
    @State private var leftAt: Date?

    init() {
        // The tab bar is UIKit's, like the navigation bar: chrome on `--bg-side` over a hairline,
        // the chosen tab in ink and the other dimmed — never the accent (§2.4).
        let bar = UITabBarAppearance()
        bar.configureWithOpaqueBackground()
        bar.backgroundColor = UIColor(ADI.bgSide)
        bar.shadowColor = UIColor(ADI.line)
        for item in [bar.stackedLayoutAppearance, bar.inlineLayoutAppearance, bar.compactInlineLayoutAppearance] {
            item.normal.iconColor = UIColor(ADI.ink3)
            item.normal.titleTextAttributes = [
                .font: ADI.uiFont(11, .regular), .foregroundColor: UIColor(ADI.ink3),
            ]
            item.selected.iconColor = UIColor(ADI.ink)
            item.selected.titleTextAttributes = [
                .font: ADI.uiFont(11, .medium), .foregroundColor: UIColor(ADI.ink),
            ]
        }
        UITabBar.appearance().standardAppearance = bar
        UITabBar.appearance().scrollEdgeAppearance = bar
    }

    var body: some View {
        TabView(selection: $tab) {
            AppsView(model: model) {
                tab = .chats
                startingChat = true
            }
            .tabItem {
                Image(uiImage: Lucide.layoutGrid.tabImage)
                Text("Apps")
            }
            .tag(Tab.apps)
            ChatsView(model: model, store: chats, starting: $startingChat)
                .tabItem {
                    Image(uiImage: Lucide.messageSquare.tabImage)
                    Text("Chats")
                }
                .tag(Tab.chats)
            ContentView(model: model, pending: $link)
                .tabItem {
                    Image(uiImage: Lucide.network.tabImage)
                    Text("Fleet")
                }
                .tag(Tab.fleet)
        }
        .task { await model.start() }
        .onChange(of: scenePhase) { _, phase in
            switch phase {
            case .background:
                if leftAt == nil { leftAt = Date() }
            case .active:
                guard let left = leftAt else { return }
                leftAt = nil
                // iOS froze the process while it was away. What that costs depends on for how
                // long, and the model draws that line (`FleetModel.resume(after:)`).
                Task { await model.resume(after: Date().timeIntervalSince(left)) }
            default:
                break
            }
        }
        // The other half of the same problem, and the one a phone actually meets: walking out of
        // the door with the app open, so the connection moves from Wi-Fi to cellular without the
        // app ever leaving the foreground.
        .task { network.start { await model.networkChanged() } }
        // A machine that could not be reached is asked again without anybody pulling to refresh:
        // the phone finds a network, the laptop wakes, and the grid should simply fill in. Only
        // while the app is in front — a backgrounded app asks nobody.
        .task(id: Retry(front: scenePhase == .active, unreachable: model.unreachable)) {
            guard scenePhase == .active, !model.unreachable.isEmpty else { return }
            var pause = Duration.seconds(4)
            while !Task.isCancelled {
                try? await Task.sleep(for: pause)
                guard !Task.isCancelled else { return }
                await model.retryUnreachable()
                // Slower each time it changes nothing; a change in who is unreachable restarts
                // this task, and with it the pace.
                pause = min(pause * 2, .seconds(30))
            }
        }
        // Here rather than on the fleet screen: a tab that has never been shown has no view to
        // receive the link, and a shortcut tapped from a cold start is exactly that case.
        .onOpenURL { url in
            guard let target = HomeScreenLink.parse(url) else { return }
            link = PendingLink(node: target.node, service: target.service)
            tab = .fleet
        }
    }
}

/// What the retry of unreachable machines hangs on: restarted whenever either half changes.
private struct Retry: Equatable {
    let front: Bool
    let unreachable: Set<String>
}

/// A Home Screen shortcut's target, held until the node it names is known.
struct PendingLink: Equatable {
    let node: String
    let service: String
}

extension Lucide {
    /// The icon as a template image, for the one place SwiftUI will not take a view: a tab item.
    var tabImage: UIImage {
        let size = CGSize(width: 24, height: 24)
        let path = LucideMarkup.path(for: self).cgPath
        return UIGraphicsImageRenderer(size: size).image { context in
            let cg = context.cgContext
            cg.addPath(path)
            cg.setLineWidth(1.5)
            cg.setLineCap(.round)
            cg.setLineJoin(.round)
            cg.setStrokeColor(UIColor.white.cgColor)
            cg.strokePath()
        }
        .withRenderingMode(.alwaysTemplate)
    }
}
