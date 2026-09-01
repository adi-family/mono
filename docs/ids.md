# Ids: names, not UUIDs — and the old ids that keep working

Every registry in the store names its entries by their directory: `tools/<id>`, `projects/<id>`,
`dashboards/<id>`. An id has always been a free-form string, and the store has always carried both
kinds side by side — `tools/sys-db` sits beside `tools/ec5bd98c-35c1-4e9e-ba25-5c2dbd3d5a99`,
`projects/adi` beside `projects/3352a5eb-…`. Nothing here introduces a new kind of id.

What changed is **how a new one is minted**: a slug of the name a human already typed, instead of a
UUID. And **what happens to the old one**: it keeps resolving, forever.

The rules live in `crates/adi-config/src/ids.rs` (`slug`, `unique_id`, `mint`, `Aliases`), so every
registry mints and resolves the same way rather than each inventing it.

## Why

Two reasons, and they are the same reason twice.

**Cost.** A UUID is 36 characters of hex that tokenise badly — around 23 tokens. They are not rare:
`adi-agent`'s `bin_tools` alone listed 49 ids, 1,458 characters of pure identifier, in a definition
read on every launch. `confirm-sync` is 12 characters and two tokens.

**Portability.** A published manifest cannot reference what it installs by a machine-local id.
`tools/ec5bd98c-…` resolves to nothing on another machine, or worse to something else. The
marketplace (ADI-37) and this are the same piece of work; if the manifest ships against UUIDs
first it has to be rebuilt, and by then other people's published manifests carry the mistake.

## The minting rule

`adi_config::mint(name, fallback, taken)`:

1. `slug(name)` — ASCII letters and digits lowercased, everything else a separator, runs collapsed,
   trimmed, capped at 48 characters. `"Confirm Sync"` → `confirm-sync`.
2. If that leaves nothing — a name written entirely in a non-Latin script, or only punctuation —
   fall back to the kind's own word: `tool`, `project`, `dashboard`. No transliteration is invented;
   an id nobody recognises in either language is worse than a generic one that can be renamed.
3. Uniquify: `base`, then `base-2`, `base-3`, … first free wins. Deterministic given the set of ids
   already taken.

The alphabet is deliberately narrower than `adi_config::valid_name` accepts — no `.` and no `_`. A
minted id can then never be `.`, `..`, a dotfile, or something that reads as a filename, so it can
never collide with `aliases.toml` or with the `.bin` / `.agent-bin` directories that share the
module dir with it.

`taken` must answer for *everything* that occupies the id — a live directory **and** an id some
entry was renamed away from. Handing out an id that still resolves elsewhere is the one failure the
whole mechanism exists to prevent.

## Scope: one segment, unique within its kind on this machine

**A local id is globally unique within its kind, and it is one path segment.** Not scoped by
project. The id *is* a directory name under one module dir, so uniqueness within the kind is what
the filesystem already enforces; a scoped `<project>/<name>` would need a nested directory, which
breaks `valid_name` — the security boundary every store applies before joining an id onto a path,
against ids arriving from the CLI, the HTTP API and webhook URLs.

**A published id carries its publisher: `<publisher>/<name>`** — `adi-family/confirm-sync`. That
form exists only in a manifest. At install time the publisher is stripped and the local id is minted
from the name through the same `mint`, which is also where a collision with something already
installed is settled: a machine that already has `confirm-sync` gets `confirm-sync-2`, and nothing
is overwritten. The install record keeps the qualified name; the local store never sees it.

## Aliases: nothing loses its id

Renaming an entry records the id it had in that registry's alias index — `tools/aliases.toml`,
`projects/aliases.toml` — and every read path resolves through it.

```toml
[aliases]
ec5bd98c-35c1-4e9e-ba25-5c2dbd3d5a99 = "confirm-sync"
```

This is not politeness. This store is a live control plane whose ids are cited verbatim in ~75 agent
definitions (`bin_tools`), in the generated `.bin` / `.agent-bin` shims, in projects' `.adi/hive.yaml`,
in store documents, and in the operator's shell history. An id that stopped resolving would break the
machine under its own operator.

- A **file** rather than a tombstone directory per old id: every registry lists its entries by reading
  its module dir, and 44 ghost directories would be 44 things each of those listings has to skip. A
  file is skipped by the `is_dir` check they already make.
- Read on the **miss path only** — an id naming a live directory is already the answer — so carrying
  every old id forever costs one small file read, and only when something asked for an id that isn't
  current.
- **Chains are collapsed, not followed.** Renaming a rename re-points the first alias at the final id,
  so a lookup is always one hop.
- **A live id is never also an alias.** Renaming back removes the redirect.
- **Deleting an entry forgets its aliases**, which releases those ids for a future mint. An alias
  pointing at nothing helps no reader.

Where resolution happens:

| Read path | Resolves an old id |
| --- | --- |
| `Tools::{get, resolve, tool_dir, script_path, read/write_script, archive, unarchive, remove, run, command, rename}` | yes |
| `Tools::{sync_agent_bin, help_for}` — the `bin_tools` membership test | yes |
| `Agents::tool_split` — what a review says an agent can run | yes |
| `Projects::{get, resolve, project_dir, hive_path, archive, unarchive, remove, rename}` | yes |
| `Config::project_dir` — where a run starts, for crates that never open the registry | yes |
| `dashboard_dir` — the one place a dashboard id becomes a directory | yes |

## Renaming

Two halves, and they answer different questions:

* The registry's **alias** is what stops anything breaking. It covers everything no crate can reach.
* Re-pointing the **references** is what makes the rename worth doing — a definition left naming the
  UUID still works and still costs exactly what it cost before.

So the whole-store rename lives in `adi-core`, where every store is in reach, exactly as
`rename_project` already did:

```bash
adi-mono tools rename <id|old-id> <new-id>      # adi_core::rename_tool
adi-mono projects rename <id|old-id> <new-id>   # adi_core::rename_project
```

`rename_tool` moves the directory, records the alias, regenerates the global `.bin`, then rewrites
every agent's `bin_tools`. `rename_project` does the same and then follows the id into tools, agents,
triggers, secrets, knowledge bases and the database.

Either may be given an id the entry no longer has, so a rename can be corrected without first working
out which id is current. Renaming to the id it already has is a no-op, not an error.

**A `sys-*` tool cannot be renamed.** Its id is the handle `Tools::seed_system` re-creates it by and
that code names as a constant (`SYS_KNOWLEDGE_ROOT`), so a renamed one would simply be created again
beside itself on the next start-up. Archive it to disable it, as with delete.

**A dashboard has no rename yet.** Its id is also its port-lease key (`<id>/frontend`, `<id>/backend`)
and the directory its `.adi/hive.yaml` names, so a move has to carry those too. New dashboards mint
slugs and `dashboard_dir` already resolves aliases, so adding the rename is additive when somebody
wants it.

## What this cost the store

Nothing that resolved before stops resolving. The one entry moved so far:

```
tools/ec5bd98c-35c1-4e9e-ba25-5c2dbd3d5a99  →  tools/confirm-sync
```

and afterwards `adi-mono tools run ec5bd98c-35c1-4e9e-ba25-5c2dbd3d5a99 --stale` still runs it, as
does the `.agent-bin` shim generated before the move.
