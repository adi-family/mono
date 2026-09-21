// `<adi-stats>` / `<adi-stat>` — §6 "Stats". Up to three numbers in a row, 20px/500, a 12px
// label under each, and one line of detail under the row.
//
//   <adi-stats note="Last 7 days, this machine only.">
//     <adi-stat value="$6.78" label="Spend"></adi-stat>
//     <adi-stat value="84.5k" label="Tokens"></adi-stat>
//   </adi-stats>
//
// Not boxes — §8 names stat cards specifically. Three numbers on the page's own surface read as
// the page's numbers; three numbers in three bordered rectangles read as a dashboard nobody
// asked for. Money to cents, tokens to one decimal: the formatting is the caller's, but those
// are the rules it is expected to have followed.

import { AdiElement, define, sheet } from "./base.js";

class AdiStats extends AdiElement {
  static observedAttributes = ["note"];

  static sheet = sheet(`
    :host { display: block; }
    .row { display: flex; gap: 32px; flex-wrap: wrap; }
    .note {
      margin-top: 8px;
      font-size: var(--fs-label);
      color: var(--ink-3);
    }
    .note:empty { display: none; }
  `);

  template() {
    return `<div class="row"><slot></slot></div><div class="note" part="note"></div>`;
  }

  update() {
    this.$(".note").textContent = this.attr("note");
  }
}

class AdiStat extends AdiElement {
  static observedAttributes = ["value", "label"];

  static sheet = sheet(`
    :host { display: flex; flex-direction: column; gap: 2px; }
    .value {
      font-size: 20px;
      font-weight: 500;
      color: var(--ink);
      font-variant-numeric: tabular-nums;
    }
    .label { font-size: var(--fs-label); color: var(--ink-3); }
  `);

  template() {
    return `<span class="value" part="value"></span><span class="label" part="label"></span>`;
  }

  update() {
    this.$(".value").textContent = this.attr("value", "—");
    this.$(".label").textContent = this.attr("label");
  }
}

define("adi-stats", AdiStats);
define("adi-stat", AdiStat);

export { AdiStat, AdiStats };
