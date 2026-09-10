import AppKit
import Combine
import SwiftUI

/// The panel window as the **menu bar** sees it: the tabs it has open, and whether it is the
/// window in front.
///
/// One shared object rather than a focused value, which is what this should have been.
/// `.focusedSceneValue` with `@FocusedValue` is the documented way to let a menu act on the window
/// holding the keyboard, and here it never delivers: measured with the panel window key and its
/// scene in front, the value arrived at the `Commands` body as nil — *Reload* stayed greyed out and
/// ⌘W kept its fallback meaning — and taking the `WKWebView` out of the window did not change it.
/// SwiftUI publishes a focused value for a scene whose content holds *focus*, and nothing in a
/// window that is one web view under a tab strip ever takes it.
///
/// A single shared object is honest here in a way it would not be in an app of many windows: the
/// panel is a `Window` scene and not a `WindowGroup`, so there is exactly one of it — one machine,
/// one panel (`ADIApp`).
///
/// Not `@MainActor`, for the same reason `PanelTabs` is not: everything that touches it is on the
/// main thread already, and the annotation would only spread through the views that set it.
final class FrontPanel: ObservableObject {
    static let shared = FrontPanel()

    /// The panel's tabs while its window is open, and nil once it is closed.
    @Published private(set) var tabs: PanelTabs?

    /// Whether the panel is the window the keyboard is talking to. Without it the menu would
    /// promise *Close Tab* while the setup window is front, where ⌘W can only mean the window.
    @Published private(set) var isFront = false

    /// Held for as long as the window is: these are what keep the menu in step with the window.
    private var strip: AnyCancellable?
    private var chosen: AnyCancellable?
    private var page: AnyCancellable?

    private init() {}

    /// Called by `PanelWindow` as its window opens, closes, and takes or loses the keyboard.
    ///
    /// `opened` takes the keyboard state rather than assuming it: a window that comes up key emits
    /// no *change* for `front` to catch, and ⌘R would stay greyed out until you clicked away from
    /// the window and back.
    func opened(_ tabs: PanelTabs, front: Bool) {
        self.tabs = tabs
        isFront = front
        // What the items say and whether they can be pressed is read off two objects that are not
        // this one, so both have to be passed on or the menu keeps the state it had when the window
        // opened. Measured on the first of them: without this, closing the last dashboard left
        // *Close Tab* on the menu over a window whose only tab is the app's, which ⌘W cannot close.
        //
        // The strip — which tabs exist, which is chosen, what is closed and can come back.
        strip = tabs.objectWillChange.sink { [weak self] _ in
            self?.objectWillChange.send()
        }
        // And the page in front — whether it can go back, how far it is zoomed, whether its find
        // bar is open. Re-subscribed whenever the selection changes, because it is a different
        // object each time. `$selection` rather than the strip's own `objectWillChange`: that one
        // fires *before* the change, so `selected` read inside it is still the outgoing tab.
        chosen = tabs.$selection.sink { [weak self] id in
            guard let self, let tabs = self.tabs else { return }
            let tab = tabs.tabs.first { $0.id == id } ?? tabs.tabs[0]
            self.page = tab.objectWillChange.sink { [weak self] _ in
                self?.objectWillChange.send()
            }
        }
    }

    func closed() {
        strip = nil
        chosen = nil
        page = nil
        tabs = nil
        isFront = false
    }

    func front(_ isFront: Bool) { self.isFront = isFront }

    /// The tab a keystroke means, when there is one to mean.
    private var tab: WebPanel? { isFront ? tabs?.selected : nil }

    /// Whether ⌘W has a tab to close, as opposed to a window. False on the app's own tab, which is
    /// never closed — and that is not "closing is unavailable", it is what turns ⌘W back into
    /// *close this window*, which is what ⌘W on a browser's last tab has always done.
    var closesTab: Bool { tab.map { !$0.pinned } ?? false }

    /// The tab, or — on the app's own tab, and from the setup window, which has no tabs — the
    /// window. Never nothing: ⌘W that sometimes does nothing at all is worse than ⌘W that always
    /// closes the nearest thing.
    func close() {
        if let tab, !tab.pinned, let tabs {
            tabs.close(tab)
        } else {
            NSApp.keyWindow?.performClose(nil)
        }
    }

    func reload() { tab?.reload() }
    func reloadFromOrigin() { tab?.reloadFromOrigin() }

    // MARK: where the page has been

    var canGoBack: Bool { tab?.canGoBack ?? false }
    var canGoForward: Bool { tab?.canGoForward ?? false }

    func goBack() { tab?.goBack() }
    func goForward() { tab?.goForward() }

    // MARK: how big it is

    /// Every zoom item wants the same thing: a page in front to apply to.
    var hasPage: Bool { tab != nil }
    var zoomed: Bool { tab?.zoomed ?? false }

