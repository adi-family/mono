# The marketplace — a manifest you host, an app you install

A marketplace is **one JSON manifest at an HTTPS URL the operator chose** — GitHub raw, a gist,
any host that serves the file. The store keeps an *array* of them; each URL is one source. There
is no platform here: no hosting, no accounts, no server, nothing to operate. The same shape
plugins ship in elsewhere, which is evidence the shape is sufficient rather than merely cheap.

The decision and its reasoning: `business/decisions/2026-08-31-marketplaces-are-a-manifest-in-a-git-repo.md`
(ADI-38, which gates the first showcase — `sc-01` does not go out until this install path is real).

## The manifest

```json
{
  "name": "ADI starter apps",
  "apps": [
    {
      "slug": "crm",
      "name": "CRM",
      "description": "Who has gone quiet, and what was last said to them.",
      "version": "0.1.0",
      "artifact": "https://raw.githubusercontent.com/…/crm.bundle.json"
    }
  ]
}
```

| field | required | meaning |
| --- | --- | --- |
| `name` | no | the publisher's name for the list. Display text only. |
| `apps[].slug` | yes | the installable identity — the second half of `<marketplace>/<slug>`, and the directory the app lands as. One safe path segment (`[A-Za-z0-9._-]`, no `/`), or the whole manifest is refused. |
| `apps[].name` | yes | the human name. |
| `apps[].description` | no | one line on what it is for. |
| `apps[].version` | no | as published. **Shown, never enforced**: v1 has no pinning, no downgrade refusal, no comparison beyond string equality. |
| `apps[].artifact` | yes | the HTTPS URL the app's bytes come from. `http://` is refused at sync. |

Unknown fields are ignored, so a newer manifest an older machine reads still lists its apps. A
manifest is validated **whole**: the first entry that does not belong refuses the fetch, rather
than a listing that quietly hides part of what was published.

### What an app is, in v1

**An app is a dashboard.** The artifact at `artifact` is a `DashboardBundle` — the same JSON the
panel's machine-to-machine transfer packs (`export`) and lands (`import`): the authored files as
base64, and *nothing generated* (no `config.toml`, no `.adi/`, no `node_modules/`, no caches).

That choice is the smallest honest one:

- The thing the showcase needs to install (the CRM) *is* a dashboard — a bun-served
  frontend/backend pair, authored as loose `.ts` files under `dashboards/<slug>/`.
- The artifact format already exists and is already proven over the mesh; publishing an app is
  "export, host the file", not a new serialization of the store.
- One landing path — the bundle jail, the size caps, the one-origin hive file — is shared between
  the panel's import and the marketplace (`crates/adi-dashboards`), so a bundle that could not
  escape its directory over the mesh cannot escape it from a marketplace either.

The other three kinds the operator named — tools, agents, agent presets — are deliberately **not**
in v1. Shipping one is how you find out what the manifest is missing before three more kinds are
built on it.

## Sources

`~/.adi/mono/marketplace/sources.toml`, an array of tables:

```toml
[[marketplaces]]
name = "adi"
url = "https://raw.githubusercontent.com/adi-family/apps/main/marketplace.json"
```

- The array **ships empty**. Adding URLs is the operator's act (`adi-mono marketplace add`).
- The configured `name` is the local identity — the first half of `<marketplace>/<slug>` and the
  cache file's name. The manifest's own `name` is display text, so two machines can alias one URL
  under different names.
- `url` must be `https://`. Fetching is HTTPS, always.

## Sync and the cache

`adi-mono marketplace sync` fetches every source's manifest and caches it under
`marketplace/cache/<name>.json` — an *envelope*: when the last successful fetch happened, from
which URL, why the attempt after it (if any) failed, and the manifest itself. The manifest inside
is byte-shape what the source served.

**A failing URL degrades to the stale cache with a warning, not an error.** The standing copy
keeps serving the listing, with the failure recorded beside it (the panel shows it; the CLI
prints it); the next successful fetch clears the note. The one case that is still an error is a
first fetch that fails with nothing to fall back on. A manifest that fetches but does not
validate counts as a failed fetch.

Install reads the entry **from the cache**, never the network — the app a person saw on the
listing is the app they get, and install works offline once a manifest is synced. Only the
artifact is fetched at install time.

## Install: inert on arrival

`adi-mono marketplace install <marketplace>/<slug>` lands the app under `dashboards/<slug>/`:

- **The slug is the entry's**, not the bundle's — `<marketplace>/<slug>` is the whole address of
  what was asked for.
- **Installed is not started.** The app arrives in the dashboard store's own inert state: files
  on disk, `archived_at` stamped, and its hive file written under the *parked* name
  (`.adi/hive.yaml.archived`) that the supervisor's import glob does not match. Nothing executes.
  This is the property the store already gives a tool — creating one gives it to nobody — kept
  for a payload whose backend is somebody else's TypeScript.
- **A slug collision is refused**, never numbered and never overwritten: a silent `crm-2` is a
  surprise, and a silent replace is worse. The refusal says how to force.
- `--force` replaces the files in place — a mirror, keeping `.adi` and `node_modules` exactly as
  a re-transfer does — and keeps whatever run state the app had: a running app stays running, an
  unstarted one stays inert.

Uninstall is the Dashboards page's own Archive → Delete; an installed app is just a dashboard.

## Start

`adi-mono marketplace start <slug>` (or `<marketplace>/<slug>`) is the one deliberate act that
lets it run: the hive file moves into the supervisor's glob, ports are leased, both bun servers
come up within a few seconds, and the app answers at `<label>.adi`. Restore on the Dashboards
page does the same thing. The label chosen at arrival is a *preference* — if a neighbour has
since taken it, a fresh one is derived, because two dashboards on one hostname is a routing
coin-flip.

## The panel

`/marketplace` lists cached entries grouped by marketplace, with the source's URL and freshness
(each source's stale state, if any, is said out loud), an install button, and a start button once
installed. **No install counts anywhere on it** — under the standing decision
(`decisions/2026-08-22-ten-thousand-counts-only-adi-installs.md`) a marketplace install does not
count toward the 10,000, and a count is not the story the page should tell. Whether the word
"marketplace" itself is what the page says at launch is still open (see the decision's "not
decided" list); the URL and the CLI verb are stable, the wording is not.

## Limits, and where they live

| limit | value | why |
| --- | --- | --- |
| fetch timeout | 30 s | a manifest or bundle that cannot arrive in half a minute is not arriving |
| fetch size | 16 MiB | bounds a misbehaving host without touching a real artifact |
| bundle size / files | 4 MiB decoded / 2000 files | the transfer caps (`adi-dashboards`), shared with the panel's import |
| trust | the operator's `add` | sources are chosen by hand; the bundle jail and inert arrival are what a bad artifact buys, not a review |

## What is deliberately not here

- **Tools, agents, agent presets** — the other three kinds. The manifest is expected to grow a
  `kind` when they land; unknown fields are ignored, so this manifest keeps parsing.
- **Version pinning / updates** — `version` is display text. `--force` is the v1 update path.
- **Uninstall command** — the Dashboards page already archives and deletes; a marketplace app is
  a dashboard, so it uses them.
- **Counts** — see the panel section.
