# ADI design system

This document is complete on its own. If you are an agent asked to build or change any ADI UI — app screen, landing section, docs page — read this file fully before writing a line, then read `tokens.css` and open one of the `examples/`. Do not "improve" the system; apply it. When something here conflicts with your instinct, this file wins.

Files in this folder:

- `DESIGN.md` — this file: logic, rules, how to make decisions.
- `tokens.css` — every color, font, radius, spacing value as CSS variables. Import it. Never hardcode a hex.
- `reference/design-system.html` — the visual brandbook: tokens, type scale, components rendered live.
- `examples/chat.html` — the main session screen (3 panels, transcript). The canonical example.
- `examples/setup-agents-fleet.html` — a form, a table page, a settings page. Three pages, one switcher at top (switcher is for review only).
- `examples/landing-hero-concepts.html` — five landing hero layouts. Light mode.
- `examples/landing.html` — the full withadi.dev landing in the system.

---

## 1. What ADI is, and what the design is for

ADI runs AI agents on the user's own machine. The user is technical, runs many agents at once, and reads a lot of agent output. The screens are dense because the work is dense.

The design's job is to keep density readable. It is not to decorate, not to add whitespace, not to look like a marketing site. A good ADI screen looks like a well-made tool: quiet chrome, clear hierarchy, the content in front.

Two things made earlier versions look amateur or AI-generated, and this system exists to prevent them:

1. **Uniform loudness.** Everything was mono, everything was uppercase, everything was in a card. When everything is emphasized, nothing is.
2. **Decoration without a rule.** Glows, gradients, orange in five places, badges on every row. Each one was a local choice; together they read as noise.

Every rule below is a fix for one of those two.

---

## 2. Principles (the six decisions every screen inherits)

**1. The transcript is the product.**
On any screen with a transcript, it sits on the lightest large surface (`--bg`), with the largest body text (15.5px), and the widest measure (80ch). Sidebars and panels sit on `--bg-side`, use 13–14px, and mostly `--ink-2`/`--ink-3`. The eye must land on what the agent said.

**2. Density is honest.**
Do not hide information to make a screen calmer. Make it scannable: weight for names, `--ink-3` for meta, hairlines between rows, dimming for values that repeat in every row. Whitespace inside the app is 4–32px; 48 is the maximum inside a panel.

**3. Mono means machine.**
Monospace is only for things a machine produced or will consume: file paths, hashes, commit ids, commands, run ids, config values (`bypassPermissions`), model ids (`opus`, `glm-5.3`), env var names, tokens, grants (`http:app`). Everything else — names of sessions, agent names in meta lines, counts, dates, times, labels, tool names in lists — is sans. Test: "would this string appear in a terminal or a config file verbatim?" If not, sans.

**4. One orange per screen.**
`--accent` fills exactly one element: the most important action or live state on that screen. Send button in chat. Save in a form. Update banner button when an update exists. The running dot. If a screen seems to need two, one of them becomes `.btn.strong` (ink fill) or a plain default button. Orange is never used for: selected states, outlines, active nav items (except a 3px marker), links inside the app, headings, labels.

**5. Surfaces, not boxes.**
Grouping is done with background tone (`--bg-side` vs `--bg`), hairlines (`--line`), and spacing. Panels are flush to the viewport edges: no radius, no border, just a 1px hairline where they meet. Cards (`--r-lg`, `--line` border) are for genuinely detachable things: a pairing block with a QR, a dialog, a code block. Never a card around a table, never a card inside a card, never stat cards.

**6. Sentence case, always.**
No uppercase tracked labels anywhere. Section headers in panels are 12px `--ink-3` sentence case ("Running now", "This chat"). Field labels are 13px `--ink-2` sentence case. Table headers are 12px `--ink-3` sentence case. Body text is never uppercase.

---

## 3. Color: how to choose

Ask in this order.

**Is it a large surface?** Use the surface ladder. Lower = further from the user.
- Sidebars, bars, right panel → `--bg-side`
- Page / transcript → `--bg`
- Hover on a row or list item → `--bg-hover`
- Input, code block, composer → `--bg-raise`
- Selected item, seg-control selected, answer chip → `--bg-active`
- Small chips/tags/pills → `rgba(255,255,255,.07)` on dark (`rgba(0,0,0,.06)` on light)

**Is it text?**
- Title, primary content, list item title, button label → `--ink`
- Secondary text, values in key-value lists, agent names in meta → `--ink-2`
- Labels, meta lines, placeholders, table headers, timestamps → `--ink-3`
- Anything in mono → `--code`
- Repeated column values (same backend in every row) → `--ink-2` in mono, or `--ink-3`

