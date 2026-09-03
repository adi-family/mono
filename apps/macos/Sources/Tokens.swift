import SwiftUI
#if canImport(AppKit)
import AppKit
#else
import UIKit
#endif

/// The design tokens, as Swift. `design/tokens.css` is the source of every value here; this file
/// restates the dark set because a native app cannot import a stylesheet. Change a value there
/// first, then here — never invent one in a view. The same file is copied into the macOS and the
/// iOS app, which is why nothing in it names a platform outside the `#if`s at the bottom.
///
/// The app is dark (`design/DESIGN.md` §3). Light mode exists for the landing and docs only.
enum ADI {
    // MARK: surfaces — higher is closer to the reader

    /// Sidebars, bars, panels — and the whole of a small window.
    static let bgSide = Color(hex: 0x101010)
    /// The page: the lightest large surface.
    static let bg = Color(hex: 0x161616)
    static let bgHover = Color(hex: 0x1B1B1B)
    /// Inputs, code blocks, composers.
    static let bgRaise = Color(hex: 0x1E1E1E)
    /// A selected item, a segmented control's chosen segment.
    static let bgActive = Color(hex: 0x232323)

    // MARK: lines

    /// Hairlines between rows, panels, sections.
    static let line = Color.white.opacity(0.07)
    /// Input borders, table header rules.
    static let lineStrong = Color.white.opacity(0.13)

    // MARK: ink

    static let ink = Color(hex: 0xECEAE6)
    /// Secondary text, values.
    static let ink2 = Color(hex: 0xA9A6A0)
    /// Labels, meta, placeholders.
    static let ink3 = Color(hex: 0x6F6C67)
    /// Monospace text.
    static let code = Color(hex: 0xD6D3CD)

    // MARK: the accent — exactly one filled orange element per screen

    static let accent = Color(hex: 0xE8532A)
    static let accentHover = Color(hex: 0xF05F35)
    static let onAccent = Color.white

    // MARK: semantic — 6px dots and pills only, never fills

    static let ok = Color(hex: 0x4CB77A)
    static let warn = Color(hex: 0xE0A84B)
    static let err = Color(hex: 0xE25C5C)
    static let okSoft = Color(red: 76 / 255, green: 183 / 255, blue: 122 / 255).opacity(0.12)
    static let warnSoft = Color(red: 224 / 255, green: 168 / 255, blue: 75 / 255).opacity(0.12)
    static let errSoft = Color(red: 226 / 255, green: 92 / 255, blue: 92 / 255).opacity(0.12)

    // MARK: derived fills (tokens.css, "Derived tokens")

    /// Tags, chips, pills.
    static let chip = Color.white.opacity(0.07)
    static let chipHover = Color.white.opacity(0.11)
    /// The default button.
    static let btn = Color.white.opacity(0.08)
    static let btnHover = Color.white.opacity(0.12)

    /// Radius (§5): inline code and segments, the default, cards and code blocks.
    enum Radius {
        static let sm: CGFloat = 4
        static let md: CGFloat = 6
        static let lg: CGFloat = 10
    }

    /// The spacing scale (§5). Nothing in between.
    enum Space {
        static let s1: CGFloat = 4
        static let s2: CGFloat = 8
        static let s3: CGFloat = 12
        static let s4: CGFloat = 16
        static let s6: CGFloat = 24
        static let s8: CGFloat = 32
        static let s12: CGFloat = 48
    }

    // MARK: type (§4)

    /// Whether the bundle registered Geist. When it did not — a build that skipped the fonts,
    /// a platform that refused them — the roles below fall back to the system face rather
    /// than to nothing.
    static let hasGeist: Bool = familyAvailable("Geist")
    static let hasGeistMono: Bool = familyAvailable("Geist Mono")

    /// Geist at a size and weight, for everything people read.
    static func sans(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
        hasGeist ? Font.custom("Geist", size: size).weight(weight) : .system(size: size, weight: weight)
    }

