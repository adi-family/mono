import { AdiElement, define } from "./base.ts";

class AdiPanel extends AdiElement {
  static observedAttributes = ["label"];

  override template() {
    return `
      <div class="head" part="head">
        <h2 part="title"></h2>
        <span class="spacer"></span>
        <slot name="actions"></slot>
      </div>
      <div class="body" part="body"><slot></slot></div>
    `;
  }

  override update() {
    this.must("h2").textContent = this.attr("label");
  }
}

define("adi-panel", AdiPanel);

export { AdiPanel };

declare global {
  interface HTMLElementTagNameMap {
    "adi-panel": AdiPanel;
  }
}
