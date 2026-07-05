import Foundation
import SwiftUI

/// The real ADI mark — the hexagonal "cage" from the `.adi` 4XX page
/// (`crates/adi-hive/src/notfound.rs`): an outer hexagon + inner hexagon with the three
/// isometric box edges, the connecting spokes (each outer node → its nearest inner
/// corners, opacity fading with distance), and an orange core. Strokes use `.primary`
/// (the page's `currentColor`), so it adapts to light/dark; the core keeps the orange.
///
/// Geometry + the spoke math are the page's SVG verbatim (200×200 viewBox, resting
/// frame), so this stays identical to the web logo and to the app icon
/// (`apps/macos/icon-gen.swift`).
struct ADILogo: View {
    var size: CGFloat = 60

    /// The core accent, `#c96422`.
    static let core = Color(red: 0.788, green: 0.392, blue: 0.133)

    // Landing-SVG coordinates (200×200 space).
    private static let outerHex = [(100.0, 2.0), (185.1, 52.9), (185.1, 147.1),
                                   (100.0, 197.3), (14.9, 147.1), (14.9, 52.9)]
    private static let innerHex = [(167.9, 100.0), (133.6, 158.2), (66.4, 158.2),
                                   (32.1, 100.0), (66.4, 41.8), (133.6, 41.8)]
    private static let boxEdges = [(167.9, 100.0), (66.4, 158.2), (66.4, 41.8)]
    /// The three fixed outer connection points (also where the nodes sit).
    private static let outerNodes = [(100.0, 2.0), (185.1, 149.0), (14.9, 149.0)]

    var body: some View {
        Canvas { ctx, canvas in
            let s = min(canvas.width, canvas.height) / 200.0
            func p(_ pt: (Double, Double)) -> CGPoint { CGPoint(x: pt.0 * s, y: pt.1 * s) }
            let ink = GraphicsContext.Shading.color(.primary)

            ctx.stroke(hexPath(Self.outerHex, at: p),
                       with: ink, style: StrokeStyle(lineWidth: 3 * s, lineJoin: .round))
            ctx.stroke(hexPath(Self.innerHex, at: p),
                       with: ink, style: StrokeStyle(lineWidth: 3 * s, lineJoin: .round))

            var edges = Path()
            for e in Self.boxEdges {
                edges.move(to: p((100, 100)))
                edges.addLine(to: p(e))
            }
            ctx.stroke(edges, with: ink, style: StrokeStyle(lineWidth: 2 * s, lineCap: .round))

            // spokes: outer node → its nearest inner corners, fading with distance
            for spoke in Self.spokes() {
                var line = Path()
                line.move(to: p(spoke.from))
                line.addLine(to: p(spoke.to))
                ctx.stroke(line, with: .color(.primary.opacity(spoke.opacity)),
                           style: StrokeStyle(lineWidth: 2 * s, lineCap: .round))
            }

            ctx.fill(disc(at: p((100, 100)), r: 30 * s), with: .color(Self.core.opacity(0.35)))
            ctx.fill(disc(at: p((100, 100)), r: 20 * s), with: .color(Self.core))

            for n in Self.outerNodes {
                ctx.fill(disc(at: p(n), r: 6 * s), with: ink)
            }
        }
        .frame(width: size, height: size)
        .accessibilityLabel("ADI")
    }

    private func hexPath(_ pts: [(Double, Double)], at map: ((Double, Double)) -> CGPoint) -> Path {
        var path = Path()
        path.addLines(pts.map(map))
        path.closeSubpath()
        return path
    }

    private func disc(at c: CGPoint, r: CGFloat) -> Path {
        Path(ellipseIn: CGRect(x: c.x - r, y: c.y - r, width: r * 2, height: r * 2))
    }

    /// Replicates the landing page's `place(theta)` at the resting frame (`theta = 0`):
    /// for each outer node, rank the six inner corners by distance and connect the three
    /// closest, fading opacity from 1 at the nearest to 0 at the drop-out (4th closest).
    static func spokes(theta: Double = 0) -> [(from: (Double, Double), to: (Double, Double), opacity: Double)] {
        let (cx, cy) = (100.0, 100.0)
        let c = cos(theta), s = sin(theta)
        let iv = innerHex.map { pt in
            (cx + (pt.0 - cx) * c - (pt.1 - cy) * s, cy + (pt.0 - cx) * s + (pt.1 - cy) * c)
        }
        var out: [(from: (Double, Double), to: (Double, Double), opacity: Double)] = []
        for o in outerNodes {
            let ds = iv.map { hypot($0.0 - o.0, $0.1 - o.1) }
            let order = Array(0..<iv.count).sorted { ds[$0] < ds[$1] }
            let near = ds[order[0]], cut = ds[order[3]]
            let span = cut - near == 0 ? 1 : cut - near
            for j in 0..<3 {
                let op = min(1, max(0, (cut - ds[order[j]]) / span))
                out.append((from: o, to: iv[order[j]], opacity: op))
            }
        }
        return out
    }
}
