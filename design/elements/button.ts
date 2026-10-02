// Variants: `primary` is accent-filled; `strong` is ink-filled.

import { AdiElement, define } from "./base.ts";
import "./icon.ts";

const VARIANTS = ["default", "primary", "strong", "quiet", "link", "danger"] as const;

class AdiButton extends AdiElement {
  static observedAttributes = ["variant", "size", "icon", "icon-end", "label", "disabled", "href"];

  override template() {
    // The initial href determines the control type for this instance.
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

  override setup() {
    // Links have no native disabled state; also suppress clicks aimed at the host.
    const preventDisabledActivation = (event: Event) => {
      if (!this.hasAttribute("disabled")) return;
      event.preventDefault();
      event.stopImmediatePropagation();
    };
    this.addEventListener("click", preventDisabledActivation, { capture: true });
    this.addEventListener("auxclick", preventDisabledActivation, { capture: true });
  }

  override update() {
    const control = this.must<HTMLButtonElement | HTMLAnchorElement>(".btn");
    const size = this.getAttribute("size") === "sm" ? 14 : 16;
    const lead = this.must('[part="icon"]');
    const trail = this.must('[part="icon-end"]');

    for (const [node, name] of [
      [lead, this.getAttribute("icon")],
      [trail, this.getAttribute("icon-end")],
    ] as const) {
      node.hidden = !name;
      if (name) {
        node.setAttribute("name", name);
        node.setAttribute("size", String(size));
      }
    }

    const label = this.getAttribute("label");
    if (label) control.setAttribute("aria-label", label);
    else control.removeAttribute("aria-label");
    if (label && !this.hasAttribute("title")) control.title = label;

    const disabled = this.hasAttribute("disabled");
    if (control instanceof HTMLButtonElement) {
      control.disabled = disabled;
    } else {
      control.href = this.attr("href");
      control.setAttribute("aria-disabled", String(disabled));
      if (disabled) control.tabIndex = -1;
      else control.removeAttribute("tabindex");
      if (this.hasAttribute("target")) control.target = this.attr("target");
    }
  }
}

define("adi-button", AdiButton);

export { AdiButton, VARIANTS };

declare global {
  interface HTMLElementTagNameMap {
    "adi-button": AdiButton;
  }
}
