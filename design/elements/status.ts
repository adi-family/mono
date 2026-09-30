import { AdiElement, define } from "./base.js";

class AdiDot extends AdiElement {

  override template() {
    return `<span class="dot" part="dot"></span>`;
  }
}

class AdiStatus extends AdiElement {

  override template() {
    return `<span class="dot" part="dot"></span><slot></slot>`;
  }
}

define("adi-dot", AdiDot);
define("adi-status", AdiStatus);

export { AdiDot, AdiStatus };

declare global {
  interface HTMLElementTagNameMap {
    "adi-status": AdiStatus;
    "adi-dot": AdiDot;
  }
}
