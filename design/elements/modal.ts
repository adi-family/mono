// Native dialog wrapper. Emits `open` and `close` on the host element.

import { AdiElement, define } from "./base.ts";
import "./icon.ts";

class AdiModal extends AdiElement {
  static observedAttributes = ["label", "open"];

  override template() {
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

  override setup() {
    this.must(".close").addEventListener("click", () => this.close());
    // Synchronize the attribute when Escape or the backdrop closes the native dialog.
    this.must<HTMLDialogElement>("dialog").addEventListener("close", () => {
      this.removeAttribute("open");
      this.emit("close");
    });
    this.must('slot[name="foot"]').addEventListener("slotchange", () => this.update());
  }

  override update() {
    this.must("h2").textContent = this.attr("label");
    // :has() cannot inspect assigned slot content; check assigned nodes instead.
    this.must(".foot").hidden = this.must<HTMLSlotElement>('slot[name="foot"]').assignedNodes().length === 0;
    const dialog = this.must<HTMLDialogElement>("dialog");
    const open = this.hasAttribute("open");
    if (open && !dialog.open) {
      dialog.showModal();
      this.emit("open");
    } else if (!open && dialog.open) {
      dialog.close();
    }
  }

  show() {
    this.setAttribute("open", "");
  }

  close() {
    this.removeAttribute("open");
  }
}

define("adi-modal", AdiModal);

export { AdiModal };

declare global {
  interface HTMLElementTagNameMap {
    "adi-modal": AdiModal;
  }
}
