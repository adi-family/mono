// Form-associated controls. The value attribute sets the default; the property holds live input.

import { AdiElement, define, watchChildren } from "./base.ts";
import "./icon.ts";

type NativeControl = HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement;

export interface SelectOption {
  value?: string;
  label?: string;
}

class AdiControl<T extends NativeControl = NativeControl> extends AdiElement {
  static formAssociated = true;

  #internals: ElementInternals;
  #pending: string | null = null;

  constructor() {
    super();
    this.#internals = this.attachInternals();
  }

  get control(): T | null {
    return this.$<T>(".control");
  }

  get value() {
    if (this.control) return this.control.value;
    return this.#pending ?? this.attr("value");
  }

  set value(next: string | number | null | undefined) {
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

  override focus(options?: FocusOptions) {
    this.control?.focus(options);
  }

  /** Whether the value has been typed into, in the sense a native control means by it. */
  dirty = false;

  override setup() {
    const control = this.must<T>(".control");
    control.value = this.#pending ?? this.attr("value");
    this.#internals.setFormValue(control.value);

    control.addEventListener("input", () => {
      this.dirty = true;
      this.#internals.setFormValue(control.value);
    });
    // input crosses the shadow boundary; change must be re-emitted on the host.
    control.addEventListener("change", () => {
      this.dirty = true;
      this.#internals.setFormValue(control.value);
      this.emit("change", { value: control.value });
    });
  }

  /** Apply defaults after options/text arrive, unless the user has already edited the value. */
  resetDefault(fallback = "") {
    if (this.dirty) return;
    const value = this.attr("value") || fallback;
    if (!value) return;
    const control = this.must<T>(".control");
    control.value = value;
    this.#internals.setFormValue(control.value);
  }

  override update() {
    const control = this.must<T>(".control");
    for (const name of ["placeholder", "name", "autocomplete", "inputmode", "min", "max", "step", "rows", "maxlength"]) {
      if (this.hasAttribute(name)) control.setAttribute(name, this.attr(name));
      else control.removeAttribute(name);
    }
    control.disabled = this.hasAttribute("disabled");
    if (control instanceof HTMLInputElement || control instanceof HTMLTextAreaElement) {
      control.readOnly = this.hasAttribute("readonly");
    }
    control.required = this.hasAttribute("required");
    if (this.hasAttribute("label")) control.setAttribute("aria-label", this.attr("label"));
  }

  formResetCallback() {
    this.value = this.attr("value");
  }

  formDisabledCallback(disabled: boolean) {
    this.toggleAttribute("disabled", disabled);
  }
}

class AdiInput extends AdiControl<HTMLInputElement> {
  static observedAttributes = ["placeholder", "type", "disabled", "readonly", "required", "name", "label"];

  override template() {
    return `<input class="control" part="control">`;
  }

  override update() {
    super.update();
    this.must<HTMLInputElement>(".control").type = this.attr("type", "text");
  }
}

class AdiTextarea extends AdiControl<HTMLTextAreaElement> {
  static observedAttributes = ["placeholder", "disabled", "readonly", "required", "name", "rows", "label"];

  override template() {
    return `<textarea class="control" part="control" rows="4"></textarea>`;
  }

  override setup() {
    super.setup();
    // Match native textarea defaults by dropping the first newline.
    watchChildren(this, () => this.resetDefault((this.textContent ?? "").replace(/^\n/, "").trimEnd()));
  }
}

class AdiSelect extends AdiControl<HTMLSelectElement> {
  static observedAttributes = ["disabled", "required", "name", "label"];

  override template() {
    return `
      <span class="wrap">
        <select class="control" part="control"></select>
        <adi-icon class="chevron" name="chevron-down" size="14"></adi-icon>
      </span>
    `;
  }

  /** Options: `["a", "b"]` or `[{ value, label }]`; declarative option children also work. */
  set options(list: readonly (string | SelectOption)[]) {
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
    // Preserve the selection when refreshing options.
    this.value = value;
  }

  get options(): Required<SelectOption>[] {
    return [...(this.control?.options ?? [])].map((o) => ({ value: o.value, label: o.textContent ?? "" }));
  }

  override setup() {
    super.setup();
    watchChildren(this, () => {
      const written = this.querySelectorAll("option, optgroup");
      if (!written.length) return;
      this.must<HTMLSelectElement>(".control").append(...written);
      this.resetDefault();
    });
  }
}

/** Label clicks focus the slotted control because label[for] cannot cross shadow roots. */
class AdiField extends AdiElement {
  static observedAttributes = ["label", "note"];

  override template() {
    return `
      <span class="label" part="label"></span>
      <slot></slot>
      <span class="note" part="note"></span>
    `;
  }

  override setup() {
    this.must(".label").addEventListener("click", () => {
      const control = this.querySelector<HTMLElement>("adi-input, adi-select, adi-textarea, input, select, textarea");
      control?.focus();
    });
  }

  override update() {
    this.must(".label").textContent = this.attr("label");
    this.must(".note").textContent = this.attr("note");
  }
}

define("adi-input", AdiInput);
define("adi-textarea", AdiTextarea);
define("adi-select", AdiSelect);
define("adi-field", AdiField);

export { AdiControl, AdiField, AdiInput, AdiSelect, AdiTextarea };

declare global {
  interface HTMLElementTagNameMap {
    "adi-input": AdiInput;
    "adi-select": AdiSelect;
    "adi-textarea": AdiTextarea;
    "adi-field": AdiField;
  }
}
