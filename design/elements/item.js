// `<adi-item>` and `<adi-kbd>` — the session-list row (§6 "List item") and the key cap in it.
//
//   <adi-item label="Port conflict on 8090" meta="adi-ui · 3m" shortcut="⌃1" live></adi-item>
//   <adi-item label="Settings" meta="Hive, ports, mesh" icon="settings-2" href="/extended/settings"></adi-item>
//
// Three things about this row are rules rather than taste. The shortcut is `opacity: 0` until
// the row is pointed at or is the active one (§8: shortcuts on hover) — a column of ⌃1…⌃9 down
// a sidebar is noise nine tenths of the time. The meta line is 12px `--ink-3` with the agent's
// name one step brighter, because that is the word somebody scans for. And `live` puts a 6px
// accent dot before the title, which is §3's one sanctioned orange that is not a button.

import { AdiElement, define, sheet } from "./base.js";
import "./icon.js";
import "./status.js";

class AdiKbd extends AdiElement {
  static sheet = sheet(`
    :host {
      display: inline-block;
      padding: 1px 5px;
      border-radius: var(--r-sm);
      background: var(--chip);
      color: var(--ink-3);
      font-family: var(--mono);
      font-size: 11.5px;
      line-height: 1.4;
      white-space: nowrap;
    }
  `);
}

class AdiItem extends AdiElement {
  static observedAttributes = ["label", "meta", "shortcut", "icon", "href", "live", "active"];

  static sheet = sheet(`
    :host { display: block; }
    .row {
      display: flex;
      align-items: center;
      gap: 8px;
      width: 100%;
      padding: 7px 8px;
      border-radius: var(--r);
      text-align: left;
      color: inherit;
      transition: background var(--transition);
    }
    .row:hover { background: var(--bg-hover); }
    :host([active]) .row { background: var(--bg-active); }
    .icon { color: var(--ink-3); flex: none; }
    .icon[hidden] { display: none; }
    .text { display: flex; flex-direction: column; gap: 2px; min-width: 0; flex: 1; }
    .title {
      display: flex;
      align-items: center;
      gap: 7px;
      font-size: var(--fs-ui-sm);
      color: var(--ink);
      white-space: nowrap;
      overflow: hidden;
      text-overflow: ellipsis;
    }
    .name { overflow: hidden; text-overflow: ellipsis; }
    .meta {
      font-size: var(--fs-label);
      color: var(--ink-3);
      white-space: nowrap;
      overflow: hidden;
      text-overflow: ellipsis;
    }
    .meta:empty { display: none; }
    adi-dot[hidden] { display: none; }
    .shortcut { flex: none; opacity: 0; transition: opacity var(--transition); }
    .shortcut[hidden] { display: none; }
    .row:hover .shortcut, :host([active]) .shortcut { opacity: 1; }
  `);

  template() {
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

  update() {
    this.$(".name").textContent = this.attr("label");
    this.$(".meta").textContent = this.attr("meta");

    const icon = this.$(".icon");
    const name = this.getAttribute("icon");
    icon.hidden = !name;
    if (name) icon.setAttribute("name", name);

    const shortcut = this.$(".shortcut");
    shortcut.hidden = !this.hasAttribute("shortcut");
    shortcut.textContent = this.attr("shortcut");

    this.$("adi-dot").hidden = !this.hasAttribute("live");

    const row = this.$(".row");
    if (row.tagName === "A") row.href = this.attr("href");
  }
}

define("adi-kbd", AdiKbd);
define("adi-item", AdiItem);

export { AdiItem, AdiKbd };
