// Generates the ADI app-icon master PNG (1024×1024): the real hexagonal ADI mark from
// the `.adi` landing page (crates/adi-dns/src/landing.rs), on a light squircle tile —
// dark strokes + orange core, matching the web logo. Keep the coordinates in sync with
// Sources/ADILogo.swift. Not part of the app; regenerate with `build.sh --regen-icon`:
//
//   swift apps/macos/icon-gen.swift /tmp/icon_1024.png
//   … then sips + iconutil → apps/macos/ADI.icns  (build.sh --regen-icon does it all)

import AppKit

let outPath = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "icon_1024.png"
let size: CGFloat = 1024

guard
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: Int(size), pixelsHigh: Int(size),
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0
    )
else { fatalError("could not allocate bitmap") }

NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

// Flip to a top-left origin so the landing SVG's coordinates map straight through.
let flip = NSAffineTransform()
flip.translateX(by: 0, yBy: size)
flip.scaleX(by: 1, yBy: -1)
flip.concat()

// macOS icon grid: content square inset from the full 1024 canvas, rounded corners.
let inset: CGFloat = 100
let content = CGRect(x: inset, y: inset, width: size - 2 * inset, height: size - 2 * inset)
let radius = content.width * 0.2237

NSGraphicsContext.saveGraphicsState()
NSBezierPath(roundedRect: content, xRadius: radius, yRadius: radius).addClip()
let top = NSColor(srgbRed: 1.0, green: 1.0, blue: 1.0, alpha: 1)
let bottom = NSColor(srgbRed: 0.93, green: 0.94, blue: 0.95, alpha: 1)
NSGradient(starting: top, ending: bottom)?.draw(in: content, angle: -90)
NSGraphicsContext.restoreGraphicsState()

// The mark, centered in the tile at ~64% of the content width.
let mark = content.width * 0.64
let scale = mark / 200.0
let ox = content.midX - mark / 2
let oy = content.midY - mark / 2
func p(_ x: Double, _ y: Double) -> NSPoint { NSPoint(x: ox + CGFloat(x) * scale, y: oy + CGFloat(y) * scale) }

let ink = NSColor(srgbRed: 0.078, green: 0.094, blue: 0.114, alpha: 1) // #14181d
let core = NSColor(srgbRed: 0.788, green: 0.392, blue: 0.133, alpha: 1) // #c96422

func polygon(_ pts: [(Double, Double)]) -> NSBezierPath {
    let path = NSBezierPath()
    path.move(to: p(pts[0].0, pts[0].1))
    for pt in pts.dropFirst() { path.line(to: p(pt.0, pt.1)) }
    path.close()
    path.lineJoinStyle = .round
    return path
}

func disc(_ x: Double, _ y: Double, _ r: Double) -> NSBezierPath {
    let c = p(x, y)
    let rr = CGFloat(r) * scale
    return NSBezierPath(ovalIn: CGRect(x: c.x - rr, y: c.y - rr, width: rr * 2, height: rr * 2))
}

// Spokes: outer node → nearest inner corners, fading with distance — the landing page's
// place(theta) at the resting frame (theta = 0). Kept in sync with Sources/ADILogo.swift.
let outerNodes: [(Double, Double)] = [(100, 2), (185.1, 149), (14.9, 149)]
let innerCorners: [(Double, Double)] = [(167.9, 100), (133.6, 158.2), (66.4, 158.2),
                                        (32.1, 100), (66.4, 41.8), (133.6, 41.8)]
func spokes(theta: Double = 0) -> [(from: (Double, Double), to: (Double, Double), opacity: Double)] {
    let (cx, cy) = (100.0, 100.0)
    let c = cos(theta), s = sin(theta)
    let iv = innerCorners.map { pt in
        (cx + (pt.0 - cx) * c - (pt.1 - cy) * s, cy + (pt.0 - cx) * s + (pt.1 - cy) * c)
    }
    var out: [(from: (Double, Double), to: (Double, Double), opacity: Double)] = []
    for o in outerNodes {
        let ds = iv.map { hypot($0.0 - o.0, $0.1 - o.1) }
        let order = Array(0..<iv.count).sorted { ds[$0] < ds[$1] }
        let near = ds[order[0]], cut = ds[order[3]]
        let span = cut - near == 0 ? 1 : cut - near
        for j in 0..<3 {
            out.append((from: o, to: iv[order[j]], opacity: min(1, max(0, (cut - ds[order[j]]) / span))))
        }
    }
    return out
}

ink.setStroke()
let outer = polygon([(100, 2), (185.1, 52.9), (185.1, 147.1), (100, 197.3), (14.9, 147.1), (14.9, 52.9)])
outer.lineWidth = 3 * scale
outer.stroke()
let inner = polygon([(167.9, 100), (133.6, 158.2), (66.4, 158.2), (32.1, 100), (66.4, 41.8), (133.6, 41.8)])
inner.lineWidth = 3 * scale
inner.stroke()

let edges = NSBezierPath()
for e in [(167.9, 100.0), (66.4, 158.2), (66.4, 41.8)] {
    edges.move(to: p(100, 100))
    edges.line(to: p(e.0, e.1))
}
edges.lineWidth = 2 * scale
edges.lineCapStyle = .round
edges.stroke()

for spoke in spokes() {
    let line = NSBezierPath()
    line.move(to: p(spoke.from.0, spoke.from.1))
    line.line(to: p(spoke.to.0, spoke.to.1))
    line.lineWidth = 2 * scale
    line.lineCapStyle = .round
    ink.withAlphaComponent(CGFloat(spoke.opacity)).setStroke()
    line.stroke()
}

core.withAlphaComponent(0.35).setFill()
disc(100, 100, 30).fill()
core.setFill()
disc(100, 100, 20).fill()

ink.setFill()
for n in outerNodes { disc(n.0, n.1, 6).fill() }

NSGraphicsContext.restoreGraphicsState()

guard let png = rep.representation(using: .png, properties: [:]) else { fatalError("PNG encode failed") }
try! png.write(to: URL(fileURLWithPath: outPath))
print("wrote \(outPath)")
