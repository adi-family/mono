import SwiftUI

/// The control panel, in a window of the app's own rather than in whatever browser the Mac opens
/// `.adi` links with.
///
/// The panel is a web app served by `adi-app` over the local front door, and this is that page in a
/// `WKWebView` (`WebPanel`) under a 48px bar — the app shell's own top bar (`design/DESIGN.md` §7),
/// carrying the three controls a page needs and the tabs. It is a viewer: everything it can do, the
/// panel in Safari can do too. What it buys is that the control panel is *in the app* — one icon in
/// the Dock, one window that remembers where it was, and no tab lost in a window of thirty.
///
/// **The first tab is the app.** Dashboards and services open beside it and are closed when they
/// are done with; the panel is neither (`PanelTabs`). There is no address bar: nothing is typed
/// into this window, because everything it can reach is a click away on the page it opens on.
struct PanelWindow: View {
    /// The scene id. `AppModel` asks for the panel and `ContentView` opens it, so the name has to
    /// be one constant rather than a string typed in both places.
    static let sceneID = "panel"

    @StateObject private var tabs: PanelTabs

    init(home: URL) {
        _tabs = StateObject(wrappedValue: PanelTabs(home: home))
    }

    var body: some View {
        VStack(spacing: 0) {
            bar
            Hairline()
            Page(panel: tabs.selected)
        }
        .frame(minWidth: 720, minHeight: 480)
        .background(ADI.bg)
        .background(WindowChrome())
        .preferredColorScheme(.dark)
        .navigationTitle(tabs.selected.label)
    }

    /// The tab bar: what is open, and the way out to a real browser.
    ///
    /// A band of its own under the window's title bar, rather than tabs sitting *in* the title bar
    /// beside the traffic lights. The lights keep their line and the tabs get theirs.
    ///
    /// There is no back, forward or reload button. They were three glyphs spent on what a page in
    /// this window almost never needs — it opens on the panel and everything it reaches is a click
    /// away on it — and the web view keeps all three on its own context menu for the times it does.
    private var bar: some View {
        HStack(spacing: 12) {
            TabStrip(tabs: tabs)
            OpenElsewhere(panel: tabs.selected)
        }
        .padding(.horizontal, 12)
        .frame(height: 48)
        .background(ADI.bgSide)
    }
}

/// What is open, in the order it was opened, starting with the app.
///
/// Scrolls rather than shrinks: a strip that divided the width by the number of tabs would answer
/// "which of these is the deploy dashboard?" with eight chips reading "da…". Tabs keep their names
/// and the strip slides.
private struct TabStrip: View {
    @ObservedObject var tabs: PanelTabs

    var body: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 4) {
                ForEach(Array(tabs.tabs.enumerated()), id: \.element.id) { index, tab in
                    TabChip(
                        panel: tab,
                        active: tab.id == tabs.selection,
                        index: index,
                        select: { tabs.select(tab) },
                        close: { tabs.close(tab) }
                    )
                }
            }
            .padding(.vertical, 6)
        }
        .scrollIndicators(.hidden)
    }
}

/// One tab: what the page calls itself, and — unless it is the app — a way to be rid of it.
///
/// A chosen segment is a tone change and not an outline or a colour (§6): `--bg-active` under ink,
/// against `--ink-2` on nothing for the rest.
private struct TabChip: View {
    @ObservedObject var panel: WebPanel
    let active: Bool
    /// Its place in the strip, which is also its ⌘-number for the first nine.
    let index: Int
    let select: () -> Void
    let close: () -> Void

    @State private var hovering = false

