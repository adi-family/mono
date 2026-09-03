# adi-css

The **adi design system** as `adi-*` classes — SCSS compiled to CSS, shared by every adi web
surface. The rules it draws are [`design/DESIGN.md`](../../design/DESIGN.md); the values it
draws them in are [`design/tokens.css`](../../design/tokens.css). Neither lives here.

## What's in it

SCSS source lives in [`scss/`](scss), compiled in `@use` order (which sets the cascade):

| File | Layer | Holds |
| ---- | ----- | ----- |
| `_tokens.scss`     | tokens     | Nothing of its own: `@use`s `design/tokens.css` and sets the app to dark |
| `_base.scss`       | base       | Minimal reset, the focus ring, platform styling taken off controls |
| `_mixins.scss`     | (helpers)  | `label`, `mono`, `card`, `chip`, `focus-ring`, `narrow` |
| `_components.scss` | components | The `adi-*` BEM classes: workbench shell, explorer, store rail, menus, page header, notice, status, buttons, segmented control, panel, table, chips, form, field, input, flash |
| `_utilities.scss`  | utilities  | `adi-mono`, `adi-muted`, `adi-label`, `adi-link`, `adi-check`, `adi-visually-hidden`, … |

Classes are `adi-` prefixed and BEM (`block__element--modifier`), so the system composes with
a host page without collisions.

## The tokens live in `design/tokens.css`

Every colour, face, radius and spacing value is a CSS custom property declared once, at the
repository root, and loaded here as plain CSS. Both compilers of this system read it directly:
dart-sass (Trunk, for the wasm webapp) and grass (`build.rs`, for the `STYLESHEET` const) both
resolve `@use "../../../design/tokens"` to that file. Change a value there and every surface
follows — the webapp, the mesh client, the server-rendered pages, the adi-ui components. Never
restate a hex in a component.

## Who uses it, and how

The source of truth is `scss/adi.scss`; the two consumers compile it independently, so the
look can't drift:

- **The wasm webapp** ([`adi-webapp`](../adi-webapp)) links it through Trunk
  (`<link data-trunk rel="scss" …>`), so the CSS lands in `<head>` — no flash of unstyled
  content.
- **Server-rendered pages** ([`adi-app`](../adi-app)'s placeholder, the front door's error
  pages) inline the [`STYLESHEET`](src/lib.rs) const, since they have no build step. The
  crate compiles the SCSS with the pure-Rust `grass` compiler in `build.rs` — no external
  Sass binary.

```rust
// server-side: drop the whole design system into a page <head>
let head = adi_css::style_tag();          // "<style>…</style>"
let css  = adi_css::STYLESHEET;            // the raw CSS
```

## Working on styles

```sh
scripts/dev.sh          # API backend + Trunk dev server
```

Open **http://127.0.0.1:9080**. Trunk watches the webapp, this crate's `scss/` and
`design/`, so editing a token or a component reloads the browser instantly — no wasm rebuild.

- **Token** — edit `design/tokens.css`. Everything using `var(--…)` updates on save, in every
  surface at once.
- **Component** — edit `_components.scss`. Use the mixins; keep classes `adi-` prefixed and
  BEM; check the new class against `DESIGN.md` §8 before adding it.

## The rules, in the shape this file enforces them

- `.adi-btn--accent` is the one filled orange a screen gets, chosen per page. `.adi-btn--primary`
  is an **ink** fill — the page's main action when orange is spent elsewhere, or when the page
  has no single live state to give it to.
- `.adi-panel` is a section, not a card: a title line, a hairline, the content. A table never
  sits in a box.
- `.adi-input` is sans. Add `.adi-mono` only when the value is a machine value.
- Labels (`.adi-field__label`, `th`, `.adi-label`) are 12–13px, grey, sentence case. There is no
  uppercase mixin any more, on purpose.
- Nothing here casts a shadow, blurs, or animates without being asked.
