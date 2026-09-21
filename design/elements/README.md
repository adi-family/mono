# adi-elements — the design system as custom elements

`design/DESIGN.md` written out as HTML tags. No framework, no build step, no npm: every file
here is an ES module defining `HTMLElement` subclasses, and the browser is the runtime.

```html
<link rel="stylesheet" href="/design/tokens.css">
<script type="module" src="/elements/adi-elements.js"></script>

<adi-button variant="primary" icon="arrow-up">Send</adi-button>
```

**Look at them:** the control panel mounts the gallery at **http://dev.adi/extended/ui** (and
`http://app.adi/extended/ui` once a release carries it) — every element, every variant, and the
markup that drew it.

## Why this exists beside `crates/adi-ui`

`adi-ui` is the same system in Leptos, compiled into the panel's wasm bundle. It is not going
anywhere, and the panel is still built from it. These are for everything that is **not** the
wasm app and has no build step of its own: the front door's pages, `adi-app`'s placeholder, a
dashboard an agent writes, a docs page, a one-off HTML file you want to look at a layout in.

Both draw from `design/tokens.css`, so they cannot drift on colour, type or spacing. Where they
differ in naming, this list is the rulebook's:

| | adi-css / adi-ui | here | DESIGN.md |
| --- | --- | --- | --- |
| orange fill | `.adi-btn--accent` | `variant="primary"` | §6: "`.primary`: `--accent` fill" |
| ink fill | `.adi-btn--primary` | `variant="strong"` | §6: "`.strong`: `--ink` fill" |

## How they work

Three decisions, all in [`base.js`](./base.js), and they explain most of what you will read:

- **Shadow DOM.** An element's CSS cannot leak out and the page's cannot leak in — so a button
  looks the same in the Leptos panel, in a server-rendered page and in a bare file. What does
  cross the boundary is custom properties, which is why every rule in here is `var(--…)` and no
  element carries a colour of its own (§8: never restate a hex).
- **One stylesheet per class.** `sheet()` compiles once; every instance adopts the same
  `CSSStyleSheet` object.
- **Build once, update often.** `template()` runs on the first connect; `update()` runs on every
  attribute change after it. Nothing rebuilds its shadow root on an attribute change, which is
  what would throw away focus, selection and scroll position in anything holding an `<input>`.

The form controls (`adi-input`, `adi-select`, `adi-textarea`) are **form-associated**: they carry
a `name`, submit with the form around them, and reset with it. Values follow native semantics —
the `value` *attribute* is the starting value, read once; the `value` *property* is the live one.

## The elements

| Tag | What it is | DESIGN.md |
| --- | --- | --- |
| `adi-icon` | one Lucide glyph, stroke 1.5, at 14/16/20/24 | §9 |
| `adi-button` | `primary` (the one orange) · `strong` · `quiet` · `link` · `danger` · `square` | §6 |
| `adi-field` `adi-input` `adi-select` `adi-textarea` | labelled controls, form-associated | §6 |
| `adi-segmented` | exclusive choices; `<button value>` children | §6 |
| `adi-tag` `adi-chip` `adi-grant` | category (sans) · machine value (mono) · value with its own × | §6 |
| `adi-status` `adi-dot` | 6px dot and a word, or a pill with a 12% tint | §6 |
| `adi-item` `adi-kbd` | list row with meta and a hover shortcut | §6 |
| `adi-panel` | a section: title, hairline, content. No box | §2.5 |
| `adi-table` | `columns`/`rows` properties, click-to-sort, `—` for nothing | §6 |
| `adi-kv` | `<dt>`/`<dd>` pairs in a 74px grid | §6 |
| `adi-stats` `adi-stat` | up to three numbers and a line of detail | §6 |
| `adi-code` | code block, optional label and copy | §6 |
| `adi-notice` `adi-empty` | a line under the header · an empty state | §6 |
| `adi-tool-call` | the collapsed receipt of what an agent ran | §6 |
| `adi-modal` | a native `<dialog>`: top layer, focus held, Escape closes | §2.5 |
| `adi-gallery` | all of the above, on one page | — |

Events are plain `CustomEvent`s on the element itself, `composed` so they cross the shadow
boundary: `change` (segmented, fields), `remove` (grant), `sort` / `select` (table), `dismiss`
(notice), `toggle` (tool call), `open` / `close` (modal), `copy` (code).

## Icons

`icons.gen.js` is **generated** — `scripts/lucide.sh` writes it from `crates/adi-ui/icons/*.svg`,
the same files the Rust `Lucide` enum is compiled from. To add a glyph: put its name in
`crates/adi-ui/icons/ICONS`, run `scripts/lucide.sh`, and it is in both libraries.

## Where they are served from

`crates/adi-webapp/index.html` copies this directory into the panel's `dist/` verbatim
(`rel="copy-dir"`, so the relative imports keep working) and loads `adi-elements.js` as a module
from `/elements/adi-elements.js`. Trunk watches `design/`, so editing a file here hot-reloads the
panel like any Rust change.

## Adding one

1. A file per component (or per family). Extend `AdiElement`, give it `static sheet = sheet(…)`,
   `template()` and `update()`.
2. Every value from a token. If the value you need is not in `design/tokens.css`, that is a
   design decision — raise it, do not write the number.
3. Add it to `adi-elements.js`, and add a section to `gallery.js` showing **every** arm of
   **every** variant it takes. A variant nobody renders is a variant nobody notices is broken.
