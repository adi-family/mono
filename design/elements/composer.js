// `<adi-composer>` — the box you type into at the top of a chat: the JavaScript twin of
// `adi_ui::Composer` (crates/adi-ui/src/composer.rs).
//
//   <adi-composer placeholder="Write to adi-agent…" stoppable></adi-composer>
//   composer.addEventListener("send", (e) => post(e.detail.text).then(() => (composer.value = "")));
//
// The three behaviours a bare textarea lacks:
//
// - **Enter sends, Shift+Enter breaks the line.** Not while an IME is composing — there Enter
//   accepts the candidate word, and the event alone does not say which it means.
// - **It grows with the message**, to a ceiling, then scrolls.
// - **It says whether it can send.** Empty or `busy`, the button is out.
//
// Sending is the caller's and so is clearing: a send that fails should not have thrown the message
// away. Events: `send` `{ text }`, `asap` `{ text }` (send now, overtaking the queue — shown only
// while `stoppable`), `stop`.
//
// The send button is the chat screen's one filled orange (§6). Stop is a quiet square that turns
// red only under the cursor.

import { AdiElement, define, sheet } from "./base.js";
import "./icon.js";

/** The tallest the box grows before it scrolls, in px — a property of the screen, not the type. */
const MAX_HEIGHT = 200;

class AdiComposer extends AdiElement {
  static observedAttributes = ["placeholder", "busy", "stoppable", "asap"];

  static sheet = sheet(`
    :host { display: block; }
    .box { display: flex; align-items: flex-end; gap: 6px; padding: 10px 10px 10px 16px;
      border: 1px solid var(--line-strong); border-radius: var(--r-lg); background: var(--bg-raise);
      transition: border-color var(--transition); }
    .box:focus-within { border-color: var(--ink-3); }
    textarea { flex: 1; align-self: center; width: 100%; min-height: 40px; padding: 8px 4px 8px 0;
      resize: none; border: 0; outline: none; background: transparent; color: var(--ink);
      font: inherit; font-size: var(--fs-body); line-height: 1.5; }
    textarea::placeholder { color: var(--ink-3); }
    textarea:focus-visible { outline: none; }
    .btn { flex: none; display: grid; place-items: center; width: 32px; height: 32px;
      border-radius: var(--r); color: var(--ink-2); transition: background var(--transition), color var(--transition); }
    .btn:hover { background: var(--bg-hover); }
    .btn[hidden] { display: none; }
    .stop:hover { color: var(--err); }
    .asap:hover { color: var(--ink); }
    .send { background: var(--accent); color: var(--on-accent); }
    .send:hover { background: var(--accent-hover); }
    .btn:disabled { opacity: .4; cursor: not-allowed; }
    @media (max-width: 620px) { textarea { font-size: 16px; } }
  `);

  template() {
    return `
      <div class="box" part="box" title="Enter sends · Shift-Enter for a new line">
        <textarea rows="1" part="input"></textarea>
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

  focus() {
    this.$("textarea")?.focus();
  }

  update() {
    const area = this.$("textarea");
    const busy = this.hasAttribute("busy");
    area.placeholder = this.attr("placeholder", "Write to the agent…");
    area.disabled = busy;
    const ready = Boolean(area.value.trim()) && !busy;
    this.$(".send").disabled = !ready;
    this.$(".asap").disabled = !ready;
    const stoppable = this.hasAttribute("stoppable");
    this.$(".stop").hidden = !stoppable;
    this.$(".asap").hidden = !(stoppable && this.hasAttribute("asap"));
  }

  #send(kind) {
    const text = this.value;
    if (!text.trim() || this.hasAttribute("busy")) return;
    this.emit(kind, { text });
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
