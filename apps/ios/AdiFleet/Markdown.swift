import SwiftUI

/// Markdown as an agent writes it, drawn natively.
///
/// Foundation parses the inline half (emphasis, code spans, links, strikethrough) but flattens
/// every block into one run of text, so the block half is done here: fenced code, headings, lists,
/// quotes, tables and rules — the GitHub subset the panel's own transcript renders. It is a
/// line-based reader rather than a CommonMark implementation, and errs towards showing text as
/// written: an answer is read while it is still arriving, so a fence with no closing line yet is
/// a code block, not a parse error.
indirect enum MarkdownBlock: Equatable {
    case paragraph(String)
    case heading(level: Int, text: String)
    case code(language: String, text: String)
    case list([Item])
    case quote([MarkdownBlock])
    case table(header: [String], rows: [[String]])
    case rule

    struct Item: Equatable {
        /// `•` for a bullet, or the number as written (`3.`).
        let marker: String
        let blocks: [MarkdownBlock]
    }
}

enum Markdown {
    /// Parsed blocks by source text. A transcript is redrawn on every poll and its settled turns
    /// never change, so parsing each one once is most of the work saved.
    private static let cache: NSCache<NSString, Parsed> = {
        let cache = NSCache<NSString, Parsed>()
        cache.countLimit = 400
        return cache
    }()

    private final class Parsed {
        let blocks: [MarkdownBlock]
        init(_ blocks: [MarkdownBlock]) { self.blocks = blocks }
    }

