import { AdiElement, define } from "./base.ts";
import "./icon.ts";

class AdiToolCall extends AdiElement {
  static observedAttributes = ["calls", "tool", "command", "open"];

  override template() {
    return `
      <button class="head" part="head" type="button" aria-expanded="false">
        <adi-icon class="chevron" name="chevron-right" size="14"></adi-icon>
        <span class="what" part="what"></span>
        <span class="command" part="command"></span>
      </button>
      <div class="body" part="body" hidden><slot></slot></div>
    `;
  }

  override setup() {
    this.must(".head").addEventListener("click", () => {
      this.toggleAttribute("open");
      this.emit("toggle", { open: this.hasAttribute("open") });
    });
  }

  override update() {
    const calls = Number(this.attr("calls", "1")) || 1;
    const tool = this.attr("tool");
    this.must(".what").textContent = [`${calls} call${calls === 1 ? "" : "s"}`, tool].filter(Boolean).join(" · ");
    this.must(".command").textContent = this.attr("command");

    const open = this.hasAttribute("open");
    this.must(".body").hidden = !open;
    this.must(".head").setAttribute("aria-expanded", String(open));
  }
}

define("adi-tool-call", AdiToolCall);

export { AdiToolCall };

declare global {
  interface HTMLElementTagNameMap {
    "adi-tool-call": AdiToolCall;
  }
}
