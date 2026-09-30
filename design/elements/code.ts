// Light-DOM text is preserved verbatim, including indentation.

import { AdiElement, define } from "./base.js";
import "./icon.js";

class AdiCode extends AdiElement {
  static observedAttributes = ["label", "copy"];

  override template() {
    return `
      <div class="head" part="head" hidden>
        <span class="label" part="label"></span>
        <button class="copy" part="copy" aria-label="Copy" hidden>
          <adi-icon name="copy" size="14"></adi-icon>
        </button>
      </div>
      <pre class="body" part="body"><slot></slot></pre>
    `;
  }

  override setup() {
    this.must(".copy").addEventListener("click", () => this.#copy());
  }

  override update() {
    const label = this.attr("label");
    const copyable = this.hasAttribute("copy");
    this.must(".label").textContent = label;
    this.must(".copy").hidden = !copyable;
    this.must(".head").hidden = !label && !copyable;
  }

  get code() {
    return (this.textContent ?? "").replace(/^\n/, "").replace(/\s+$/, "");
  }

  async #copy() {
    const icon = this.must(".copy adi-icon");
    try {
      await navigator.clipboard.writeText(this.code);
    } catch (error) {
      console.warn("adi-code: could not copy", error);
      return;
    }
    icon.setAttribute("name", "check");
    setTimeout(() => icon.setAttribute("name", "copy"), 1200);
    this.emit("copy", { code: this.code });
  }
}

define("adi-code", AdiCode);

export { AdiCode };

declare global {
  interface HTMLElementTagNameMap {
    "adi-code": AdiCode;
  }
}
