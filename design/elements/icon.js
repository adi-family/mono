// `<adi-icon>` — one Lucide glyph, drawn to DESIGN.md §9.
//
// The paths come from `icons.gen.js`, which `scripts/lucide.sh` writes from the same
// `crates/adi-ui/icons/*.svg` the Rust `adi_ui::Icon` compiles in — one set of glyphs, two
// languages drawing them. The wrapper is this component's: stroke 1.5 (Lucide ships 2, which is
// too heavy against Geist 400), `currentColor`, and one of four sizes.
//
//   <adi-icon name="server"></adi-icon>                  <!-- 16, decorative -->
//   <adi-icon name="x" size="14" label="Dismiss"></adi-icon>

import { AdiElement, define, sheet } from "./base.js";
import { ICONS } from "./icons.gen.js";

// §9: 14 in meta and tags, 16 in tables, buttons and panel heads, 20 on the landing, 24 for an
// empty state. Nothing else — a size off this list is a decision the design system already made.
const SIZES = [14, 16, 20, 24];

// One warning per bad name, not one per instance: a mistyped icon in a table column would
// otherwise print a line per row.
const warned = new Set();

function snap(value) {
  const size = Number(value);
  if (!size) return 16;
  if (SIZES.includes(size)) return size;
  return SIZES.reduce((best, s) => (Math.abs(s - size) < Math.abs(best - size) ? s : best));
}

class AdiIcon extends AdiElement {
  static observedAttributes = ["name", "size", "label"];

  static sheet = sheet(`
    :host { display: inline-flex; flex: none; color: inherit; }
    svg {
      display: block;
      fill: none;
      stroke: currentColor;
      stroke-width: var(--icon-stroke);
      stroke-linecap: round;
      stroke-linejoin: round;
    }
  `);

  template() {
    return `<svg part="svg" viewBox="0 0 24 24"></svg>`;
  }

  update() {
    const svg = this.$("svg");
    const name = this.attr("name");
    const body = ICONS[name];
    if (body === undefined && !warned.has(name)) {
      warned.add(name);
      console.warn(
        `adi-icon: no Lucide icon named "${name}". Add it to crates/adi-ui/icons/ICONS and run scripts/lucide.sh.`,
      );
    }
    svg.innerHTML = body ?? "";

    const size = snap(this.getAttribute("size"));
    svg.setAttribute("width", size);
    svg.setAttribute("height", size);

    // An icon beside its own label is read twice by a screen reader; one without a label is the
    // only case that needs a name of its own (§9: send, close, filter, the ⋯ menu).
    const label = this.getAttribute("label");
    if (label) {
      this.setAttribute("role", "img");
      this.setAttribute("aria-label", label);
      this.removeAttribute("aria-hidden");
    } else {
      this.setAttribute("aria-hidden", "true");
      this.removeAttribute("role");
      this.removeAttribute("aria-label");
    }
  }
}

define("adi-icon", AdiIcon);

/** Every icon name in the set, sorted — what the gallery lists and what `name` accepts. */
export const ICON_NAMES = Object.keys(ICONS);

export { AdiIcon };
