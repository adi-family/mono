# Fonts

The three faces `design/DESIGN.md` §4 names — **Geist** for everything people read, **Geist
Mono** for everything machines wrote, **Bricolage Grotesque** for landing headlines — self-hosted
so the control panel makes no third-party request and the installed app keeps its type offline.

`scripts/fonts.sh` fetches them from Google Fonts as per-script variable woff2 subsets and
writes [`fonts.css`](./fonts.css), the `@font-face` rules with the matching `unicode-range`s. A
browser downloads only the scripts a page actually uses.

| Family | Files | Weights | Scripts |
| --- | --- | --- | --- |
| Geist | `geist-*.woff2` | 100–900 (variable) | latin, latin-ext, cyrillic, cyrillic-ext |
| Geist Mono | `geist-mono-*.woff2` | 100–900 (variable) | latin, latin-ext, cyrillic, cyrillic-ext, symbols2 (box drawing) |
| Bricolage Grotesque | `bricolage-*.woff2` | 200–800 (variable), `opsz` 12–96 | latin, latin-ext |

Cyrillic is not optional: transcripts are read in Russian as often as in English. No italics —
the design specifies none, and a browser synthesises an oblique where Markdown asks for one.

The URLs in `fonts.css` are absolute (`/fonts/…`) because the sheet is imported from other
crates; every consumer copies this directory to its dist root (`rel="copy-dir"` in Trunk).

## Licence

All three are SIL Open Font License 1.1 — `Geist-OFL.txt` and `BricolageGrotesque-OFL.txt`
travel with the files, as the licence requires.
