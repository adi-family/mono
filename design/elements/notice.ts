// Dismissal emits `dismiss` and hides the notice; persistence belongs to the caller.

import { AdiElement, define } from "./base.js";
import "./icon.js";

class AdiNotice extends AdiElement {
  static observedAttributes = ["lead", "tone", "dismissible"];

  override template() {
    return `
      <span class="text"><b class="lead" part="lead"></b><slot></slot></span>
      <slot name="action"></slot>
      <button class="close" part="close" aria-label="Dismiss" hidden>
        <adi-icon name="x" size="14"></adi-icon>
      </button>
    `;
  }

  override setup() {
    this.must(".close").addEventListener("click", () => {
      this.hidden = true;
      this.emit("dismiss");
    });
  }

  override update() {
    this.must(".lead").textContent = this.attr("lead");
    this.must(".close").hidden = !this.hasAttribute("dismissible");
  }
}

class AdiEmpty extends AdiElement {
  static observedAttributes = ["icon"];

  override template() {
    return `<adi-icon size="24" hidden></adi-icon><slot></slot>`;
  }

  override update() {
    const icon = this.must("adi-icon");
    const name = this.getAttribute("icon");
    icon.hidden = !name;
    if (name) icon.setAttribute("name", name);
  }
}

define("adi-notice", AdiNotice);
define("adi-empty", AdiEmpty);

export { AdiEmpty, AdiNotice };

declare global {
  interface HTMLElementTagNameMap {
    "adi-notice": AdiNotice;
    "adi-empty": AdiEmpty;
  }
}