    /// Geist Mono, for machine strings only: paths, keys, commands, ids, hosts.
    static func mono(_ size: CGFloat = 12.5) -> Font {
        hasGeistMono ? Font.custom("Geist Mono", size: size) : .system(size: size, design: .monospaced)
    }

    /// The roles, named by what they are for rather than by size.
    enum TextStyle {
        /// One per page (20–22 / 600).
        static let title = ADI.sans(20, .semibold)
        /// Panel headers, form sections, card titles.
        static let section = ADI.sans(16, .semibold)
        /// The wordmark beside the mark.
        static let wordmark = ADI.sans(15, .semibold)
        /// Prose a person reads at length.
        static let body = ADI.sans(15.5)
        /// Inputs, table cells.
        static let ui = ADI.sans(14)
        /// List items.
        static let row = ADI.sans(13.5)
        /// Button labels.
        static let button = ADI.sans(13.5, .medium)
        /// Help text, notices, second lines — in `ink2`.
        static let small = ADI.sans(13)
        /// Field labels, table headers, section dividers — in `ink3`, sentence case.
        static let label = ADI.sans(12)
        /// Machine strings — in `code`.
        static let mono = ADI.mono(12.5)
    }

    private static func familyAvailable(_ family: String) -> Bool {
        #if canImport(AppKit)
        return NSFontManager.shared.availableFontFamilies.contains(family)
        #else
        return UIFont.familyNames.contains(family)
        #endif
    }

    #if canImport(UIKit)
    /// The UIKit chrome SwiftUI does not draw itself — the navigation bar and the segmented
    /// control — set once at launch so it wears the same tokens as the views under it.
    @MainActor
    static func applyAppearance() {
        let bar = UINavigationBarAppearance()
        bar.configureWithOpaqueBackground()
        bar.backgroundColor = UIColor(bgSide)
        bar.shadowColor = UIColor(line)
        bar.titleTextAttributes = [.font: uiFont(16, .semibold), .foregroundColor: UIColor(ink)]
        bar.largeTitleTextAttributes = [.font: uiFont(22, .semibold), .foregroundColor: UIColor(ink)]
        let navigationBar = UINavigationBar.appearance()
        navigationBar.standardAppearance = bar
        navigationBar.scrollEdgeAppearance = bar
        navigationBar.compactAppearance = bar
        navigationBar.tintColor = UIColor(ink)

        let toolbar = UIToolbarAppearance()
        toolbar.configureWithOpaqueBackground()
        toolbar.backgroundColor = UIColor(bgSide)
        toolbar.shadowColor = UIColor(line)
        UIToolbar.appearance().standardAppearance = toolbar
        UIToolbar.appearance().scrollEdgeAppearance = toolbar

        // A segmented control (§6): the raised surface, the chosen segment a tone change.
        let segmented = UISegmentedControl.appearance()
        segmented.backgroundColor = UIColor(bgRaise)
        segmented.selectedSegmentTintColor = UIColor(bgActive)
        segmented.setTitleTextAttributes(
            [.font: uiFont(13.5, .regular), .foregroundColor: UIColor(ink2)], for: .normal)
        segmented.setTitleTextAttributes(
            [.font: uiFont(13.5, .medium), .foregroundColor: UIColor(ink)], for: .selected)
    }

    /// Geist as a `UIFont`, for the appearance proxies above.
    static func uiFont(_ size: CGFloat, _ weight: UIFont.Weight) -> UIFont {
        guard hasGeist else { return .systemFont(ofSize: size, weight: weight) }
        let descriptor = UIFontDescriptor(fontAttributes: [
            .family: "Geist",
            .traits: [UIFontDescriptor.TraitKey.weight: weight],
        ])
        return UIFont(descriptor: descriptor, size: size)
    }
    #endif
}

extension Color {
    /// A token's hex, as written in `design/tokens.css`.
    init(hex: UInt32) {
        self.init(
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255
        )
    }
}