    func zoomIn() { tab?.zoomIn() }
    func zoomOut() { tab?.zoomOut() }
    func resetZoom() { tab?.resetZoom() }

    // MARK: moving between tabs, and putting one back

    /// One tab is not a ring, so ⌃⇥ has nowhere to go and says so.
    var canSwitchTabs: Bool { isFront && (tabs?.tabs.count ?? 0) > 1 }
    var canReopen: Bool { isFront && !(tabs?.closed.isEmpty ?? true) }

    func nextTab() { if canSwitchTabs { tabs?.selectNext() } }
    func previousTab() { if canSwitchTabs { tabs?.selectPrevious() } }
    func reopenTab() { if isFront { tabs?.reopenLast() } }

    // MARK: find

    func startFind() { tab?.startFind() }

    /// *Find Next* / *Find Previous* mean nothing until something has been looked for, and a ⌘G
    /// that silently did nothing would read as broken rather than as empty.
    var canFindAgain: Bool { !(tab?.query.isEmpty ?? true) }

    func findNext() { tab?.find() }
    func findPrevious() { tab?.find(backwards: true) }
}

/// The keystrokes the panel window answers, as menu items.
///
/// **Menu commands, not `keyboardShortcut` on a view** — which is what the tab strip's ⌘1…⌘9 are
/// (`TabShortcut`). Either would fire: with a `WKWebView` made first responder, a synthesized ⌘1
/// reached the view's button and a synthesized ⌘R reached the menu item, so the web view eats
/// neither. What a menu item buys is that it *says what it does*. A keystroke on no menu is one
/// somebody has to be told about; ⌘R under *View › Reload* is one they find, and one that System
/// Settings' Keyboard pane can rebind.
///
/// Neither was taken from anyone. The app's scenes are `Window`s rather than a `WindowGroup`, and
/// SwiftUI gives such an app no File menu and no Close item at all until something asks for one —
/// so ⌘W was an unbound keystroke that did nothing (measured on a scene shaped like this one).
///
/// **The placement of the close item is load-bearing.** Asking for a File menu conjures SwiftUI's
/// own *Close* / *Close All* beside whatever was asked for, and those two close the **window** —
/// which on the panel is every open tab at once. Two items then hold ⌘W and AppKit gives it to one
/// of them: measured from `CommandGroup(after: .newItem)`, ⌘W sat on ours while SwiftUI's *Close*
/// was disabled and moved to *Close* the moment the panel window made it valid, so the keystroke
/// closed tabs until it silently began closing the window instead. `replacing: .saveItem` is where
/// those two live, so putting the item there takes their slot rather than competing for it, and
/// leaves the File menu with exactly one ⌘W — ours. The cost is *Close All* (⌥⌘W), which in an app
/// of two windows was never worth a keystroke.
struct PanelCommands: Commands {
    var body: some Commands {
        // File › Close and Reopen, in the slot SwiftUI keeps its own Close in.
        CommandGroup(replacing: .saveItem) {
            CloseCommand()
            ReopenCommand()
        }
        // View › Back, Forward, Reload, Zoom. A browser keeps back and forward under *History*,
        // which this app has no other reason to have: two items do not earn a menu of their own,
        // and the page they move is the one *View* is already about.
        CommandGroup(after: .toolbar) {
            BackCommand()
            ForwardCommand()
            Divider()
            ReloadCommand()
            HardReloadCommand()
            Divider()
            ZoomCommands()
        }
        // Edit › Find, where every Mac app has kept it.
        CommandGroup(after: .textEditing) { FindCommands() }
        // Window › the tabs, above the list of windows.
        CommandGroup(before: .windowList) { TabCommands() }
    }
}

/// ⌘W. One item rather than the browser's pair, named for what pressing it will actually do: on
/// the app's own tab there is nothing left for ⌘W to mean except the window, and an item reading
/// *Close Tab* while it closed the window would be lying about the keystroke.
private struct CloseCommand: View {
    // `@ObservedObject` and not a focused value: a menu item is an ordinary SwiftUI view, so it
    // observes what any view observes, and this is the object that actually knows.
    @ObservedObject private var panel = FrontPanel.shared

    var body: some View {
        Button(panel.closesTab ? "Close Tab" : "Close Window") { panel.close() }
            .keyboardShortcut("w", modifiers: .command)
    }
}

