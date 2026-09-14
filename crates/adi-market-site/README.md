# adi-market-site

**The public marketplace** — the shelf a person can read before they have adi, and a crawler can
read before anybody links to it. Plain HTML, generated from a marketplace manifest.

The control panel already lists these bundles, and lists them well
([`adi-webapp`](../adi-webapp)'s Marketplace screen). But it is a wasm app served from somebody's
own machine, so nothing outside can read it: not a person who has never installed adi, not a link
in a message, not Google. This crate writes the same listing out as files.

```text
index.html                  the shelf — everything published, grouped by marketplace
<marketplace>/<slug>/       one page per item, at the address it installs by
sitemap.xml  robots.txt     so it can be crawled
site.css  fonts/  favicon   the design system, once, for the whole site
```

## Using it

```sh
# look at it, from a manifest on disk
cargo run -p adi-market-site -- serve --source adi=apps/marketplace.json

# write it out, ready to publish
cargo run -p adi-market-site -- build \
  --source adi=apps/marketplace.json \
  --source-url adi=https://raw.githubusercontent.com/adi-family/marketplace/main/apps/marketplace.json \
  --base-url https://marketplace.withadi.dev \
  --out dist
```

`scripts/market-site.sh` is the same thing against this repo's dev fixtures, for looking at the
result without publishing anything.

| flag | what it decides |
| --- | --- |
| `--source <name>=<path>` | a marketplace to publish. The name is the first half of every install address (`adi/crm-suite`) **and** the directory its pages live in. Repeatable. |
| `--source-url <name>=<url>` | where that manifest is published. It is what lets a page print the `marketplace add` line; without it the line is left out rather than guessed. |
| `--base-url <url>` | where the site itself will live. Every canonical URL, the sitemap and the structured data come from it. Required for `build`; `serve` defaults it to the address it is serving on. |
| `--name <text>` | what this marketplace is called on its own pages. |

The input is the manifest **file**, not a machine's synced cache. A public listing is published by
whoever publishes the manifest; building it from a local cache would publish whatever that machine
last managed to fetch.

## What is deliberate

- **No script, anywhere.** Every page is complete in its first response, so a crawler that runs
  nothing still reads the name, the description, the contents and the JSON-LD. It also means the
  output is a directory any static host will serve, with no runtime to keep alive.
- **Relative links.** `../../site.css`, never `/site.css` — so the same output serves correctly at
  a domain root, under a path prefix, and from `file://`.
- **One schema.** The manifest is parsed by [`adi-marketplace`](../adi-marketplace)'s own reader
  (`parse_manifest`), so a manifest that would be refused on a machine cannot be published as a
  page here, and both give the same error for one still in the retired artifact shape.
- **Every word is somebody else's.** A manifest is published by whoever publishes it: names,
  descriptions, keywords, readmes and URLs are all escaped (`html::escape`), the readme is rendered
  by this crate's own markdown subset rather than inserted as markup, and a link whose scheme is
  not on the list is rendered as text.
- **The URL is the install address.** `/<marketplace>/<slug>/` is exactly what goes after
  `marketplace install`, so what somebody copies out of the address bar is what they paste.
- **No counts.** Under the standing decision an install counts toward nothing
  (`docs/marketplace.md`), so the only number on the site is how many items a manifest publishes.

## The look

`design/DESIGN.md`, dark set. The tokens are inlined at build time from `design/tokens.css` and the
faces come from [`adi-ui`](../adi-ui)'s `fonts/`, so the site cannot restate a colour or a size.
`assets/site.css` is the whole of its own styling — one stylesheet, one request.

Two things worth knowing before changing it:

- The mark in the bar (`assets/mark.svg`) is a **copy** of one drawing, kept honest by
  `the_mark_agrees_with_the_canonical_drawing` in `src/shell.rs`.
- The icons are `crates/adi-ui/icons/*.svg` included verbatim and normalized in `src/icons.rs`.
  Adding one is `scripts/lucide.sh add <name>` plus a line in the `Icon` enum — never an inline
  path, never a glyph.
