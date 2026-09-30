// Build untrusted Markdown with DOM nodes and textContent; validate link targets with safeHref.
// Unrecognized syntax stays text, and an unclosed code fence runs to the end.

import { AdiElement, define } from "./base.ts";

export type MarkdownAlignment = "left" | "center" | "right";

export type MarkdownBlock =
  | { kind: "code" | "quote" | "para"; text: string }
  | { kind: "heading"; level: number; text: string }
  | { kind: "rule" }
  | { kind: "list"; ordered: boolean; items: string[] }
  | { kind: "table"; head: string[]; rows: string[][]; aligns: MarkdownAlignment[] };

export function blocks(src: string | null | undefined): MarkdownBlock[] {
  const lines = String(src ?? "").split(/\r?\n/);
  const out: MarkdownBlock[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i].trimStart();
    const title = heading(line);
    const item = listItem(line);
    const aligns = tableAt(lines, i);
    if (!line) {
      i += 1;
    } else if (line.startsWith("```")) {
      const text = [];
      i += 1;
      while (i < lines.length && !lines[i].trimStart().startsWith("```")) {
        text.push(lines[i]);
        i += 1;
      }
      i += 1;
      out.push({ kind: "code", text: text.join("\n") });
    } else if (title) {
      out.push({ kind: "heading", ...title });
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
    } else if (item) {
      const { ordered, text } = item;
      const items = [text];
      i += 1;
      while (i < lines.length) {
        const l = lines[i].trimStart();
        const next = listItem(l);
        if (next && next.ordered === ordered) {
          items.push(next.text);
          i += 1;
        } else if (!next && l && !opensBlock(lines, i)) {
          // Preserve Markdown lazy continuation within a list item.
          items[items.length - 1] = soft(items[items.length - 1], l);
          i += 1;
        } else {
          break;
        }
      }
      out.push({ kind: "list", ordered, items });
    } else if (aligns) {
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

function soft(buf: string, line: string): string {
  return buf ? `${buf} ${line}` : line;
}

function opensBlock(lines: string[], i: number): boolean {
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

// A table requires a following delimiter row with the same column count.
function tableAt(lines: string[], i: number): MarkdownAlignment[] | null {
  if (!lines[i].includes("|") || i + 1 >= lines.length) return null;
  const aligns = delimiter(lines[i + 1]);
  return aligns && aligns.length === splitRow(lines[i]).length ? aligns : null;
}

function delimiter(line: string): MarkdownAlignment[] | null {
  const cells = splitRow(line);
  if (!cells.length) return null;
  const aligns: MarkdownAlignment[] = [];
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

function splitRow(line: string): string[] {
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

function heading(line: string): { level: number; text: string } | null {
  const m = /^(#{1,6}) (.*)$/.exec(line);
  return m ? { level: m[1].length, text: m[2].trim() } : null;
}

function isRule(line: string): boolean {
  const bare = line.replace(/\s/g, "");
  return bare.length >= 3 && /^(-+|\*+|_+)$/.test(bare);
}

function listItem(line: string): { ordered: boolean; text: string } | null {
  const un = /^[-*+] (.*)$/.exec(line);
  if (un) return { ordered: false, text: un[1].trim() };
  const ord = /^\d+\. (.*)$/.exec(line);
  return ord ? { ordered: true, text: ord[1].trim() } : null;
}

/** Allow only http, https, mailto, and relative links; reject executable URL schemes. */
export function safeHref(url: string): string | null {
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

function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls?: string | null, text?: string): HTMLElementTagNameMap[K];
function el(tag: string, cls?: string | null, text?: string): HTMLElement;
function el(tag: string, cls?: string | null, text?: string): HTMLElement {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text !== undefined) node.textContent = text;
  return node;
}

function delimited(src: string, i: number, delim: string): { text: string; next: number } | null {
  const start = i + delim.length;
  const end = src.indexOf(delim, start);
  return end > start ? { text: src.slice(start, end), next: end + delim.length } : null;
}

export function inline<T extends HTMLElement>(src: string, into: T): T {
  let plain = "";
  let i = 0;
  const flush = () => {
    if (plain) into.append(document.createTextNode(plain));
    plain = "";
  };
  while (i < src.length) {
    const c = src[i];
    let node: HTMLElement | null = null;
    let next = i;
    if (c === "`") {
      const m = delimited(src, i, "`");
      if (m) [node, next] = [el("code", null, m.text), m.next];
    } else if (c === "*" && src[i + 1] === "*") {
      const m = delimited(src, i, "**");
      if (m) [node, next] = [inline(m.text, el("strong")), m.next];
    } else if ((c === "*" || c === "_") && !(c === "_" && /\w/.test(src[i - 1] ?? ""))) {
      // An underscore inside a word does not open emphasis.
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

export function renderMarkdown(src: string | null | undefined): DocumentFragment {
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
        table.createTHead().append(tr);
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

class AdiMarkdown extends AdiElement {
  static observedAttributes = ["source"];

  #source: string | null = null;
  #drawn: string | null = null;

  /** The document. The property wins over the attribute once it is set. */
  get source(): string {
    return this.#source ?? this.attr("source");
  }

  set source(value: string | null | undefined) {
    this.#source = String(value ?? "");
    this.update();
  }

  override template(): string {
    return `<div class="md" part="body"></div>`;
  }

  override update(): void {
    const src = this.source;
    // Skip reparsing unchanged documents during polling.
    if (src === this.#drawn || !this.shadowRoot.firstChild) return;
    this.#drawn = src;
    this.must(".md").replaceChildren(renderMarkdown(src));
  }
}

define("adi-markdown", AdiMarkdown);

export { AdiMarkdown };

declare global {
  interface HTMLElementTagNameMap {
    "adi-markdown": AdiMarkdown;
  }
}
