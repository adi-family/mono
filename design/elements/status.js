// `<adi-dot>` and `<adi-status>` — DESIGN.md §6 "Status". A 6px dot before a word, or a pill
// with a 12% tint. Never a filled badge, and never a colour outside the five.
//
//   <adi-status tone="ok">Online</adi-status>
//   <adi-status tone="running">Running</adi-status>      <!-- the accent dot, 6px: §3's exception -->
//   <adi-status tone="idle" pill>Idle</adi-status>
//   <adi-dot tone="err"></adi-dot>
//
// `running` is the one place the accent appears without being the screen's one filled orange —
// §3 allows it because the dot is 6px and nothing else about it is orange.

import { AdiElement, define, sheet } from "./base.js";

// idle is the absence of a state, so it is grey — not a fourth colour.
const DOT = `
  .dot {
    width: 6px;
    height: 6px;
    flex: none;
    border-radius: 50%;
    background: var(--ink-3);
  }
  :host([tone="ok"]) .dot { background: var(--ok); }
  :host([tone="running"]) .dot { background: var(--accent); }
  :host([tone="warn"]) .dot { background: var(--warn); }
  :host([tone="err"]) .dot { background: var(--err); }
`;

class AdiDot extends AdiElement {
  static sheet = sheet(`
    :host { display: inline-flex; align-items: center; vertical-align: 1px; }
    ${DOT}
  `);

  template() {
    return `<span class="dot" part="dot"></span>`;
  }
}

class AdiStatus extends AdiElement {
  static sheet = sheet(`
    :host {
      display: inline-flex;
      align-items: center;
      gap: 7px;
      color: var(--ink-2);
      font-size: var(--fs-ui-sm);
      white-space: nowrap;
      vertical-align: middle;
    }
    ${DOT}

    /* The pill form: the tone's own 12% tint behind it, and the word in the tone (§3). */
    :host([pill]) {
      gap: 6px;
      padding: 2px 8px 2px 7px;
      border-radius: var(--r-pill);
      background: var(--chip);
      font-size: 11.5px;
    }
    :host([pill][tone="ok"]) { background: var(--ok-soft); color: var(--ok); }
    :host([pill][tone="warn"]) { background: var(--warn-soft); color: var(--warn); }
    :host([pill][tone="err"]) { background: var(--err-soft); color: var(--err); }
    :host([pill][tone="running"]) { color: var(--ink-2); }
  `);

  template() {
    return `<span class="dot" part="dot"></span><slot></slot>`;
  }
}

define("adi-dot", AdiDot);
define("adi-status", AdiStatus);

export { AdiDot, AdiStatus };
