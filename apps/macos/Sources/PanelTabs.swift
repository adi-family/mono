import Foundation

/// The tabs in the panel window, and the one rule that governs them: **the first tab is the app**.
///
/// It is created with the window, it is never closed, and nothing can navigate it off the panel's
/// own host — a link that would opens a tab instead (`WebPanel.leavesPinnedHost`). So there is
/// always somewhere to go back to, which is the whole reason a dashboard is allowed to take the
/// window over at all.
///
/// Every other tab is a place on this machine that something asked for: a dashboard, a hive
/// service, a project's own host. They can be closed, and closing the last of them leaves the app.
///
/// Not `@MainActor`, for the same reason `WebPanel` is not — its `openInNewTab` is called from a
/// WebKit delegate. Everything here runs on the main thread regardless.
final class PanelTabs: ObservableObject {
    @Published private(set) var tabs: [WebPanel] = []
    @Published var selection: UUID

    init(home: URL) {
        let app = WebPanel(home: home, pinned: true)
        tabs = [app]
        selection = app.id
        adopt(app)
    }

    /// The tab on screen. Falls back to the first, which always exists.
    var selected: WebPanel {
        tabs.first { $0.id == selection } ?? tabs[0]
    }

    /// Show a place on this machine, in a tab.
    ///
    /// A host that is already open is brought forward rather than opened twice — pressing a
    /// dashboard's link a second time means "show me it", not "give me another copy of it" — and
    /// taken to the page the link named if that is not where it already is.
    func open(_ url: URL) {
        if let existing = tabs.first(where: { $0.home.host == url.host }) {
            selection = existing.id
            if existing.webView.url != url { existing.load(url) }
            return
        }
        let tab = WebPanel(home: url)
        adopt(tab)
        tabs.append(tab)
        selection = tab.id
        // Loaded here rather than when its view appears: a tab that only started loading once it
        // was looked at would open blank and then fill in, and one opened in the background would
        // never load at all.
        tab.start()
    }

    /// Close a tab. The app's own is not closable, so this is a no-op on it.
    ///
    /// Selection falls to the tab on its left, which is where the eye already is — and, for the
    /// last dashboard closed, is the app.
    func close(_ tab: WebPanel) {
        guard !tab.pinned, let index = tabs.firstIndex(where: { $0.id == tab.id }) else { return }
        tabs.remove(at: index)
        if selection == tab.id {
            selection = tabs[max(0, index - 1)].id
        }
    }

    func select(_ tab: WebPanel) { selection = tab.id }

    /// A tab's pages can ask for another tab; this is where that request is granted.
    private func adopt(_ tab: WebPanel) {
        tab.openInNewTab = { [weak self] url in self?.open(url) }
    }
}
