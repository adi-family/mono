import { AdiElement, define } from "./base.ts";

class AdiStats extends AdiElement {
  static observedAttributes = ["note"];

  override template() {
    return `<div class="row"><slot></slot></div><div class="note" part="note"></div>`;
  }

  override update() {
    this.must(".note").textContent = this.attr("note");
  }
}

class AdiStat extends AdiElement {
  static observedAttributes = ["value", "label"];

  override template() {
    return `<span class="value" part="value"></span><span class="label" part="label"></span>`;
  }

  override update() {
    this.must(".value").textContent = this.attr("value", "—");
    this.must(".label").textContent = this.attr("label");
  }
}

define("adi-stats", AdiStats);
define("adi-stat", AdiStat);

export { AdiStat, AdiStats };

declare global {
  interface HTMLElementTagNameMap {
    "adi-stat": AdiStat;
    "adi-stats": AdiStats;
  }
}
