import { AdiElement, define } from "./base.js";
import "./icon.js";
import "./status.js";

class AdiKbd extends AdiElement {
}

class AdiItem extends AdiElement {
  static observedAttributes = ["label", "meta", "shortcut", "icon", "href", "live", "active"];

  override template() {
    const tag = this.hasAttribute("href") ? "a" : "button";
    const type = tag === "button" ? ' type="button"' : "";
    return `
      <${tag} class="row" part="row"${type}>
        <adi-icon class="icon" size="16" hidden></adi-icon>
        <span class="text">
          <span class="title">
            <adi-dot tone="running" hidden></adi-dot>
            <span class="name" part="title"></span>
          </span>
          <span class="meta" part="meta"></span>
        </span>
        <adi-kbd class="shortcut" part="shortcut" hidden></adi-kbd>
      </${tag}>
    `;
  }

  override update() {
    this.must(".name").textContent = this.attr("label");
    this.must(".meta").textContent = this.attr("meta");

    const icon = this.must(".icon");
    const name = this.getAttribute("icon");
    icon.hidden = !name;
    if (name) icon.setAttribute("name", name);

    const shortcut = this.must(".shortcut");
    shortcut.hidden = !this.hasAttribute("shortcut");
    shortcut.textContent = this.attr("shortcut");

    this.must("adi-dot").hidden = !this.hasAttribute("live");

    const row = this.must<HTMLButtonElement | HTMLAnchorElement>(".row");
    if (row instanceof HTMLAnchorElement) row.href = this.attr("href");
  }
}

define("adi-kbd", AdiKbd);
define("adi-item", AdiItem);

export { AdiItem, AdiKbd };

declare global {
  interface HTMLElementTagNameMap {
    "adi-item": AdiItem;
    "adi-kbd": AdiKbd;
  }
}
