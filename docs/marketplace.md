# The marketplace — a manifest you host, a repository you install

A marketplace is **one JSON manifest at an HTTPS URL the operator chose** — GitHub raw, a gist,
any host that serves the file. The store keeps an *array* of them; each URL is one source. There
is no platform here: no hosting, no accounts, no server, nothing to operate.

An entry in that manifest names **a git repository and one commit in it**. Installing an app is
`git clone`, checked out at that commit, under a name the operator types — and what lands stays a
clone, so it can be read, edited, committed to and pulled.

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
      "repo": "https://github.com/adi-family/crm.git",
      "commit": "9f2c1d4e5a6b7c8d9e0f1a2b3c4d5e6f70819a2b",
      "branch": "main"
    }
  ]
}
```

| field | required | meaning |
| --- | --- | --- |
| `name` | no | the publisher's name for the list. Display text only. |
| `apps[].slug` | yes | the **published** identity — the second half of `<marketplace>/<slug>`, and how an install is addressed. One safe path segment (`[A-Za-z0-9._-]`, no `/`). It is *not* the directory the app lands as. |
| `apps[].name` | yes | the human name, offered as the default when somebody names their copy. |
| `apps[].description` | no | one line on what it is for. |
| `apps[].version` | no | as published. **Display text**: the commit is the identity of what installs, and this is the label a person recognizes it by. |
| `apps[].repo` | yes | the repository to clone. `https://`, or a `file://` path while an app is being developed. `ssh://` and `git@host:path` are refused — those reach for the operator's agent and keys, which a URL out of somebody else's manifest has no business doing. |
| `apps[].commit` | yes | the pin: a full 40-hex object name. A branch, a tag or a short sha is refused. |
| `apps[].branch` | no | the branch that commit sits on, when it is not the repository's default. It decides what a later `git pull` in the installed copy follows, and nothing else. |

Unknown fields are ignored, so a newer manifest an older machine reads still lists its apps. A
manifest is validated **whole**: the first entry that does not belong refuses the fetch, rather
than a listing that quietly hides part of what was published.

### Why a commit and not a branch

The pin is the whole security story, and it is worth being explicit about what it does and does
not buy:

- **What the operator read is what installs.** A publisher who pushes to `main` after the listing
  was synced changes nothing about what a later install produces. Moving onto newer code is
  `marketplace update`, an act somebody takes — with the diff already public in the repository.
- **What is installed is knowable.** `git -C dashboards/<id> log` is the provenance of every byte
  on disk; there is no unpacking step and no artifact whose relationship to the source is a
  claim rather than a fact.
- It does **not** make an app safe to run. A pinned commit is a pinned commit of somebody else's
  code. That is what "installed is not started" (below) is for.

### What an app is

**An app is a dashboard**: a bun-served frontend/backend pair (`guides/dashboards.md`). Its
repository must carry `frontend/index.ts` and `backend/index.ts` at the root — the two files the
hive file written on arrival runs — or the install is refused rather than landing something that
could never start.

A published repository should **not** version `config.toml` or `.adi/`. Those are the store's:
`config.toml` is the dashboard's own manifest (the name is the operator's, not the publisher's)
and `.adi/` holds the hive file this machine wrote for its own paths. An install adds both to the
clone's `.git/info/exclude`, so `git status` in an installed app is clean and a pull never fights
over them. A repository that versions them anyway installs, with a note saying so.

The three **generated entry points** — `frontend/index.html`, `frontend/index.ts`,
`backend/index.ts` — are a different case: an app has to ship them (nothing writes a missing one,
and two of them are what the hive file runs), *and* the panel rewrites them in place whenever its
own templates move on, which is how a shell fix reaches dashboards that already exist. Left alone,
the first such upgrade after an install would read as uncommitted work and stop every update. So
an install marks them `--skip-worktree` in the clone: git stops noticing the panel's rewrites, and
an update clears the mark, restores them to the pin, merges, and sets it again. A dashboard's own
work lives in `frontend/modules/` and `backend/routes/`, which migration never touches — that is
why discarding a rewrite of an entry point costs nothing.

Anything a clone carries at `.adi/` or a root `hive.yaml` is **dropped on arrival**, and the
clone is assembled in `marketplace/staging/` and moved in whole — because `dashboards/*/.adi/hive.yaml`
is the supervisor's import glob, and a repository that shipped one of its own would otherwise
have a window in which its runner was live.

The other kinds the operator named — tools, agents, agent presets — are deliberately **not** here.
Shipping one is how you find out what the manifest is missing before three more kinds are built on
it.

## Sources

`~/.adi/mono/marketplace/sources.toml`, an array of tables:

```toml
[[marketplaces]]
name = "adi"
url = "https://raw.githubusercontent.com/adi-family/marketplace/main/apps/marketplace.json"
```

- The array **ships empty**. Adding URLs is the operator's act (`adi-mono marketplace add`).
- The configured `name` is the local identity — the first half of `<marketplace>/<slug>` and the
  cache file's name. The manifest's own `name` is display text, so two machines can alias one URL
  under different names.
- `url` must be `https://`. Fetching a manifest is HTTPS, always.

## Sync and the cache

`adi-mono marketplace sync` fetches every source's manifest and caches it under
`marketplace/cache/<name>.json` — an *envelope*: when the last successful fetch happened, from
which URL, why the attempt after it (if any) failed, and the manifest itself.

**A failing URL degrades to the stale cache with a warning, not an error.** The standing copy
keeps serving the listing, with the failure recorded beside it (the panel shows it; the CLI
prints it); the next successful fetch clears the note. The one case that is still an error is a
first fetch that fails with nothing to fall back on. A manifest that fetches but does not
validate counts as a failed fetch.

