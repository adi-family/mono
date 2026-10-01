# adi-elements

Custom HTML elements implementing [`design/DESIGN.md`](../DESIGN.md), with no runtime framework.

```html
<link rel="stylesheet" href="/design/tokens.css">
<script type="module" src="/elements/adi-elements.js"></script>

<adi-button variant="primary" icon="arrow-up">Send</adi-button>
```

The gallery at `http://dev.adi/extended/ui` shows the elements, variants, and example markup.

## Why this exists beside `crates/adi-ui`

`adi-ui` implements the system in Leptos. These elements also work in server-rendered pages,
dashboards, and standalone HTML. Both use `design/tokens.css`; button variant names differ:

| | adi-css / adi-ui | here | DESIGN.md |
| --- | --- | --- | --- |
| orange fill | `.adi-btn--accent` | `variant="primary"` | §6: "`.primary`: `--accent` fill" |
| ink fill | `.adi-btn--primary` | `variant="strong"` | §6: "`.strong`: `--ink` fill" |

## How they work

All element styles live in [`elements.css`](./elements.css). [`base.ts`](./base.ts) imports it
as text and constructs one `CSSStyleSheet`, adopted by every element's shadow root. Rules target
custom tags and their attributes, such as `:host(adi-button)` and `:host(adi-input[disabled])`.
Typography, controls, pills, and Markdown share rules across tags; components have no individual
stylesheets or static `sheet` declarations.

Shadow DOM preserves slots and isolates internal markup while inheriting design tokens from the
page. `template()` and `setup()` run once; `update()` reflects attribute changes without replacing
the root and losing focus or selection.

`adi-input`, `adi-select`, and `adi-textarea` remain form-associated: `name` controls submission,
form reset restores their defaults, the `value` attribute supplies the starting value, and the
`value` property holds the live value.

## Build and type checking

```sh
scripts/elements.sh build  # writes target/elements
scripts/elements.sh check  # checks TypeScript separately
scripts/elements.sh test   # type-checks, builds, and runs browser behavioral tests
node scripts/elements.test.mjs --shared  # gallery/style smoke checks after building
```

All element sources are TypeScript. Write relative imports with `.ts`, such as `./base.ts`.
TypeScript's `rewriteRelativeImportExtensions` converts them to `.js` during the build, then
esbuild inlines `elements.css` into `base.js`. Relative modules stay separate, and source maps
point back to the TypeScript files. Builds skip type checking; `check` and CI run it separately.

Trunk runs the build through `crates/adi-webapp/Trunk.toml`, then copies `target/elements` into
`dist/elements`. Pages load `/elements/adi-elements.js` from that output. Trunk watches `design/`
for rebuilds; CI runs the type check before release builds.

## Behavioral tests

The suite uses Node's test runner and Playwright with a real headless Chromium browser, so
Shadow DOM, native forms, focus, keyboard input, and dialogs behave as they do in the app.
Requires Node 22+ and the build tools above. Install the small, dedicated test dependency set:

```sh
npm ci --prefix scripts/elements-tests
scripts/elements-tests/node_modules/.bin/playwright install chromium
scripts/elements.sh test
```

On macOS, an installed Google Chrome is used automatically. Set `ELEMENTS_CHROME` to use
another Chrome/Chromium executable. Linux CI installs Playwright's Chromium with `--with-deps`.
An existing `apps/docs` Playwright installation also works for local runs.

Tests live in [`scripts/elements-tests`](../../scripts/elements-tests). Each named case has
a fresh browser context and mounts only its own fixture. The suite loads built modules from
local files, blocks HTTP requests, and needs no running app, API, credentials, or listening port.
Clipboard calls are stubbed; file-picker events carry in-memory test files.

Coverage includes form submission/reset and events, focus preservation, button and segmented
keyboard interaction, composer send/stop/asap and attachments, question replies, table sorting
and row actions, transcript reconciliation, modal closing/focus, dismissal, removal, and copying.
The API-backed `adi-chat` and microphone integration are not covered by this behavioral suite.

Run a focused case (still type-checks and rebuilds), or rerun an already built suite:

```sh
scripts/elements.sh test --test-name-pattern='segmented'
node --test scripts/elements-tests/*.test.mjs
```

The `Elements` GitHub Actions workflow runs the behavioral suite and gallery smoke checks on
pull requests and pushes to `main` that touch elements or their test/build infrastructure.

To add a case, import `test` from `./helpers.mjs`, call `await mount(markup)`, interact through
Playwright's `page` locators, then assert observable values and emitted events with
`node:assert/strict`. Prefer actual clicks, typing, and key presses; use `page.evaluate` for
public properties, event capture, and browser APIs that cannot receive real automation input.
Uncaught browser errors fail the test automatically.

## The elements

| Tag | What it is | DESIGN.md |
| --- | --- | --- |
| `adi-icon` | one Lucide glyph, stroke 1.5, at 14/16/20/24 | §9 |
| `adi-button` | `primary` (the one orange) · `strong` · `quiet` · `link` · `danger` · `square` | §6 |
| `adi-field` `adi-input` `adi-select` `adi-textarea` | field wrapper and form-associated controls | §6 |
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
| `adi-markdown` | safe Markdown rendering using `adi_ui::Markdown`'s subset | §4 |
| `adi-transcript` | newest-first feed keyed by `entries[].key` | §2.1 |
| `adi-message` `adi-note` `adi-tool-run` | chat message · platform note · grouped tool-call receipt | §6 |
| `adi-composer` `adi-mic` `adi-ask` | message and attachment composer · dictation · question card | §6 |
| `adi-chat` | chat window over `/api/agents`, including pickers, goals, transcript, composer, and queue | — |
| `adi-gallery` | all of the above, on one page | — |

Events are plain `CustomEvent`s on the element itself, `composed` so they cross the shadow
boundary: `change` (segmented, fields), `remove` (grant), `sort` / `select` (table), `dismiss`
(notice), `toggle` (tool call), `open` / `close` (modal), `copy` (code), `toggle` (tool run), `unqueue` (message), `send` / `asap` / `stop` (composer), `answer` (ask), `open` (chat).

## Icons

`scripts/lucide.sh` generates `icons.gen.ts` and the Rust icon set from
`crates/adi-ui/icons/*.svg`. To add a glyph, add its name to `crates/adi-ui/icons/ICONS` and run
the script.

## Adding one

1. Extend `AdiElement` in a component or family module; implement `template()`, `setup()`, and
   `update()` as needed.
2. Add tag selectors to `elements.css`, reuse shared rules, and use `design/tokens.css` values.
3. Register the module in `adi-elements.ts` and show each variant in `gallery.ts`.
