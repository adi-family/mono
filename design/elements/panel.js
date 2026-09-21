// `<adi-panel>` — a section of a page: a title line, a hairline under it, the content below.
//
//   <adi-panel label="Reserved ports">
//     <adi-button slot="actions" variant="link">Refresh</adi-button>
//     <adi-table></adi-table>
//   </adi-panel>
//
// No border, no fill, no radius. Grouping on an adi screen is done with tone, hairlines and
// space (§2.5) — a panel that draws a box around itself is the single most common way a screen
// in this system starts to look like somebody else's.
//
// The attribute is `label`, not `title`: `title` on any element is the browser's tooltip, and a
// panel heading that also floats a yellow box over the cursor is not what anyone meant.

import { AdiElement, define, sheet } from "./base.js";

class AdiPanel extends AdiElement {
  static observedAttributes = ["label"];

  static sheet = sheet(`
    :host { display: block; }
    :host([hidden]) { display: none; }
    .head {
      display: flex;
      align-items: center;
      gap: 12px;
      min-height: 32px;
      padding-bottom: 10px;
      margin-bottom: 12px;
      border-bottom: 1px solid var(--line);
    }
    h2 {
      margin: 0;
      font-size: var(--fs-section);
      font-weight: 600;
      color: var(--ink);
    }
    .spacer { flex: 1; }
    .body { display: flex; flex-direction: column; gap: 12px; }
  `);

  template() {
    return `
      <div class="head" part="head">
        <h2 part="title"></h2>
        <span class="spacer"></span>
        <slot name="actions"></slot>
      </div>
      <div class="body" part="body"><slot></slot></div>
    `;
  }

  update() {
    this.$("h2").textContent = this.attr("label");
  }
}

define("adi-panel", AdiPanel);

export { AdiPanel };