/// ⌘R. Disabled off the panel, where there is no page to reload — the setup window is three
/// buttons and a status line, and reloading a window nobody is looking at is not what the
/// keystroke means.
///
/// A disabled item can only be trusted if it stops being disabled in time, and SwiftUI keeps these
/// items lazily: read from outside, a title and an enabled flag are whatever they were when
/// somebody last looked, and `NSMenu.update()` alone did not always freshen them. What does is the
/// menu **delegate** — `menuNeedsUpdate:`, which AppKit sends when the menu is opened and on the
/// key-equivalent path. Measured both ways round on a real bundle: with the panel key the item read
/// `enabled=true`, with the panel behind it read `enabled=false`, and a synthesized ⌘R through
/// `NSApp.sendEvent` reloaded the page (11 changes published by the tab). The same run's ⌘W really
/// did close the tab, 2 down to 1.
private struct ReloadCommand: View {
    @ObservedObject private var panel = FrontPanel.shared

    var body: some View {
        Button("Reload") { panel.reload() }
            .keyboardShortcut("r", modifiers: .command)
            .disabled(!panel.isFront)
    }
}

/// ⇧⌘R. The one browser habit that earns its place here on more than habit: a dashboard is a bundle
/// somebody may have just rebuilt, and an ordinary reload is entitled to hand back the JavaScript it
/// already has.
private struct HardReloadCommand: View {
    @ObservedObject private var panel = FrontPanel.shared

    var body: some View {
        Button("Reload Ignoring Cache") { panel.reloadFromOrigin() }
            .keyboardShortcut("r", modifiers: [.command, .shift])
            .disabled(!panel.isFront)
    }
}

/// ⇧⌘T. Puts back the last tab closed, and keeps going back through them.
private struct ReopenCommand: View {
    @ObservedObject private var panel = FrontPanel.shared

    var body: some View {
        Button("Reopen Closed Tab") { panel.reopenTab() }
            .keyboardShortcut("t", modifiers: [.command, .shift])
            .disabled(!panel.canReopen)
    }
}

/// ⌘[ and ⌘]. **Only** those, and not ⌘← / ⌘→ as well, which browsers also take: those two are
/// *move to the start / end of the line* in every text field on the Mac, and this window now has a
/// text field in it (the find bar). A second binding that eats a keystroke the field needs is worse
/// than one binding that does not.
private struct BackCommand: View {
    @ObservedObject private var panel = FrontPanel.shared

    var body: some View {
        Button("Back") { panel.goBack() }
            .keyboardShortcut("[", modifiers: .command)
            .disabled(!panel.canGoBack)
    }
}

private struct ForwardCommand: View {
    @ObservedObject private var panel = FrontPanel.shared

    var body: some View {
        Button("Forward") { panel.goForward() }
            .keyboardShortcut("]", modifiers: .command)
            .disabled(!panel.canGoForward)
    }
}

/// ⌘+, ⌘− and ⌘0, on a browser's ladder of stops rather than a multiplier per press.
///
/// *Actual Size* first, as it is in Safari and Chrome, and greyed out on a page already at it — so
/// the menu answers "am I zoomed?" without anyone having to remember.
private struct ZoomCommands: View {
    @ObservedObject private var panel = FrontPanel.shared

    var body: some View {
        Button("Actual Size") { panel.resetZoom() }
            .keyboardShortcut("0", modifiers: .command)
            .disabled(!panel.zoomed)
        Button("Zoom In") { panel.zoomIn() }
            .keyboardShortcut("+", modifiers: .command)
            .disabled(!panel.hasPage)
        Button("Zoom Out") { panel.zoomOut() }
            .keyboardShortcut("-", modifiers: .command)
            .disabled(!panel.hasPage)
    }
}

/// ⌘F, ⌘G, ⇧⌘G — the three that have meant find, again and back since before there were tabs.
private struct FindCommands: View {
    @ObservedObject private var panel = FrontPanel.shared

    var body: some View {
        Button("Find…") { panel.startFind() }
            .keyboardShortcut("f", modifiers: .command)
            .disabled(!panel.hasPage)
        Button("Find Next") { panel.findNext() }
            .keyboardShortcut("g", modifiers: .command)
            .disabled(!panel.canFindAgain)
        Button("Find Previous") { panel.findPrevious() }
            .keyboardShortcut("g", modifiers: [.command, .shift])
            .disabled(!panel.canFindAgain)
    }
}

/// ⌃⇥ and ⌃⇧⇥, which wrap. Safari's ⇧⌘] and ⇧⌘[ do the same thing on the same tabs, and are not
/// here for the reason a second binding is never here: a menu item carries one key equivalent, and
/// two items reading *Show Next Tab* would be a worse menu than one.
private struct TabCommands: View {
    @ObservedObject private var panel = FrontPanel.shared

    var body: some View {
        Button("Show Next Tab") { panel.nextTab() }
            .keyboardShortcut(.tab, modifiers: .control)
            .disabled(!panel.canSwitchTabs)
        Button("Show Previous Tab") { panel.previousTab() }
            .keyboardShortcut(.tab, modifiers: [.control, .shift])
            .disabled(!panel.canSwitchTabs)
    }
}
