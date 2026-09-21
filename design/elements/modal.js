// `<adi-modal>` — a dialog, which §2.5 counts as one of the few genuinely detachable things and
// so lets be a card: `--line` hairline, `--r-lg`, on `--bg` over the `--scrim`.
//
//   <adi-modal label="Reserve a port">
//     <adi-field label="Port"><adi-input num></adi-input></adi-field>
//     <adi-button slot="foot" variant="quiet" onclick="this.closest('adi-modal').close()">Cancel</adi-button>
//     <adi-button slot="foot" variant="primary">Reserve</adi-button>
//   </adi-modal>
//
// It is a native `<dialog>` underneath, which is worth more than it sounds: the top layer (so no
// z-index argument with anything on the page), focus trapped inside it, Escape closing it, and
// the backdrop drawn by the browser. All this element adds is the frame and the events —
// `open` and `close`, raised on the element itself.

import { AdiElement, define, sheet } from "./base.js";
import "./icon.js";

class AdiModal extends AdiElement {
  static observedAttributes = ["label", "open"];

  static sheet = sheet(`
    :host { display: contents; }
    dialog {
      width: min(560px, calc(100vw - 32px));
      max-height: 80vh;
      padding: 0;
      border: 1px solid var(--line);
      border-radius: var(--r-lg);
      background: var(--bg);
      color: var(--ink);
      font-family: var(--sans);
      font-size: var(--fs-ui-sm);
    }
    dialog::backdrop { background: var(--scrim); }
    :host([wide]) dialog { width: min(880px, calc(100vw - 32px)); }

    .head {
      display: flex;
      align-items: center;
      gap: 12px;
      padding: 10px 12px 10px 16px;
      border-bottom: 1px solid var(--line);
    }
    h2 { flex: 1; margin: 0; font-size: var(--fs-section); font-weight: 600; }
    .close {
      display: inline-grid;
      place-items: center;
      width: 28px;
      height: 28px;
      border-radius: var(--r);
      color: var(--ink-3);
      transition: background var(--transition), color var(--transition);
    }
    .close:hover { background: var(--bg-hover); color: var(--ink); }

    .body { padding: 16px; overflow: auto; }
    .foot {
      display: flex;
      justify-content: flex-end;
      gap: 8px;
      padding: 12px 16px;
      border-top: 1px solid var(--line);
    }
    .foot[hidden] { display: none; }
  `);

  template() {
    return `
      <dialog part="dialog">
        <div class="head" part="head">
          <h2 part="title"></h2>
          <button class="close" part="close" type="button" aria-label="Close">
            <adi-icon name="x" size="16"></adi-icon>
          </button>
        </div>
        <div class="body" part="body"><slot></slot></div>
        <div class="foot" part="foot"><slot name="foot"></slot></div>
      </dialog>
    `;
  }

  setup() {
    this.$(".close").addEventListener("click", () => this.close());
    // Escape and the backdrop go through the dialog's own close, not through ours — so the
    // attribute is kept in step here rather than in three places.
    this.$("dialog").addEventListener("close", () => {
      this.removeAttribute("open");
      this.emit("close");
    });
    this.$('slot[name="foot"]').addEventListener("slotchange", () => this.update());
  }

  update() {
    this.$("h2").textContent = this.attr("label");
    // A footer with nothing in it is still a hairline across the dialog, so it is the assigned
    // nodes that decide — `:has()` cannot see past the slot element itself.
    this.$(".foot").hidden = this.$('slot[name="foot"]').assignedNodes().length === 0;
    const dialog = this.$("dialog");
    const open = this.hasAttribute("open");
    if (open && !dialog.open) {
      dialog.showModal();
      this.emit("open");
    } else if (!open && dialog.open) {
      dialog.close();
    }
  }

  /** Open it. The attribute is the state; this is the verb. */
  show() {
    this.setAttribute("open", "");
  }

  close() {
    this.removeAttribute("open");
  }
}

define("adi-modal", AdiModal);

export { AdiModal };
