// `<adi-button>` — DESIGN.md §6 "Button", and §2.4's one orange.
//
//   <adi-button>Cancel</adi-button>
//   <adi-button variant="primary" icon="arrow-up">Send</adi-button>   <!-- the screen's one orange -->
//   <adi-button variant="strong">Save</adi-button>                    <!-- ink fill, when the orange is spent -->
//   <adi-button variant="quiet" href="/extended/settings">Settings</adi-button>
//   <adi-button square icon="ellipsis" label="Row actions"></adi-button>
//
// A naming clash worth knowing about: adi-css spells the ink fill `.adi-btn--primary` and the
// orange `.adi-btn--accent`. DESIGN.md §6 names them the other way round, and this library
// follows the rulebook — `primary` here is the orange, `strong` is the ink fill.

import { AdiElement, define, sheet } from "./base.js";
import "./icon.js";

/** Every fill a button has (§6). `primary` is the screen's one orange; nothing else is. */
const VARIANTS = ["default", "primary", "strong", "quiet", "link", "danger"];

class AdiButton extends AdiElement {
  static observedAttributes = ["variant", "size", "icon", "icon-end", "label", "disabled", "href"];

  static sheet = sheet(`
    :host { display: inline-flex; vertical-align: middle; }
    :host([full]) { display: flex; }
    :host([full]) .btn { width: 100%; }

    .btn {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      gap: 6px;
      padding: 7px 14px;
      border-radius: var(--r);
      background: var(--btn);
      color: var(--ink);
      font-size: var(--fs-ui-sm);
      font-weight: 500;
      line-height: 1.2;
      white-space: nowrap;
      cursor: pointer;
      transition: background var(--transition), color var(--transition);
    }
    .btn:hover { background: var(--btn-hover); }

    :host([variant="primary"]) .btn { background: var(--accent); color: var(--on-accent); }
    :host([variant="primary"]) .btn:hover { background: var(--accent-hover); }

    :host([variant="strong"]) .btn { background: var(--ink); color: var(--bg); }
    /* Lifted rather than swapped for a lighter ink: there is no token for "one step above
       --ink", and §8 forbids inventing one here (adi-css hardcodes #fff at this spot). */
    :host([variant="strong"]) .btn:hover { filter: brightness(1.08); }

    :host([variant="quiet"]) .btn { background: none; color: var(--ink-2); }
    :host([variant="quiet"]) .btn:hover { background: var(--bg-hover); color: var(--ink); }

    /* An action written as a word in a line of text — in a panel head, after a value. */
    :host([variant="link"]) .btn {
      padding: 3px 6px;
      background: none;
      color: var(--ink-2);
      font-weight: 400;
      font-size: var(--fs-small);
    }
    :host([variant="link"]) .btn:hover { background: var(--bg-hover); color: var(--ink); }

    /* Destructive: red text, whatever fill it has (§3). */
    :host([variant="danger"]) .btn { color: var(--err); }

    :host([size="sm"]) .btn { padding: 5px 10px; font-size: var(--fs-small); }

    /* The square that holds one icon, for a bar, a panel head or a row. */
    :host([square]) .btn {
      width: 28px;
      height: 28px;
      padding: 0;
      background: none;
      color: var(--ink-3);
    }
    :host([square][size="sm"]) .btn { width: 24px; height: 24px; }
    :host([square]) .btn:hover { background: var(--bg-hover); color: var(--ink); }

    :host([disabled]) .btn { opacity: .5; cursor: default; pointer-events: none; }
  `);

  template() {
    // `<a>` or `<button>` is settled once, on the first build: a control that changes from one
    // to the other under an attribute is not a thing any caller wants, and the branch would
    // have to survive every update below.
    const tag = this.hasAttribute("href") ? "a" : "button";
    const type = tag === "button" ? ' type="button"' : "";
    return `
      <${tag} class="btn" part="button"${type}>
        <adi-icon part="icon" hidden></adi-icon>
        <slot></slot>
        <adi-icon part="icon-end" hidden></adi-icon>
      </${tag}>
    `;
  }

  update() {
    const control = this.$(".btn");
    const size = this.getAttribute("size") === "sm" ? 14 : 16;
    const lead = this.$('[part="icon"]');
    const trail = this.$('[part="icon-end"]');

    for (const [node, name] of [
      [lead, this.getAttribute("icon")],
      [trail, this.getAttribute("icon-end")],
    ]) {
      node.hidden = !name;
      if (name) {
        node.setAttribute("name", name);
        node.setAttribute("size", size);
      }
    }

    // A square button has no visible words in it, so its name has to come from somewhere (§9).
    const label = this.getAttribute("label");
    if (label) control.setAttribute("aria-label", label);
    else control.removeAttribute("aria-label");
    if (label && !this.hasAttribute("title")) control.title = label;

    const disabled = this.hasAttribute("disabled");
    if (control.tagName === "BUTTON") {
      control.disabled = disabled;
    } else {
      control.href = this.attr("href");
      control.setAttribute("aria-disabled", String(disabled));
      if (this.hasAttribute("target")) control.target = this.attr("target");
    }
  }
}

define("adi-button", AdiButton);

export { AdiButton, VARIANTS };
