// Render every variant and derive its example markup from the same specimen.

import { AdiElement, define, detail, esc } from "./base.ts";
import type { AdiInput } from "./field.ts";
import { ICON_NAMES } from "./icon.ts";
import type { AdiModal } from "./modal.ts";
import type { AdiTable } from "./table.ts";

export interface GallerySection {
  id: string;
  title: string;
  tag: string;
  blurb: string;
  rows: [label: string, html: string][];
  mount?: (root: HTMLElement) => void;
}

const TYPE = [
  ["--fs-title", "Page title · 600", "font-size: var(--fs-title); font-weight: 600; color: var(--ink)"],
  ["--fs-section", "Section · 600", "font-size: var(--fs-section); font-weight: 600; color: var(--ink)"],
  ["--fs-body", "Transcript · 1.6 · 80ch", "font-size: var(--fs-body); color: var(--ink)"],
  ["--fs-ui", "Inputs, table cells", "font-size: var(--fs-ui); color: var(--ink)"],
  ["--fs-ui-sm", "List items, buttons", "font-size: var(--fs-ui-sm); color: var(--ink)"],
  ["--fs-small", "Help text, notices", "font-size: var(--fs-small); color: var(--ink-2)"],
  ["--fs-label", "Labels, table headers", "font-size: var(--fs-label); color: var(--ink-3)"],
  ["--fs-mono", "Paths, ids, commands", "font-family: var(--mono); font-size: var(--fs-mono); color: var(--code)"],
];

const SURFACES = ["--bg-side", "--bg", "--bg-hover", "--bg-raise", "--bg-active"];
const INKS = ["--ink", "--ink-2", "--ink-3", "--code"];
const SIGNALS = ["--accent", "--accent-hover", "--ok", "--warn", "--err"];

function section({ id, title, tag, blurb, rows, mount }: GallerySection): GallerySection {
  return { id, title, tag, blurb, rows, mount };
}

