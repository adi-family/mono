# adi-ui

The **adi component library** — [Leptos](https://leptos.dev) components styled with
[Tailwind](https://tailwindcss.com) utilities over its own design tokens.

It is self-contained: tokens, reset, type scale and utilities all come from
[`styles/ui.css`](./styles/ui.css), and nothing here depends on
[`adi-css`](../adi-css). That crate still owns the `adi-*` BEM layer every existing screen
is written against and is untouched by anything in here.

**The two do share a page**, and `adi-webapp` is where: its title bars are built from this
crate while everything under them is still `adi-*`. That works because of load order and
one property of the reset — see
[`adi-webapp/styles/tailwind.css`](../adi-webapp/styles/tailwind.css). In short: this
stylesheet is linked *first*, so where the two name the same token (`--ink`, `--accent`,
`--on-accent`, `--shadow`) adi-css wins and nothing that already exists changes colour;
everything only this crate names (`--card`, `--bar`, `--edge`, …) comes from here. The
reset lives in `@layer base` and adi-css is unlayered, so adi-css outranks it wherever they
overlap. Migrate a screen by rewriting it, not by hoping the palettes meet in the middle.

## What's in it

| Component | Notes |
| --- | --- |
| `Button` | 5 variants × 2 sizes; `submit`, `disabled`, and an `icon` drawn in `currentColor` at the size the button picks. Handlers attach as `on:click`, not a prop |
| `Badge` | 5 status tones, `mono` for ids/ports/counts |
| `Panel` | titled surface with optional header `actions`; `flush` for a child that owns its edges |
| `Form` / `Hint` | the strip that closes a panel; `toolbar` for bare controls. Stacks below 620px |
| `Field` | label + a `?` that explains the control without costing the row any height |
| `Input` / `Textarea` / `Select` | one shared frame; optional two-way binding to an `RwSignal<String>` |
| `Flash` / `Empty` | inline or card feedback in 3 kinds; the quiet line an empty list shows |
| `TopBar` | the window's lid: the wordmark (a link home when given one), a middle slot for where you are, actions right. Wall to wall and `sticky` — the one component that is not an island |
| `Modal` | a dialog over a scrim, with three ways out: the close button, the scrim, `Escape` |
| `Faq` / `Qna` | questions folded up under themselves, on native `<details>`. Answers are Markdown |
| `Crumbs` / `Crumb` | the path to what is open, for the bar's middle slot. The last segment is never a link |
| `PathPicker` / `DirEntry` / `PathRoot` | a directory, typed **or** browsed to — one value, so pasting a path lands you in it and clicking a folder grows the text. Filters as you type, completes on `Tab`, walks on `↓↑`. Lists nothing itself: it names the one directory it wants read, the caller reads it |
| `Tree` / `TreeNode` / `TreeState` | an IDE tree from one flat, depth-annotated list: indent rails, a turning chevron, selection, keyboard activation. Knows nothing about files |
| `CodeEditor` | a painted `<pre>` under a transparent `<textarea>` — the browser keeps the caret, undo, IME and paste. `Lang::from_path` picks the scanner: Rust, TOML, JSON, YAML, TS, shell, SQL, Markdown |
| `CodeFrame` | the card a file is read in: the name on the left, `actions` on the right, whatever is showing the file underneath. Not part of `CodeEditor`, so a preview wears the same chrome |
| `Markdown` | the rendered half of a `.md` — and of anything an agent says. Renders through views, never `inner_html`, and allow-lists link schemes |
| `Rail` / `RailGroup` / `RailCard` | the column down either side of a chat: a title, an optional filter box that sticks while the title scrolls away, labelled bands, and the card every row is. Knows nothing about what a row holds |
| `SessionItem` / `SessionState` | a conversation, as a row in the left rail: done, waiting, error, working |
| `AppItem` / `AppState` / `RowMenu` | a **living app**, as a row in the right rail: its favicon leads, the band is its project, the name under its title is the fleet node it runs on. Live, offline, view-only — and the state says its own words, so no row can put an age there |

## Develop here

```sh
cd crates/adi-ui && trunk serve --open      # http://127.0.0.1:9081
```

That is the **playground** ([`playground/main.rs`](./playground/main.rs)): every component,
every variant, one page, hot-reloaded. It runs standalone — no API, no `adi-app`, nothing
to start first — and it is deliberately not port `9080`, so it can run beside
`scripts/dev.sh`'s webapp server.

Its first two panels are the design system itself — the type scale and the whole palette,
rendered from the live tokens. A wrong value in `tokens.css` shows up there rather than
three components deep.

The header toggle switches OS / light / dark. Check a component in all three before calling
it done.

**When you add a component, add a row to the playground showing every arm of every enum it
takes.** A variant nothing renders is a variant nobody notices is broken.

## The style: islands

**A screen is a few distinct objects floating on the canvas, not one edge-to-edge plane cut
into regions by hairlines.** The rail is an island, the panel is an island, the editor is an
island; between them is canvas, and the gap is what says they are separate things. Nothing
here is full-bleed and nothing is divided by a bare border down the middle of the window.

The shape is a utility, so it is written once:

```css
@utility island {
  border-radius: var(--radius-md);   /* 8px */
  border: 1px solid var(--edge);
  box-shadow: var(--shadow);         /* a hairline — depth is a line, not a lift */
}
```

Two things it deliberately leaves alone:

- **No fill.** The surface is still yours: `island bg-panel` for a rail, `island bg-card`
  for a panel. That is what lets a rail sit a shade behind the panel next to it.
- **No `overflow`.** A [`Field`](./src/field.rs)'s hint bubble has to be able to leave the
  panel it is anchored in. An island whose children must be clipped to its corners — a
  header strip's fill, a scrolling body — adds `overflow-hidden` itself, as
  [`CodeFrame`](./src/code.rs) and [`Rail`](./src/rail.rs) do.

**The one exception is [`TopBar`](./src/topbar.rs).** It goes wall to wall on `bg-bar` with a
hairline under it, because it is the screen's own edge rather than an object on the screen —
an edge that floats reads as a card stuck to the ceiling. Everything below it is islands.

**A component that is a thing draws its own island.** `Rail` does not wait for a
caller to put a border around it; a rail is an object on the screen, not a region of one, and
the wrapper that used to draw it was the same four utilities at every call site. A component
that is *part* of a thing — a row, a group, a form strip — draws nothing.

## The one rule

**A class name must appear in the source as a whole string literal.**

Tailwind generates CSS by scanning this crate's `.rs` files for text that looks like a
utility. It never runs the code, so it cannot see a name that only exists at runtime:

```rust
// found — the literal is right there
Self::Primary => "bg-accent-fill text-on-accent hover:opacity-90",

// NOT found — no `.bg-accent` is ever generated, and the element renders unstyled
let class = format!("bg-{}", tone.name());
```

Write the complete list per branch, as [`ButtonVariant::classes`](./src/button.rs) does.
This is the single most common way to get a component that looks right in one place and
unstyled in another.

## Tokens

[`styles/tokens.css`](./styles/tokens.css) is the palette. Every token is a single
`light-dark()` declaration rather than the usual four blocks (OS light, OS dark, and an
explicit override each way): `color-scheme` picks the half, so the theme toggle only flips
that one property and a token cannot drift between copies it does not have.

> **There are no `dark:` variants in this crate, and there should never be one.**
> `bg-card` compiles to `background-color: var(--card)`, and that token is already both
> themes.

**The dark half is the specified palette; the light half is derived from it** — same
green-tinted neutrals and mint accent, darkened where a value has to carry text on white.
Correct any of them in `tokens.css` and every component follows.

### Colour

| Group | Utilities |
| --- | --- |
| surfaces | `canvas` `stage` `panel` `panel-alt` `bar` `card` `bubble` `selected` |
| lines | `divider` `frame` `edge` `edge-2` `dim` |
| text | `ink` `body` `secondary` `meta` `placeholder` `faint` `fainter` |
| accent | `accent` `accent-fill` `on-accent` `accent-soft` `accent-soft-edge` `tip` `tip-edge` |
| states | `err` `err-btn` `err-bg` `err-bg-2` `err-edge` `err-edge-2` `queue` `queue-ink` `queue-bg` `queue-edge` `attention` |
| syntax | `syn-plain` `syn-comment` `syn-str` `syn-num` `syn-key` `syn-kw` `syn-func` `syn-punct` |

The `syn-*` family is the code view's own palette, and the only place a hue steps outside
the green-tinted system: five colours have to stay apart at a glance inside a file. It is
what [`Tok::classes`](./src/highlight.rs) returns, so correcting one value in `tokens.css`
recolours every editor.

Each works everywhere Tailwind takes a colour — `bg-card`, `text-meta`, `border-edge`,
`bg-accent/12`. Tailwind's own 22-family palette is removed, so these are the only colours
reachable.

### Type

Named by role, not by size — there is no `text-lg` to reach for when what you meant was a
metric.

| Utility | Size | For |
| --- | --- | --- |
| `text-caps` | 10.5px | small caps labels |
| `text-mini` | 12px | meta and secondary |
| `text-row` | 13.5px | list rows and buttons |
| `text-msg` | 14.5px | chat body |
| `text-sub` | 17px | an answer's subheading |
| `text-title` | 20px | screen titles |
| `text-metric` | 23px | metric numbers |

Three composite utilities bundle what a role always wants together:

- **`caps`** — mono, 10.5px, `0.12em` tracking, uppercase. Sets no colour, so it composes
  with `text-faint` / `text-meta`.
- **`metric`** — mono, 23px, tabular figures, so a counter ticking upward never shifts the
  layout under it.
- **`attention-pulse`** — a 12% `--attention` wash on a `::before`, breathing between 40%
  and 100% opacity every 5s. For the one row that is waiting on *you*. It is a wash rather
  than a background, so the element keeps its own fill and its own `hover:`; it inherits
  the radius, so it composes with any card; and `prefers-reduced-motion` holds it still
  rather than dropping it, because the tint is the state and only the motion is decoration.

Spacing is `--spacing: 4px`, so `p-1/2/3/4` is the 4/8/12/16 rhythm with everything between
still available. Radii are `rounded-sm` (5px) and `rounded-md` (8px).

### Fonts

**IBM Plex Sans** (`font-sans`) for interface text and **JetBrains Mono** (`font-mono`) for
the logo, agent and machine names, commands, metric digits and caps labels.

Both are **self-hosted** from [`fonts/`](./fonts) — no third-party request, and the PWA
keeps its typography offline. Both are variable fonts, so one 119 KB + 53 KB pair covers
every weight; `@font-face` declares a weight *range* and the browser interpolates 500 out
of the same file it used for 400. Latin, Cyrillic and Greek all survive intact — nothing
was subsetted. [`fonts/README.md`](./fonts/README.md) records how they were built and their
OFL licences.

No italics are shipped; a browser synthesises an oblique where markdown asks for emphasis.

## Using it from another crate

Three lines, no build-script changes. In the consumer's Tailwind entry:

```css
@import "../../adi-ui/styles/ui.css";   /* Tailwind, tokens, reset, and adi-ui's own @source */
@source "../src";                        /* plus the consumer's own components */
```

then in its `index.html`:

```html
<link data-trunk rel="tailwind-css" href="styles/tailwind.css" />
<!-- The @font-face rules resolve against the dist root, so the files have to land there. -->
<link data-trunk rel="copy-dir" href="../adi-ui/fonts" />
```

and in its `Cargo.toml`, `adi-ui = { workspace = true }`.

`ui.css` scans **its own** `src/` through a path relative to itself, so the consumer gets
the components' classes generated without knowing where this crate lives. Do not import
`tailwindcss` separately — `ui.css` already does, and twice would duplicate the whole
utility layer. The tokens and the reset travel with the stylesheet; the fonts are the one
thing that does not, because they are files rather than CSS — hence the `copy-dir` above.
Without it everything still works, in the system font stack.

## Notes

- Targets `wasm32-unknown-unknown` and is **excluded from the workspace's
  `default-members`**, so a bare `cargo build`/`cargo test` skips it. Build it with
  `cargo build -p adi-ui --target wasm32-unknown-unknown`, or just run Trunk.
- [`Trunk.toml`](./Trunk.toml) pins **`tailwindcss = "4.2.1"`**. Trunk's default is 3.3.5,
  which cannot parse a v4 stylesheet — it dies inside `postcss-import` on the first
  `@import`. Pinned, Trunk downloads and caches the binary itself: no Node, no npm, nothing
  to preinstall, and CI needs no new step.
- Utilities are **unlayered** on purpose. Unlayered CSS beats layered CSS whatever the
  specificity, so `@layer utilities` would lose to any stray unlayered rule in a host page.
- The reset strips a control's **own border and background** (`border: 0 solid`,
  `background-color: transparent`), the way Tailwind's preflight does. Without it a
  `<button>` keeps the platform's `outset` border on every side a utility does not set, so
  a *partial* border — the sessions row's `border-l-2` — comes out as a 3D box. Every
  component sets its own fill and its own lines; nothing should inherit either.
- `dist/` is a dev artifact and is not committed.
