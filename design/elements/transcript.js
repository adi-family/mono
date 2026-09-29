// The transcript — the JavaScript twin of `adi_ui::Chat` (crates/adi-ui/src/chat.rs), and the
// parts it is drawn from:
//
//   <adi-transcript>   the feed: newest first, keyed, cheap at any length
//   <adi-message>      one thing said — yours on the raised surface, the agent's as plain prose,
//                      or a queued message, hollowed out
//   <adi-tool-run>     a run of tool calls: one receipt line, opened on demand (§6)
//   <adi-note>         something the platform said into the conversation — a wake, a settled ask
//
//   const t = document.createElement("adi-transcript");
//   t.entries = [
//     { key: "t0", kind: "said", role: "user", body: "Why is the build red?" },
//     { key: "t1", kind: "did", run: { id: "r1", count: 3, tools: ["Bash"], preview: "cargo build",
//       state: "ok", calls: null } },
//     { key: "t1-1", kind: "said", role: "agent", body: "A missing feature flag. **Fixed.**" },
//   ];
//
// # Newest first
//
// The reason is in chat.rs and holds here unchanged: an agent's run is long, mostly tool calls,
// and you come back to it to find out what just happened. The answer is where the eye already is,
// there is no scroll anchoring to fight, and a message landing while you read pushes content
// *away* from you.
//
// # Keyed, and why that is simpler here than in Leptos
//
// Entries arrive oldest first carrying a `key`; the element keeps one child per key and never
// rebuilds one whose content is unchanged — a poll that lands the next turn touches no settled
// turn's DOM, so an open run stays open and a remembered height stays true. Where the Rust side
// has to split the transcript into a keyed "settled" list and an unkeyed "live" turn (a keyed
// `<For>` never redraws a key it has seen), a custom element can simply be told its content
// changed: an entry whose signature moved is updated *in place*, the same element, with its
// open state and scroll intact.
//
// Every entry is `content-visibility: auto` with an intrinsic height, which is the whole of the
// virtualisation: the engine skips what is off screen, and find-in-page, anchors and screen
// readers still see everything.

import { AdiElement, define, esc, sheet } from "./base.js";
import { MARKDOWN_SHEET, renderMarkdown } from "./markdown.js";
import "./icon.js";

// On every top-level entry, and every half is load-bearing — see `LAZY` in chat.rs: `flex: none`
// because an entry in a column with a height gets squeezed otherwise (an opened run came out
// 343px around 607px of content), `content-visibility` plus an intrinsic size for the skip, and
// the transcript's 80ch measure (§4).
const LAZY = `
  :host {
    display: block;
    flex: none;
    max-width: 80ch;
    content-visibility: auto;
    contain-intrinsic-size: auto 120px;
  }
`;

/** Where the words `from`/`by` put above a message: see `Said` in chat.rs for the rule. */
function sourceLine(role, from, by) {
  if (by) return by;
  if (!from) return "";
  return role === "user" ? `You · ${from}` : from;
}

// ---- <adi-message> ---------------------------------------------------------------------------

class AdiMessage extends AdiElement {
  static observedAttributes = ["role", "from", "by", "queued", "asap", "removable"];

