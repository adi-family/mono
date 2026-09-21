// The three pills — DESIGN.md §6 "Tag · Chip · Grant". Same shape, three different jobs, and
// the difference between them is the whole point of having three names.
//
//   <adi-tag>research</adi-tag>            <!-- a category. Sans: a category is not a machine string -->
//   <adi-chip>opus</adi-chip>              <!-- a machine value offered as a choice. Mono (§2.3) -->
//   <adi-grant>http:app</adi-grant>        <!-- a machine value with its own remove -->
//
// `<adi-grant>` carries the × inside the pill, which is why no table of grants ever repeats the
// word "Revoke" down a column (§8). It emits `remove` with `{ value }`; the row is not taken
// away here, because what removing means is the caller's.

import { AdiElement, define, sheet } from "./base.js";
import "./icon.js";

// Every pill's fill: the chip tone, `--ink-2`, fully rounded.
const PILL = `
  :host {
    display: inline-flex;
    align-items: center;
    background: var(--chip);
    color: var(--ink-2);
    border-radius: var(--r-pill);
    white-space: nowrap;
    vertical-align: middle;
  }
`;

class AdiTag extends AdiElement {
  static sheet = sheet(`
    ${PILL}
    :host { padding: 2px 8px; font-size: var(--fs-label); }
  `);
}

class AdiChip extends AdiElement {
  static sheet = sheet(`
    ${PILL}
    :host {
      padding: 3px 9px;
      font-family: var(--mono);
      font-size: var(--fs-label);
      line-height: 1.4;
    }
    /* A chip that is a choice: pressed is a tone change, never orange (§8). */
    :host([clickable]) { cursor: pointer; transition: background var(--transition), color var(--transition); }
    :host([clickable]:hover) { background: var(--chip-hover); color: var(--ink); }
    :host([aria-pressed="true"]) { background: var(--bg-active); color: var(--ink); }
  `);

  setup() {
    if (this.hasAttribute("clickable") && !this.hasAttribute("tabindex")) this.tabIndex = 0;
  }
}

class AdiGrant extends AdiElement {
  static sheet = sheet(`
    ${PILL}
    :host {
      gap: 4px;
      padding: 3px 4px 3px 9px;
      font-family: var(--mono);
      font-size: var(--fs-label);
      line-height: 1.4;
    }
    .remove {
      display: inline-grid;
      place-items: center;
      width: 16px;
      height: 16px;
      border-radius: var(--r-pill);
      color: var(--ink-3);
      transition: background var(--transition), color var(--transition);
    }
    .remove:hover { background: var(--chip-hover); color: var(--ink); }
  `);

  template() {
    return `
      <slot></slot>
      <button class="remove" part="remove">
        <adi-icon name="x" size="14"></adi-icon>
      </button>
    `;
  }

  setup() {
    const remove = this.$(".remove");
    remove.addEventListener("click", () => this.emit("remove", { value: this.value }));
  }

  update() {
    this.$(".remove").setAttribute("aria-label", `Remove ${this.value}`);
  }

  /** What this grant names — its `value`, or failing that the text in it. */
  get value() {
    return this.attr("value", this.textContent.trim());
  }
}

define("adi-tag", AdiTag);
define("adi-chip", AdiChip);
define("adi-grant", AdiGrant);

export { AdiChip, AdiGrant, AdiTag };
