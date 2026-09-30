// Entries arrive oldest first and render newest first, keyed to preserve open state across polls.

import { AdiElement, define, esc } from "./base.ts";
import { renderMarkdown } from "./markdown.ts";
import "./icon.ts";

export interface Picture {
  url: string;
  name: string;
  picture: boolean;
}

export type NotePart = { text: string } | { code: string };

export interface Note {
  icon?: string;
  head?: NotePart[];
  id?: string | null;
  body?: string;
}

export type CallState = "running" | "ok" | "failed" | "unanswered";

export interface Call {
  name: string;
  params?: [string, string][];
  state: CallState;
  result?: string | null;
  anchor?: string | null;
}

export interface CallRun {
  id: string;
  count: number;
  tools?: string[];
  preview?: string;
  state: CallState;
  calls: Call[] | null;
  open?: boolean;
}

export type TranscriptPart =
  | { kind: "said"; role: "user" | "agent"; body?: string; images?: Picture[]; from?: string | null; by?: string | null }
  | { kind: "did"; run: CallRun }
  | { kind: "note"; note: Note };

export type TranscriptEntry = TranscriptPart & { key: string; id?: string };

type TranscriptNode = AdiMessage | AdiToolRun | AdiNote;

interface KeptEntry {
  node: TranscriptNode;
  sig: string | null;
  kind: TranscriptEntry["kind"];
}

function sourceLine(role: "user" | "agent", from: string, by: string): string {
  if (by) return by;
  if (!from) return "";
  return role === "user" ? `You · ${from}` : from;
}

class AdiMessage extends AdiElement {
  static observedAttributes = ["role", "from", "by", "queued", "asap", "removable"];

  #body = "";
  #images: Picture[] = [];
  #drawn: string | null = null;

  get body(): string {
    return this.#body;
  }

  set body(value: string | null | undefined) {
    this.#body = String(value ?? "");
    this.#draw();
  }

  get images(): Picture[] {
    return this.#images;
  }

  set images(value: Picture[] | null | undefined) {
    this.#images = Array.isArray(value) ? value : [];
    this.#draw();
  }

  override template(): string {
    return `
      <div class="box" part="box">
        <div class="head" part="head">
          <span class="source"></span>
          <button class="unqueue" type="button" title="Don't send this after all" hidden>
            <adi-icon name="x" size="14" label="Remove from queue"></adi-icon>
          </button>
        </div>
        <div class="pictures"></div>
        <div class="md" part="body"></div>
      </div>
    `;
  }

  override setup(): void {
    this.must<HTMLButtonElement>(".unqueue").addEventListener("click", () => this.emit("unqueue"));
  }

  override update(): void {
    const role = this.pick("role", ["agent", "user"]);
    const queued = this.hasAttribute("queued");
    let head = sourceLine(role, this.attr("from"), this.attr("by"));
    if (queued) {
      const by = this.attr("by");
      const from = this.attr("from");
      head = by ? `${by} · queued` : from ? `You · queued · ${from}` : "You · queued";
      if (this.hasAttribute("asap")) head += " · asap";
    }
    this.must(".source").textContent = head;
    const removable = queued && this.hasAttribute("removable");
    this.must<HTMLButtonElement>(".unqueue").hidden = !removable;
    this.must(".head").hidden = !head && !removable;
    this.#draw();
  }

