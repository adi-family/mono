// `<adi-markdown>` — the rendered half of anything an agent says: the JavaScript twin of
// `adi_ui::Markdown` (crates/adi-ui/src/markdown.rs), block for block.
//
//   <adi-markdown source="**Done.** The build is green."></adi-markdown>
//   el.source = text;                       // or set the property
//
// A small subset, scanned rather than parsed: headings, fenced code, lists, quotes, rules,
// GitHub-style tables, paragraphs, and inline `code` / **strong** / *em* / [links](url). It is
// **total** — no input is invalid, an unterminated fence runs to the end, and anything it does
// not recognise stays text.
//
// Everything is built from DOM nodes and `textContent`, never `innerHTML`, so a document cannot
// inject markup however it is written. Link targets are checked separately (`safeHref`) because
// a URL is the one thing here that becomes a live capability.
//
// Fenced code is drawn plain: the Rust twin highlights, and this one does not yet.

import { AdiElement, define, sheet } from "./base.js";

/** Split a document into blocks. Line-based, single pass; whatever it cannot classify is a paragraph. */
export function blocks(src) {
  const lines = String(src ?? "").split(/\r?\n/);
  const out = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i].trimStart();
    if (!line) {
      i += 1;
    } else if (line.startsWith("```")) {
      const text = [];
      i += 1;
      while (i < lines.length && !lines[i].trimStart().startsWith("```")) {
        text.push(lines[i]);
        i += 1;
      }
      // Past the closing fence — or past the end, for a fence nobody closed.
      i += 1;
      out.push({ kind: "code", text: text.join("\n") });
    } else if (heading(line)) {
      out.push({ kind: "heading", ...heading(line) });
      i += 1;
    } else if (isRule(line)) {
      out.push({ kind: "rule" });
      i += 1;
    } else if (line.startsWith(">")) {
      let text = "";
      while (i < lines.length && lines[i].trimStart().startsWith(">")) {
        text = soft(text, lines[i].trimStart().replace(/^>+/, "").trim());
        i += 1;
      }
      out.push({ kind: "quote", text });
    } else if (listItem(line)) {
      const { ordered, text } = listItem(line);
      const items = [text];
      i += 1;
      while (i < lines.length) {
        const l = lines[i].trimStart();
        const next = listItem(l);
        if (next && next.ordered === ordered) {
          items.push(next.text);
          i += 1;
        } else if (!next && l && !opensBlock(lines, i)) {
          // Markdown's lazy continuation: a hard-wrapped bullet is still one bullet.
          items[items.length - 1] = soft(items[items.length - 1], l);
          i += 1;
        } else {
          break;
        }
      }
      out.push({ kind: "list", ordered, items });
    } else if (tableAt(lines, i)) {
      const aligns = tableAt(lines, i);
      const head = splitRow(line);
      i += 2;
      const rows = [];
      while (i < lines.length) {
        const l = lines[i].trim();
        if (!l || !l.includes("|")) break;
        rows.push(splitRow(l));
        i += 1;
      }
      out.push({ kind: "table", head, rows, aligns });
    } else {
      let text = "";
      while (i < lines.length && lines[i].trim() && !opensBlock(lines, i)) {
        text = soft(text, lines[i].trim());
        i += 1;
      }
      out.push({ kind: "para", text });
    }
  }
  return out;
}

function soft(buf, line) {
  return buf ? `${buf} ${line}` : line;
}

function opensBlock(lines, i) {
  const line = lines[i].trim();
  return (
    line.startsWith("```") ||
    line.startsWith(">") ||
    Boolean(heading(line)) ||
    isRule(line) ||
    Boolean(listItem(line)) ||
    Boolean(tableAt(lines, i))
  );
}

// A table is the one block not recognisable from its first line: `a | b` is a sentence until the
// row of dashes under it agrees on the column count.
function tableAt(lines, i) {
  if (!lines[i].includes("|") || i + 1 >= lines.length) return null;
  const aligns = delimiter(lines[i + 1]);
  return aligns && aligns.length === splitRow(lines[i]).length ? aligns : null;
}

