# The ADI design language

**Look like a tool, not a toy.** Glass for surfaces, opacity for content, fast flat motion,
one accent, and let the user turn the effects down.

This is the language the macOS disk image was built to (`apps/macos/dmg`, the first surface
that follows it end to end) and the one the app should be redesigned onto. It is written to
be argued with: every rule below has a reason and, where it matters, a number.

## Where it comes from

Both platform vendors arrived at the same place in 2026 from opposite directions. Apple went
maximalist with Liquid Glass in 2025 — transparency, refraction, reflective layers — and
corrected at WWDC 2026 after users found text-heavy sidebars and transparent controls hard to
read: a transparency slider from fully opaque to fully clear, less glossy icons, better
system-wide contrast. Google came the other way, from Material 3 Expressive's springy,
brightly coloured, deliberately friendly register, and then added blur and frosted glass
across Android 17 with a stronger readability focus from the start.

The convergence is the useful part: **depth and translucency, contrast non-negotiable, and
the user holding the intensity dial.** What separates a product that reads as infrastructure
from one that reads as a consumer toy is not whether it uses glass — both do now — but where
it refuses to.

## The rules

### 1. Opaque under content, glass on chrome

Blur belongs on sidebars, toolbars, overlays and bars. It does not go behind body text, data,
or anything a person has to read carefully. This is precisely the mistake Apple had to walk
back, and it is not a matter of taste — translucency makes the luminance under text depend on
whatever happens to be behind the window, which means contrast stops being a property you can
guarantee.

In the disk image this rule is load-bearing rather than stylistic: Finder writes each icon
label straight onto the background, so the icons sit on opaque cards and the translucency is
confined to the masthead.

### 2. Contrast is the trust signal, and it is measured

4.5:1 minimum on anything readable. A UI that is hard to read reads as unreliable no matter
how good it looks.

**Measure the rendered pixels, not the intent.** A flat colour being correct is not evidence
that the surface is: a texture, a gradient, a shadow or a noise layer over it moves the value
under the text. `apps/macos/dmg/check-contrast.py` renders the exact glyphs, dilates the mask
by a pixel for the antialias fringe, and reports the worst single pixel — an average hides one
dark speck under one letter. It exists because an earlier draft of that background measured
4.60:1 as a flat colour and really shipped 3.42:1 once a print texture went over it.

Any surface generated rather than hand-picked — a chart fill, a themed avatar, a blurred
photo behind text — needs the same treatment.

### 3. One accent, and it earns its place

`#FA5019`, used on the few things that are genuinely interactive, selected, or the one place
the eye has to travel. Dynamic and multi-colour theming is the playful register; neutrals plus
a single restrained accent reads as infrastructure.

Semantic colour (ok / warning / critical) is a separate axis and does **not** count as the
accent. Two accents is zero accents.

### 4. Motion under 200ms, ease-out, no overshoot

Spring physics with bounce is the expressive register — it signals fun. Ease-out with zero
bounce signals competence. Nothing decorative animates. Honour `prefers-reduced-motion`.

### 5. Geometric shapes, moderate radii

8–14px on surfaces, 8px on small chips. Sharper than about 6px reads technical to the point of
severe; 20px and above reads consumer. Corner radius is a register control, so pick one scale
and hold it.

### 6. Type carries hierarchy, colour does not

Weight and size do the work. Four levels is usually enough — name, secondary, meta,
instruction. Resist coloured labels and badge inflation: the disk image gets exactly one chip,
and it is the only place in the window besides the arrow where the accent appears.

### 7. Give the user the dial

The most copyable idea of 2026: Apple shipped a transparency *slider*, not an on/off switch.
Transparency, density and reduced motion belong in preferences. It signals that we respect
somebody's setup rather than insisting on ours.

A disk image background is a flat picture, so the dial has no home there — but the macOS app
window already runs on `NSVisualEffectView` (`apps/macos/Sources/VisualEffectView.swift`) and
the webapp already blurs in two places (`crates/adi-webapp/styles/main.scss`), so both have
something to attach a preference to.

## Colour

### The accent: `#FA5019`

Luminance 0.266. That value is the whole story — it is bright enough that white sits on it
poorly and dark surfaces set it off well:

