// `<adi-kv>` — the key-value list a right panel is mostly made of (§6): a 74px column of keys
// in `--ink-3`, values in `--ink-2`, machine values in mono.
//
//   <adi-kv>
//     <dt>Model</dt><dd class="mono">opus</dd>
//     <dt>Started</dt><dd>3 minutes ago</dd>
//   </adi-kv>
//
// `<dt>`/`<dd>` rather than an element per row: the pairing is what a description list is for,
// and a screen reader already knows how to read one. They stay in the light DOM — `display:
// contents` on the slot lets them be the grid's own items, so the columns line up across rows
// the way a two-element-per-row design never manages.

import { AdiElement, define, sheet } from "./base.js";

class AdiKv extends AdiElement {
  static sheet = sheet(`
    :host {
      display: grid;
      grid-template-columns: var(--adi-kv-keys, 74px) minmax(0, 1fr);
      gap: 6px 10px;
      font-size: var(--fs-small);
    }
    slot { display: contents; }
    ::slotted(dt) { color: var(--ink-3); }
    ::slotted(dd) { margin: 0; color: var(--ink-2); min-width: 0; overflow-wrap: anywhere; }
    ::slotted(dd.mono) { font-family: var(--mono); font-size: var(--fs-mono); color: var(--code); }
  `);
}

define("adi-kv", AdiKv);

export { AdiKv };
