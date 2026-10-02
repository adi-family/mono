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

export function blocks(source: string | null | undefined): MarkdownBlock[] {
  const lines = String(source ?? "").split(/\r?\n/);
  const parsedBlocks: MarkdownBlock[] = [];
  let lineIndex = 0;
  while (lineIndex < lines.length) {
    const line = lines[lineIndex].trimStart();
    const title = heading(line);
    const item = listItem(line);
    const aligns = tableAt(lines, lineIndex);
    if (!line) {
      lineIndex += 1;
    } else if (line.startsWith("```")) {
      const codeLines = [];
      lineIndex += 1;
      while (lineIndex < lines.length && !lines[lineIndex].trimStart().startsWith("```")) {
        codeLines.push(lines[lineIndex]);
        lineIndex += 1;
      }
      lineIndex += 1;
      parsedBlocks.push({ kind: "code", text: codeLines.join("\n") });
    } else if (title) {
      parsedBlocks.push({ kind: "heading", ...title });
      lineIndex += 1;
    } else if (isRule(line)) {
      parsedBlocks.push({ kind: "rule" });
      lineIndex += 1;
    } else if (line.startsWith(">")) {
      let text = "";
      while (lineIndex < lines.length && lines[lineIndex].trimStart().startsWith(">")) {
        text = joinLines(text, lines[lineIndex].trimStart().replace(/^>+/, "").trim());
        lineIndex += 1;
      }
      parsedBlocks.push({ kind: "quote", text });
    } else if (item) {
      const { ordered, text } = item;
      const items = [text];
      lineIndex += 1;
      while (lineIndex < lines.length) {
        const nextLine = lines[lineIndex].trimStart();
        const nextItem = listItem(nextLine);
        if (nextItem && nextItem.ordered === ordered) {
          items.push(nextItem.text);
          lineIndex += 1;
        } else if (!nextItem && nextLine && !opensBlock(lines, lineIndex)) {
          // Preserve Markdown lazy continuation within a list item.
          items[items.length - 1] = joinLines(items[items.length - 1], nextLine);
          lineIndex += 1;
        } else {
          break;
        }
      }
      parsedBlocks.push({ kind: "list", ordered, items });
    } else if (aligns) {
      const head = splitRow(line);
      lineIndex += 2;
      const rows = [];
      while (lineIndex < lines.length) {
        const nextLine = lines[lineIndex].trim();
        if (!nextLine || !nextLine.includes("|")) break;
        rows.push(splitRow(nextLine));
        lineIndex += 1;
      }
      parsedBlocks.push({ kind: "table", head, rows, aligns });
    } else {
      let text = "";
      while (lineIndex < lines.length && lines[lineIndex].trim() && !opensBlock(lines, lineIndex)) {
        text = joinLines(text, lines[lineIndex].trim());
        lineIndex += 1;
      }
      parsedBlocks.push({ kind: "para", text });
    }
  }
  return parsedBlocks;
}

function joinLines(text: string, line: string): string {
  return text ? `${text} ${line}` : line;
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
    const startsWithColon = cell.startsWith(":");
    const endsWithColon = cell.endsWith(":");
    if (startsWithColon && endsWithColon) aligns.push("center");
    else if (endsWithColon) aligns.push("right");
    else aligns.push("left");
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

function delimited(source: string, index: number, delimiter: string): { text: string; next: number } | null {
  const start = index + delimiter.length;
  const end = source.indexOf(delimiter, start);
  return end > start ? { text: source.slice(start, end), next: end + delimiter.length } : null;
}

function inlineLink(source: string, index: number): { node: HTMLAnchorElement; next: number } | null {
  const labelEnd = source.indexOf("]", index + 1);
  if (labelEnd <= 0 || source[labelEnd + 1] !== "(") return null;

  const urlEnd = source.indexOf(")", labelEnd + 2);
  const href = urlEnd > 0 ? safeHref(source.slice(labelEnd + 2, urlEnd)) : null;
  if (!href) return null;

  const link = el("a", null, source.slice(index + 1, labelEnd));
  link.href = href;
  link.rel = "noreferrer noopener";
  link.target = "_blank";
  return { node: link, next: urlEnd + 1 };
}

export function inline<T extends HTMLElement>(source: string, container: T): T {
  let plain = "";
  let index = 0;
  const flushText = () => {
    if (plain) container.append(document.createTextNode(plain));
    plain = "";
  };
  while (index < source.length) {
    const character = source[index];
    let node: HTMLElement | null = null;
    let next = index;
    if (character === "`") {
      const match = delimited(source, index, "`");
      if (match) {
        node = el("code", null, match.text);
        next = match.next;
      }
    } else if (character === "*" && source[index + 1] === "*") {
      const match = delimited(source, index, "**");
      if (match) {
        node = inline(match.text, el("strong"));
        next = match.next;
      }
    } else if ((character === "*" || character === "_") && !(character === "_" && /\w/.test(source[index - 1] ?? ""))) {
      // An underscore inside a word does not open emphasis.
      const match = delimited(source, index, character);
      if (match) {
        node = inline(match.text, el("em"));
        next = match.next;
      }
    } else if (character === "[") {
      const link = inlineLink(source, index);
      if (link) {
        node = link.node;
        next = link.next;
      }
    }
    if (node) {
      flushText();
      container.append(node);
      index = next;
    } else {
      plain += character;
      index += 1;
    }
  }
  flushText();
  return container;
}

function renderTable(block: Extract<MarkdownBlock, { kind: "table" }>): HTMLDivElement {
  const wrapper = el("div", "table");
  const table = el("table");
  const headerRow = el("tr");
  block.head.forEach((cell, column) => {
    const header = el("th", `a-${block.aligns[column]}`);
    headerRow.append(inline(cell.trim(), header));
  });
  table.createTHead().append(headerRow);

  const body = el("tbody");
  for (const cells of block.rows) {
    const row = el("tr");
    block.aligns.forEach((alignment, column) => {
      const cell = el("td", `a-${alignment}`);
      row.append(inline((cells[column] ?? "").trim(), cell));
    });
    body.append(row);
  }
  table.append(body);
  wrapper.append(table);
  return wrapper;
}

export function renderMarkdown(source: string | null | undefined): DocumentFragment {
  const fragment = document.createDocumentFragment();
  for (const block of blocks(source)) {
    switch (block.kind) {
      case "heading":
        fragment.append(inline(block.text, el(`h${Math.min(block.level, 3)}`)));
        break;
      case "code": {
        const pre = el("pre");
        pre.append(el("code", null, block.text));
        fragment.append(pre);
        break;
      }
      case "list": {
        const list = el(block.ordered ? "ol" : "ul");
        for (const item of block.items) list.append(inline(item, el("li")));
        fragment.append(list);
        break;
      }
      case "quote":
        fragment.append(inline(block.text, el("blockquote")));
        break;
      case "rule":
        fragment.append(el("hr"));
        break;
      case "table":
        fragment.append(renderTable(block));
        break;
      default:
        fragment.append(inline(block.text, el("p")));
    }
  }
  return fragment;
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