| Accent on | Ratio | Verdict |
|---|---|---|
| Dark background `#0A0B0D` | 5.84:1 | text and shapes both fine |
| Dark card `#0F1312` | 5.55:1 | fine |
| White | 3.37:1 | **not for text** — use `#C43A0C` (4.91:1 on the light background) |
| The slate surface `#6E7684` | 1.36:1 | invisible by value; hue alone carries it, so no text and no thin shapes |

The practical rule that falls out: **dark surfaces hold the accent.** On light surfaces it is
a fill with dark text on it, or a darkened variant, never bright orange type. In the disk
image the arrow is orange because it sits on the deep field; on a mid-value surface the same
arrow has to go to ink.

### The balanced neutral: `#6E7684`

Luminance 0.179, which is not an aesthetic choice. Where a surface must stay readable under
text whose colour we do not control — Finder's icon labels are the sharp case, but so is any
user-supplied or theme-inverted content — only luminance **0.175 to 0.183** clears 4.5:1
against black *and* white. `#6E7684` sits in the middle of that window at 4.59 / 4.58.

The constraint pins the value, not the hue. Neutral grey at that luminance reads as
unconsidered filler; the same value with a cool bias reads as system UI, for free.

## What already exists

`apps/macos/dmg` is the reference implementation: a translucent masthead over a depth field,
opaque cards under the two icons, one accent hit on the arrow, the guardrail wired into asset
generation so art that misses cannot ship. `apps/macos/README.md` documents the mechanics.
Read that before applying this language to a new surface — most of what it learned was about
where a platform quietly overrides you.

## What this replaces

Two token systems exist today and both say something else. Any redesign has to reconcile them
before it can apply this language, and *that* is probably the first task, not the colours.

- **`crates/adi-css/scss/_tokens.scss`** — teal accent (`#0d9488` / `#2dd4bf`), near-neutral
  greys, and an explicit stance in its own header comment: *"Surfaces are separated by borders
  rather than elevation, so `shadow` stays a hairline — depth is a last resort, not the house
  style."* That is the direct opposite of rule 1, and it is a deliberate position that
  deserves a deliberate reversal rather than a quiet overwrite.
- **`crates/adi-ui/styles/tokens.css`** — mint accent (`#3ddc9c` / `#0e8a63`), green-tinted
  neutrals, one `light-dark()` declaration per token.

Neither uses `#FA5019`. The macOS app and the pages adi-hive serves have moved (see **The
mark** below); the two token files have not.

## The mark

**Trefoil**: three hexagons at 120°, painted back to front, weak to strong. The geometry is
`apps/macos/Sources/Trefoil.swift` on the Swift side and SVG literals in
`crates/adi-hive/src/notfound.rs`, which re-derives them in a test so the two cannot drift.

Two things about it are load-bearing:

- **Paint order is the design.** Back to front runs weak to strong (52% / 74% / 100%). An
  earlier version ran the tones the other way, so the lobe visually in front was the faintest;
  on a dark tile that is merely odd, on white it lays a wash of pale grey over the whole mark.
- **The mark never names its own colour.** Every lobe is the caller's ink at one of those
  tones, which is what lets it sit on white, black, an image or the accent itself — and what
  lets it drop into a control and inherit the state. Explicit ink, not `.primary`: that is
  `labelColor`, only 85% opaque, and the front lobe let what was behind bleed through it.

**It is lit, and that is a deliberate exception to "solid, low-gloss" above.** The lighting is
a specular across the top of each lobe and a shade under the bottom, laid *over* the lobe —
never a ramp in its own alpha, which turns the front lobe translucent at its foot. That is
material depth applied to the mark rather than gloss for its own sake, and it is the one place
in the system where a gradient is structural. Elsewhere rule 3 still holds.

Four builds of the same geometry, because one drawing cannot serve a 16px Dock icon and a
168px error page: **Cut** (hairline gaps between lobes — icons and controls, the default),
**Solid** (above ~64px), **Accent** (the middle lobe takes `#FA5019` — surfaces whose ground we
control, never on an accent ground), and **Glass** (lobes mix rather than stack — 96px and up).

## Open

- **Which token file survives.** They are already duplicated; picking one is a prerequisite,
  not a follow-up.
- **Where the dial lives**, and whether transparency, density and motion are one preference or
  three.
- **The semantic palette**, which rule 3 separates from the accent but does not yet specify.