const SECTIONS = [
  section({
    id: "button",
    title: "Button",
    tag: "adi-button",
    blurb: "One filled orange per screen, and it is primary. When the orange is already spent elsewhere, the page's main action is strong.",
    rows: [
      ["Variants", `
        <adi-button variant="primary">Reserve a port</adi-button>
        <adi-button variant="strong">Save</adi-button>
        <adi-button>Refresh</adi-button>
        <adi-button variant="quiet">Cancel</adi-button>
        <adi-button variant="danger">Delete</adi-button>
        <adi-button variant="link">Open the log</adi-button>
      `],
      ["With an icon", `
        <adi-button icon="plus">New agent</adi-button>
        <adi-button icon-end="arrow-up-right" variant="quiet">Docs</adi-button>
        <adi-button square icon="ellipsis" label="Row actions"></adi-button>
        <adi-button square size="sm" icon="x" label="Dismiss"></adi-button>
      `],
      ["Small", `
        <adi-button size="sm">Restart</adi-button>
        <adi-button size="sm" icon="refresh-cw">Reload</adi-button>
      `],
      ["Disabled · link", `
        <adi-button disabled>Unavailable</adi-button>
        <adi-button href="/extended/settings/hive" variant="quiet" icon="server">Hive</adi-button>
      `],
    ],
  }),

  section({
    id: "field",
    title: "Fields",
    tag: "adi-field · adi-input · adi-select · adi-textarea",
    blurb: "Form-associated: they carry a name, submit with the form around them and reset with it. Mono only when the value is a machine value.",
    rows: [
      ["Input", `
        <adi-input placeholder="Search agents"></adi-input>
        <adi-input mono value="~/adi-family"></adi-input>
        <adi-input num value="8090"></adi-input>
        <adi-input value="Refused" invalid></adi-input>
        <adi-input value="Read only" disabled></adi-input>
      `],
      ["Select", `
        <adi-select>
          <option value="local">Local always</option>
          <option value="cdn">CDN when remote</option>
        </adi-select>
      `],
      ["Labelled", `
        <adi-field label="Port" note="Left empty, the ports manager picks one.">
          <adi-input num placeholder="8090"></adi-input>
        </adi-field>
      `],
      ["Textarea", `
        <adi-textarea rows="3" placeholder="What should this agent do?"></adi-textarea>
      `],
      ["Code editor", `
        <adi-textarea code rows="4">services:
  - name: dev-ui
    port: 9080</adi-textarea>
      `],
    ],
  }),

  section({
    id: "segmented",
    title: "Segmented control",
    tag: "adi-segmented",
    blurb: "Two to four exclusive choices where a select would hide what they are. Selected is a tone change and a weight — never an outline, never orange.",
    rows: [
      ["Choices", `
        <adi-segmented value="table">
          <button value="table">Table</button>
          <button value="json">JSON</button>
          <button value="raw">Raw</button>
        </adi-segmented>
      `],
    ],
    mount(root) {
      const seg = root.querySelector("adi-segmented");
      if (!seg) return;
      const said = document.createElement("span");
      said.style.cssText = "font-size: var(--fs-label); color: var(--ink-3); margin-left: 12px";
      said.textContent = "change → table";
      seg.after(said);
      seg.addEventListener("change", (event) => {
        said.textContent = `change → ${detail<{ value: string }>(event).value}`;
      });
    },
  }),

  section({
    id: "pills",
    title: "Tag, chip, grant",
    tag: "adi-tag · adi-chip · adi-grant",
    blurb: "Same pill, three jobs. A tag is a category and is sans; a chip is a machine value offered as a choice and is mono; a grant carries its own remove, which is why no table repeats the word Revoke down a column.",
    rows: [
      ["Tag", `
        <adi-tag>research</adi-tag>
        <adi-tag>infra</adi-tag>
      `],
      ["Chip", `
        <adi-chip>opus</adi-chip>
        <adi-chip clickable>glm-5.3</adi-chip>
        <adi-chip clickable aria-pressed="true">haiku-4.5</adi-chip>
      `],
      ["Grant", `
        <adi-grant>http:app</adi-grant>
        <adi-grant>fs:~/adi-family</adi-grant>
      `],
      ["Key cap", `
        <adi-kbd>⌘K</adi-kbd>
        <adi-kbd>⌃1</adi-kbd>
      `],
    ],
    mount(root) {
      for (const grant of root.querySelectorAll("adi-grant")) {
        grant.addEventListener("remove", (event) => {
          grant.replaceWith(said(`removed ${detail<{ value: string }>(event).value}`));
        });
      }
    },
  }),

  section({
    id: "status",
    title: "Status",
    tag: "adi-status · adi-dot",
    blurb: "A 6px dot before a word, or a pill with the tone's own 12% tint. Never a filled badge. The running dot is the one accent that is not the screen's orange.",
    rows: [
      ["Dot and word", `
        <adi-status tone="ok">Online</adi-status>
        <adi-status tone="running">Running</adi-status>
        <adi-status tone="warn">Degraded</adi-status>
        <adi-status tone="err">Down</adi-status>
        <adi-status>Idle</adi-status>
      `],
      ["Pill", `
        <adi-status tone="ok" pill>set</adi-status>
        <adi-status tone="warn" pill>stale</adi-status>
        <adi-status tone="err" pill>blocked</adi-status>
        <adi-status pill>archived</adi-status>
      `],
      ["Bare dot", `
        <adi-dot tone="ok"></adi-dot>
        <adi-dot tone="running"></adi-dot>
        <adi-dot tone="err"></adi-dot>
        <adi-dot></adi-dot>
      `],
    ],
  }),

  section({
    id: "item",
    title: "List item",
    tag: "adi-item",
    blurb: "The sessions rail, and any list of things you open. The shortcut appears on hover and on the active row; a live row carries a 6px accent dot.",
    rows: [
      ["Rows", `
        <div style="width: 264px; background: var(--bg-side); padding: 6px; border-radius: var(--r)">
          <adi-item label="Port conflict on 8090" meta="adi-ui · 3m" shortcut="⌃1" live></adi-item>
          <adi-item label="Fleet pairing" meta="adi-agent · 1h" shortcut="⌃2" active></adi-item>
          <adi-item label="Rewrite the tokens page" meta="adi-agent · yesterday" shortcut="⌃3"></adi-item>
        </div>
      `],
      ["With an icon", `
        <div style="width: 264px">
          <adi-item icon="server" label="Hive" meta="Services, and the .adi names in front of them"></adi-item>
          <adi-item icon="key-round" label="Secrets" meta="Encrypted keys and API keys"></adi-item>
        </div>
      `],
    ],
  }),

  section({
    id: "table",
    title: "Table",
    tag: "adi-table",
    blurb: "No card around it. Header sentence case over a strong hairline, identifier columns in mono one step dimmer, repeated values dimmed, — for nothing, and the row's actions behind a ⋯ at the far right. Click a header to sort.",
    rows: [["Services", `<adi-table sort="port" empty="No services"></adi-table>`]],
    mount(root) {
      const table = root.querySelector<AdiTable>("adi-table");
      if (!table) return;
      const status = (tone: string, text: string) => {
        const el = document.createElement("adi-status");
        el.setAttribute("tone", tone);
        el.textContent = text;
        return el;
      };
      const menu = () => {
        const el = document.createElement("adi-button");
        el.setAttribute("square", "");
        el.setAttribute("size", "sm");
        el.setAttribute("icon", "ellipsis");
        el.setAttribute("label", "Row actions");
        return el;
      };
      table.columns = [
        { key: "service", label: "Service" },
        { key: "host", label: "Host", mono: true },
        { key: "port", label: "Port", mono: true, align: "right" },
        { key: "owner", label: "Project", muted: true },
        { key: "state", label: "State", sortable: false },
        { key: "actions", label: "", actions: true, sortable: false },
      ];
      table.rows = [
        { service: "dev-ui", host: "dev.adi", port: 9080, owner: "adi", state: status("ok", "up"), actions: menu() },
        { service: "dev-api", host: "dev.adi", port: 8090, owner: "adi", state: status("running", "starting"), actions: menu() },
        { service: "ui-playground", host: "ui.adi", port: 9081, owner: "adi", state: status("", "idle"), actions: menu() },
        { service: "llm-gateway", host: "llm.adi", port: 8004, owner: "adi", state: status("err", "down"), actions: menu() },
        { service: "front door", host: "", port: 80, owner: "adi", state: status("ok", "up"), actions: menu() },
      ];
    },
  }),

  section({
    id: "kv",
    title: "Key-value list",
    tag: "adi-kv",
    blurb: "What a right panel is mostly made of: a narrow column of keys in the dimmest ink, values one step brighter, machine values in mono.",
    rows: [
      ["Pairs", `
        <div style="width: 320px">
          <adi-kv>
            <dt>Agent</dt><dd>adi-ui</dd>
            <dt>Model</dt><dd class="mono">opus</dd>
            <dt>Started</dt><dd>3 minutes ago</dd>
            <dt>Run</dt><dd class="mono">1789977068962-0000</dd>
          </adi-kv>
        </div>
      `],
    ],
  }),

  section({
    id: "stats",
    title: "Stats",
    tag: "adi-stats · adi-stat",
    blurb: "Up to three numbers in a row, with one line of detail under them. Not boxes — §8 names stat cards specifically.",
    rows: [
      ["Row", `
        <adi-stats note="Last 7 days, this machine only. Money to cents, tokens to one decimal.">
          <adi-stat value="$6.78" label="Spend"></adi-stat>
          <adi-stat value="84.5k" label="Tokens"></adi-stat>
          <adi-stat value="31" label="Runs"></adi-stat>
        </adi-stats>
      `],
    ],
  }),

  section({
    id: "code",
    title: "Code block",
    tag: "adi-code",
    blurb: "One of the few things allowed to be a box: a block of a machine's own text is genuinely detachable.",
    rows: [
      ["Block", `
        <adi-code label="~/.adi/mono/projects/adi/.adi/hive.yaml" copy>services:
  - name: dev-ui
    cmd: trunk serve
    port: 9080</adi-code>
      `],
    ],
  }),

  section({
    id: "notice",
    title: "Notice and empty",
    tag: "adi-notice · adi-empty",
    blurb: "A line under the header with a hairline below it — not a banner, not a box. An empty state is the one place a 24px icon is allowed.",
    rows: [
      ["Notice", `
        <div style="width: 100%">
          <adi-notice lead="Recommended:" dismissible>let adi-agent manage this — say what you want in chat and it sets up projects, services and tools the way this store expects.</adi-notice>
          <adi-notice tone="err" flush>Couldn't load this: connection refused</adi-notice>
        </div>
      `],
      ["Empty", `
        <adi-empty icon="inbox">No ports reserved yet</adi-empty>
      `],
    ],
  }),

  section({
    id: "tool-call",
    title: "Tool call",
    tag: "adi-tool-call",
    blurb: "A receipt, not a message. Collapsed to one line so the transcript stays the product; click it to open.",
    rows: [
      ["Collapsed", `
        <div style="width: 100%; max-width: 640px">
          <adi-tool-call calls="6" tool="Bash" command="cargo build -p adi-webapp --target wasm32-unknown-unknown">
            <adi-code>Compiling adi-webapp v1.19.0
Finished in 21.06s</adi-code>
          </adi-tool-call>
        </div>
      `],
    ],
  }),

  section({
    id: "panel",
    title: "Panel",
    tag: "adi-panel",
    blurb: "A section of a page: a title, a hairline, the content. No border, no fill, no radius — grouping is done with tone and space.",
    rows: [
      ["Section", `
        <div style="width: 100%; max-width: 640px">
          <adi-panel label="Reserved ports">
            <adi-button slot="actions" variant="link" icon-end="arrow-up-right">Open the manager</adi-button>
            <adi-kv>
              <dt>Leased</dt><dd>4 of 64</dd>
              <dt>Range</dt><dd class="mono">8000–8099</dd>
            </adi-kv>
          </adi-panel>
        </div>
      `],
    ],
  }),

  section({
    id: "modal",
    title: "Modal",
    tag: "adi-modal",
    blurb: "A native dialog underneath: the top layer, focus held inside, Escape closes it, and the browser draws the backdrop.",
    rows: [
      ["Dialog", `
        <adi-button id="open-modal">Reserve a port…</adi-button>
        <adi-modal label="Reserve a port">
          <adi-field label="Port" note="Left empty, the ports manager picks one.">
            <adi-input num placeholder="8090"></adi-input>
          </adi-field>
          <adi-button slot="foot" variant="quiet" data-close>Cancel</adi-button>
          <adi-button slot="foot" variant="strong">Reserve</adi-button>
        </adi-modal>
      `],
    ],
    mount(root) {
      const modal = root.querySelector<AdiModal>("adi-modal");
      if (!modal) return;
      root.querySelector("#open-modal")?.addEventListener("click", () => modal.show());
      for (const button of modal.querySelectorAll("[data-close]")) {
        button.addEventListener("click", () => modal.close());
      }
    },
  }),
];

