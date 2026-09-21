// `<adi-segmented>` — DESIGN.md §6 "Segmented control". Two to four exclusive choices, where a
// select would hide what the options are.
//
//   <adi-segmented value="table">
//     <button value="table">Table</button>
//     <button value="json">JSON</button>
//   </adi-segmented>
//
// The options are plain `<button value="…">` children rather than an element of their own: they
// are already the right thing — focusable, clickable, labelled by their own text. The selected
// one is a tone change and a weight, never an outline and never orange (§8).
//
// **The buttons are moved into the shadow root**, not slotted, and that is deliberate. A slotted
// element stays in the light DOM, where the *page's* CSS wins over the component's `::slotted()`
// rules whatever the specificity — so on a page carrying adi-css (whose reset is
// `button { background-color: transparent; border: 0 }`) the selected option lost its fill and
// every option lost its padding. Owning the nodes is what makes the control look the same
// wherever it is dropped. The consequence to know: after upgrade they are no longer found by
// `querySelector` from outside. Drive it through `value` and the `change` event, which is what
// it is for.

import { AdiElement, define, sheet, watchChildren } from "./base.js";

class AdiSegmented extends AdiElement {
  static observedAttributes = ["value"];

  static sheet = sheet(`
    :host { display: inline-block; vertical-align: middle; }
    .options {
      display: inline-grid;
      grid-auto-flow: column;
      gap: 2px;
      padding: 3px;
      background: var(--bg-raise);
      border: 1px solid var(--line-strong);
      border-radius: var(--r);
    }
    .options button {
      padding: 6px 12px;
      border-radius: var(--r-sm);
      color: var(--ink-2);
      font: inherit;
      font-size: var(--fs-ui-sm);
      white-space: nowrap;
      transition: background var(--transition), color var(--transition);
    }
    .options button:hover { color: var(--ink); }
    .options button[aria-pressed="true"] {
      background: var(--bg-active);
      color: var(--ink);
      font-weight: 500;
    }
  `);

  get value() {
    return this.attr("value");
  }

  set value(next) {
    this.setAttribute("value", next);
  }

  /** The options, in order. */
  get options() {
    return [...(this.$(".options")?.children ?? [])].filter((child) => child.hasAttribute("value"));
  }

  template() {
    return `<div class="options" part="options" role="group"></div>`;
  }

  setup() {
    watchChildren(this, () => {
      const written = [...this.children].filter((child) => child.hasAttribute("value"));
      if (!written.length) return;
      this.$(".options").append(...written);
      this.update();
    });

    // Bound on the options themselves, not on the host: an event listened for on the host has
    // its target retargeted *to* the host, and this host carries a `value` attribute of its
    // own — so `event.target.closest("[value]")` up there finds the control rather than the
    // option that was clicked, and every click re-chooses what was already chosen.
    const options = this.$(".options");
    options.addEventListener("click", (event) => {
      const option = event.target.closest("[value]");
      if (!option || option.disabled) return;
      this.#choose(option.getAttribute("value"));
    });

    // Left/right move the choice, as they do in a native radio group — a segmented control is
    // one control, not three tab stops.
    options.addEventListener("keydown", (event) => {
      const step = { ArrowLeft: -1, ArrowRight: 1 }[event.key];
      if (!step) return;
      const options = this.options;
      const at = options.findIndex((o) => o.getAttribute("value") === this.value);
      const next = options[(at + step + options.length) % options.length];
      if (!next) return;
      event.preventDefault();
      this.#choose(next.getAttribute("value"));
      next.focus();
    });
  }

  #choose(value) {
    if (value === this.value) return;
    this.value = value;
    this.emit("change", { value });
  }

  update() {
    const options = this.options;
    if (!options.length) return;
    // With nothing chosen the first option is, rather than leaving a control that looks broken.
    // Written back to the attribute so `.value` answers what the control is showing; the next
    // pass finds it valid and stops there.
    if (!options.some((o) => o.getAttribute("value") === this.value)) {
      this.setAttribute("value", options[0].getAttribute("value"));
      return;
    }
    for (const option of options) {
      option.setAttribute("aria-pressed", String(option.getAttribute("value") === this.value));
      // A `<button>` inside a form submits it unless told otherwise, and these are choices.
      if (option.tagName === "BUTTON" && !option.hasAttribute("type")) option.type = "button";
    }
  }
}

define("adi-segmented", AdiSegmented);

export { AdiSegmented };