    var body: some View {
        Button(action: select) {
            label
                // Room for the close button to sit over, so a long title is cut by the glyph
                // rather than running under it.
                .padding(.trailing, panel.pinned ? 0 : 18)
                .frame(maxWidth: 200, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
        }
        .buttonStyle(TabStyle(active: active, hovering: hovering))
        .modifier(TabShortcut(index: index))
        .overlay(alignment: .trailing) {
            if !panel.pinned {
                Button(action: close) {
                    LucideIcon(icon: .x, size: .sm, label: "Close \(panel.label)")
                }
                .buttonStyle(.plain)
                .foregroundStyle(hovering || active ? ADI.ink2 : ADI.ink3)
                .padding(.trailing, 8)
            }
        }
        .onHover { hovering = $0 }
        .help(panel.webView.url?.absoluteString ?? panel.home.absoluteString)
    }

    /// What the tab wears.
    ///
    /// The app's tab is the mark and the wordmark rather than whatever the panel currently calls
    /// itself: it is not one page among the open ones, it is the thing they were opened from, and
    /// it says so by being the only one that never changes. §10's top-bar treatment — the mark
    /// beside the wordmark at 15/600 — at the tab-sized 16.
    @ViewBuilder
    private var label: some View {
        if panel.pinned {
            HStack(spacing: 8) {
                ADILogo(size: 16)
                Text("adi")
                    .font(ADI.TextStyle.wordmark)
            }
        } else {
            Text(panel.label)
                .font(ADI.TextStyle.row)
                .lineLimit(1)
                .truncationMode(.tail)
        }
    }
}

/// ⌘1…⌘9 select a tab, as everywhere else. Past the ninth there is no number left to press, so
/// the chip is simply clicked — the same place the shortcut would have taken you.
private struct TabShortcut: ViewModifier {
    let index: Int

    func body(content: Content) -> some View {
        if index < 9, let key = "\(index + 1)".first {
            content.keyboardShortcut(KeyEquivalent(key), modifiers: .command)
        } else {
            content
        }
    }
}

/// A tab's fill (§6): the chosen one is a tone change on the bar, the rest are nothing until
/// pointed at.
private struct TabStyle: ButtonStyle {
    let active: Bool
    let hovering: Bool

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(active ? ADI.ink : ADI.ink2)
            .padding(.horizontal, 10)
            .padding(.vertical, 5)
            .background(fill(pressed: configuration.isPressed),
                        in: RoundedRectangle(cornerRadius: ADI.Radius.md))
            .contentShape(RoundedRectangle(cornerRadius: ADI.Radius.md))
    }

    private func fill(pressed: Bool) -> Color {
        if active { return ADI.bgActive }
        if pressed { return ADI.bgActive }
        return hovering ? ADI.bgHover : Color.clear
    }
}

/// The escape hatch: this page, in the default browser.
private struct OpenElsewhere: View {
    @ObservedObject var panel: WebPanel

    var body: some View {
        BarButton(icon: .arrowUpRight, label: "Open in browser") {
            panel.openInDefaultBrowser()
        }
    }
}

/// The tab that is open, and — over it — the reason it is not.
private struct Page: View {
    @ObservedObject var panel: WebPanel

    var body: some View {
        ZStack {
            WebViewHost(panel: panel)
            if let failure = panel.failure {
                unreachable(failure)
            }
        }
        // The tab's own identity, so switching tabs swaps the web view rather than reusing the
        // one that is already mounted with another tab's page in it.
        .id(panel.id)
        .onAppear { panel.start() }
    }

    /// The page could not be loaded — the front door is down, the name does not resolve, the host
    /// refused. Over the page rather than beside it: what is underneath is the *previous* page, and
    /// leaving it there unmarked would say the navigation had worked.
    private func unreachable(_ message: String) -> some View {
        VStack(spacing: 12) {
            LucideIcon(icon: .triangleAlert, size: .xl)
                .foregroundStyle(ADI.ink3)
            Text("Cannot open this page")
                .font(ADI.TextStyle.section)
                .foregroundStyle(ADI.ink)
            Text(message)
                .font(ADI.TextStyle.small)
                .foregroundStyle(ADI.ink2)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: 380)
            Button("Try again") { panel.reload() }
                .buttonStyle(.adi(.normal))
                .padding(.top, 4)
        }
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(ADI.bg)
    }
}

/// One of the bar's icon buttons.
///
/// Unlabelled, which §9 allows only for a named few — close, filter, the ⋯ menu. These join that
/// set on the same argument: back, forward, reload and "opens elsewhere" are the shape browsers
/// have had for thirty years, and four words across the bar would crowd out the tabs. Each still
/// names itself to a screen reader and on hover.
private struct BarButton: View {
    let icon: Lucide
    let label: String
    var enabled = true
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            LucideIcon(icon: icon, size: .md, label: label)
        }
        .buttonStyle(.adi(.quiet, .small))
        .disabled(!enabled)
        .help(label)
    }
}