**Is it a line?**
- Between rows, panels, sections → `--line`
- Input border, table header rule → `--line-strong`

**Is it a state or a signal?**
- Online, set, success → `--ok`, as a 6px dot before text or a pill with 12% tint background
- Warning → `--warn`, same forms
- Error, destructive → `--err`; destructive text actions may be `--err` text
- Running / live → `--accent` dot (this counts as the screen's one orange only if there is no orange button; a running dot plus a send button is acceptable because the dot is 6px)

**Is it the main action?** `--accent` fill, white text, `--accent-hover` on hover. One per screen. Otherwise `.btn` default or `.btn.strong`.

**Light mode** exists only for the landing and docs. Use the `.light` token set in `tokens.css`. Orange is darker there (`#D6431C`) for contrast on paper. Never mix: a screen is dark or light, and the app is dark.

---

## 4. Type

Fonts: **Geist** for all UI and content. **Geist Mono** for machine strings. **Bricolage Grotesque 500** only for landing headlines. Load from Google Fonts. Always `font-feature-settings: "tnum"`.

| Role | Spec | Where |
|---|---|---|
| Display | Bricolage 500 · 48–76px · letter-spacing −0.035em · line-height 0.98 | Landing hero only |
| Page title | Geist 600 · 20–22px · −0.01em | One per page ("Agents", "Fleet", "Reconfigure your agent") |
| Section | Geist 600 · 15–17px | Panel headers, form sections, card titles |
| Transcript | Geist 400 · 15.5px · line-height 1.6 · max-width 80ch | Agent and user messages |
| UI | Geist 400 · 13.5–14px · line-height 1.5 | List items, table cells, inputs; buttons at 500 |
| Small | Geist 400 · 13px · `--ink-2` | Help text, notices, second lines |
| Label | Geist 400/500 · 12px · `--ink-3` · sentence case | Field labels, table headers, panel section dividers |
| Mono | Geist Mono 400 · 12.5–13px · `--code` | Paths, hashes, commands, ids, config values |

Bold (600) is for at most one phrase per paragraph — a verdict, a name. Never whole sentences.
Line length: 80ch in transcript, 64ch in forms and help text. Numbers are always tabular.
Headings never use letter-spacing above 0. Never track out text.

---

## 5. Space, radius, layout

Spacing scale: 4, 8, 12, 16, 24, 32, 48, 64, 96. Nothing else.
- Inside a component (padding, gaps between icon and text): 4–12
- Between elements (fields, rows, list items): 8–16
- Between groups and sections inside the app: 24–32
- Between sections on the landing: 64–96
- Never more than 48 inside an app panel

Radius: `--r-sm` 4 (inline code, seg-control items) · `--r` 6 (buttons, inputs, list items — default) · `--r-lg` 10 (cards, code blocks) · pill (tags, chips, status). Panels: none.

App shell (`examples/chat.html`, `examples/setup-agents-fleet.html`):
```
grid-template-rows:    48px (top bar) · 1fr · 28px (status bar)
grid-template-columns: 248–272px (left) · 1fr · 316px (right, if present)
content padding:       20–32px
forms:                 centered, max-width 640px, no card around them
```
Landing: one container of 1200px. Nav, hero, and every section share the same left edge. Headline column ≤ 32ch. Screenshot at readable scale, never the whole app shrunk to 40%.

---

## 6. Components

Copy the CSS from the examples; the specs are here so you can build one from scratch and get the same result.

**Button** — `padding 7px 14px; radius 6; font 13.5px/500`.
- `.primary`: `--accent` fill, white text. One per screen.
- `.strong`: `--ink` fill, `--bg` text. The page's main action when orange is already used elsewhere.
- default: `rgba(255,255,255,.08)` fill, `--ink` text. Hover `.12`.
- `.quiet`: no fill, `--ink-2` text. Cancel, tertiary.
Never: gradients, glows, colored shadows, outlined orange.

**Input / select** — `--bg-raise`, 1px `--line-strong`, radius 6, `padding 9px 12px`, 14px. `.mono` when the value is a machine value. Select shows a small chevron at right. Placeholder is `--ink-3`.

**Segmented control** — container `--bg-raise` + `--line-strong`, padding 3px. Items 8px padding, `--ink-2`. Selected: `--bg-active` fill, `--ink`, 500 weight. Never orange, never outlined.

**Tag** (category, sans, 12px, pill, `.07` fill, `--ink-2`) · **Chip** (suggested value, mono 12px, same look, clickable) · **Grant** (mono pill with an × button inside; the remove action lives in the object, so the word "Revoke" is never repeated per row).

**Status** — 6px dot + text (`online`, `running`), or pill (`idle`, `set`). Never a filled badge.

**List item** (sessions) — `padding 7px 8px; radius 6`. Title 13.5px `--ink`, one line, ellipsis. Meta 12px `--ink-3` with the agent name in `--ink-2` 500. Active: `--bg-active`. Hover: `--bg-hover`. Keyboard shortcut (`⌃1`) at right, `opacity 0`, shown on hover and on the active item. Live item: 6px `--accent` dot before the title.

**Table** — no card. Header 12px `--ink-3` sentence case, sorted column `--ink-2`, `--line-strong` rule. Rows 9px vertical padding, `--line` separators, hover `--bg-hover`. Identifier columns in mono `--ink-2`. Repeated values dimmed. Empty cell is `—` in `--ink-3`. Row actions in a `⋯` menu at the far right, never inline words per row.

**Key-value list** (right panel) — `grid 74px 1fr; gap 6px 10px; 13px`. Keys `--ink-3`, values `--ink-2`, machine values mono.

**Stats** — a single row of up to three numbers, 20px/500, with a 12px `--ink-3` label under each, then one 12px line of detail. Not boxes. Money to cents, tokens to one decimal (`$6.78`, `84.5k`).

**Tool call** — collapsed row: chevron, "6 calls · Bash", command in mono truncated with ellipsis. 1px `--line` border, radius 8, `--ink-3`. It is a receipt, not a message.

**Ask block** (agent asked, user answered) — 2px `--line-strong` left border, 18px left padding. Small header line in `--ink-3`, question in 15px, answer as a `--bg-active` chip, footer in `--ink-3`.

**Code block** — `--bg-raise`, `--line` border, radius 8, mono 12.5px, line-height 1.6.

**Notice** — one line of 13px `--ink-3` text with a bold `--ink-2` lead word, a link at the right, an × to dismiss. Sits under the header with a hairline below. Not a banner, not a box.

**Composer** — `--bg-raise`, `--line-strong` border, radius 10. Icons `--ink-2`. Send button 32px square, radius 6, `--accent`. This is the chat screen's one orange.

**Form page** — centered 640px, page title 22px + lead in `--ink-2`, stepper as a small row (current step's number filled with `--ink`), then sections. Fields: label above, help text below in 13px `--ink-3`. Footer: Cancel (quiet) left, Save (primary) right. No card around the form.

---

## 7. How to design a new screen (do this in order)

1. **Name the one thing.** What is this screen for? Which single element is its main action or live state? That gets the orange. Nothing else does.
2. **Pick the shell.** Chat layout (3 panels), page layout (explorer + content), or form (centered 640). Don't invent a fourth.
3. **Sort the strings.** For every piece of text: machine (mono) or human (sans)? Title (`--ink`), value (`--ink-2`), or meta (`--ink-3`)?
4. **Group with tone, not boxes.** Which content is the lightest surface? What goes on `--bg-side`? Where do hairlines fall?
5. **Remove repetition.** Any word repeated in every row goes into a menu, an icon, or a column header. Any value repeated in every row gets dimmed.
6. **Check the list.** Section 8. If anything on the "Never" list is present, it is wrong even if it looks fine.
7. **Squint test.** Shrink to 25%. One light area (content), dim chrome around it, one orange dot. If it's uniform grey, hierarchy failed.

---

## 8. Never / Always

**Never**
- Uppercase tracked labels, anywhere
- A card around a table, cards inside cards, stat cards
- Mono for names, counts, dates, labels, tool names in lists
- More than one filled orange element per screen
- Orange outlines or orange backgrounds as a selected/active state
- Gradients, glows, blurred blobs, colored shadows, drop shadows on flat UI
- Repeating an action word per row ("Revoke", "Rename", "Open")
- Animating the mark, or any motion the user didn't trigger
- Body text in uppercase
- Four decimal places on money; raw seconds where minutes read better
- Inventing a new grey. Use the ladder.

**Always**
- Transcript on `--bg`, chrome on `--bg-side`
- Sidebars in smaller, dimmer type than content
- Repeated column values dimmed
- Tool calls collapsed to "N calls · Tool · command…"
- Row actions in a ⋯ menu
- Shortcuts and secondary actions on hover
- Tabular numbers
- Sentence case

---

## 9. Icons

**Library: Lucide** (lucide.dev, ISC licence, npm `lucide` / `lucide-react` / `@lucide/astro`). One library, everywhere — app, landing, docs, menubar popover. Never mix in another set, never draw a custom icon unless Lucide has no equivalent after a real search.

Why Lucide and not the alternatives: it is a stroke set with strict geometry (24px grid, round caps and joins, 2px default), which sits naturally next to Geist and reads as a tool, not a brand. Phosphor is more decorative; Tabler is heavier; Heroicons is too small a set for a product with this many nouns.

**Rendering rules**
- Stroke width **1.5** everywhere (`strokeWidth={1.5}` / `stroke-width="1.5"`). Lucide's default 2 is too heavy against Geist 400; 1.5 matches the type.
- Sizes: **14** inside list-item meta and tags · **16** in tree/explorer, table cells, buttons, panel headers · **20** in landing feature blocks · **24** only for empty states. Nothing else.
- Color follows the text next to it: `--ink-2` beside `--ink` text, `--ink-3` in meta and explorer. Icons are never orange except the send arrow and never a semantic color except inside a status pill.
- Always paired with a text label in the app. The only unlabeled icons are: send, attach, dictate, filter, close, the ⋯ menu — all with `aria-label`.
- No filled variants, no duotone, no circles or squares behind icons.

**Noun → icon** (use these, don't re-pick per screen)
| Noun | Lucide name |
|---|---|
| Sessions / chat | `message-square` |
| Agent | `bot` |
| Project | `folder` |
| Tools | `wrench` |
| Knowledge | `book-open` |
| Code index | `code` |
| Tasks | `list-tree` |
| Triggers | `zap` |
| Services | `server` |
| Dashboards | `layout-dashboard` |
| Secrets | `key-round` |
| Database | `database` |
| Mesh / fleet | `network` |
| Memory | `brain` |
| Settings | `settings-2` |
| Update available | `arrow-up` |
| Send | `arrow-up` (in the orange square) |
| Attach / dictate | `paperclip` / `mic` |
| Running | 6px `--accent` dot, not an icon |
| Online / set | 6px `--ok` dot, not an icon |
| Row actions | `ellipsis` |
| Revoke / remove | `x` inside the pill |
| External link | `arrow-up-right` at 14, after the label |

Landing: 20px, `--ink-2`, stroke 1.5, in feature block headings and the store grid. Never as decoration in the hero.

---

## 10. The mark

Three hexagons, one in front (`reference/design-system.html` has the SVG). Monochrome by default: `currentColor` with the back shapes at 52% and 74% opacity. Colored version (grey / orange / ink) only for the app icon and the landing. Minimum 16px. In the top bar: 18px, next to the wordmark `adi` in 15px/600. No glow, no shadow, no gradient, no animation. It is not a mascot.

---

## 11. Prompt block

Paste at the top of any prompt that asks a model to build or change ADI UI.

```
DESIGN CONSTRAINTS — ADI (follow exactly, do not "improve")

Read DESIGN.md and tokens.css in ~/adi-family/design first. Use the CSS variables from tokens.css; never hardcode colors.

Fonts: Geist (UI), Geist Mono (paths, hashes, commands, ids, config values, model names — nothing else). Landing only: Bricolage Grotesque 500 headlines.
Icons: Lucide only, stroke-width 1.5, sizes 14/16/20, color of the adjacent text, always labeled in the app. Noun→icon map in DESIGN.md §9.
Surfaces (dark): side #101010, page #161616, hover #1B1B1B, raise #1E1E1E, active #232323. Hairline white 7%, input border white 13%.
Ink: #ECEAE6 primary, #A9A6A0 secondary, #6F6C67 labels/meta, #D6D3CD mono.
Accent #E8532A — exactly one filled orange element per screen. Semantic ok #4CB77A / warn #E0A84B / err #E25C5C as 6px dots or pills only.
Spacing 4/8/12/16/24/32/48. Radius 6 default, 4 inline, 10 cards & code, pill for tags. Panels flush: no radius, no border, hairlines only.
Type: page title 20–22px/600, section 15–17px/600, transcript 15.5px/1.6 max 80ch, UI 13.5–14px, labels 12px grey sentence case, tabular numbers.

Never: uppercase labels; nested cards; a card around a table; stat cards; orange as selected state; gradients, glows, blobs, shadows on flat UI; mono for names/counts/dates; repeated per-row action words; unprompted animation.
Always: transcript on the lightest surface; sidebars dimmer and smaller; repeated column values dimmed; tool calls collapsed; row actions in a ⋯ menu; shortcuts on hover.

Before finishing, run section 7 of DESIGN.md (the checklist) against the screen.
```
