#!/usr/bin/env python3
"""Write `Lucide.swift` for the two native apps from the Lucide set the web UI already carries.

    python3 apps/macos/lucide-gen.py            # regenerates both copies

The source is `crates/adi-ui/icons/*.svg` (fetched by `scripts/lucide.sh`, ISC licence in that
directory), so the phone, the menubar and the control panel draw the same glyphs from the same
files. Only the names in ICONS are compiled in: an icon is a few hundred bytes of markup, and the
apps use two dozen.

The apps have no asset catalogue to speak of — the macOS bundle is assembled by a script and
compiled with `swiftc` — so the glyphs ship as their SVG markup and `LucideShape` in the generated
file turns that into a SwiftUI `Path` at draw time. This script validates every path it emits
against the same tokenizer the Swift side uses, so a compact SVG form the parser does not know
fails here rather than drawing nothing on a phone.
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
SRC = ROOT / "crates/adi-ui/icons"
OUT = [
    ROOT / "apps/macos/Sources/Lucide.swift",
    ROOT / "apps/ios/AdiFleet/Lucide.swift",
]

# The nouns the two apps have. `design/DESIGN.md` §9 is the noun → icon map.
ICONS = [
    "arrow-up",          # update available
    "arrow-up-right",    # opens elsewhere: the dashboard, an issue
    "bug",               # something not working
    "camera",            # scan
    "check",             # copied
    "chevron-right",
    "circle-alert",
    "clipboard",         # paste
    "code",              # an api service
    "copy",
    "ellipsis",          # another service, row actions
    "globe",             # a plain service
    "info",
    "key-round",
    "layout-dashboard",  # a dashboard
    "layout-grid",       # the control panel (`app`)
    "link",              # pair
    "monitor",           # a node
    "network",           # the fleet / the mesh
    "plus",
    "power",
    "qr-code",
    "radio",             # the relay session coming up
    "scan-line",
    "settings-2",
    "share",
    "triangle-alert",
    "x",
]

VERSION_RE = re.compile(r"lucide-static v([\d.]+)")
ELEMENT_RE = re.compile(r"<(path|circle|rect|line|polyline|polygon|ellipse)\b([^>]*?)/?>")
ATTR_RE = re.compile(r'([\w-]+)="([^"]*)"')
NUMBER_RE = re.compile(r"[-+]?(?:\d*\.\d+|\d+\.?)(?:[eE][-+]?\d+)?")
ARGS = {"M": 2, "L": 2, "H": 1, "V": 1, "C": 6, "S": 4, "Q": 4, "T": 2, "A": 7, "Z": 0}


def inner(svg: str) -> str:
    start = svg.index("<svg")
    body_start = svg.index(">", start) + 1
    body_end = svg.rindex("</svg>")
    return " ".join(svg[body_start:body_end].split()).replace("> <", "><")


def check_path(name: str, d: str) -> None:
    """The Swift tokenizer, in Python: a command letter, then numbers in runs of its arity."""
    pos = 0
    cmd = None
    while pos < len(d):
        ch = d[pos]
        if ch.isspace() or ch == ",":
            pos += 1
            continue
        if ch.isalpha():
            cmd = ch.upper()
            if cmd not in ARGS:
                sys.exit(f"{name}: unsupported path command {ch!r}")
            pos += 1
            if ARGS[cmd] == 0:
                continue
            continue
        m = NUMBER_RE.match(d, pos)
        if not m:
            sys.exit(f"{name}: cannot tokenize {d[pos:pos+12]!r}")
        pos = m.end()
        # A run of numbers belongs to the last command, repeating in groups of its arity. Arc
        # flags are single digits that the Swift side reads the same way this regex does, which
        # is what the check is for: `0 0 1` parses, `017.54` would not.
    if cmd is None:
        sys.exit(f"{name}: empty path")


def variant(name: str) -> str:
    parts = name.split("-")
    return parts[0] + "".join(p[:1].upper() + p[1:] for p in parts[1:])


def main() -> None:
    version = None
    entries = []
    for name in ICONS:
        path = SRC / f"{name}.svg"
        if not path.exists():
            sys.exit(f"{name}: not in {SRC} — add it to crates/adi-ui/icons/ICONS and run scripts/lucide.sh")
        svg = path.read_text()
        m = VERSION_RE.search(svg)
        version = version or (m.group(1) if m else "?")
        markup = inner(svg)
        for element, attrs in ELEMENT_RE.findall(markup):
            attributes = dict(ATTR_RE.findall(attrs))
            if element == "path":
                check_path(name, attributes["d"])
        entries.append((name, markup))

    swift = TEMPLATE.replace("@VERSION@", version or "?")
    cases = "\n".join(f'    case {variant(n)} = "{n}"' for n, _ in entries)
    markups = "\n".join(f"        case .{variant(n)}: {swift_string(m)}" for n, m in entries)
    swift = swift.replace("@CASES@", cases).replace("@MARKUP@", markups)
    for out in OUT:
        out.write_text(swift)
        print(f"wrote {out.relative_to(ROOT)} ({len(entries)} icons, lucide-static {version})")


def swift_string(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


TEMPLATE = '''// GENERATED by apps/macos/lucide-gen.py from crates/adi-ui/icons/*.svg — do not edit.
// Regenerate with `python3 apps/macos/lucide-gen.py`. Lucide @VERSION@, ISC licence
// (crates/adi-ui/icons/LICENSE). The same file is written to both native apps.

import SwiftUI

/// One Lucide icon, by its lucide.dev name (`design/DESIGN.md` §9). The only icon library in the
/// product; nothing here draws its own glyph and nothing uses SF Symbols.
enum Lucide: String, CaseIterable {
@CASES@

    /// The inside of the icon's `<svg>`, on Lucide's 24×24 grid.
    var markup: String {
        switch self {
@MARKUP@
        }
    }
}

/// The four sizes an icon may be (§9). Nothing else.
enum LucideSize: CGFloat {
    /// Inside list-item meta and tags.
    case sm = 14
    /// Tree and explorer rows, table cells, buttons, panel headers. The default.
    case md = 16
    /// Feature blocks.
    case lg = 20
    /// Empty states only.
    case xl = 24
}

/// An icon, drawn the one way §9 allows: stroke 1.5 on the 24 grid, round caps and joins, in the
/// colour of the text beside it (`foregroundStyle`). Hidden from assistive technology unless given
/// a `label`, because an icon in this product sits beside the word it stands for.
struct LucideIcon: View {
    let icon: Lucide
    var size: LucideSize = .md
    var label: String?

    var body: some View {
        let px = size.rawValue
        LucideShape(icon: icon)
            .stroke(style: StrokeStyle(lineWidth: 1.5 * px / 24, lineCap: .round, lineJoin: .round))
            .frame(width: px, height: px)
            .accessibilityHidden(label == nil)
            .accessibilityLabel(label ?? "")
    }
}

/// The icon's outline as a `Shape`, scaled from the 24 grid into whatever rect it is given.
struct LucideShape: Shape {
    let icon: Lucide

    func path(in rect: CGRect) -> Path {
        let scale = min(rect.width, rect.height) / 24
        let transform = CGAffineTransform(translationX: rect.minX, y: rect.minY)
            .scaledBy(x: scale, y: scale)
        return LucideMarkup.path(for: icon).applying(transform)
    }
}

/// SVG markup → `Path`, for the subset of SVG Lucide is drawn in: `path`, `circle`, `rect`,
/// `line`, `polyline`, `polygon`, `ellipse`, and path data using M L H V C S Q T A Z. Parsed once
/// per icon and cached — an icon is drawn on every frame a list scrolls.
enum LucideMarkup {
    private static var cache: [Lucide: Path] = [:]
    private static let lock = NSLock()

    static func path(for icon: Lucide) -> Path {
        lock.lock()
        defer { lock.unlock() }
        if let cached = cache[icon] { return cached }
        let built = build(icon.markup)
        cache[icon] = built
        return built
    }

    private static let element = try! NSRegularExpression(
        pattern: "<(path|circle|rect|line|polyline|polygon|ellipse)\\\\b([^>]*?)/?>")
    private static let attribute = try! NSRegularExpression(pattern: "([\\\\w-]+)=\\"([^\\"]*)\\"")

    private static func build(_ markup: String) -> Path {
        var path = Path()
        let whole = NSRange(markup.startIndex..., in: markup)
        for match in element.matches(in: markup, range: whole) {
            let name = String(markup[Range(match.range(at: 1), in: markup)!])
            let attrs = String(markup[Range(match.range(at: 2), in: markup)!])
            var values: [String: String] = [:]
            for a in attribute.matches(in: attrs, range: NSRange(attrs.startIndex..., in: attrs)) {
                let key = String(attrs[Range(a.range(at: 1), in: attrs)!])
                values[key] = String(attrs[Range(a.range(at: 2), in: attrs)!])
            }
            func number(_ key: String, _ fallback: CGFloat = 0) -> CGFloat {
                values[key].flatMap { Double($0) }.map { CGFloat($0) } ?? fallback
            }
            switch name {
            case "path":
                if let d = values["d"] { PathData(d).append(to: &path) }
            case "circle":
                let r = number("r")
                path.addEllipse(in: CGRect(x: number("cx") - r, y: number("cy") - r,
                                           width: 2 * r, height: 2 * r))
            case "ellipse":
                let rx = number("rx"), ry = number("ry")
                path.addEllipse(in: CGRect(x: number("cx") - rx, y: number("cy") - ry,
                                           width: 2 * rx, height: 2 * ry))
            case "rect":
                let rect = CGRect(x: number("x"), y: number("y"),
                                  width: number("width"), height: number("height"))
                let rx = number("rx", number("ry"))
                let ry = number("ry", rx)
                if rx > 0 || ry > 0 {
                    path.addRoundedRect(in: rect, cornerSize: CGSize(width: rx, height: ry))
                } else {
                    path.addRect(rect)
                }
            case "line":
                path.move(to: CGPoint(x: number("x1"), y: number("y1")))
                path.addLine(to: CGPoint(x: number("x2"), y: number("y2")))
            case "polyline", "polygon":
                let points = PathData.numbers(in: values["points"] ?? "")
                var first = true
                for i in stride(from: 0, to: points.count - 1, by: 2) {
                    let point = CGPoint(x: points[i], y: points[i + 1])
                    if first { path.move(to: point); first = false } else { path.addLine(to: point) }
                }
                if name == "polygon" { path.closeSubpath() }
            default:
                break
            }
        }
        return path
    }
}

/// The `d` attribute of an SVG `<path>`, appended to a `Path`.
///
/// Arcs are the one command a `Path` has no primitive for; they are converted to cubic Béziers
/// through the endpoint-to-centre parameterisation in the SVG specification, a quarter turn per
/// segment. Relative forms and the implicit line after a move are handled; everything else in
/// the grammar (only ever numbers and letters) is left alone.
struct PathData {
    private let d: String

    init(_ d: String) { self.d = d }

    static func numbers(in text: String) -> [CGFloat] {
        var out: [CGFloat] = []
        var current = ""
        func flush() {
            if let v = Double(current) { out.append(CGFloat(v)) }
            current = ""
        }
        for ch in text {
            switch ch {
            case "0"..."9":
                current.append(ch)
            case ".":
                // A second point starts a new number: `7.54.54` is two.
                if current.contains(".") { flush() }
                current.append(ch)
            case "-", "+":
                // A sign starts a new number unless it follows an exponent.
                if !current.isEmpty, !current.hasSuffix("e"), !current.hasSuffix("E") { flush() }
                current.append(ch)
            case "e", "E":
                current.append(ch)
            default:
                flush()
            }
        }
        flush()
        return out
    }

    func append(to path: inout Path) {
        var tokens: [Token] = []
        var run = ""
        func flushRun() {
            if !run.isEmpty {
                tokens.append(contentsOf: Self.numbers(in: run).map(Token.number))
                run = ""
            }
        }
        for ch in d {
            if ch.isLetter, ch != "e", ch != "E" {
                flushRun()
                tokens.append(.command(ch))
            } else {
                run.append(ch)
            }
        }
        flushRun()

        var current = CGPoint.zero
        var start = CGPoint.zero
        var control: CGPoint? = nil   // the reflected control point for S / T
        var lastCommand: Character = "M"
        var i = 0

        func take() -> CGFloat {
            defer { i += 1 }
            if case let .number(v) = tokens[i] { return v }
            return 0
        }
        func hasNumbers(_ n: Int) -> Bool {
            guard i + n <= tokens.count else { return false }
            for k in 0..<n { if case .command = tokens[i + k] { return false } }
            return true
        }

        while i < tokens.count {
            var command: Character
            if case let .command(c) = tokens[i] {
                command = c
                i += 1
            } else {
                // Numbers after a command repeat it; after a move they draw lines.
                command = lastCommand
                if command == "M" { command = "L" } else if command == "m" { command = "l" }
            }
            let relative = command.isLowercase
            let upper = Character(command.uppercased())
            func point(_ x: CGFloat, _ y: CGFloat) -> CGPoint {
                relative ? CGPoint(x: current.x + x, y: current.y + y) : CGPoint(x: x, y: y)
            }

            switch upper {
            case "M":
                guard hasNumbers(2) else { i = tokens.count; break }
                current = point(take(), take())
                start = current
                path.move(to: current)
                control = nil
            case "L":
                guard hasNumbers(2) else { i = tokens.count; break }
                current = point(take(), take())
                path.addLine(to: current)
                control = nil
            case "H":
                guard hasNumbers(1) else { i = tokens.count; break }
                let x = take()
                current = CGPoint(x: relative ? current.x + x : x, y: current.y)
                path.addLine(to: current)
                control = nil
            case "V":
                guard hasNumbers(1) else { i = tokens.count; break }
                let y = take()
                current = CGPoint(x: current.x, y: relative ? current.y + y : y)
                path.addLine(to: current)
                control = nil
            case "C":
                guard hasNumbers(6) else { i = tokens.count; break }
                let c1 = point(take(), take())
                let c2 = point(take(), take())
                let end = point(take(), take())
                path.addCurve(to: end, control1: c1, control2: c2)
                control = c2
                current = end
            case "S":
                guard hasNumbers(4) else { i = tokens.count; break }
                let c1 = control.map { CGPoint(x: 2 * current.x - $0.x, y: 2 * current.y - $0.y) } ?? current
                let c2 = point(take(), take())
                let end = point(take(), take())
                path.addCurve(to: end, control1: c1, control2: c2)
                control = c2
                current = end
            case "Q":
                guard hasNumbers(4) else { i = tokens.count; break }
                let c = point(take(), take())
                let end = point(take(), take())
                path.addQuadCurve(to: end, control: c)
                control = c
                current = end
            case "T":
                guard hasNumbers(2) else { i = tokens.count; break }
                let c = control.map { CGPoint(x: 2 * current.x - $0.x, y: 2 * current.y - $0.y) } ?? current
                let end = point(take(), take())
                path.addQuadCurve(to: end, control: c)
                control = c
                current = end
            case "A":
                guard hasNumbers(7) else { i = tokens.count; break }
                let rx = take(), ry = take(), rotation = take()
                let largeArc = take() != 0, sweep = take() != 0
                let end = point(take(), take())
                Self.arc(from: current, to: end, rx: rx, ry: ry, rotation: rotation,
                         largeArc: largeArc, sweep: sweep, into: &path)
                current = end
                control = nil
            case "Z":
                path.closeSubpath()
                current = start
                control = nil
            default:
                i = tokens.count
            }
            lastCommand = command
        }
    }

    private enum Token {
        case command(Character)
        case number(CGFloat)
    }

    /// SVG arc → cubic Béziers (SVG 1.1 implementation notes, F.6.5 and F.6.6).
    private static func arc(from p1: CGPoint, to p2: CGPoint, rx: CGFloat, ry: CGFloat,
                            rotation: CGFloat, largeArc: Bool, sweep: Bool, into path: inout Path) {
        if p1 == p2 { return }
        var rx = abs(rx), ry = abs(ry)
        if rx == 0 || ry == 0 {
            path.addLine(to: p2)
            return
        }
        let phi = rotation * .pi / 180
        let cosPhi = cos(phi), sinPhi = sin(phi)
        let dx = (p1.x - p2.x) / 2, dy = (p1.y - p2.y) / 2
        let x1p = cosPhi * dx + sinPhi * dy
        let y1p = -sinPhi * dx + cosPhi * dy
        // Scale the radii up if the endpoints are too far apart for them.
        let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry)
        if lambda > 1 {
            rx *= sqrt(lambda)
            ry *= sqrt(lambda)
        }
        let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p
        let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p
        var coefficient = den == 0 ? 0 : sqrt(max(0, num / den))
        if largeArc == sweep { coefficient = -coefficient }
        let cxp = coefficient * (rx * y1p / ry)
        let cyp = coefficient * -(ry * x1p / rx)
        let cx = cosPhi * cxp - sinPhi * cyp + (p1.x + p2.x) / 2
        let cy = sinPhi * cxp + cosPhi * cyp + (p1.y + p2.y) / 2

        func angle(_ ux: CGFloat, _ uy: CGFloat, _ vx: CGFloat, _ vy: CGFloat) -> CGFloat {
            let dot = ux * vx + uy * vy
            let len = sqrt(ux * ux + uy * uy) * sqrt(vx * vx + vy * vy)
            var a = acos(max(-1, min(1, dot / len)))
            if ux * vy - uy * vx < 0 { a = -a }
            return a
        }
        let theta1 = angle(1, 0, (x1p - cxp) / rx, (y1p - cyp) / ry)
        var delta = angle((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry)
        if !sweep, delta > 0 { delta -= 2 * .pi }
        if sweep, delta < 0 { delta += 2 * .pi }

        // Split into segments of at most a quarter turn; each is one cubic.
        let segments = max(1, Int(ceil(abs(delta) / (.pi / 2) - 0.0001)))
        let step = delta / CGFloat(segments)
        var theta = theta1
        let t = 4 / 3 * tan(step / 4)
        for _ in 0..<segments {
            let cos1 = cos(theta), sin1 = sin(theta)
            let cos2 = cos(theta + step), sin2 = sin(theta + step)
            // Points on the unit circle, then the control handles, then back through the ellipse.
            func map(_ x: CGFloat, _ y: CGFloat) -> CGPoint {
                let ex = rx * x, ey = ry * y
                return CGPoint(x: cosPhi * ex - sinPhi * ey + cx, y: sinPhi * ex + cosPhi * ey + cy)
            }
            let c1 = map(cos1 - t * sin1, sin1 + t * cos1)
            let c2 = map(cos2 + t * sin2, sin2 - t * cos2)
            let end = map(cos2, sin2)
            path.addCurve(to: end, control1: c1, control2: c2)
            theta += step
        }
    }
}
'''

if __name__ == "__main__":
    main()
