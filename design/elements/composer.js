// `<adi-composer>` — the box you type into at the top of a chat: the JavaScript twin of
// `adi_ui::Composer` (crates/adi-ui/src/composer.rs), paperclip and tray included.
//
//   <adi-composer placeholder="Write to adi-agent…" attach stoppable asap>
//     <adi-mic slot="tools"></adi-mic>
//   </adi-composer>
//   composer.addEventListener("send", (e) => post(e.detail.text).then(() => (composer.value = "")));
//
// The three behaviours a bare textarea lacks:
//
// - **Enter sends, Shift+Enter breaks the line.** Not while an IME is composing — there Enter
//   accepts the candidate word, and the event alone does not say which it means.
// - **It grows with the message**, to a ceiling, then scrolls.
// - **It says whether it can send.** Nothing to send, `busy`, or an attachment still uploading
//   (a send now would arrive without it): the button is out.
//
// Sending is the caller's and so is clearing: a send that fails should not have thrown the message
// away. So is storing a file — with `attach` set the box takes files pasted, dropped or picked
// through the paperclip and hands them over as a `files` event; the caller uploads them and shows
// where they got to by setting `attachments` (`[{ key, name, preview, image, state }]`, `state`
// `uploading | ready | failed`). Without `attach`, `refusal` is the line said instead of offering one.
//
// Events: `send` / `asap` `{ text }` (asap: send now, overtaking the queue — shown only while
// `stoppable`), `stop`, `files` `{ files }`, `unattach` `{ key }`.
//
// The send button is the chat screen's one filled orange (§6). Stop is a quiet square that turns
// red only under the cursor. The `tools` slot sits furthest from Send — a microphone is pressed
// while composing, not when finishing, and one under the thumb that goes for Send gets pressed.

import { AdiElement, define, esc, sheet } from "./base.js";
import "./icon.js";

/** The tallest the box grows before it scrolls, in px — a property of the screen, not the type. */
const MAX_HEIGHT = 200;

class AdiComposer extends AdiElement {
  static observedAttributes = ["placeholder", "busy", "stoppable", "asap", "attach", "refusal"];

  static sheet = sheet(`
    :host { display: block; }
    .box { display: flex; flex-direction: column; gap: 4px; padding: 10px 10px 10px 16px;
      border: 1px solid var(--line-strong); border-radius: var(--r-lg); background: var(--bg-raise);
      transition: border-color var(--transition); }
    .box:focus-within { border-color: var(--ink-3); }
    .row { display: flex; align-items: flex-end; gap: 6px; }
    textarea { flex: 1; align-self: center; width: 100%; min-height: 40px; padding: 8px 4px 8px 0;
      resize: none; border: 0; outline: none; background: transparent; color: var(--ink);
      font: inherit; font-size: var(--fs-body); line-height: 1.5; }
    textarea::placeholder { color: var(--ink-3); }
    textarea:focus-visible { outline: none; }
    .btn { flex: none; display: grid; place-items: center; width: 32px; height: 32px;
      border-radius: var(--r); color: var(--ink-2); transition: background var(--transition), color var(--transition); }
    .btn:hover { background: var(--bg-hover); color: var(--ink); }
    .btn[hidden] { display: none; }
    .stop:hover { color: var(--err); }
    .send { background: var(--accent); color: var(--on-accent); }
    .send:hover { background: var(--accent-hover); color: var(--on-accent); }
    .btn:disabled { opacity: .4; cursor: not-allowed; }
    ::slotted([slot="tools"]) { flex: none; }
    .tray { display: flex; gap: 8px; overflow-x: auto; padding: 0 4px 8px; }
    .tray:empty { display: none; }
    .thumb { position: relative; flex: none; width: 64px; height: 64px; overflow: hidden;
      border: 1px solid var(--line); border-radius: var(--r); background: var(--bg-raise); }
    .thumb.failed { border-color: var(--err); }
    .thumb img { width: 100%; height: 100%; object-fit: cover; }
    .thumb.uploading img, .thumb.uploading .file { opacity: .5; }
    .file { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 4px;
      width: 100%; height: 100%; padding: 0 4px; text-align: center; color: var(--ink-2);
      font-size: 10px; line-height: 1.2; word-break: break-all; }
    .over { position: absolute; inset: 0; display: grid; place-items: center; font-size: var(--fs-label);
      color: var(--ink-3); }
    .failed .over { color: var(--err); background: color-mix(in srgb, var(--bg-raise) 80%, transparent); }
    .remove { position: absolute; top: 2px; right: 2px; display: grid; place-items: center; width: 20px;
      height: 20px; border-radius: var(--r); color: var(--ink-2);
      background: color-mix(in srgb, var(--bg-raise) 90%, transparent); }
    .remove:hover { color: var(--ink); }
    .line { font-size: var(--fs-label); color: var(--ink-3); }
    .line:empty { display: none; }
    input[type=file] { display: none; }
    @media (max-width: 620px) { textarea { font-size: 16px; } }
  `);

  #attachments = [];

  template() {
    return `
      <div class="box" part="box" title="Enter sends · Shift-Enter for a new line">
        <div class="tray" role="list"></div>
        <div class="row">
          <textarea rows="1" part="input"></textarea>
          <input type="file" multiple>
          <button class="btn clip" type="button" title="Attach a file — or paste one, or drop it here" hidden>
            <adi-icon name="paperclip" size="16" label="Attach a file"></adi-icon>
          </button>
          <slot name="tools"></slot>
          <button class="btn stop" type="button" title="Stop" hidden>
            <adi-icon name="square" size="16" label="Stop"></adi-icon>
          </button>
          <button class="btn asap" type="button" title="Send now — overtakes what is already waiting" hidden>
            <adi-icon name="zap" size="16" label="Send asap"></adi-icon>
          </button>
          <button class="btn send" type="button" title="Send" disabled>
            <adi-icon name="arrow-up" size="16" label="Send"></adi-icon>
          </button>
        </div>
        <div class="line"></div>
      </div>
    `;
  }