    static func blocks(_ text: String) -> [MarkdownBlock] {
        let key = text as NSString
        if let hit = cache.object(forKey: key) { return hit.blocks }
        let blocks = parse(text.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n"))
        cache.setObject(Parsed(blocks), forKey: key)
        return blocks
    }

    private static func parse(_ lines: [String]) -> [MarkdownBlock] {
        var blocks: [MarkdownBlock] = []
        var paragraph: [String] = []
        var i = 0

        func flush() {
            if !paragraph.isEmpty {
                blocks.append(.paragraph(paragraph.joined(separator: "\n")))
                paragraph = []
            }
        }

        while i < lines.count {
            let line = lines[i]
            let trimmed = line.trimmingCharacters(in: .whitespaces)

            if trimmed.isEmpty {
                flush()
                i += 1
                continue
            }

            if let fence = fence(trimmed) {
                flush()
                var body: [String] = []
                i += 1
                while i < lines.count {
                    let candidate = lines[i].trimmingCharacters(in: .whitespaces)
                    if candidate.hasPrefix(fence.marker), candidate.allSatisfy({ $0 == fence.marker.first }) {
                        break
                    }
                    body.append(lines[i])
                    i += 1
                }
                i += 1
                blocks.append(.code(language: fence.language, text: body.joined(separator: "\n")))
                continue
            }

            if let heading = heading(trimmed) {
                flush()
                blocks.append(heading)
                i += 1
                continue
            }

            if isRule(trimmed) {
                flush()
                blocks.append(.rule)
                i += 1
                continue
            }

            if trimmed.hasPrefix(">") {
                flush()
                var inner: [String] = []
                while i < lines.count {
                    let candidate = lines[i].trimmingCharacters(in: .whitespaces)
                    guard candidate.hasPrefix(">") else { break }
                    var rest = candidate.dropFirst()
                    if rest.hasPrefix(" ") { rest = rest.dropFirst() }
                    inner.append(String(rest))
                    i += 1
                }
                blocks.append(.quote(parse(inner)))
                continue
            }

            if trimmed.contains("|"), i + 1 < lines.count, isTableRule(lines[i + 1]) {
                flush()
                let header = cells(line)
                var rows: [[String]] = []
                i += 2
                while i < lines.count, lines[i].contains("|"),
                      !lines[i].trimmingCharacters(in: .whitespaces).isEmpty
                {
                    rows.append(cells(lines[i]))
                    i += 1
                }
                blocks.append(.table(header: header, rows: rows))
                continue
            }

            if listMarker(line) != nil {
                flush()
                var items: [MarkdownBlock.Item] = []
                while i < lines.count, let marker = listMarker(lines[i]) {
                    // The item is its first line and everything after it indented past the marker,
                    // with blank lines allowed in between — which is what makes a nested list or a
                    // code block inside an item part of that item.
                    var body = [String(lines[i].dropFirst(marker.content))]
                    i += 1
                    while i < lines.count {
                        let next = lines[i]
                        if next.trimmingCharacters(in: .whitespaces).isEmpty {
                            guard i + 1 < lines.count, indent(lines[i + 1]) > marker.indent,
                                  !lines[i + 1].trimmingCharacters(in: .whitespaces).isEmpty
                            else { break }
                            body.append("")
                            i += 1
                            continue
                        }
                        guard indent(next) > marker.indent else { break }
                        body.append(String(next.dropFirst(min(marker.content, indent(next)))))
                        i += 1
                    }
                    items.append(.init(marker: marker.label, blocks: parse(body)))
                    // One blank line between items keeps them in the same list.
                    if i + 1 < lines.count, lines[i].trimmingCharacters(in: .whitespaces).isEmpty,
                       listMarker(lines[i + 1]) != nil
                    {
                        i += 1
                    }
                }
                blocks.append(.list(items))
                continue
            }

            paragraph.append(trimmed)
            i += 1
        }
        flush()
        return blocks
    }

    private static func fence(_ line: String) -> (marker: String, language: String)? {
        for marker in ["```", "~~~"] where line.hasPrefix(marker) {
            let run = line.prefix { $0 == marker.first }
            let language = line.dropFirst(run.count).trimmingCharacters(in: .whitespaces)
            // A backtick fence cannot carry backticks in its info string: that is inline code
            // which happens to open the line.
            if marker == "```", language.contains("`") { return nil }
            return (String(run), language)
        }
        return nil
    }

    private static func heading(_ line: String) -> MarkdownBlock? {
        let hashes = line.prefix { $0 == "#" }
        guard (1...6).contains(hashes.count) else { return nil }
        let rest = line.dropFirst(hashes.count)
        guard rest.isEmpty || rest.hasPrefix(" ") else { return nil }
        var text = rest.trimmingCharacters(in: .whitespaces)
        while text.hasSuffix("#") { text.removeLast() }
        return .heading(level: hashes.count, text: text.trimmingCharacters(in: .whitespaces))
    }

    private static func isRule(_ line: String) -> Bool {
        let marks = line.filter { !$0.isWhitespace }
        guard marks.count >= 3, let first = marks.first, "-*_".contains(first) else { return false }
        return marks.allSatisfy { $0 == first }
    }

    private static func isTableRule(_ line: String) -> Bool {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        guard trimmed.contains("-"), trimmed.contains("|") || trimmed.contains(":") else { return false }
        return trimmed.allSatisfy { "|-: ".contains($0) }
    }

    private static func cells(_ line: String) -> [String] {
        var row = line.trimmingCharacters(in: .whitespaces)
        if row.hasPrefix("|") { row.removeFirst() }
        if row.hasSuffix("|"), !row.hasSuffix("\\|") { row.removeLast() }
        // Split on bars that are not escaped; `\|` is how a cell carries one.
        var cells: [String] = []
        var cell = ""
        var escaped = false
        for character in row {
            if escaped {
                if character != "|" { cell.append("\\") }
                cell.append(character)
                escaped = false
            } else if character == "\\" {
                escaped = true
            } else if character == "|" {
                cells.append(cell.trimmingCharacters(in: .whitespaces))
                cell = ""
            } else {
                cell.append(character)
            }
        }
        cells.append(cell.trimmingCharacters(in: .whitespaces))
        return cells
    }

    private static func indent(_ line: String) -> Int {
        line.prefix { $0 == " " || $0 == "\t" }.reduce(0) { $0 + ($1 == "\t" ? 4 : 1) }
    }

    /// A list item's opening: how far the marker is indented, where its text starts, and what to
    /// draw for it.
    private static func listMarker(_ line: String) -> (indent: Int, content: Int, label: String)? {
        let lead = line.prefix { $0 == " " || $0 == "\t" }
        let rest = line.dropFirst(lead.count)
        guard let first = rest.first else { return nil }
        if "-*+".contains(first) {
            let after = rest.dropFirst()
            guard after.hasPrefix(" ") else { return nil }
            return (indent(line), lead.count + 2, "•")
        }
        let digits = rest.prefix { $0.isNumber }
        guard !digits.isEmpty, digits.count <= 9 else { return nil }
        let after = rest.dropFirst(digits.count)
        guard let delimiter = after.first, ".)".contains(delimiter), after.dropFirst().hasPrefix(" ") else {
            return nil
        }
        return (indent(line), lead.count + digits.count + 2, "\(digits).")
    }

    /// The inline half: emphasis, code spans, links.
    static func inline(_ text: String) -> AttributedString {
        let options = AttributedString.MarkdownParsingOptions(
            interpretedSyntax: .inlineOnlyPreservingWhitespace,
            failurePolicy: .returnPartiallyParsedIfPossible)
        guard var string = try? AttributedString(markdown: text, options: options) else {
            return AttributedString(text)
        }
        for run in string.runs {
            if let intent = run.inlinePresentationIntent, intent.contains(.code) {
                string[run.range].font = ADI.mono(13.5)
                string[run.range].foregroundColor = ADI.code
                string[run.range].backgroundColor = ADI.chip
            }
            if run.link != nil {
                // A link is ink, underlined (§2.4: orange is never a link inside the app).
                string[run.range].foregroundColor = ADI.ink
                string[run.range].underlineStyle = .single
            }
        }
        return string
    }
}

/// A markdown string as a column of blocks.
struct MarkdownView: View, Equatable {
    let text: String
    var ink: Color = ADI.ink

