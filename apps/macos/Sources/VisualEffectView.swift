import AppKit
import SwiftUI

/// A translucent, desktop-blurring window background. Uses an `NSVisualEffectView`
/// with behind-window blending and makes its host window non-opaque so the blur of
/// what's behind the window actually shows through.
struct VisualEffectView: NSViewRepresentable {
    var material: NSVisualEffectView.Material = .hudWindow
    var blending: NSVisualEffectView.BlendingMode = .behindWindow

    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = TranslucentEffectView()
        view.material = material
        view.blendingMode = blending
        view.state = .active
        return view
    }

    func updateNSView(_ nsView: NSVisualEffectView, context: Context) {
        nsView.material = material
        nsView.blendingMode = blending
    }
}

/// `NSVisualEffectView` that clears its window's background once attached, so the
/// behind-window blur is visible rather than sitting on an opaque window.
private final class TranslucentEffectView: NSVisualEffectView {
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        window?.isOpaque = false
        window?.backgroundColor = .clear
    }
}