function delimiter(line) {
  const cells = splitRow(line);
  if (!cells.length) return null;
  const aligns = [];
  for (const raw of cells) {
    const cell = raw.trim();
    const rule = cell.replace(/^:+|:+$/g, "");
    if (!rule || !/^-+$/.test(rule)) return null;
    const l = cell.startsWith(":");
    const r = cell.endsWith(":");
    aligns.push(l && r ? "center" : r ? "right" : "left");
  }
  return aligns;
}

function splitRow(line) {
  const cells = [];
  let cur = "";
  const s = line.trim();
  for (let i = 0; i < s.length; i += 1) {
    if (s[i] === "\\" && s[i + 1] === "|") {
      cur += "|";
      i += 1;
    } else if (s[i] === "|") {
      cells.push(cur);
      cur = "";
    } else {
      cur += s[i];
    }
  }
  cells.push(cur);
  if (cells.length && !cells[0].trim()) cells.shift();
  if (cells.length && !cells[cells.length - 1].trim()) cells.pop();
  return cells;
}

function heading(line) {
  const m = /^(#{1,6}) (.*)$/.exec(line);
  return m ? { level: m[1].length, text: m[2].trim() } : null;
}

function isRule(line) {
  const bare = line.replace(/\s/g, "");
  return bare.length >= 3 && /^(-+|\*+|_+)$/.test(bare);
}

function listItem(line) {
  const un = /^[-*+] (.*)$/.exec(line);
  if (un) return { ordered: false, text: un[1].trim() };
  const ord = /^\d+\. (.*)$/.exec(line);
  return ord ? { ordered: true, text: ord[1].trim() } : null;
}

/**
 * A link target, if it is one worth handing to a browser. An allow-list — http, https, mailto,
 * or a relative / same-document target — because `javascript:` turns a document into code, and a
 * document here can come from anyone who can write a file.
 */
export function safeHref(url) {
  const trimmed = url.trim();
  const lower = trimmed.toLowerCase();
  const colon = lower.indexOf(":");
  const relative = colon < 0 || lower.slice(0, colon).includes("/");
  const ok =
    lower.startsWith("http://") ||
    lower.startsWith("https://") ||
    lower.startsWith("mailto:") ||
    trimmed.startsWith("/") ||
    trimmed.startsWith("#") ||
    relative;
  return ok ? trimmed : null;
}

function el(tag, cls, text) {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** The text between `delim` at `i` and the next `delim`, or null when it never closes. */
function delimited(src, i, delim) {
  const start = i + delim.length;
  const end = src.indexOf(delim, start);
  return end > start ? { text: src.slice(start, end), next: end + delim.length } : null;
}

/** Inline spans into `into`. Anything unmatched — a lone asterisk, an open bracket — stays text. */
export function inline(src, into) {
  let plain = "";
  let i = 0;
  const flush = () => {
    if (plain) into.append(document.createTextNode(plain));
    plain = "";
  };
  while (i < src.length) {
    const c = src[i];
    let node = null;
    let next = i;
    if (c === "`") {
      const m = delimited(src, i, "`");
      if (m) [node, next] = [el("code", null, m.text), m.next];
    } else if (c === "*" && src[i + 1] === "*") {
      const m = delimited(src, i, "**");
      if (m) [node, next] = [inline(m.text, el("strong")), m.next];
    } else if ((c === "*" || c === "_") && !(c === "_" && /\w/.test(src[i - 1] ?? ""))) {
      // `_` inside a word is a word (`run_id`), as CommonMark reads it — not the start of an em.
      const m = delimited(src, i, c);
      if (m) [node, next] = [inline(m.text, el("em")), m.next];
    } else if (c === "[") {
      const close = src.indexOf("]", i + 1);
      const end = close > 0 && src[close + 1] === "(" ? src.indexOf(")", close + 2) : -1;
      const href = end > 0 ? safeHref(src.slice(close + 2, end)) : null;
      if (href) {
        const a = el("a", null, src.slice(i + 1, close));
        a.href = href;
        a.rel = "noreferrer noopener";
        a.target = "_blank";
        [node, next] = [a, end + 1];
      }
    }
    if (node) {
      flush();
      into.append(node);
      i = next;
    } else {
      plain += c;
      i += 1;
    }
  }
  flush();
  return into;
}

/** A document as a fragment of plain DOM, for a caller that wants it outside this element. */
export function renderMarkdown(src) {
  const out = document.createDocumentFragment();
  for (const b of blocks(src)) {
    switch (b.kind) {
      case "heading":
        out.append(inline(b.text, el(`h${Math.min(b.level, 3)}`)));
        break;
      case "code": {
        const pre = el("pre");
        pre.append(el("code", null, b.text));
        out.append(pre);
        break;
      }
      case "list": {
        const list = el(b.ordered ? "ol" : "ul");
        for (const item of b.items) list.append(inline(item, el("li")));
        out.append(list);
        break;
      }
      case "quote":
        out.append(inline(b.text, el("blockquote")));
        break;
      case "rule":
        out.append(el("hr"));
        break;
      case "table": {
        const wrap = el("div", "table");
        const table = el("table");
        const tr = el("tr");
        b.head.forEach((cell, c) => tr.append(inline(cell.trim(), el("th", `a-${b.aligns[c]}`))));
        table.append(el("thead"));
        table.tHead.append(tr);
        const body = el("tbody");
        for (const row of b.rows) {
          const r = el("tr");
          b.aligns.forEach((align, c) => r.append(inline((row[c] ?? "").trim(), el("td", `a-${align}`))));
          body.append(r);
        }
        table.append(body);
        wrap.append(table);
        out.append(wrap);
        break;
      }
      default:
        out.append(inline(b.text, el("p")));
    }
  }
  return out;
}

/**
 * The document styles, for any shadow root that renders Markdown — `<adi-message>` adopts it
 * rather than nesting a whole element per paragraph.
 */
export const MARKDOWN_SHEET = sheet(`
  .md { display: flex; flex-direction: column; gap: var(--s4); font-size: var(--fs-body);
    line-height: 1.6; color: var(--ink); overflow-wrap: anywhere; }
  .md > * { margin: 0; }
  .md strong { font-weight: 600; }
  .md h1 { font-size: var(--fs-title); font-weight: 600; }
  .md h2 { font-size: var(--fs-section); font-weight: 600; }
  .md h3 { font-size: inherit; font-weight: 600; }
  .md code { padding: 2px 6px; border-radius: var(--r-sm); background: var(--chip);
    font-family: var(--mono); font-size: .85em; color: var(--code); }
  .md pre { flex: none; overflow-x: auto; padding: 12px 14px; border: 1px solid var(--line);
    border-radius: var(--r-lg); background: var(--bg-raise); font-family: var(--mono);
    font-size: var(--fs-mono); line-height: 1.6; color: var(--code); }
  .md pre code { padding: 0; background: none; font-size: inherit; }
  .md ul, .md ol { padding-left: 20px; }
  .md li + li { margin-top: 4px; }
  .md blockquote { padding-left: var(--s4); border-left: 2px solid var(--line-strong); color: var(--ink-2); }
  .md hr { height: 1px; border: 0; background: var(--line); }
  .md a { color: inherit; text-decoration: underline; text-decoration-color: var(--ink-3);
    text-underline-offset: 3px; }
  .md a:hover { text-decoration-color: var(--ink-2); }
  .md .table { flex: none; overflow-x: auto; }
  .md table { width: 100%; border-collapse: collapse; font-size: var(--fs-ui); }
  .md th { padding: 0 12px 8px; border-bottom: 1px solid var(--line-strong);
    font-size: var(--fs-label); font-weight: 400; color: var(--ink-3); }
  .md td { padding: 9px 12px; border-bottom: 1px solid var(--line); }
  .md th:first-child, .md td:first-child { padding-left: 0; }
  .md .a-left { text-align: left; } .md .a-center { text-align: center; } .md .a-right { text-align: right; }
`);

class AdiMarkdown extends AdiElement {
  static observedAttributes = ["source"];
  static sheet = MARKDOWN_SHEET;

  #source = null;
  #drawn = null;

  /** The document. The property wins over the attribute once it is set. */
  get source() {
    return this.#source ?? this.attr("source");
  }

  set source(value) {
    this.#source = String(value ?? "");
    this.update();
  }

  template() {
    return `<div class="md" part="body"></div>`;
  }

  update() {
    const src = this.source;
    // A transcript re-reads every poll; re-parsing an unchanged document is what makes that slow.
    if (src === this.#drawn || !this.shadowRoot.firstChild) return;
    this.#drawn = src;
    this.$(".md").replaceChildren(renderMarkdown(src));
  }
}

define("adi-markdown", AdiMarkdown);

export { AdiMarkdown };
