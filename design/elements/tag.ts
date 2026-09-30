// adi-grant emits `remove` with `{ value }`; the caller removes the element.

import { AdiElement, define } from "./base.js";
import "./icon.js";

class AdiTag extends AdiElement {}

class AdiChip extends AdiElement {

  override setup() {
    if (this.hasAttribute("clickable") && !this.hasAttribute("tabindex")) this.tabIndex = 0;
  }
}

class AdiGrant extends AdiElement {

  override template() {
    return `
      <slot></slot>
      <button class="remove" part="remove">
        <adi-icon name="x" size="14"></adi-icon>
      </button>
    `;
  }

  override setup() {
    const remove = this.must(".remove");
    remove.addEventListener("click", () => this.emit("remove", { value: this.value }));
  }

  override update() {
    this.must(".remove").setAttribute("aria-label", `Remove ${this.value}`);
  }

  /** What this grant names — its `value`, or failing that the text in it. */
  get value() {
    return this.attr("value", (this.textContent ?? "").trim());
  }
}

define("adi-tag", AdiTag);
define("adi-chip", AdiChip);
define("adi-grant", AdiGrant);

export { AdiChip, AdiGrant, AdiTag };

declare global {
  interface HTMLElementTagNameMap {
    "adi-tag": AdiTag;
    "adi-chip": AdiChip;
    "adi-grant": AdiGrant;
  }
}
