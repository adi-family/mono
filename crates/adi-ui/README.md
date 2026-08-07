# adi-ui

The **adi component library** — [Leptos](https://leptos.dev) components styled with
[Tailwind](https://tailwindcss.com) utilities over its own design tokens.

It is self-contained: tokens, reset, type scale and utilities all come from
[`styles/ui.css`](./styles/ui.css), and nothing here depends on
[`adi-css`](../adi-css). That crate still owns the `adi-*` BEM layer every existing screen
is written against and is untouched by anything in here — but adi-ui has a **different
palette**, so the two do not mix on one page.

## What's in it

| Component | Notes |
| --- | --- |
| `Button` | 5 variants × 2 sizes; `submit`, `disabled`. Handlers attach as `on:click`, not a prop |
| `Badge` | 5 status tones, `mono` for ids/ports/counts |
| `Panel` | titled surface with optional header `actions`; `flush` for a child that owns its edges |
| `Form` / `Hint` | the strip that closes a panel; `toolbar` for bare controls. Stacks below 620px |
| `Field` | label + a `?` that explains the control without costing the row any height |
| `Input` / `Textarea` / `Select` | one shared frame; optional two-way binding to an `RwSignal<String>` |
| `Flash` / `Empty` | inline or card feedback in 3 kinds; the quiet line an empty list shows |
| `SessionList` / `SessionGroup` / `SessionItem` / `SessionRollup` | the sessions rail: a filter box over labelled bands of rows. 3 states × selected, plus the dashed line a run of repeats folds into |

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

Two composite utilities bundle what a role always wants together:

- **`caps`** — mono, 10.5px, `0.12em` tracking, uppercase. Sets no colour, so it composes
  with `text-faint` / `text-meta`.
- **`metric`** — mono, 23px, tabular figures, so a counter ticking upward never shifts the
  layout under it.

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
