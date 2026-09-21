// `<adi-notice>` and `<adi-empty>` — the two ways a screen says something without a box.
//
//   <adi-notice lead="Recommended:" dismissible>let adi-agent manage this…</adi-notice>
//   <adi-notice tone="err">Couldn't load this: connection refused</adi-notice>
//   <adi-empty icon="inbox">No ports reserved yet</adi-empty>
//
// A notice is one line under the header with a hairline below it — not a banner, not a box (§6),
// and never tinted: the lead word carries the weight and, when something failed, `--err` carries
// the colour. Dismissing raises `dismiss` and hides it; whether it stays dismissed is the
// caller's to remember, because only the caller knows what "this one" means.

import { AdiElement, define, sheet } from "./base.js";
import "./icon.js";

class AdiNotice extends AdiElement {
  static observedAttributes = ["lead", "tone", "dismissible"];

  static sheet = sheet(`
    :host {
      display: flex;
      align-items: baseline;
      gap: 10px;
      padding-bottom: 12px;
      margin-bottom: 16px;
      border-bottom: 1px solid var(--line);
      font-size: var(--fs-small);
      color: var(--ink-3);
      line-height: 1.5;
    }
    :host([tone="err"]) { color: var(--err); }
    :host([flush]) { padding-bottom: 0; margin-bottom: 0; border-bottom: 0; }
    .text { flex: 1; min-width: 0; }
    .lead { color: var(--ink-2); font-weight: 600; margin-right: 4px; }
    .lead:empty { display: none; }
    :host([tone="err"]) .lead { color: inherit; }
    .close {
      flex: none;
      padding: 0 2px;
      color: var(--ink-3);
      line-height: 1;
      transition: color var(--transition);
    }
    .close:hover { color: var(--ink); }
    .close[hidden] { display: none; }
  `);

  template() {
    return `
      <span class="text"><b class="lead" part="lead"></b><slot></slot></span>
      <slot name="action"></slot>
      <button class="close" part="close" aria-label="Dismiss" hidden>
        <adi-icon name="x" size="14"></adi-icon>
      </button>
    `;
  }

  setup() {
    this.$(".close").addEventListener("click", () => {
      this.hidden = true;
      this.emit("dismiss");
    });
  }

  update() {
    this.$(".lead").textContent = this.attr("lead");
    this.$(".close").hidden = !this.hasAttribute("dismissible");
  }
}

class AdiEmpty extends AdiElement {
  static observedAttributes = ["icon"];

  static sheet = sheet(`
    :host {
      display: flex;
      align-items: center;
      gap: 10px;
      padding: 24px 0;
      color: var(--ink-3);
      font-size: var(--fs-small);
    }
    adi-icon[hidden] { display: none; }
  `);

  template() {
    // 24 is the one place §9 allows it: an empty state is the only thing on the surface.
    return `<adi-icon size="24" hidden></adi-icon><slot></slot>`;
  }

  update() {
    const icon = this.$("adi-icon");
    const name = this.getAttribute("icon");
    icon.hidden = !name;
    if (name) icon.setAttribute("name", name);
  }
}

define("adi-notice", AdiNotice);
define("adi-empty", AdiEmpty);

export { AdiEmpty, AdiNotice };