    var body: some View {
        MarkdownBlocks(blocks: Markdown.blocks(text), ink: ink)
    }
}

private struct MarkdownBlocks: View {
    let blocks: [MarkdownBlock]
    let ink: Color

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                view(for: block)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private func view(for block: MarkdownBlock) -> some View {
        switch block {
        case let .paragraph(text):
            prose(text)

        case let .heading(level, text):
            Text(Markdown.inline(text))
                .font(ADI.sans(level == 1 ? 20 : level == 2 ? 17 : 15.5, .semibold))
                .foregroundStyle(ADI.ink)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, level <= 2 ? 4 : 0)

        case let .code(language, text):
            CodeBlock(language: language, text: text)

        case let .list(items):
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(item.marker)
                            .font(ADI.TextStyle.body.monospacedDigit())
                            .foregroundStyle(ADI.ink3)
                        MarkdownBlocks(blocks: item.blocks, ink: ink)
                    }
                }
            }

        case let .quote(inner):
            MarkdownBlocks(blocks: inner, ink: ADI.ink2)
                .padding(.leading, 12)
                .overlay(alignment: .leading) {
                    Rectangle().fill(ADI.lineStrong).frame(width: 2)
                }

        case let .table(header, rows):
            TableBlock(header: header, rows: rows)

        case .rule:
            Hairline()
        }
    }

    private func prose(_ text: String) -> some View {
        Text(Markdown.inline(text))
            .font(ADI.TextStyle.body)
            .foregroundStyle(ink)
            .lineSpacing(4)
            .fixedSize(horizontal: false, vertical: true)
            .textSelection(.enabled)
    }
}

/// A fenced block (§6): the raised surface, a hairline, mono, scrolled sideways rather than
/// wrapped — a wrapped line of code is a different line of code.
private struct CodeBlock: View {
    let language: String
    let text: String
    @State private var copied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(language.isEmpty ? "text" : language)
                    .font(ADI.mono(11.5))
                    .foregroundStyle(ADI.ink3)
                Spacer()
                Button {
                    UIPasteboard.general.string = text
                    copied = true
                } label: {
                    LucideIcon(icon: copied ? .check : .copy, size: .sm, label: "Copy code")
                        .foregroundStyle(ADI.ink3)
                        .padding(6)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            .padding(.leading, 12)
            .padding(.trailing, 6)
            Hairline()
            ScrollView(.horizontal, showsIndicators: false) {
                Text(text)
                    .font(ADI.mono(12.5))
                    .foregroundStyle(ADI.code)
                    .lineSpacing(3)
                    .textSelection(.enabled)
                    .padding(12)
            }
        }
        .background(ADI.bgRaise, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(ADI.line, lineWidth: 1))
    }
}

/// A table (§6): no card, a strong rule under the header, hairlines between rows. It scrolls
/// sideways on its own, because a phone is narrower than almost any table an agent writes.
private struct TableBlock: View {
    let header: [String]
    let rows: [[String]]

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            Grid(alignment: .topLeading, horizontalSpacing: 16, verticalSpacing: 0) {
                GridRow {
                    ForEach(Array(header.enumerated()), id: \.offset) { _, cell in
                        Text(Markdown.inline(cell))
                            .font(ADI.TextStyle.label)
                            .foregroundStyle(ADI.ink3)
                            .padding(.vertical, 8)
                    }
                }
                Rectangle().fill(ADI.lineStrong).frame(height: 1)
                    .gridCellUnsizedAxes(.horizontal)
                ForEach(Array(rows.enumerated()), id: \.offset) { index, row in
                    GridRow {
                        ForEach(0..<header.count, id: \.self) { column in
                            Text(Markdown.inline(column < row.count ? row[column] : ""))
                                .font(ADI.TextStyle.ui)
                                .foregroundStyle(ADI.ink)
                                .frame(maxWidth: 260, alignment: .leading)
                                .fixedSize(horizontal: false, vertical: true)
                                .padding(.vertical, 8)
                        }
                    }
                    if index < rows.count - 1 { Hairline().gridCellUnsizedAxes(.horizontal) }
                }
            }
        }
    }
}
