# adi-ui

The **adi component library** — [Leptos](https://leptos.dev) components styled with
[Tailwind](https://tailwindcss.com) utilities that resolve to the product's design tokens.

The rules are [`design/DESIGN.md`](../../design/DESIGN.md); the values are
[`design/tokens.css`](../../design/tokens.css). This crate holds neither: it names the tokens
for Tailwind ([`styles/ui.css`](./styles/ui.css)), draws the components against them, and
ships the two things a stylesheet cannot — the fonts and the icons. Read `DESIGN.md` before
touching a component; the checklist in its §7 is what "done" means here.

It shares a page with [`adi-css`](../adi-css), the `adi-*` BEM layer the control panel's
workbench is written against. Both resolve to the same token file, so the two never disagree
on a colour; where they overlap, the one linked second (adi-css) wins on the reset, and
nothing else is shared.

## What's in it

| Component | Notes |
| --- | --- |
| `Icon` / `IconSize` / `Lucide` | One Lucide glyph, drawn the one way §9 allows: stroke 1.5, sizes 14/16/20/24, `currentColor`. `Lucide` is generated from `icons/*.svg` — an icon not in the set is a compile error, never a blank |
| `Mark` / `MarkVariant` | The three hexagons (§10). Monochrome by default; `accent` is the coloured build, for the app icon and the landing only. No gloss, no gradient, no motion |
| `Button` | `Default` (translucent), `Primary` (**the** orange — one per screen), `Strong` (ink fill, the main action when orange is spent), `Ghost` (quiet), `Danger` (red text), `Link`; two sizes; an optional `Lucide` icon |
| `Badge` / `BadgeTone` | A pill on a 12% tint — `set`, `idle`, `blocked`. Never a filled block, never orange |
| `Dot` / `DotTone` | The 6px dot before a word: `Ok`, `Live` (orange — the one live state), `Warn`, `Err`, `Idle` |
| `Panel` | A section, not a card: a 16px/600 title line, a hairline, the content. Flush. An `id` makes it an anchor, for a page long enough to be linked into by chapter |
| `Form` / `Hint` | The strip of controls under a section — a hairline above, fields aligned on their inputs; stacks below 620px. `toolbar` for bare controls |
| `Field` | Label above in 13px `ink-2`, an optional `?` whose text opens on hover or focus |
| `Input` / `Textarea` / `Select` | The raised frame with a strong hairline. **Sans by default**; `mono=true` when the value is a machine value. `Select` draws its own Lucide chevron |
| `Flash` / `Empty` | One line of 13px text — `Ok` in `ok`, `Err` in `err`, `Info` in `ink-2`; and the quiet line an empty list shows |
| `TopBar` / `Crumbs` / `Crumb` | 48px on `bg-side` with a hairline under it. The monochrome mark at 18px, `adi` in 15px/600 sans, the path in 13px sans (a location is not a machine string) |
| `Modal` | A card over a scrim; three ways out. No blur, no shadow, no fade |
| `Menu` / `MenuAt` / `MenuHead` / `MenuItem` / `MenuTick` / `MenuNote` / `MenuLink` | The popover a control drops under itself — a row's `⋯`, a right-click, a checklist. Fixed to the viewport (so a scroll container can never clip it), anchored from the left at a point or in from the right edge, and dismissed three ways like a dialog. `MenuItem` is an action, or a `checked` box (or `radio`), or `danger`, or listed-but-`disabled` |
| `Faq` / `Qna` | Questions folded under themselves, on native `<details>` |
| `PathPicker` / `DirEntry` / `PathRoot` | A directory, typed or browsed to. Names in mono, the sheet raised, the confirm `Strong` |
| `Tree` / `TreeNode` / `TreeState` | The explorer: rows in `ink-2`, the open row `bg-active` with a 3px `accent` marker, glyphs at 14 in `ink-3`, a Lucide chevron for the twisty. Knows nothing about files |
| `CodeEditor` / `CodeFrame` / `CodeLog` | A code block you can type in: `bg-raise`, hairline, radius 10, mono 12.5/1.6 |
| `Markdown` | The rendered half of a `.md` and of anything an agent says; transcript type, 80ch |
| `Rail` / `RailGroup` / `RailCard` | The column down either side of a chat. **Flush**: it draws no edge and no radius — the pane it sits in owns the surface and the hairline. 15px title, 12px sentence-case bands, `7px 8px` rows on `bg-hover`/`bg-active` |
| `SessionItem` / `SessionState` | A conversation as a row: title in `ink`, meta in `ink-3` with the agent in `ink-2`, a 6px dot for the states that have one, the keyboard `shortcut` shown on hover and on the open row |
| `Kbd` | A shortcut, as quiet 12px `ink-3` text |
| `AppItem` / `AppState` / `RowMenu` | A living app as a row in the right rail; its state is a dot and its own words |
| `Chat` / `Composer` / `Ask` / `Attached` / `MicButton` / `TurnBlocks` / `StopLine` | The transcript and the box you type in: `bg` under the words, 15.5px/1.6 at 80ch, tool calls collapsed to a receipt, the send button the screen's one orange |
| `TokenStream` / `PromptText` / `Token` | One prompt, two readings; template seams marked by weight, not colour |
| `ToolForm` / `Param` / `ParamKind` | A tool's parameters as a form, from its own declaration |
| `Simulator` / `ToolDecl` | The screen where a person takes the model's seat |
| `PairCard` / `PairQueue` / `Pair` / `Verdict` / `Ruling` | Two facts side by side and the four ways to rule on them; `Verdict::title()` for a button, `label()` for a pill |
| `Fact` / `FactRow` / `FactCard` / `StaleList` / `FactHistory` | One sentence with both its names on it, and what an edit left out of date, as was/now |
| `FlagMark` / `FlagList` / `Flag` | Select a passage and mark what is wrong with it |
| `TxPanel` | The open transaction and the commit that is off until nothing is |
| `Table` / `Column` / `Row` / `EmptyRow` / `TableState` | No card. 12px `ink-3` headers over a strong hairline, the sorted column in `ink-2` with its arrow, hairlines between rows, `bg-hover`, the first cell flush left, the column picker behind a `Settings2` gear |

## Develop here

```sh
cd crates/adi-ui && trunk serve --open      # http://127.0.0.1:9081
```

That is the **playground** ([`playground/main.rs`](./playground/main.rs)): every component,
every variant, one page, hot-reloaded. It runs standalone — no API, no `adi-app` — and is
deliberately not port `9080`, so it can run beside `scripts/dev.sh`'s webapp server.

Its first panels are the design system itself — the type scale, the surface ladder, the
inks, the accent and the semantic colours, rendered from the live tokens, then the whole icon
set. A wrong value in `design/tokens.css` shows up there rather than three components deep.

**When you add a component, add a row to the playground showing every arm of every enum it
takes.** A variant nothing renders is a variant nobody notices is broken.

## The style: surfaces, not boxes

**Grouping is done with background tone and hairlines, never with cards** (§2.5). A sidebar
is `bg-side`; the page and the transcript are `bg`; an input or a code block is `bg-raise`; a
hovered row is `bg-hover`, the open one `bg-active`. Panels are flush to the edges — no
radius, no border, a 1px `border-line` where two meet. Nothing casts a shadow, and nothing
blurs.

A **card** (`island`: a `border-line` hairline and `rounded-lg`) is for a genuinely
detachable thing — a pairing block with a QR, a dialog, a code block. Never around a table,
never inside another card, never for a stat.

**One orange per screen** (§2.4). `text-accent` / `bg-accent` fill exactly one element: the
most important action or live state. `ButtonVariant::Primary` is that element; every other
main action is `Strong`. Orange is never a selected state, an outline, a link, or a heading.

**Mono means machine** (§2.3). `font-mono` — or the `mono` utility, or a component's `mono`
prop — is for paths, hashes, commands, ids, config values, model ids. Names, counts, dates,
labels and tool names are sans.

**Sentence case, always** (§2.6). A label is 12px `ink-3` sentence case; the `label` utility
says so. There is no uppercase utility any more, on purpose.

## The one rule

**A class name must appear in the source as a whole string literal.**

Tailwind generates CSS by scanning this crate's `.rs` files for text that looks like a
utility. It never runs the code, so it cannot see a name that only exists at runtime:

```rust
// found — the literal is right there
Self::Primary => "bg-accent text-on-accent hover:bg-accent-hover",

// NOT found — no `.bg-accent` is ever generated, and the element renders unstyled
let class = format!("bg-{}", tone.name());
```

Write the complete list per branch, as [`ButtonVariant::classes`](./src/button.rs) does.

## Tokens

[`styles/tokens.css`](./styles/tokens.css) holds nothing of its own: it imports
`design/tokens.css` and the self-hosted faces, and sets the app to dark. **The app is dark**
(§3); light mode is for the landing and docs, which opt in with `.light` themselves. There is
no theme toggle and there are no `dark:` variants — a token is already the app's value.

[`styles/ui.css`](./styles/ui.css) maps the tokens onto Tailwind's theme and wipes everything
else — Tailwind's palette, its type scale, its radii, its shadows — so the design system
cannot be worked around by accident.

### Colour

| Group | Utilities |
| --- | --- |
| surfaces | `bg` `side` `hover` `raise` `active` |
| lines | `line` `line-strong` |
| ink | `ink` `ink-2` `ink-3` `code` |
| accent | `accent` `accent-hover` `on-accent` |
| semantic | `ok` `warn` `err` and their 12% tints `ok-soft` `warn-soft` `err-soft` |
| fills | `chip` `chip-hover` (tags, pills) · `btn` `btn-hover` (the default button) · `scrim` |
| syntax | `syn-plain` `syn-comment` `syn-str` `syn-num` `syn-key` `syn-kw` `syn-func` `syn-punct` — the code view's own palette, the one place a hue steps outside the greys |

Each works everywhere Tailwind takes a colour: `bg-side`, `text-ink-3`, `border-line`,
`bg-ink/10`.

### Type

Named by role, not by size — there is no `text-lg` to reach for.

| Utility | Size | For |
| --- | --- | --- |
| `text-label` | 12px | field labels, table headers, section dividers — `ink-3`, sentence case |
| `text-mini` | 12px | meta lines, timestamps, second lines |
| `text-mono` | 12.5px | paths, hashes, commands, ids |
| `text-small` | 13px | help text, notices — `ink-2` |
| `text-row` | 13.5px | list items, buttons (500) |
| `text-ui` | 14px | inputs, table cells |
| `text-body` | 15.5px / 1.6 | the transcript, at most 80ch |
| `text-section` | 16px | panel headers, form sections, card titles (600) |
| `text-title` | 22px | one per page (600) |
| `text-metric` | 20px | the number in a stats row (500) |

Four composite utilities: **`label`** (12px sentence case, sets no colour), **`mono`** (the
machine face at 12.5px in `code`), **`metric`** (sans 20px/500, tabular figures, so a counter
ticking upward never shifts the layout), and **`island`** (a card — see above).

Spacing is `--spacing: 4px`, so `p-1/2/3/4/6/8/12` is the 4/8/12/16/24/32/48 scale (§5).
Radii are `rounded-sm` (4, inline code and seg-control items), `rounded-md` (6, the default),
`rounded-lg` (10, cards and code blocks) and `rounded-full` (pills). Shadows are gone from
the theme; `shadow-*` generates nothing.

### Fonts

**Geist** (`font-sans`) for everything people read, **Geist Mono** (`font-mono`) for
everything machines wrote, and **Bricolage Grotesque** (`font-display`) for landing headlines
only — all three self-hosted from [`fonts/`](./fonts) as per-script variable woff2 subsets,
so the control panel makes no third-party request and the installed app keeps its type
offline. Cyrillic is included: transcripts are read in Russian as often as in English.

`scripts/fonts.sh` refetches them and rewrites `fonts/fonts.css`; [`fonts/README.md`](./fonts/README.md)
has the table and the licences.

### Icons

**Lucide, and only Lucide** (§9). The set is [`icons/`](./icons) — one SVG per name, verbatim,
licence header included — and `build.rs` turns the directory into the `Lucide` enum, so
adding an icon is:

```sh
scripts/lucide.sh add arrow-up-right      # checks the name against lucide.dev, fetches it
```

and then `Lucide::ArrowUpRight` exists. Never draw a glyph by hand, never use a Unicode
character as an icon (`⋯`, `★`, `▸`, `×` — those are `Ellipsis`, `Star`, `ChevronRight`,
`X`), and never paste a path. The noun → icon map in `DESIGN.md` §9 decides which glyph a
thing gets; pick from it, not per screen.

## Using it from another crate

Three lines, no build-script changes. In the consumer's Tailwind entry:

```css
@import "../../adi-ui/styles/ui.css";   /* Tailwind, tokens, fonts, reset, and adi-ui's own @source */
@source "../src";                        /* plus the consumer's own components */
```

then in its `index.html`:

```html
<link data-trunk rel="tailwind-css" href="styles/tailwind.css" />
<!-- The @font-face rules resolve against the dist root, so the files have to land there. -->
<link data-trunk rel="copy-dir" href="../adi-ui/fonts" />
```

and in its `Cargo.toml`, `adi-ui = { workspace = true }`.

`ui.css` scans **its own** `src/` through a path relative to itself, so the consumer gets the
components' classes generated without knowing where this crate lives. Do not import
`tailwindcss` separately — `ui.css` already does, and twice would duplicate the whole utility
layer.

## Notes

- Targets `wasm32-unknown-unknown` and is **excluded from the workspace's
  `default-members`**, so a bare `cargo build`/`cargo test` skips it. Build it with
  `cargo build -p adi-ui --target wasm32-unknown-unknown`, or just run Trunk. `cargo test -p
  adi-ui` runs the host-side tests (the mark's geometry, the icon set, the highlighter, the
  table's sort and layout).
- [`Trunk.toml`](./Trunk.toml) pins **`tailwindcss = "4.2.1"`**. Trunk's default is 3.3.5,
  which cannot parse a v4 stylesheet. Pinned, Trunk downloads and caches the binary itself:
  no Node, no npm, nothing to preinstall.
- Utilities are **unlayered** on purpose. Unlayered CSS beats layered CSS whatever the
  specificity, so `@layer utilities` would lose to any stray unlayered rule in a host page.
- The reset strips a control's **own border and background** (`border: 0 solid`,
  `background-color: transparent`), the way Tailwind's preflight does, so a component may set
  a *partial* border without the platform drawing the other three sides in `outset`.
- `dist/` is a dev artifact and is not committed.