  static sheet = sheet(`
    ${LAZY}
    :host([role="user"]) .box { padding: 12px 16px; border-radius: var(--r-lg); background: var(--bg-raise); }
    /* Queued: the user's block, emptied — a dashed hairline, no fill, dimmed. Intent, not history. */
    :host([queued]) .box { padding: 12px 16px; border: 1px dashed var(--line-strong);
      border-radius: var(--r-lg); background: none; }
    :host([queued]) .md { color: var(--ink-3); }
    .head { display: flex; align-items: center; gap: 4px; margin-bottom: 4px;
      font-size: var(--fs-label); color: var(--ink-3); }
    .head[hidden] { display: none; }
    .unqueue { margin: -2px 0 -2px auto; display: grid; place-items: center; width: 24px; height: 24px;
      border-radius: var(--r); color: var(--ink-3); transition: background var(--transition), color var(--transition); }
    .unqueue:hover { background: var(--bg-hover); color: var(--ink); }
    .unqueue[hidden] { display: none; }
    .pictures { display: flex; flex-wrap: wrap; align-items: flex-start; gap: 8px; }
    .pictures:empty { display: none; }
    .pictures:not(:empty):has(+ .md:not(:empty)) { margin-bottom: 12px; }
    .picture { display: block; max-width: 100%; overflow: hidden; border: 1px solid var(--line);
      border-radius: var(--r); }
    .picture img { display: block; max-width: 100%; max-height: 256px; object-fit: contain; }
    .file { display: flex; align-items: center; gap: 8px; max-width: 100%; padding: 8px 12px;
      border: 1px solid var(--line); border-radius: var(--r); background: var(--bg-raise);
      font-size: var(--fs-small); color: var(--ink-2); }
    .file:hover { color: var(--ink); }
    .file span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  `);

  #body = "";
  #images = [];
  #drawn = null;

  /** Markdown. */
  get body() {
    return this.#body;
  }

  set body(value) {
    this.#body = String(value ?? "");
    this.#draw();
  }

  /** `[{ url, name, picture }]` — a picture is drawn, anything else is a named link. */
  get images() {
    return this.#images;
  }

  set images(value) {
    this.#images = Array.isArray(value) ? value : [];
    this.#draw();
  }

