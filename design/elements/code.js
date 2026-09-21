// `<adi-code>` — a code block (§6): `--bg-raise`, a hairline, the large radius, mono 12.5 at
// line-height 1.6. One of the few places the design system allows a box, because a block of a
// machine's own text is a genuinely detachable thing (§2.5).
//
//   <adi-code label="hive.yaml" copy>services:
//   - name: dev-ui</adi-code>
//
// Whitespace is the content's. The text is taken from the light DOM exactly as written, so the
// opening tag has to sit hard against the first line — an indented `<adi-code>` block indents
// every line of what it shows.

import { AdiElement, define, sheet } from "./base.js";
import "./icon.js";

class AdiCode extends AdiElement {
  static observedAttributes = ["label", "copy"];

  static sheet = sheet(`
    :host {
      display: block;
      border: 1px solid var(--line);
      border-radius: var(--r-lg);
      background: var(--bg-raise);
      overflow: hidden;
    }
    .head {
      display: flex;
      align-items: center;
      gap: 8px;
      padding: 6px 8px 6px 14px;
      border-bottom: 1px solid var(--line);
      font-size: var(--fs-label);
      color: var(--ink-3);
    }
    .head[hidden] { display: none; }
    .label { flex: 1; min-width: 0; font-family: var(--mono); overflow: hidden; text-overflow: ellipsis; }
    .copy {
      display: inline-grid;
      place-items: center;
      width: 24px;
      height: 24px;
      border-radius: var(--r);
      color: var(--ink-3);
      transition: background var(--transition), color var(--transition);
    }
    .copy:hover { background: var(--bg-hover); color: var(--ink); }
    .copy[hidden] { display: none; }
    .body {
      display: block;
      margin: 0;
      padding: 12px 14px;
      font-family: var(--mono);
      font-size: var(--fs-mono);
      color: var(--code);
      line-height: 1.6;
      white-space: pre;
      overflow: auto;
      tab-size: 2;
    }
  `);

  template() {
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

  setup() {
    this.$(".copy").addEventListener("click", () => this.#copy());
  }

  update() {
    const label = this.attr("label");
    const copyable = this.hasAttribute("copy");
    this.$(".label").textContent = label;
    this.$(".copy").hidden = !copyable;
    this.$(".head").hidden = !label && !copyable;
  }

  /** The text this block shows, as a machine would take it. */
  get code() {
    return this.textContent.replace(/^\n/, "").replace(/\s+$/, "");
  }

  async #copy() {
    const icon = this.$(".copy adi-icon");
    try {
      await navigator.clipboard.writeText(this.code);
    } catch (error) {
      // Clipboard access is refused outside a secure context, which http://app.adi is — so this
      // is a normal outcome here, not an exception worth throwing at the page.
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