function said(text: string): HTMLSpanElement {
  const el = document.createElement("span");
  el.style.cssText = "font-size: var(--fs-label); color: var(--ink-3)";
  el.textContent = text;
  return el;
}

function dedent(text: string): string {
  const lines = text.replace(/^\n/, "").replace(/\s+$/, "").split("\n");
  const indent = Math.min(
    ...lines.filter((line) => line.trim()).map((line) => line.match(/^ */)?.[0].length ?? 0),
  );
  return lines.map((line) => line.slice(indent)).join("\n");
}

class AdiGallery extends AdiElement {

  override template(): string {
    const index = [
      ["type", "Type"],
      ["colour", "Colour"],
      ["icons", "Icons"],
      ...SECTIONS.map((s) => [s.id, s.title]),
    ];
    return `
      <adi-notice lead="Live, from this build.">
        Every element in <span class="tag">design/elements</span>, drawn by the same tokens the
        rest of the panel is. The block under each section is the markup that rendered it.
      </adi-notice>

      <nav class="index" part="index">
        ${index.map(([id, label]) => `<button type="button" data-goto="${id}">${esc(label)}</button>`).join("")}
      </nav>

      ${this.#foundations()}
      ${SECTIONS.map((s) => this.#section(s)).join("")}
    `;
  }

