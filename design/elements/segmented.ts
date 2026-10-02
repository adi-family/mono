// Options move into the shadow root. Use `value` and `change` after upgrade.

import { AdiElement, define, watchChildren } from "./base.ts";

class AdiSegmented extends AdiElement {
  static observedAttributes = ["value"];

  get value() {
    return this.attr("value");
  }

  set value(next: string) {
    this.setAttribute("value", next);
  }

  get options() {
    return [...(this.$(".options")?.children ?? [])].filter(
      (child): child is HTMLElement => child instanceof HTMLElement && child.hasAttribute("value"),
    );
  }

  override template() {
    return `<div class="options" part="options" role="group"></div>`;
  }

  override setup() {
    watchChildren(this, () => {
      const written = [...this.children].filter((child) => child.hasAttribute("value"));
      if (!written.length) return;
      this.must(".options").append(...written);
      this.update();
    });

    // Listen on each option: shadow retargeting makes event.target the host outside the root.
    const options = this.must(".options");
    options.addEventListener("click", (event) => {
      const option = event.target instanceof Element ? event.target.closest<HTMLElement>("[value]") : null;
      if (!option || ("disabled" in option && option.disabled)) return;
      this.#choose(option.getAttribute("value") ?? "");
    });

    // Arrow keys wrap selection and focus through enabled options.
    options.addEventListener("keydown", (event) => {
      const step = event.key === "ArrowLeft" ? -1 : event.key === "ArrowRight" ? 1 : 0;
      if (!step) return;
      const options = this.options;
      const at = options.findIndex((o) => o.getAttribute("value") === this.value);
      for (let offset = 1; offset <= options.length; offset++) {
        const next = options[(at + step * offset + options.length) % options.length];
        if (!next || ("disabled" in next && next.disabled)) continue;
        event.preventDefault();
        this.#choose(next.getAttribute("value") ?? "");
        next.focus();
        break;
      }
    });
  }

  #choose(value: string) {
    if (value === this.value) return;
    this.value = value;
    this.emit("change", { value });
  }

  override update() {
    const options = this.options;
    if (!options.length) return;
    if (!options.some((o) => o.getAttribute("value") === this.value)) {
      this.setAttribute("value", options[0].getAttribute("value") ?? "");
      return;
    }
    for (const option of options) {
      option.setAttribute("aria-pressed", String(option.getAttribute("value") === this.value));
      if (option instanceof HTMLButtonElement && !option.hasAttribute("type")) option.type = "button";
    }
  }
}

define("adi-segmented", AdiSegmented);

export { AdiSegmented };

declare global {
  interface HTMLElementTagNameMap {
    "adi-segmented": AdiSegmented;
  }
}