Install reads the entry **from the cache**, never the network — the app a person saw on the
listing is the app they get. Only the repository is fetched at install time, by `git`.

## Install: named by you, inert on arrival

```
adi-mono marketplace install adi/crm --name "Sales CRM"
```

- **The operator names their copy.** The name becomes the dashboard's name, its id (`Sales CRM` →
  `sales-crm`) and its hostname (`sales-crm.adi`), and it is renameable afterwards like any
  dashboard's. `--name` is optional; the entry's own name is the default.
- **Nothing collides.** The id is minted the way every store id is, so installing the same app
  twice gives `crm` and `crm-2` — two copies of one app is ordinary, not an error.
- **It stays a clone.** `.git` is kept, the pinned commit sits on a branch that tracks `origin`,
  and the working tree is clean. `git -C ~/.adi/mono/dashboards/<id> pull` is a normal thing to do
  afterwards; so is committing your own edits on top.
- **Installed is not started.** The app arrives in the dashboard store's own inert state: files on
  disk, `archived_at` stamped, and its hive file written under the *parked* name
  (`.adi/hive.yaml.archived`) that the supervisor's import glob does not match. Nothing executes.
  `--start` installs and starts in one act, for when that is what you meant.
- **The panel's form starts it by default; the CLI does not.** That difference is deliberate.
  Pressing Install on a page is itself the deliberate act, and an install that leaves nothing to
  open reads as one that failed. Unticking "Start it right away" is one click. A script, by
  contrast, should have to say `--start` out loud.
- **An app that has never been started is listed, not filed away.** It carries `archived_at` like
  an archived dashboard and means something else entirely by it: nobody put it away, it has just
  not been run. So the Dashboards page keeps it in the main list, its services read *not started*
  rather than *not allocated*, and both its name and a **Start** button on the row do the one
  thing there is to do with it. The record's `started_at` is what tells the two apart afterwards —
  stamped the first time it runs, so a later archive is an archive like any other.

Every installed copy carries `.adi/marketplace.json` — which entry it came from, which commit it
stands at, and when. That file is what makes "installed" a fact about a directory rather than a
guess from its name, and what `update` reads.

Uninstall is the Dashboards page's own Archive → Delete; an installed app is just a dashboard.

## Start

`adi-mono marketplace start <id>` is the one deliberate act that lets it run: the hive file moves
into the supervisor's glob, ports are leased, both bun servers come up within a few seconds, and
the app answers at `<label>.adi`. Restore on the Dashboards page does the same thing. The label
chosen at arrival is a *preference* — if a neighbour has since taken it, a fresh one is derived,
because two dashboards on one hostname is a routing coin-flip.

## Update

`adi-mono marketplace update <id>` moves a copy onto the commit its marketplace pins **now**
(sync first, or the pin has not changed). It is a **fast-forward**:

- Uncommitted changes stop it outright — an update that silently discarded them would make
  editing an installed app a trap.
- A branch carrying your own commits that cannot fast-forward stops it too, and says so. Merge or
  rebase yourself, or pass `--force`, which resets onto the pin and loses local work.
- A running app needs no restart: bun hot-reloads it.

The other update path is the one every clone has — `git -C ~/.adi/mono/dashboards/<id> pull` —
which follows the branch rather than the manifest's pin. Both are legitimate; the difference is
whether you are trusting the publisher's *latest* or the publisher's *published*.

## The panel

`/marketplace` lists cached entries grouped by marketplace, with the source's URL and freshness
(each source's stale state is said out loud), the repository and pinned commit on each row, and
the install form that asks what to call your copy. Every installed copy appears under its entry
with where it stands, a Start button while it is inert, and Update when the manifest has moved
past it. **No install counts anywhere on it** — under the standing decision
(`decisions/2026-08-22-ten-thousand-counts-only-adi-installs.md`) a marketplace install does not
count toward the 10,000, and a count is not the story the page should tell.

## Limits, and where they live

| limit | value | why |
| --- | --- | --- |
| manifest fetch timeout | 30 s | a manifest that cannot arrive in half a minute is not arriving |
| manifest fetch size | 16 MiB | bounds a misbehaving host without touching a real manifest |
| clone stall cut-off | 30 s under 1 kB/s | a child `git` has no timeout; a transfer that stalls ends rather than hanging the caller |
| submodules | never fetched | a submodule is a second repository from a URL the manifest never named |
| credentials | never prompted | `GIT_TERMINAL_PROMPT=0`: a private repository fails in a second instead of blocking a web request forever |
| trust | the operator's `add` | sources are chosen by hand; the pin, the arrival strip and inert arrival are what a bad repository buys, not a review |

## What is deliberately not here

- **Tools, agents, agent presets** — the other three kinds. The manifest is expected to grow a
  `kind` when they land; unknown fields are ignored, so this manifest keeps parsing.
- **Version comparison** — `version` is display text. The commit is the identity, and
  `update` is the path.
- **Uninstall command** — the Dashboards page already archives and deletes; a marketplace app is
  a dashboard, so it uses them.
- **Counts** — see the panel section.

## Publishing an app

1. Build it as a dashboard (`guides/dashboards.md`), and push its directory — without
   `config.toml`, `.adi/` or `node_modules/` — as a repository.
2. Take the commit: `git rev-parse HEAD`.
3. Add an entry to your manifest with `repo` and that `commit`, and host the manifest anywhere
   over HTTPS.
4. Publishing a new version is one line: change `commit` (and `version`, for the reader). Every
   machine sees it on its next sync, and nothing moves until somebody updates.
