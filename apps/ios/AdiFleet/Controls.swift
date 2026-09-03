import SwiftUI

/// The handful of controls `design/DESIGN.md` §6 describes, as SwiftUI. Copied into both native
/// apps; nothing here names a platform.

/// A button (§6): `padding 7 14; radius 6; 13.5/500`.
///
/// `.accent` is the one filled orange a screen gets and is chosen per screen; `.strong` is the
/// screen's main action when orange is spent elsewhere. Everything else is the translucent default.
struct ADIButtonStyle: ButtonStyle {
    enum Kind {
        /// The one orange.
        case accent
        /// Ink on the page.
        case strong
        /// The translucent default.
        case normal
        /// No fill until pointed at: cancel, tertiary.
        case quiet
        /// Destructive: red text, the default fill.
        case danger
    }

    enum Size {
        case regular
        /// For a 48px bar and a 40px row.
        case small
    }

    var kind: Kind = .normal
    var size: Size = .regular
    /// Stretch to the width available — a form's footer, a sheet's one action.
    var wide = false

    @Environment(\.isEnabled) private var enabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(size == .small ? ADI.sans(13, .medium) : ADI.TextStyle.button)
            .foregroundStyle(foreground)
            .padding(.horizontal, size == .small ? 10 : 14)
            .padding(.vertical, size == .small ? 5 : 7)
            .frame(maxWidth: wide ? .infinity : nil)
            .background(fill(pressed: configuration.isPressed),
                        in: RoundedRectangle(cornerRadius: ADI.Radius.md))
            .opacity(enabled ? 1 : 0.5)
            .contentShape(RoundedRectangle(cornerRadius: ADI.Radius.md))
    }

    private var foreground: Color {
        switch kind {
        case .accent: ADI.onAccent
        case .strong: ADI.bg
        case .normal: ADI.ink
        case .quiet: ADI.ink2
        case .danger: ADI.err
        }
    }

    private func fill(pressed: Bool) -> Color {
        switch kind {
        case .accent: pressed ? ADI.accentHover : ADI.accent
        case .strong: pressed ? Color.white : ADI.ink
        case .normal, .danger: pressed ? ADI.btnHover : ADI.btn
        case .quiet: pressed ? ADI.bgHover : Color.clear
        }
    }
}

extension ButtonStyle where Self == ADIButtonStyle {
    static func adi(_ kind: ADIButtonStyle.Kind = .normal, _ size: ADIButtonStyle.Size = .regular,
                    wide: Bool = false) -> ADIButtonStyle {
        ADIButtonStyle(kind: kind, size: size, wide: wide)
    }
}

/// A status (§6): the 6px dot that goes before a word. Never a filled badge.
struct StatusDot: View {
    let color: Color

    var body: some View {
        Circle()
            .fill(color)
            .frame(width: 6, height: 6)
            .accessibilityHidden(true)
    }
}

/// A hairline between rows, panels, sections.
struct Hairline: View {
    var body: some View {
        Rectangle()
            .fill(ADI.line)
            .frame(height: 1)
            .accessibilityHidden(true)
    }
}

/// A tag (§6): a category, sans, 12px, pill, the translucent fill.
struct Chip: View {
    let text: String

    var body: some View {
        Text(text)
            .font(ADI.TextStyle.label)
            .foregroundStyle(ADI.ink2)
            .padding(.horizontal, 8)
            .padding(.vertical, 2)
            .background(ADI.chip, in: Capsule())
    }
}

/// A one-line note in the label style, with an icon in the same ink beside it.
struct Note: View {
    let icon: Lucide
    let text: String

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            LucideIcon(icon: icon, size: .sm)
                .foregroundStyle(ADI.ink3)
                .alignmentGuide(.firstTextBaseline) { $0[.bottom] - 2 }
            Text(text)
                .font(ADI.TextStyle.small)
                .foregroundStyle(ADI.ink3)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// An input (§6): the raised surface, a strong hairline, radius 6, `padding 9 12`.
struct ADIField: ViewModifier {
    func body(content: Content) -> some View {
        content
            .padding(.horizontal, 12)
            .padding(.vertical, 9)
            .background(ADI.bgRaise, in: RoundedRectangle(cornerRadius: ADI.Radius.md))
            .overlay(RoundedRectangle(cornerRadius: ADI.Radius.md).stroke(ADI.lineStrong, lineWidth: 1))
    }
}

/// A code block (§6): the raised surface, a hairline, the large radius.
struct ADICodeBlock: ViewModifier {
    func body(content: Content) -> some View {
        content
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
            .background(ADI.bgRaise, in: RoundedRectangle(cornerRadius: ADI.Radius.lg))
            .overlay(RoundedRectangle(cornerRadius: ADI.Radius.lg).stroke(ADI.line, lineWidth: 1))
    }
}

extension View {
    func adiField() -> some View { modifier(ADIField()) }
    func adiCodeBlock() -> some View { modifier(ADICodeBlock()) }
}

#if os(iOS)
extension View {
    /// A row in a list on the page surface: the page's fill, a hairline under it, the shell's
    /// inset. Every row in the app wears this, so a list is one surface cut by hairlines.
    func adiRow() -> some View {
        listRowBackground(ADI.bg)
            .listRowSeparatorTint(ADI.line)
            .listRowInsets(EdgeInsets(top: 8, leading: 20, bottom: 8, trailing: 20))
    }
}
#endif