  #draw(): void {
    if (!this.shadowRoot.firstElementChild) return;
    const sig = JSON.stringify([this.#body, this.#images]);
    if (sig === this.#drawn) return;
    this.#drawn = sig;
    this.must(".md").replaceChildren(this.#body.trim() ? renderMarkdown(this.#body) : "");
    this.must(".pictures").replaceChildren(...this.#images.map(attachment));
  }
}

function attachment({ url, name, picture }: Picture): HTMLAnchorElement {
  const a = document.createElement("a");
  a.href = url;
  a.target = "_blank";
  a.rel = "noreferrer";
  if (picture) {
    a.className = "picture";
    a.title = `${name} — open full size`;
    const img = document.createElement("img");
    img.src = url;
    img.alt = name;
    img.loading = "lazy";
    a.append(img);
  } else {
    a.className = "file";
    a.title = `${name} — open this file`;
    a.innerHTML = `<adi-icon name="paperclip" size="14" label="Attached file"></adi-icon><span>${esc(name)}</span>`;
  }
  return a;
}

class AdiNote extends AdiElement {

  #note: Note | null = null;

  get note(): Note | null {
    return this.#note;
  }

  set note(value: Note | null) {
    this.#note = value;
    this.update();
  }

  override template(): string {
    return `<div class="head"></div><div class="md"></div>`;
  }

  override update(): void {
    const note = this.#note;
    if (!note || !this.shadowRoot.firstElementChild) return;
    const head = this.must(".head");
    head.innerHTML = `<adi-icon name="${esc(note.icon ?? "info")}" size="14"></adi-icon>`;
    for (const word of note.head ?? []) {
      const span = document.createElement("span");
      if ("code" in word) {
        span.className = "code";
        span.textContent = word.code;
      } else {
        span.textContent = word.text;
      }
      head.append(span);
    }
    if (note.id) {
      const id = document.createElement("span");
      id.className = "id";
      id.textContent = note.id;
      head.append(id);
    }
    this.must(".md").replaceChildren(note.body?.trim() ? renderMarkdown(note.body) : "");
  }
}

const RUN_NOTE: Partial<Record<CallState, [string, string]>> = {
  running: ["running", "var(--accent)"],
  unanswered: ["no result", "var(--warn)"],
  failed: ["failed", "var(--err)"],
};

const CALL_DOT: Record<CallState, string> = {
  running: "var(--accent)",
  ok: "var(--ink-3)",
  failed: "var(--err)",
  unanswered: "var(--warn)",
};

export function toolsLine(tools: string[]): string {
  return tools.length <= 3 ? tools.join(", ") : `${tools.slice(0, 3).join(", ")}, …`;
}

class AdiToolRun extends AdiElement {

  #run: CallRun | null = null;
  #open = false;
  #drawn: string | null = null;

  get run(): CallRun | null {
    return this.#run;
  }

  set run(value: CallRun | null) {
    if (!this.#run && value?.open) this.#open = true;
    this.#run = value;
    this.update();
  }

  get open(): boolean {
    return this.#open;
  }

  override template(): string {
    return `
      <button class="head" type="button" aria-expanded="false">
        <adi-icon class="chevron" name="chevron-right" size="14"></adi-icon>
        <span class="what"></span>
        <span class="preview"></span>
        <span class="note"></span>
      </button>
      <div class="body" hidden></div>
    `;
  }

  override setup(): void {
    // Keep open state on the element so polling cannot collapse an expanded run.
    this.must<HTMLButtonElement>(".head").addEventListener("click", () => {
      this.#open = !this.#open;
      this.update();
      this.emit("toggle", { id: this.#run?.id, open: this.#open });
    });
  }

  override update(): void {
    const run = this.#run;
    if (!run || !this.shadowRoot.firstElementChild) return;
    this.toggleAttribute("open", this.#open);
    this.must<HTMLButtonElement>(".head").setAttribute("aria-expanded", String(this.#open));
    const count = run.count === 1 ? "1 call" : `${run.count} calls`;
    this.must(".what").textContent = `${count} · ${toolsLine(run.tools ?? [])}`;
    this.must(".preview").textContent = run.preview ?? "";
    const note = RUN_NOTE[run.state];
    this.must(".note").innerHTML = note
      ? `<span class="dot" style="background:${note[1]}" aria-hidden="true"></span>${note[0]}`
      : "";
    const body = this.must(".body");
    body.hidden = !this.#open;
    if (!this.#open) return;
    const sig = JSON.stringify(run.calls);
    if (sig === this.#drawn) return;
    this.#drawn = sig;
    if (!run.calls) {
      body.innerHTML = `<div class="fetching">Fetching these calls…</div>`;
      return;
    }
    body.replaceChildren(...run.calls.map(callBlock));
  }
}

/** Render model arguments as invoke/parameter markup with verbatim values. */
function callBlock(call: Call): HTMLDivElement {
  const box = document.createElement("div");
  box.className = "call";
  if (call.anchor) box.id = call.anchor;
  const params = (call.params ?? [])
    .map(([k, v]) => `<span class="tag">  &lt;parameter name="${esc(k)}"&gt;</span>${esc(v)}<span class="tag">&lt;/parameter&gt;\n</span>`)
    .join("");
  box.innerHTML = `
    <div class="call-head">
      <span class="dot" style="background:${CALL_DOT[call.state] ?? CALL_DOT.ok}" aria-hidden="true"></span>
      <span>${esc(call.name)}</span>
    </div>
    <pre><span class="tag">&lt;invoke name="${esc(call.name)}"&gt;\n</span>${params}<span class="tag">&lt;/invoke&gt;</span></pre>
  `;
  if (call.result) {
    const pre = document.createElement("pre");
    pre.className = "result";
    pre.innerHTML = `<span class="tag">&lt;result&gt;\n</span>${esc(call.result)}<span class="tag">\n&lt;/result&gt;</span>`;
    box.append(pre);
  }
  return box;
}

const TAG = { said: "adi-message", did: "adi-tool-run", note: "adi-note" } as const;

function fill(node: TranscriptNode, entry: TranscriptEntry): void {
  switch (entry.kind) {
    case "said":
      if (!(node instanceof AdiMessage)) return;
      node.setAttribute("role", entry.role === "user" ? "user" : "agent");
      setAttr(node, "from", entry.from);
      setAttr(node, "by", entry.by);
      node.images = entry.images ?? [];
      node.body = entry.body ?? "";
      break;
    case "did":
      if (!(node instanceof AdiToolRun)) return;
      node.run = entry.run;
      break;
    case "note":
      if (!(node instanceof AdiNote)) return;
      node.note = entry.note;
      break;
  }
}

function setAttr(node: HTMLElement, name: string, value: string | null | undefined): void {
  if (value) node.setAttribute(name, value);
  else node.removeAttribute(name);
}

class AdiTranscript extends AdiElement {

  #entries: TranscriptEntry[] = [];
  #kept = new Map<string, KeptEntry>();

  /** The transcript, oldest first — the order a store keeps it. The element flips it. */
  get entries(): TranscriptEntry[] {
    return this.#entries;
  }

  set entries(value: TranscriptEntry[] | null | undefined) {
    this.#entries = Array.isArray(value) ? value : [];
    this.update();
  }

  override template(): string {
    return `<slot name="lead"></slot><div class="entries"></div><slot name="foot"></slot>`;
  }

  override update(): void {
    const box = this.$(".entries");
    if (!box) return;
    const seen = new Set<string>();
    let cursor = box.firstChild;
    for (let i = this.#entries.length - 1; i >= 0; i -= 1) {
      const entry = this.#entries[i];
      seen.add(entry.key);
      const sig = JSON.stringify(entry);
      let kept = this.#kept.get(entry.key);
      // Replace the node if a live entry changes kind while retaining its key.
      if (kept && kept.kind !== entry.kind) {
        if (kept.node === cursor) cursor = kept.node.nextSibling;
        kept.node.remove();
        kept = undefined;
      }
      if (!kept) {
        kept = { node: document.createElement(TAG[entry.kind] ?? "adi-message"), sig: null, kind: entry.kind };
        this.#kept.set(entry.key, kept);
      }
      kept.node.id = entry.id ?? entry.key;
      if (kept.sig !== sig) {
        fill(kept.node, entry);
        kept.sig = sig;
      }
      if (kept.node !== cursor) box.insertBefore(kept.node, cursor);
      else cursor = kept.node.nextSibling;
    }
    for (const [key, kept] of this.#kept) {
      if (!seen.has(key)) {
        kept.node.remove();
        this.#kept.delete(key);
      }
    }
  }
}

define("adi-message", AdiMessage);
define("adi-note", AdiNote);
define("adi-tool-run", AdiToolRun);
define("adi-transcript", AdiTranscript);

export { AdiMessage, AdiNote, AdiToolRun, AdiTranscript };

declare global {
  interface HTMLElementTagNameMap {
    "adi-message": AdiMessage;
    "adi-note": AdiNote;
    "adi-tool-run": AdiToolRun;
    "adi-transcript": AdiTranscript;
  }
}
