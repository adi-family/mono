# Fonts

Self-hosted, so the app renders its own typography with no network request and no
third-party origin in the page — which is what makes the PWA look right offline.

| File | Family | Weights | Size |
| --- | --- | --- | --- |
| `ibm-plex-sans-var.woff2` | IBM Plex Sans | 400–600 (variable) | 119 KB |
| `jetbrains-mono-var.woff2` | JetBrains Mono | 400–700 (variable) | 53 KB |

Both are **variable** fonts, so one file per family covers every weight the design uses —
and any value between them, which is why `@font-face` declares a `font-weight` *range*
rather than one number per file.

Both keep full **Latin, Cyrillic and Greek** coverage; nothing was subsetted away.

## How they were made

From the Google Fonts distribution, with `fonttools`:

- **Pinned `wdth: 100` on IBM Plex Sans.** Its variable font ships a width axis (75–100)
  for the Condensed and SemiCondensed cuts, which the design never uses; pinning it drops
  that half of the design space.
- **Clamped the weight axes** to the range each family actually needs (400–600 sans,
  400–700 mono) instead of the full 100–700/100–800.
- **Compressed to woff2.** Together: 704 KB of TTF → 172 KB.

No italics. The design specifies none, and shipping them would roughly double the payload;
a browser synthesises an oblique where markdown asks for emphasis. Add the
`*-Italic-VariableFont` cuts the same way if that becomes visible enough to matter.

## Licence

Both are SIL Open Font License 1.1 — see `IBM_Plex_Sans-OFL.txt` and
`JetBrains_Mono-OFL.txt`, which the licence requires to travel with the files.
