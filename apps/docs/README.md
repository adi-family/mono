# adi-docs

The docs site: [Astro](https://astro.build) + [Starlight](https://starlight.astro.build),
deployed as a Cloudflare **Pages** project, static output only (no adapter, no Function —
`astro build` is plain HTML/CSS/JS).

It's meant to live at `docs.withadi.dev/mono/`, alongside a sibling `/cloud/` section added
later by a separate effort; that's why `astro.config.mjs` sets `base: '/mono/'` and every
internal link in the built HTML carries that prefix even though the files themselves sit at the
root of `dist/` (Astro doesn't nest a static build under its own `base` — whatever serves this
project is expected to route `/mono/*` here). `public/_redirects` sends the bare domain to
`/mono/` so it isn't a 404 in the meantime.

Content is currently placeholder-only. Migrating the repo's existing `docs/` and `guides/` into
this collection is a separate, later task.

## Wikilinks

`[[Page Name]]` and `[[Page Name|Alias Text]]` (Obsidian syntax) work in any page under
`src/content/docs/`, matched case- and hyphenation-insensitively against another page's title or
slug. A match renders as `<a class="wiki-link">`; no match renders as
`<a class="wiki-link wiki-link-broken">` instead of failing the build — see
[`src/content/docs/guides/getting-started.md`](src/content/docs/guides/getting-started.md) for
both cases.

The resolution lives in [`wiki-links.mjs`](wiki-links.mjs), on top of
[`remark-wiki-link`](https://github.com/landakram/remark-wiki-link): since it runs from
`astro.config.mjs`, before Vite's content-collection pipeline exists, it reads
`src/content/docs/` straight off disk to build its permalink table rather than importing
`astro:content`.

## Develop

```bash
cd apps/docs
bun install

bun run dev                 # astro dev — localhost:4321
bun run typecheck           # astro check
bun run build                # -> dist/
```

## Standing it up on a Cloudflare account

Two things have to be true, and only the first is in git:

1. the Pages project `adi-docs` exists, with a build deployed to it
2. `docs.withadi.dev` is attached to the project as a **custom domain**

[`scripts/setup-cf.sh`](scripts/setup-cf.sh) does both and is idempotent, so it doubles as the
"has anything drifted?" check. It authenticates with `CLOUDFLARE_API_TOKEN`, read from this
machine's secret store — never `wrangler login`, which needs a browser this shell doesn't have:

```bash
adi-mono secrets set CLOUDFLARE_API_TOKEN   # Account > Cloudflare Pages > Edit,
                                             # Zone > Zone > Read + Zone > DNS > Edit on withadi.dev
./scripts/setup-cf.sh
```

If the secret isn't set, the script prints those instructions and exits cleanly rather than
failing. See [`apps/oauth-router/README.md`](../oauth-router/README.md) for the fuller notes on
why attaching a custom domain needs the REST API, why a 522 mid-setup is expected, and why the
DNS record isn't created by attaching the domain — this script follows the same shape.

Routine redeploys afterwards need none of that:

```bash
bun run deploy               # wrangler pages deploy
```