  #foundations(): string {
    const swatches = (tokens: string[]) =>
      tokens
        .map(
          (token) => `
            <div class="swatch">
              <div class="chipbox" style="background: var(${token})"></div>
              <span class="token">${token}</span>
              <span class="value" data-value="${token}"></span>
            </div>`,
        )
        .join("");

    return `
      <section id="type">
        <div class="sec-head"><h2>Type</h2><span class="tag">design/tokens.css</span></div>
        <p class="blurb">Geist everywhere, Geist Mono for machine strings only — a path, a hash, a
          command, an id, a config value. Sentence case throughout; nothing is ever tracked out.</p>
        <div class="rows">
          ${TYPE.map(
            ([token, role, style]) => `
              <div class="type-row">
                <span class="token">${token}</span>
                <span class="role">${esc(role)}</span>
                <span style="${style}">Agent finished the run</span>
              </div>`,
          ).join("")}
        </div>
      </section>

      <section id="colour">
        <div class="sec-head"><h2>Colour</h2><span class="tag">design/tokens.css</span></div>
        <p class="blurb">Surfaces first — lower is further from the reader. Then three inks and one
          mono ink, then the accent and the three signals. The values below are read off the live
          token file, not written here.</p>
        <div class="rows">
          <div class="row"><span class="label">Surfaces</span><div class="specimens"><div class="swatches">${swatches(SURFACES)}</div></div></div>
          <div class="row"><span class="label">Ink</span><div class="specimens"><div class="swatches">${swatches(INKS)}</div></div></div>
          <div class="row"><span class="label">Signals</span><div class="specimens"><div class="swatches">${swatches(SIGNALS)}</div></div></div>
        </div>
      </section>

      <section id="icons">
        <div class="sec-head"><h2>Icons</h2><span class="tag">adi-icon</span></div>
        <p class="blurb">Lucide, stroke 1.5, at 14 / 16 / 20 / 24 and nothing else. One set everywhere —
          adding a glyph is a name in crates/adi-ui/icons/ICONS and a run of scripts/lucide.sh.</p>
        <div class="icon-head">
          <adi-input id="icon-filter" placeholder="Filter ${ICON_NAMES.length} icons"></adi-input>
          <span class="icon-count"></span>
        </div>
        <div class="icons"></div>
      </section>
    `;
  }

  #section({ id, title, tag, blurb, rows }: GallerySection): string {
    const markup = rows.map(([, html]) => dedent(html)).join("\n");
    return `
      <section id="${id}">
        <div class="sec-head"><h2>${esc(title)}</h2><span class="tag">${esc(tag)}</span></div>
        <p class="blurb">${esc(blurb)}</p>
        <div class="rows">
          ${rows
            .map(
              ([label, html]) => `
                <div class="row">
                  <span class="label">${esc(label)}</span>
                  <div class="specimens">${html}</div>
                </div>`,
            )
            .join("")}
        </div>
        <details>
          <summary><adi-icon name="code" size="14"></adi-icon> Markup</summary>
          <adi-code copy>${esc(markup)}</adi-code>
        </details>
      </section>
    `;
  }

  override setup(): void {
    const root = this.shadowRoot;

    this.must(".index").addEventListener("click", (event) => {
      const id = event.target instanceof Element ? event.target.closest<HTMLElement>("[data-goto]")?.dataset.goto : undefined;
      // Document fragment navigation cannot reach IDs inside this shadow root.
      if (id) root.getElementById(id)?.scrollIntoView({ behavior: "smooth", block: "start" });
    });

    const computed = getComputedStyle(document.documentElement);
    for (const node of root.querySelectorAll<HTMLElement>("[data-value]")) {
      node.textContent = computed.getPropertyValue(node.dataset.value ?? "").trim();
    }

    this.#icons();

    for (const spec of SECTIONS) {
      if (spec.mount) spec.mount(this.must(`#${spec.id}`));
    }
  }

  #icons(): void {
    const grid = this.must<HTMLDivElement>(".icons");
    const count = this.must<HTMLSpanElement>(".icon-count");
    const filter = this.must<AdiInput>("#icon-filter");

    const draw = (query: string) => {
      const names = ICON_NAMES.filter((name) => name.includes(query.trim().toLowerCase()));
      grid.innerHTML = names
        .map(
          (name) => `
            <div class="icon-cell" title="${name}">
              <adi-icon name="${name}" size="20"></adi-icon>
              <span>${name}</span>
            </div>`,
        )
        .join("");
      count.textContent = `${names.length} of ${ICON_NAMES.length}`;
    };

    draw("");
    filter.addEventListener("input", () => draw(String(filter.value ?? "")));
  }
}

define("adi-gallery", AdiGallery);

export { AdiGallery, SECTIONS };

declare global {
  interface HTMLElementTagNameMap {
    "adi-gallery": AdiGallery;
  }
}
