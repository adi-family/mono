// The form controls — DESIGN.md §6 "Input / select" and "Form page".
//
//   <adi-field label="Port" note="Left empty, the manager picks one.">
//     <adi-input mono placeholder="8080"></adi-input>
//   </adi-field>
//   <adi-select name="mode"><option value="a">Local always</option></adi-select>
//   <adi-textarea code rows="8"></adi-textarea>
//
// Two things here are worth knowing before changing them.
//
// **They are form-associated.** `static formAssociated` plus `attachInternals()` is what lets a
// control inside a shadow root take part in a real `<form>` — submit with the rest, carry a
// name, be reset. Without it a custom element is invisible to the form it sits in, which is the
// usual reason people give up on web components for anything with fields in it.
//
// **The value lives in the inner control, not in the attribute.** Like a native input, the
// `value` *attribute* is only the starting value: it is read once, on build. Re-reading it on
// every update would overwrite whatever the user has typed the moment any other attribute
// changed. The `value` *property* is the live one.

import { AdiElement, define, sheet, watchChildren } from "./base.js";
import "./icon.js";

// §6: `--bg-raise`, 1px `--line-strong`, radius 6, padding 9px 12px, 14px. Focus steps the
// border up one tone — no ring, no glow (§8).
const CONTROL = `
  .control {
    font: inherit;
    font-size: var(--fs-ui);
    font-family: var(--sans);
    width: 100%;
    padding: 9px 12px;
    background: var(--bg-raise);
    color: var(--ink);
    border: 1px solid var(--line-strong);
    border-radius: var(--r);
    transition: border-color var(--transition);
  }
  .control::placeholder { color: var(--ink-3); }
  .control:focus-visible { outline: none; border-color: var(--ink-3); }
  :host([mono]) .control { font-family: var(--mono); font-size: 13px; }
  :host([invalid]) .control { border-color: var(--err); }
  :host([disabled]) { opacity: .5; }
  :host([disabled]) .control { cursor: default; }
`;

/** The half of a control that is the same whether it wraps an input, a select or a textarea. */
class AdiControl extends AdiElement {
  static formAssociated = true;

  #internals;
  #pending = null;

  constructor() {
    super();
    this.#internals = this.attachInternals();
  }

  /** The real `<input>` / `<select>` / `<textarea>` inside the shadow root. */
  get control() {
    return this.$(".control");
  }

  get value() {
    if (this.control) return this.control.value;
    return this.#pending ?? this.attr("value");
  }

  set value(next) {
    const value = next == null ? "" : String(next);
    this.#pending = value;
    if (this.control) {
      this.control.value = value;
      this.#internals.setFormValue(value);
    }
  }

  get form() {
    return this.#internals.form;
  }

  get name() {
    return this.attr("name");
  }

  focus(options) {
    this.control?.focus(options);
  }

  /** Whether the value has been typed into, in the sense a native control means by it. */
  dirty = false;

  setup() {
    const control = this.control;
    control.value = this.#pending ?? this.attr("value");
    this.#internals.setFormValue(control.value);

    control.addEventListener("input", () => {
      this.dirty = true;
      this.#internals.setFormValue(control.value);
    });
    // `input` is a composed event and crosses the shadow boundary on its own, retargeted to
    // this element. `change` is not — so it is the one that has to be raised again out here,
    // or a listener bound on `<adi-input>` would never hear a field being committed.
    control.addEventListener("change", () => {
      this.dirty = true;
      this.#internals.setFormValue(control.value);
      this.emit("change", { value: control.value });
    });
  }

  /**
   * Put the starting value back, once whatever it depends on has arrived.
   *
   * `setup()` runs before a `<select>` has its options or a `<textarea>` has its text, so the
   * value it set was applied to an empty control. Nothing is written over a control somebody
   * has already typed in — that is what `dirty` is for, and what a native control means by it.
   */
  resetDefault(fallback = "") {
    if (this.dirty) return;
    const value = this.attr("value") || fallback;
    if (!value) return;
    this.control.value = value;
    this.#internals.setFormValue(this.control.value);
  }

  update() {
    const control = this.control;
    for (const name of ["placeholder", "name", "autocomplete", "inputmode", "min", "max", "step", "rows", "maxlength"]) {
      if (this.hasAttribute(name)) control.setAttribute(name, this.attr(name));
      else control.removeAttribute(name);
    }
    control.disabled = this.hasAttribute("disabled");
    control.readOnly = this.hasAttribute("readonly");
    control.required = this.hasAttribute("required");
    if (this.hasAttribute("label")) control.setAttribute("aria-label", this.attr("label"));
  }

  /** Called by the form when it is reset: back to the attribute, as a native control would go. */
  formResetCallback() {
    this.value = this.attr("value");
  }

  formDisabledCallback(disabled) {
    this.toggleAttribute("disabled", disabled);
  }
}

class AdiInput extends AdiControl {
  static observedAttributes = ["placeholder", "type", "disabled", "readonly", "required", "name", "label"];

  static sheet = sheet(`
    :host { display: inline-block; width: 200px; vertical-align: middle; }
    :host([wide]) { display: block; width: 100%; }
    :host([num]) { width: 80px; }
    ${CONTROL}
  `);

