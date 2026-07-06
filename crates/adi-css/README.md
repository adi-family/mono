# adi-css

The **adi design system** — tokens + components as SCSS, compiled to CSS. One visual
identity, shared by every adi web surface.

## What's in it

SCSS source lives in [`scss/`](scss), compiled in `@use` order (which sets the cascade):

| File | Layer | Holds |
| ---- | ----- | ----- |
| `_tokens.scss`     | tokens     | Colour/space/type/radius/shadow as CSS custom properties; light + dark, plus explicit `data-theme` overrides |
| `_base.scss`       | base       | Minimal reset, document defaults, the focus ring |
| `_mixins.scss`     | (helpers)  | Sass mixins: `card`, `eyebrow`, `focus-ring`, `narrow` breakpoint |
| `_components.scss` | components | `adi-*` BEM classes: shell/sidebar/nav, bar, status pill, buttons, card/tiles, panel, table, chip, form, flash, footer |
| `_utilities.scss`  | utilities  | Small helpers: `adi-mono`, `adi-muted`, `adi-visually-hidden`, … |

Classes are `adi-` prefixed and BEM (`block__element--modifier`), so the system composes
with a host page without collisions. Theme flips at runtime via `<html data-theme="…">`.

## Who uses it, and how

The source of truth is `scss/adi.scss`; the two consumers compile it independently, so the
look can't drift:

- **The wasm webapp** ([`adi-webapp`](../adi-webapp)) links it through Trunk
  (`<link data-trunk rel="scss" …>`), so the CSS lands in `<head>` — no flash of unstyled
  content.
- **Server-rendered pages** ([`adi-app`](../adi-app)'s placeholder, future front-door
  pages) inline the [`STYLESHEET`](src/lib.rs) const, since they have no build step. The
  crate compiles the SCSS with the pure-Rust `grass` compiler in `build.rs` — no external
  Sass binary.

```rust
// server-side: drop the whole design system into a page <head>
let head = adi_css::style_tag();          // "<style>…</style>"
let css  = adi_css::STYLESHEET;            // the raw CSS
```

## Working on styles

Develop the design system against the live webapp with hot reload.

### Start the loop

```sh
scripts/dev.sh          # API backend + Trunk dev server
```

Open **http://127.0.0.1:9080**. Trunk watches both the webapp and this crate's `scss/` (via
the webapp's `Trunk.toml [watch]`), so editing **`scss/*.scss`** (a token, a component)
reloads the browser **instantly** — no wasm rebuild. Use the ◐ button on the page to check
light **and** dark.

### Change a token or a component

- **Token** — edit `_tokens.scss`. Colours go in the `$light` / `$dark` maps and flow to
  both themes through the `theme()` mixin; the space/type/radius scales are the
  theme-neutral block. Everything using `var(--…)` updates on save.
- **Component** — edit `_components.scss`. Reuse the `card` / `eyebrow` mixins from
  `_mixins.scss` for consistency, and keep classes `adi-` prefixed and BEM.