/// The two things about this window SwiftUI has no modifier for.
///
/// Keeps the title bar **AppKit's** while it wears the app's colour — which is the whole trick, and
/// took four wrong turns to find.
///
/// A title bar is worth more than the pixels it occupies: double-click to zoom or minimise (as that
/// Mac is set to), drag, the window menu. All of it comes free while AppKit owns the strip, and
/// none of it can be had once a view covers it — SwiftUI's hosting view claims every click in the
/// band it spans, and `allowsHitTesting(false)` does not give them back (measured: the hit is the
/// host, not the title bar).
///
/// So the band must be coloured *without* covering it, which is `titlebarAppearsTransparent` over a
/// window whose background is `--bg-side`. Each half needed a different fight:
///
/// * **The transparency** is `.hiddenTitleBar`'s doing, not this type's. Set by hand on an ordinary
///   window it is silently reverted — measured true at t+3s and false at t+6s, which is exactly how
///   a working setting looks like a broken one.
/// * **`fullSizeContentView`** comes with `.hiddenTitleBar` and is the half that hurts, so it is
///   removed here and kept out. With it gone the strip hit-tests as `NSThemeFrame` again.
/// * **The background colour** holds once set — it was never the problem, though it looked like it
///   while the transparency underneath kept flipping back.
///
/// The cost is the window's title, which `.hiddenTitleBar` also hides and which cannot be restored
/// (`titleVisibility = .visible` is re-hidden from `makeNSView`, from `updateNSView`, and deferred
/// a turn of the loop past either). The tab strip under the band names every open page anyway.
///
/// It also sets `isMovableByWindowBackground`, so the window can be dragged by its tab bar the way
/// any window with a strip of tabs can.
private struct WindowChrome: NSViewRepresentable {
    final class Coordinator {
        var observer: NSObjectProtocol?
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        // On the next turn of the loop: a view has no window at the moment it is made.
        DispatchQueue.main.async {
            guard let window = view.window else { return }
            Self.apply(to: window)
            // And on every window update from then on, because SwiftUI re-asserts the window's
            // configuration long after the view is built — a bounded timer loses that race, and
            // `updateNSView` alone stops running once the page settles.
            context.coordinator.observer = NotificationCenter.default.addObserver(
                forName: NSWindow.didUpdateNotification, object: window, queue: .main
            ) { note in
                Self.apply(to: note.object as? NSWindow)
            }
        }
        return view
    }

    func updateNSView(_ view: NSView, context: Context) {
        DispatchQueue.main.async { Self.apply(to: view.window) }
    }

    static func dismantleNSView(_ view: NSView, coordinator: Coordinator) {
        if let observer = coordinator.observer {
            NotificationCenter.default.removeObserver(observer)
        }
    }

    /// Only what has drifted, because this runs on every window update and assigning either one
    /// unconditionally would invalidate the title bar continuously.
    private static func apply(to window: NSWindow?) {
        guard let window else { return }
        // The content view must NOT span the title bar: that is what hands the band's clicks to
        // SwiftUI and takes double-click-to-zoom away. Removed here rather than avoided at the
        // scene, because the same modifier that brings it also brings the transparency the colour
        // depends on.
        if window.styleMask.contains(.fullSizeContentView) {
            window.styleMask.remove(.fullSizeContentView)
        }
        // Belongs to `.hiddenTitleBar`; asserted anyway so the intent is stated where the colour is.
        if !window.titlebarAppearsTransparent {
            window.titlebarAppearsTransparent = true
        }
        let surface = NSColor(ADI.bgSide)
        if window.backgroundColor != surface {
            window.backgroundColor = surface
        }
        if !window.isMovableByWindowBackground {
            window.isMovableByWindowBackground = true
        }
    }
}

/// The `WKWebView`, in SwiftUI.
///
/// The view is made by `WebPanel` rather than here: the bar drives it, and a representable that
/// built its own would hand SwiftUI a fresh, blank page every time it decided to rebuild the body —
/// losing the history, the scroll position and whatever was half-typed into the panel.
private struct WebViewHost: NSViewRepresentable {
    let panel: WebPanel

    func makeNSView(context: Context) -> some NSView { panel.webView }
    func updateNSView(_ view: some NSView, context: Context) {}
}
