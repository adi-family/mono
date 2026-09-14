# adi-market-site — the ADI Store

**The ADI Store** is the public half of the marketplace: the shelf a person can read before they
have adi, and a crawler can read before anybody links to it. Plain HTML, generated from a
marketplace manifest, published at **`withadi.dev/store`**.

The control panel already lists these bundles, and lists them well
([`adi-webapp`](../adi-webapp)'s Marketplace screen). But it is a wasm app served from somebody's
own machine, so nothing outside can read it: not a person who has never installed adi, not a link
in a message, not Google. This crate writes the same listing out as files.

```text
index.html                  the shelf — everything published, grouped by marketplace
<slug>/                     one page per item
get/                        how to get adi, for the reader who has not got it
sitemap.xml  robots.txt     so it can be crawled (robots.txt only at a host root)
site.css  site.js  fonts/   the design system, once, for the whole site
```

**Two names, on purpose.** A reader sees the *ADI Store*; what it is made of is a *marketplace*
manifest, which is the word the CLI (`adi-mono marketplace add`), the panel and
[`docs/marketplace.md`](../../docs/marketplace.md) all use — and the pages print those commands,
so they use that word too. The crate is not called `adi-store` because in this tree "the store" is
already the operator's own `~/.adi/mono`, and one of those is enough.

## Using it

```sh
# look at it, from a manifest on disk
cargo run -p adi-market-site -- serve --source store=apps/marketplace.json

# write it out, ready to publish
cargo run -p adi-market-site -- build \
  --source store=apps/marketplace.json \
  --source-url store=https://raw.githubusercontent.com/adi-family/marketplace/main/apps/marketplace.json \
  --base-url https://withadi.dev/store \
  --out <landing>/public/store
```

`scripts/store.sh` is the first against this repo's dev fixtures; `scripts/store.sh
--into-landing` is the second, pointed at the landing checkout.

| flag | what it decides |
| --- | --- |
| `--source <name>=<path>` | a marketplace to publish. The name is the first half of every install address (`store/crm-suite`) — it is **not** in the URL, see below. Repeatable. |
| `--source-url <name>=<url>` | where that manifest is published. It is what lets a page print the `marketplace add` line; without it the line is left out rather than guessed. |
| `--base-url <url>` | where the store itself will live. Every canonical URL, the sitemap and the structured data come from it, and a base with a path in it (`…/store`) means the store is a directory of a bigger site: no `robots.txt` is written, because only a host's root one is ever read. Required for `build`; `serve` defaults it to the address it is serving on. |
| `--name <text>` | what the store is called on its own pages. Defaults to *ADI Store*. |

## The URL of an item

`/<slug>/`, under wherever the store is published — so `withadi.dev/store/crm-suite/`, and the
last two segments read as the install address (`marketplace install store/crm-suite`) when the
marketplace is named `store`, which is what the published one is meant to be added as.

The marketplace is **not** a URL segment. It would make `/store/store/crm-suite` of the intended
setup, and — the reason that matters — adding a second marketplace later would move every URL that
already existed, on the one surface whose whole job is to be found by search. Two marketplaces
publishing the same slug is refused at build time instead.

The input is the manifest **file**, not a machine's synced cache. A public listing is published by
whoever publishes the manifest; building it from a local cache would publish whatever that machine
last managed to fetch.

## Have you got adi? — the one thing the site cannot know

A page cannot look at somebody's disk, and both mechanisms that would answer this are closed:
`http://app.adi` cannot be *fetched* from an https page (mixed content), adi-app refuses an `/api`
request whose `Origin` is not its own `Host` anyway, and there is no `adi://` scheme registered to
deep-link into and time out on. Guessing from the user agent would be a guess.

So the site carries **both answers in the markup**, leads with the one that is right for a
stranger, and asks once:

| the reader | what the page leads with |
| --- | --- |
| has not said (and anyone with no script) | **Get adi**, and under it the commands, which is the order a first-time reader wants anyway |
| said "I already have adi" | the two commands, and **Open it in your panel** — the download disappears |

`assets/site.js` is the whole of it: it remembers the answer in `localStorage` and sets
`data-adi` on `<html>`, and the stylesheet hides what does not apply. Three of those lines are
inlined in `<head>`, because a deferred file runs after the first paint and the swap would
otherwise be a visible flicker on every page. Nothing on any page *depends* on it having run.

**Two commands, in that order.** `marketplace install <name>/<slug>` names a marketplace, so a
machine that has never added it fails on the address — every page that offers the install also
offers `marketplace add <name> <url>` above it, numbered. The first version of this site printed
only the second line, which was a command that worked on exactly one machine in the world.

## What is deliberate

- **Complete without script.** Every page is complete in its first response, so a crawler that
  runs nothing still reads the name, the description, the contents, the commands and the JSON-LD.
  It also means the output is a directory any static host will serve, with no runtime to keep
  alive.
- **Relative links.** `../site.css`, never `/site.css` — which is what lets the same output serve
  at a host root, inside a bigger site (`withadi.dev/store`), and from `file://`.
- **One schema.** The manifest is parsed by [`adi-marketplace`](../adi-marketplace)'s own reader
  (`parse_manifest`), so a manifest that would be refused on a machine cannot be published as a
  page here, and both give the same error for one still in the retired artifact shape.
- **Every word is somebody else's.** A manifest is published by whoever publishes it: names,
  descriptions, keywords, readmes and URLs are all escaped (`html::escape`), the readme is rendered
  by this crate's own markdown subset rather than inserted as markup, and a link whose scheme is
  not on the list is rendered as text.
- **No counts.** Under the standing decision an install counts toward nothing
  (`docs/marketplace.md`), so the only number on the site is how many items a manifest publishes.

## Where it is published

**It is part of the landing.** `withadi.dev/store`, not a site of its own: the landing's masthead
and footer link it beside Docs, Source and Download, and a reader who finds an app through search
lands one click from the thing that runs it.

The landing is an Astro site in its own repository (`adi-family/withadi.dev`), and it copies
`public/` into `dist/` verbatim — so `public/store/` is served at `/store`. Its Cloudflare build
has no Rust, so the pages are generated **here** and committed there:

```sh
ADI_STORE_MANIFEST=<the published manifest> \
ADI_STORE_MANIFEST_URL=<where it is published> \
  scripts/store.sh --into-landing        # writes <landing>/public/store
```

Two things the landing does for free once they are there: its `for-agents` integration walks
`dist/`, so every store page also gets a Markdown mirror at `/store/<slug>/index.md` and a line in
`llms.txt`, and the site's own `sitemap.xml` lists the store's URLs.

`public/store/` is **gitignored** in the landing, because the copy a developer generates is built
from this repo's dev fixtures — five invented bundles — and committing those would publish invented
apps. `git add -f public/store` is the deliberate act that publishes the real ones.

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