  template() {
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

  setup() {
    this.shadowRoot.adoptedStyleSheets = [MARKDOWN_SHEET, ...this.shadowRoot.adoptedStyleSheets];
    this.$(".unqueue").addEventListener("click", () => this.emit("unqueue"));
  }

  update() {
    const role = this.pick("role", ["agent", "user"]);
    const queued = this.hasAttribute("queued");
    let head = sourceLine(role, this.attr("from"), this.attr("by"));
    if (queued) {
      const by = this.attr("by");
      const from = this.attr("from");
      head = by ? `${by} · queued` : from ? `You · queued · ${from}` : "You · queued";
      if (this.hasAttribute("asap")) head += " · asap";
    }
    this.$(".source").textContent = head;
    const removable = queued && this.hasAttribute("removable");
    this.$(".unqueue").hidden = !removable;
    this.$(".head").hidden = !head && !removable;
    this.#draw();
  }

  #draw() {
    if (!this.shadowRoot.firstElementChild) return;
    const sig = JSON.stringify([this.#body, this.#images]);
    if (sig === this.#drawn) return;
    this.#drawn = sig;
    this.$(".md").replaceChildren(this.#body.trim() ? renderMarkdown(this.#body) : "");
    this.$(".pictures").replaceChildren(...this.#images.map(attachment));
  }
}

/** One attachment above a message: a capped picture that opens full size, or a named link. */
function attachment({ url, name, picture }) {
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

// ---- <adi-note> ------------------------------------------------------------------------------

class AdiNote extends AdiElement {
  // A third speaker, drawn as neither of the other two: the Ask block's shape (§6) — a 2px rule
  // and 18px of padding — with a 12px header and a body a step quieter than the agent's words.
  static sheet = sheet(`
    ${LAZY}
    :host { padding: 4px 0 4px 18px; border-left: 2px solid var(--line-strong); }
    .head { display: flex; align-items: center; gap: 6px; font-size: var(--fs-label); color: var(--ink-3); }
    .code, .id { font-family: var(--mono); color: var(--code); }
    .id { margin-left: auto; padding-left: 12px; flex: none; }
    .md { margin-top: 6px; font-size: var(--fs-small); color: var(--ink-2); }
    .md:empty { display: none; }
  `);

  #note = null;

  /** `{ icon, head: [{ text } | { code }], id, body }`. */
  get note() {
    return this.#note;
  }

  set note(value) {
    this.#note = value;
    this.update();
  }

  template() {
    return `<div class="head"></div><div class="md"></div>`;
  }

  setup() {
    this.shadowRoot.adoptedStyleSheets = [MARKDOWN_SHEET, ...this.shadowRoot.adoptedStyleSheets];
  }

  update() {
    const note = this.#note;
    if (!note || !this.shadowRoot.firstElementChild) return;
    const head = this.$(".head");
    head.innerHTML = `<adi-icon name="${esc(note.icon ?? "info")}" size="14"></adi-icon>`;
    for (const word of note.head ?? []) {
      const span = document.createElement("span");
      if (word.code !== undefined) {
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
    this.$(".md").replaceChildren(note.body?.trim() ? renderMarkdown(note.body) : "");
  }
}

// ---- <adi-tool-run> --------------------------------------------------------------------------

/** The word and dot a closed run says about its head call. "running" must be true right now. */
const RUN_NOTE = {
  running: ["running", "var(--accent)"],
  unanswered: ["no result", "var(--warn)"],
  failed: ["failed", "var(--err)"],
};

const CALL_DOT = {
  running: "var(--accent)",
  ok: "var(--ink-3)",
  failed: "var(--err)",
  unanswered: "var(--warn)",
};

/** "Read, Grep, Edit" — every tool when there are three or fewer, the first three and … after. */
export function toolsLine(tools) {
  return tools.length <= 3 ? tools.join(", ") : `${tools.slice(0, 3).join(", ")}, …`;
}

class AdiToolRun extends AdiElement {
  static sheet = sheet(`
    ${LAZY}
    :host { border: 1px solid var(--line); border-radius: var(--r-lg); color: var(--ink-3); }
    .head { display: flex; align-items: center; gap: 10px; width: 100%; padding: 8px 12px;
      border-radius: var(--r-lg); font-size: var(--fs-small); text-align: left; user-select: none;
      transition: background var(--transition), color var(--transition); }
    .head:hover { background: var(--bg-hover); color: var(--ink-2); }
    .chevron { flex: none; transition: transform var(--transition); }
    :host([open]) .chevron { transform: rotate(90deg); }
    .what { flex: none; font-weight: 500; color: var(--ink-2); }
    .preview { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
      font-family: var(--mono); font-size: var(--fs-label); }
    .note { margin-left: auto; flex: none; display: flex; align-items: center; gap: 6px;
      font-size: var(--fs-label); }
    .note:empty { display: none; }
    .dot { width: 6px; height: 6px; border-radius: 50%; flex: none; }
    .body { display: flex; flex-direction: column; gap: 12px; padding: 12px; border-top: 1px solid var(--line); }
    .body[hidden] { display: none; }
    .fetching { font-size: var(--fs-small); color: var(--ink-3); }
    .call { display: flex; flex-direction: column; gap: 6px; }
    .call-head { display: flex; align-items: center; gap: 8px; font-size: var(--fs-small);
      font-weight: 500; color: var(--ink-2); }
    pre { margin: 0; flex: none; overflow-x: auto; padding: 12px 14px; border: 1px solid var(--line);
      border-radius: var(--r-lg); background: var(--bg-raise); font-family: var(--mono);
      font-size: var(--fs-mono); line-height: 1.6; white-space: pre-wrap; word-break: break-word;
      color: var(--code); }
    pre.result { color: var(--ink-2); }
    .tag { color: var(--ink-3); }
  `);

  #run = null;
  #open = false;
  #drawn = null;

  /**
   * `{ id, count, tools, preview, state, calls }` — `state` is `ok | running | failed |
   * unanswered`, `calls` is `null` until they are here (a folded run), then
   * `[{ name, params: [[key, value]], state, result, anchor }]`.
   */
  get run() {
    return this.#run;
  }

  set run(value) {
    if (!this.#run && value?.open) this.#open = true;
    this.#run = value;
    this.update();
  }

  get open() {
    return this.#open;
  }

  template() {
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

  setup() {
    // Open is the element's own, not the data's: it is the same element for as long as the run is
    // on screen, so a poll that brings its calls cannot snap it shut in the reader's face.
    this.$(".head").addEventListener("click", () => {
      this.#open = !this.#open;
      this.update();
      this.emit("toggle", { id: this.#run?.id, open: this.#open });
    });
  }

  update() {
    const run = this.#run;
    if (!run || !this.shadowRoot.firstElementChild) return;
    this.toggleAttribute("open", this.#open);
    this.$(".head").setAttribute("aria-expanded", String(this.#open));
    const count = run.count === 1 ? "1 call" : `${run.count} calls`;
    this.$(".what").textContent = `${count} · ${toolsLine(run.tools ?? [])}`;
    this.$(".preview").textContent = run.preview ?? "";
    const note = RUN_NOTE[run.state];
    this.$(".note").innerHTML = note
      ? `<span class="dot" style="background:${note[1]}" aria-hidden="true"></span>${note[0]}`
      : "";
    const body = this.$(".body");
    body.hidden = !this.#open;
    if (!this.#open) return;
    // The calls are drawn only once opened, and redrawn only when they changed.
    const sig = JSON.stringify(run.calls);
    if (sig === this.#drawn) return;
    this.#drawn = sig;
    if (!run.calls) {
      // A sentence rather than a spinner: it is one small read, and motion the reader did not ask
      // for is §8's.
      body.innerHTML = `<div class="fetching">Fetching these calls…</div>`;
      return;
    }
    body.replaceChildren(...run.calls.map(callBlock));
  }
}

/**
 * One call, the way the model wrote it: an `<invoke>` with a `<parameter>` per argument, values
 * verbatim. Not JSON — see `Invoke` in chat.rs for why the wire format of some transport would be
 * teaching the reader a shape the model never saw.
 */
function callBlock(call) {
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

// ---- <adi-transcript> ------------------------------------------------------------------------

const TAG = { said: "adi-message", did: "adi-tool-run", note: "adi-note" };

/** Put one entry's data onto its element. */
function fill(node, entry) {
  switch (entry.kind) {
    case "said":
      node.setAttribute("role", entry.role === "user" ? "user" : "agent");
      setAttr(node, "from", entry.from);
      setAttr(node, "by", entry.by);
      node.images = entry.images ?? [];
      node.body = entry.body ?? "";
      break;
    case "did":
      node.run = entry.run;
      break;
    case "note":
      node.note = entry.note;
      break;
  }
}

function setAttr(node, name, value) {
  if (value) node.setAttribute(name, value);
  else node.removeAttribute(name);
}

class AdiTranscript extends AdiElement {
  static sheet = sheet(`
    :host { display: flex; flex-direction: column; gap: var(--s4); overflow-y: auto; background: var(--bg); }
    ::slotted([slot="lead"]), ::slotted([slot="foot"]) { flex: none; max-width: 80ch; }
    .entries { display: contents; }
  `);

  #entries = [];
  /** key → { node, sig, kind } */
  #kept = new Map();

  /** The transcript, oldest first — the order a store keeps it. The element flips it. */
  get entries() {
    return this.#entries;
  }

  set entries(value) {
    this.#entries = Array.isArray(value) ? value : [];
    this.update();
  }

  template() {
    return `<slot name="lead"></slot><div class="entries"></div><slot name="foot"></slot>`;
  }

  update() {
    const box = this.$(".entries");
    if (!box) return;
    const seen = new Set();
    let cursor = box.firstChild;
    for (let i = this.#entries.length - 1; i >= 0; i -= 1) {
      const entry = this.#entries[i];
      seen.add(entry.key);
      const sig = JSON.stringify(entry);
      let kept = this.#kept.get(entry.key);
      // A key whose kind changed is a different thing at the same place — a live turn's part that
      // was a run is now something said — so it gets a new element rather than a confused one.
      if (kept && kept.kind !== entry.kind) {
        if (kept.node === cursor) cursor = cursor.nextSibling;
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
      else cursor = cursor.nextSibling;
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
