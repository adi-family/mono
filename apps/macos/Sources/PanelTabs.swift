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
/// One can also be *pinned*, which moves it up behind the app's tab and keeps it there
/// (`togglePin`) — so the strip reads, left to right: the app, what was kept, what is merely open.
///
/// Not `@MainActor`, for the same reason `WebPanel` is not — its `openInNewTab` is called from a
/// WebKit delegate. Everything here runs on the main thread regardless.
final class PanelTabs: ObservableObject {
    @Published private(set) var tabs: [WebPanel] = []
    @Published var selection: UUID

    init(home: URL) {
        let app = WebPanel(home: home, isApp: true)
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
    /// A *pinned* tab is closable here: pinning guards against the gestures that close a tab under
    /// the pointer, not against the two ways of asking for it by name (⌘W and the tab's own menu),
    /// both of which say what they are about to do.
    ///
    /// Selection falls to the tab on its left, which is where the eye already is — and, for the
    /// last dashboard closed, is the app.
    func close(_ tab: WebPanel) {
        guard !tab.isApp, let index = tabs.firstIndex(where: { $0.id == tab.id }) else { return }
        tabs.remove(at: index)
        remember(tab)
        if selection == tab.id {
            selection = tabs[max(0, index - 1)].id
        }
    }

    func select(_ tab: WebPanel) { selection = tab.id }

    // MARK: keeping one

    /// Pin a tab, or let it go again.
    ///
    /// A pinned tab moves to the front of the strip — after the app's, which is always first, and
    /// after the tabs already pinned — and that is most of what pinning is *for* here: the strip is
    /// numbered by position (⌘1…⌘9), so a tab that has been kept deliberately is one whose number
    /// stops moving every time a dashboard opens and closes beside it.
    ///
    /// Unpinning puts it back at the head of the ordinary tabs rather than where it came from. It
    /// has not been where it came from for a while, and a tab that jumped across the strip on being
    /// unpinned would be harder to follow than one that simply stopped being first.
    ///
    /// The app's own tab is neither pinnable nor unpinnable: it is already the thing pinning makes a
    /// tab, and it is that whether anyone asks or not.
    func togglePin(_ tab: WebPanel) {
        guard !tab.isApp, let index = tabs.firstIndex(where: { $0.id == tab.id }) else { return }
        tab.pinned.toggle()
        tabs.remove(at: index)
        // One computation for both directions, and it reads the same either way: the tab goes where
        // the pinned run ends. Taken *after* the removal, so the tab being moved is not counted in
        // its own destination.
        tabs.insert(tab, at: firstOrdinary)
    }

    /// Where the pinned run ends: the first tab that is neither the app's nor pinned, or the end of
    /// the strip when every tab is kept.
    private var firstOrdinary: Int {
        tabs.firstIndex { !$0.isApp && !$0.pinned } ?? tabs.count
    }

    // MARK: moving between tabs

    /// ⌃⇥ and ⌃⇧⇥. They wrap, because a strip of tabs is a ring in every browser there is — and
    /// the app's own tab is in the ring like any other, since it is the one place ⌃⇥ most often
    /// wants to get back to.
    func selectNext() { step(by: 1) }
    func selectPrevious() { step(by: -1) }

    private func step(by offset: Int) {
        guard tabs.count > 1, let index = tabs.firstIndex(where: { $0.id == selection }) else { return }
        let next = (index + offset + tabs.count) % tabs.count
        selection = tabs[next].id
    }

    // MARK: what ⇧⌘T puts back

    /// A tab that was closed, as much of it as is worth keeping: where it had got to, what it was
    /// called, and whether it was kept. Not the tab itself — holding the web views alive to make
    /// undo instant would mean a window that never gives back the memory of anything you closed.
    struct Closed {
        let url: URL
        let name: String?
        let pinned: Bool
    }

    /// The tabs that have been closed, oldest first.
    ///
    /// Published so the menu item can grey out when there is nothing to put back.
    @Published private(set) var closed: [Closed] = []

    private func remember(_ tab: WebPanel) {
        // Where it was, not where it started: ⇧⌘T means "put back what I was looking at", and a
        // dashboard three pages deep reopened at its front page has not been put back — nor has a
        // tab that was called something coming back calling itself what the page does.
        closed.append(Closed(url: tab.webView.url ?? tab.home, name: tab.name, pinned: tab.pinned))
        // Deep enough to cover the ones closed by mistake, shallow enough that it is not a history.
        if closed.count > 16 { closed.removeFirst() }
    }

    /// Reopen the most recently closed tab. Repeatable, back through the stack.
    ///
    /// It goes through `open`, so a host that has since been opened again is brought forward rather
    /// than doubled — and the entry is spent either way, which is what keeps pressing ⇧⌘T walking
    /// backwards instead of sticking on one tab.
    func reopenLast() {
        guard let last = closed.popLast() else { return }
        let known = tabs.contains { $0.home.host == last.url.host }
        open(last.url)
        // The name and the pin come back only with a tab that was actually made. A host that is
        // open again is brought forward instead of doubled, and renaming the tab somebody is
        // working in — or moving it up the strip — is not what a keystroke that undoes a close
        // should do.
        guard !known, let restored = tabs.last else { return }
        if let name = last.name { restored.rename(name) }
        if last.pinned { togglePin(restored) }
    }

    /// A tab's pages can ask for another tab; this is where that request is granted.
    private func adopt(_ tab: WebPanel) {
        tab.openInNewTab = { [weak self] url in self?.open(url) }
    }
}