  setup() {
    const area = this.$("textarea");
    area.addEventListener("input", () => {
      this.#fit();
      this.update();
    });
    area.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter" && !ev.shiftKey && !ev.altKey && !ev.ctrlKey && !ev.metaKey && !ev.isComposing) {
        ev.preventDefault();
        this.#send("send");
      }
    });
    // A screenshot on the clipboard. A text paste carries no files and falls through to the
    // browser, which is what puts the words in the box.
    area.addEventListener("paste", (ev) => {
      const files = [...(ev.clipboardData?.files ?? [])];
      if (files.length && this.hasAttribute("attach")) {
        ev.preventDefault();
        this.emit("files", { files });
      }
    });
    const box = this.$(".box");
    // `dragover` has to be cancelled or the browser navigates to the dropped file instead — the
    // one default here whose absence loses the page you are typing on.
    box.addEventListener("dragover", (ev) => {
      if (this.hasAttribute("attach")) ev.preventDefault();
    });
    box.addEventListener("drop", (ev) => {
      const files = [...(ev.dataTransfer?.files ?? [])];
      if (files.length && this.hasAttribute("attach")) {
        ev.preventDefault();
        this.emit("files", { files });
      }
    });
    const picker = this.$("input[type=file]");
    this.$(".clip").addEventListener("click", () => picker.click());
    picker.addEventListener("change", () => {
      const files = [...picker.files];
      picker.value = "";
      if (files.length) this.emit("files", { files });
    });
    this.$(".send").addEventListener("click", () => this.#send("send"));
    this.$(".asap").addEventListener("click", () => this.#send("asap"));
    this.$(".stop").addEventListener("click", () => this.emit("stop"));
  }

  /** What is typed. Setting it (to `""` after a send, say) resizes the box to match. */
  get value() {
    return this.$("textarea")?.value ?? "";
  }

  set value(text) {
    const area = this.$("textarea");
    if (!area) return;
    area.value = text ?? "";
    this.#fit();
    this.update();
  }

  /** What the message carries — see the header. */
  get attachments() {
    return this.#attachments;
  }

  set attachments(list) {
    this.#attachments = Array.isArray(list) ? list : [];
    this.#drawTray();
    this.update();
  }

  focus() {
    this.$("textarea")?.focus();
  }

  update() {
    const area = this.$("textarea");
    const busy = this.hasAttribute("busy");
    area.placeholder = this.attr("placeholder", "Write to the agent…");
    area.disabled = busy;
    const ready = this.#ready();
    this.$(".send").disabled = !ready;
    this.$(".asap").disabled = !ready;
    const stoppable = this.hasAttribute("stoppable");
    this.$(".stop").hidden = !stoppable;
    this.$(".asap").hidden = !(stoppable && this.hasAttribute("asap"));
    const attach = this.hasAttribute("attach");
    this.$(".clip").hidden = !attach;
    const uploading = this.#attachments.some((a) => a.state === "uploading");
    this.$(".line").textContent = uploading ? "attaching…" : !attach ? this.attr("refusal") : "";
  }

  #ready() {
    const typed = Boolean(this.value.trim());
    const attached = this.#attachments.some((a) => a.state === "ready");
    const uploading = this.#attachments.some((a) => a.state === "uploading");
    return (typed || attached) && !uploading && !this.hasAttribute("busy");
  }

  #drawTray() {
    const tray = this.$(".tray");
    if (!tray) return;
    tray.replaceChildren(
      ...this.#attachments.map((a) => {
        const thumb = document.createElement("div");
        thumb.className = `thumb ${a.state}`;
        thumb.setAttribute("role", "listitem");
        thumb.title = a.error ? `${a.name} — ${a.error}` : a.name;
        thumb.innerHTML = `
          ${a.image && a.preview ? `<img alt="${esc(a.name)}" src="${esc(a.preview)}">` : `<div class="file"><adi-icon name="paperclip" size="14" label="File"></adi-icon><span>${esc(a.name)}</span></div>`}
          ${a.state === "uploading" ? `<div class="over">…</div>` : a.state === "failed" ? `<div class="over">failed</div>` : ""}
          <button class="remove" type="button" title="Remove this attachment">
            <adi-icon name="x" size="14" label="Remove ${esc(a.name)}"></adi-icon>
          </button>`;
        thumb.querySelector(".remove").addEventListener("click", () => this.emit("unattach", { key: a.key }));
        return thumb;
      }),
    );
  }

  #send(kind) {
    if (!this.#ready()) return;
    this.emit(kind, { text: this.value });
  }

  // Reset, then measure: reading `scrollHeight` without the reset only ever grows. An empty box is
  // left at its one row — for an empty textarea the browser measures the *placeholder*, and a
  // sent-and-cleared composer would otherwise settle a row taller than an untouched one.
  #fit() {
    const area = this.$("textarea");
    area.style.height = "auto";
    if (!area.value) return;
    area.style.height = `${Math.min(area.scrollHeight, MAX_HEIGHT)}px`;
  }
}

define("adi-composer", AdiComposer);

export { AdiComposer };
