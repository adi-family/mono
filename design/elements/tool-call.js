// `<adi-tool-call>` — §6 "Tool call": the collapsed receipt of what an agent ran.
//
//   <adi-tool-call calls="6" tool="Bash" command="cargo build -p adi-webapp">
//     <adi-code>…the output…</adi-code>
//   </adi-tool-call>
//
// "It is a receipt, not a message" is the whole specification. The transcript is the product
// (§2.1), and six shell invocations rendered as six blocks is the transcript buried under its
// own plumbing — so this collapses to one line of `--ink-3` and opens only when asked.
//
// Radius is `--r-lg`: §6 says 8px here, but §5's ladder has no 8, and a value that is in no
// token is a value this library will not restate (§8).

import { AdiElement, define, sheet } from "./base.js";
import "./icon.js";

class AdiToolCall extends AdiElement {
  static observedAttributes = ["calls", "tool", "command", "open"];

  static sheet = sheet(`
    :host {
      display: block;
      border: 1px solid var(--line);
      border-radius: var(--r-lg);
      color: var(--ink-3);
      font-size: var(--fs-small);
      overflow: hidden;
    }
    .head {
      display: flex;
      align-items: center;
      gap: 8px;
      width: 100%;
      padding: 7px 10px;
      color: inherit;
      text-align: left;
      transition: background var(--transition);
    }
    .head:hover { background: var(--bg-hover); }
    .chevron { flex: none; transition: transform var(--transition); }
    :host([open]) .chevron { transform: rotate(90deg); }
    .what { flex: none; }
    .command {
      flex: 1;
      min-width: 0;
      font-family: var(--mono);
      font-size: var(--fs-mono);
      white-space: nowrap;
      overflow: hidden;
      text-overflow: ellipsis;
    }
    .body { padding: 0 10px 10px; }
    .body[hidden] { display: none; }
  `);

  template() {
    return `
      <button class="head" part="head" type="button" aria-expanded="false">
        <adi-icon class="chevron" name="chevron-right" size="14"></adi-icon>
        <span class="what" part="what"></span>
        <span class="command" part="command"></span>
      </button>
      <div class="body" part="body" hidden><slot></slot></div>
    `;
  }

  setup() {
    this.$(".head").addEventListener("click", () => {
      this.toggleAttribute("open");
      this.emit("toggle", { open: this.hasAttribute("open") });
    });
  }

  update() {
    const calls = Number(this.attr("calls", "1")) || 1;
    const tool = this.attr("tool");
    // "6 calls · Bash" — sans, because a count is not a machine string and a tool's name in a
    // list is not either (§2.3). Only the command it ran is mono.
    this.$(".what").textContent = [`${calls} call${calls === 1 ? "" : "s"}`, tool].filter(Boolean).join(" · ");
    this.$(".command").textContent = this.attr("command");

    const open = this.hasAttribute("open");
    this.$(".body").hidden = !open;
    this.$(".head").setAttribute("aria-expanded", String(open));
  }
}

define("adi-tool-call", AdiToolCall);

export { AdiToolCall };