  template() {
    return `<input class="control" part="control">`;
  }

  update() {
    super.update();
    this.control.type = this.attr("type", "text");
  }
}

class AdiTextarea extends AdiControl {
  static observedAttributes = ["placeholder", "disabled", "readonly", "required", "name", "rows", "label"];

  static sheet = sheet(`
    :host { display: block; width: 100%; }
    ${CONTROL}
    .control { resize: vertical; line-height: 1.5; }
    /* A code editor is a code block you can type in (§6): mono, the large radius, tabs at 2. */
    :host([code]) .control {
      font-family: var(--mono);
      font-size: var(--fs-mono);
      color: var(--ink);
      line-height: 1.6;
      padding: 12px 14px;
      border-color: var(--line);
      border-radius: var(--r-lg);
      tab-size: 2;
      white-space: pre;
      overflow: auto;
    }
  `);

  template() {
    return `<textarea class="control" part="control" rows="4"></textarea>`;
  }

  setup() {
    super.setup();
    // A textarea's default value is the text written inside it, as a native one's is — with the
    // first newline dropped, so the opening tag can sit on its own line.
    watchChildren(this, () => this.resetDefault(this.textContent.replace(/^\n/, "").trimEnd()));
  }
}

class AdiSelect extends AdiControl {
  static observedAttributes = ["disabled", "required", "name", "label"];

  static sheet = sheet(`
    :host { display: inline-block; vertical-align: middle; min-width: 160px; max-width: 100%; }
    :host([wide]) { display: block; width: 100%; }
    .wrap { position: relative; display: block; }
    ${CONTROL}
    .control { padding-right: 30px; cursor: pointer; appearance: none; }
    /* The chevron is drawn rather than set as a background image, so it is the same Lucide
       glyph as everywhere else and inherits the label's colour (§9). */
    .chevron {
      position: absolute;
      right: 10px;
      top: 50%;
      transform: translateY(-50%);
      color: var(--ink-3);
      pointer-events: none;
    }
  `);

  template() {
    return `
      <span class="wrap">
        <select class="control" part="control"></select>
        <adi-icon class="chevron" name="chevron-down" size="14"></adi-icon>
      </span>
    `;
  }

  /**
   * The options, as `["a", "b"]` or `[{ value, label }]`.
   *
   * The declarative form is plain `<option>` children in the light DOM, which `setup()` moves
   * in; this is for a list that comes from data. Either way they end up in the same `<select>`.
   */
  set options(list) {
    const select = this.control;
    if (!select) return;
    const value = this.value;
    select.replaceChildren(
      ...list.map((item) => {
        const option = document.createElement("option");
        if (typeof item === "string") {
          option.value = item;
          option.textContent = item;
        } else {
          option.value = item.value ?? "";
          option.textContent = item.label ?? item.value ?? "";
        }
        return option;
      }),
    );
    // Restoring the selection is the caller's intent nine times in ten — a select that silently
    // jumps to its first option when a list is refreshed is a bug that reads as a data bug.
    this.value = value;
  }

  get options() {
    return [...(this.control?.options ?? [])].map((o) => ({ value: o.value, label: o.textContent }));
  }

  setup() {
    super.setup();
    watchChildren(this, () => {
      const written = this.querySelectorAll("option, optgroup");
      if (!written.length) return;
      // Moved, not copied: an `<option>` left in the light DOM with no slot to land in renders
      // nowhere, but it would still be found by `querySelector` from the outside.
      this.control.append(...written);
      this.resetDefault();
    });
  }
}

/**
 * `<adi-field>` — a control with a label above it and its explanation below (§6 "Form page").
 *
 * The label cannot be a `<label for>`: the control it names lives in another element's shadow
 * root, and `for` does not cross that boundary. Clicking it focuses the slotted control instead,
 * which is the behaviour `for` was there for.
 */
class AdiField extends AdiElement {
  static observedAttributes = ["label", "note"];

  static sheet = sheet(`
    :host { display: block; }
    :host([grow]) { flex: 1 1 240px; min-width: 0; }
    .label {
      display: block;
      margin-bottom: 6px;
      font-size: var(--fs-small);
      color: var(--ink-2);
      cursor: default;
    }
    .note {
      display: block;
      margin-top: 6px;
      max-width: 64ch;
      font-size: var(--fs-small);
      color: var(--ink-3);
      line-height: 1.5;
    }
    .note:empty { display: none; }
  `);

  template() {
    return `
      <span class="label" part="label"></span>
      <slot></slot>
      <span class="note" part="note"></span>
    `;
  }

  setup() {
    this.$(".label").addEventListener("click", () => {
      const control = this.querySelector("adi-input, adi-select, adi-textarea, input, select, textarea");
      control?.focus();
    });
  }

  update() {
    this.$(".label").textContent = this.attr("label");
    this.$(".note").textContent = this.attr("note");
  }
}

define("adi-input", AdiInput);
define("adi-textarea", AdiTextarea);
define("adi-select", AdiSelect);
define("adi-field", AdiField);

export { AdiControl, AdiField, AdiInput, AdiSelect, AdiTextarea };
