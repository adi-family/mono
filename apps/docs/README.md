# adi-docs

The docs site: [Astro](https://astro.build) + [Starlight](https://starlight.astro.build),
deployed as a Cloudflare **Pages** project, static output only (no adapter, no Function —
`astro build` is plain HTML/CSS/JS).

It's served at the root of `docs.withadi.dev` — `astro.config.mjs` sets `base: '/'`, so the
files in `dist/` and every internal link in the built HTML agree on the root.

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

## Deploy

Every push to `main` that touches `apps/docs/**` (or the workflow itself) runs
[`.github/workflows/docs.yml`](../../.github/workflows/docs.yml), which builds the site and
deploys it to the Cloudflare Pages project `adi-docs`. It's also runnable by hand from the
Actions tab (`workflow_dispatch`). The `docs-deploy` concurrency group with
`cancel-in-progress: false` means a second push mid-deploy queues behind the first rather than
racing it onto the same project.

Steps: checkout, `bun install --frozen-lockfile`, install a Playwright Chromium, `bun run build`,
then `wrangler pages deploy dist`. Two of those are non-obvious enough to have cost real time
getting right:

- **`wrangler` runs under node, never bun.** It declares `engines: node >=22` and checks it at
  startup. Under bun, `wrangler pages deploy` uploads only part of the files and then **exits 0
  having registered no deployment at all** — a green run that shipped nothing. The tell is
  `adi-docs.pages.dev` answering 522 while the project's deployment list is empty. This is why
  the workflow installs a real node alongside bun and invokes `npx wrangler`, never `bunx
  wrangler` — `bunx` would hand the same binary straight back to bun.
- **The build needs a Playwright Chromium, not just the `playwright` package.**
  `rehype-mermaid` (see `astro.config.mjs`) renders every ` ```mermaid ` fence to a static `<svg>`
  at build time by driving a headless browser. `playwright` is a `package.json` dependency, but
  the browser binary is a separate download that `bun install` never fetches. A fresh machine —
  or a fresh runner — fails with `browserType.launch: Executable doesn't exist`, reported against
  whichever `.mdx` file happens to render first rather than against the config that needs it.

A green workflow run is not proof the site updated — deploys can silently no-op (above). Confirm
the deployment actually registered:

```bash
curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
  https://api.cloudflare.com/client/v4/accounts/5b81c76ca545338aa9e85215c001a768/pages/projects/adi-docs/deployments \
  | jq '.result[0].latest_stage'
```

and expect `"deploy/success"` on the newest entry, then check the site itself. A 522 from
`adi-docs.pages.dev` means the project exists with no successful deployment behind it — that's a
failed deploy, not propagation delay.

Deploying by hand is possible (`bun run deploy`, i.e. `wrangler pages deploy dist`) but needs a
real node ≥22 on the machine running it, for the same reason as above. CI is the normal path;
reach for this only when CI itself is unavailable.

A page with `draft: true` in its frontmatter (see
[`src/content/docs/example.mdx`](src/content/docs/example.mdx)) is excluded from `astro build`
but still served by `astro dev` — that's how a page stays available to authors locally without
shipping. Worth confirming absent from the live site after a deploy if you're relying on it.

## Cloudflare setup

The Pages project is **direct upload, not git-integrated** — Cloudflare never builds this repo;
GitHub Actions builds it and pushes the result with `wrangler`. Two things have to be true for
`docs.withadi.dev` to resolve, and neither implies the other:

1. it's attached to the `adi-docs` project as a **custom domain**
2. a **proxied** CNAME `docs → adi-docs.pages.dev` exists in the `withadi.dev` zone

Attaching without the record leaves the domain `pending` forever — the dashboard wizard offers
to create the record, the API does not. The record without attaching does nothing. The zone
lives on the same Cloudflare account as the project (account id `5b81c76ca545338aa9e85215c001a768`,
which is why it's passed as a plain env var in the workflow rather than a secret — it isn't
one), which is what lets the certificate issue automatically once both pieces are in place. The
record has to stay proxied (orange cloud) for that to work, and — a trap if you go looking —
**it cannot be checked with `dig CNAME`**: Cloudflare flattens a proxied CNAME to `A` records at
its edge, so the query comes back empty however correctly the record is set.

Auth for CI is the `CLOUDFLARE_API_TOKEN` repo secret on `adi-family/mono`, scoped to just
`Account > Cloudflare Pages > Edit`.

[`scripts/setup-cf.sh`](scripts/setup-cf.sh) is **first-time setup only** — it creates the
project, attaches the custom domain, and writes the DNS record. It isn't part of routine deploys,
but it's idempotent, so it doubles as a "has anything drifted?" check. It needs a broader token
than CI does — `Zone > Zone > Read` and `Zone > DNS > Edit` on `withadi.dev` as well:

```bash
adi-mono secrets set CLOUDFLARE_API_TOKEN   # Account > Cloudflare Pages > Edit,
                                             # Zone > Zone > Read + Zone > DNS > Edit on withadi.dev
./scripts/setup-cf.sh
```

If the secret isn't set, the script prints those instructions and exits cleanly rather than
failing. It authenticates with `CLOUDFLARE_API_TOKEN` read from this machine's secret store —
never `wrangler login`, which needs a browser this shell doesn't have. See
[`apps/oauth-router/README.md`](../oauth-router/README.md) for the fuller notes on why attaching
a custom domain needs the REST API and why a 522 mid-setup is expected — this script follows the
same shape.
