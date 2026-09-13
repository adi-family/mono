# Marketplace bundles — a design for v2

**Status: design.** Nothing described here is built. It extends the shipped v1 spec
(`docs/marketplace.md`) rather than replacing it — every v1 property without a stated reason to
break it still holds: one HTTPS JSON manifest the operator adds by hand, an entry pins a git repo
at a full 40-hex commit, install reads the cache and never the network, what lands is inert until
somebody starts it, and there is no server, no accounts, no hosting. This document is what changes
on top of that, and why. Tracked as ADI-MONO-49.

## What changed, in one paragraph

v1 shipped exactly one kind of item — an app, which is a dashboard. This round generalizes what an
item *is*, without adding a second kind: **every marketplace item is a bundle, a named collection
of platform elements from one repository, and an app is the special case of a bundle whose only
element is a dashboard** — which is exactly what v1 already shipped, byte for byte. A bundle may
carry any of eight kinds of element (agents, tools, dashboards, LLM backends, embedding backends,
hive services, triggers, a project scaffold), and the operator may install all of it or exactly one
element of it. v1's own "what is deliberately not here" section predicted a `kind` field on the
manifest; that prediction was half right. A `kind` **is** needed, but it does not belong on the top
level — the top-level thing is always a bundle — it belongs on each element *inside* one.

## The manifest

The wire shape barely moves. The array is renamed `bundles` (an old manifest's `apps` key is read
as an alias for the same array, so nothing published for v1 needs to change), and each entry keeps
every v1 field — `slug`, `name`, `description`, `icon`, `keywords`, `readme`, `gallery`, `version`,
`repo`, `commit`, `branch` — meaning exactly what it meant before. One field is added:

```json
{
  "name": "ADI starter apps",
  "bundles": [
    {
      "slug": "crm-suite",
      "name": "CRM suite",
      "description": "A follow-up agent, its import tool, and the dashboard that watches both.",
      "repo": "https://github.com/adi-family/crm-suite.git",
      "commit": "9f2c1d4e5a6b7c8d9e0f1a2b3c4d5e6f70819a2b",
      "elements": [
        { "kind": "agent", "name": "sales-bot", "description": "Drafts the follow-up." },
        { "kind": "tool", "name": "csv-import", "description": "Loads a contacts export." },
        { "kind": "llm", "name": "gpt5", "description": "The model sales-bot answers on." },
        { "kind": "dashboard", "name": "crm", "description": "The list itself." }
      ]
    }
  ]
}
```

| field | required | meaning |
| --- | --- | --- |
| `bundles[].elements` | no | a **preview** of what installing this bundle offers, for a listing page that has not cloned anything yet — exactly why `readme` and `gallery` are carried in the manifest rather than fetched from the repository. It is advisory, not authoritative: what actually installs is read off the pinned commit's own tree at install time (below), and a manifest that oversells or undersells what is there is a per-element surprise at install, not a validation failure at sync. An entry with no `elements` array is legal (an old-shape publisher, or one who has not written it yet) and installs exactly as read from the tree, just without a preview. |
| `elements[].kind` | yes (if `elements` is present) | one of `agent` \| `tool` \| `dashboard` \| `llm` \| `embedding` \| `service` \| `trigger` \| `project`. |
| `elements[].name` | yes | the published name — the file or directory stem this element carries in the repository, and the third address coordinate (below). |
| `elements[].description` | no | one line, for the listing. |

Everything else about validation is unchanged: a manifest is checked whole, an entry that fails
refuses the fetch, unknown fields are ignored. `elements[].kind` naming something this build does
not understand is **not** a validation failure — it is dropped from the preview and the listing
says so, the same tolerance `docs/marketplace.md` already asks of every other unknown field, so a
newer manifest an older machine reads still lists what it can.

## The repository layout

A bundle repository is laid out by kind, one top-level directory per kind that applies, all
optional:

```text
agents/<name>.toml        # an agent definition
tools/<name>.{sh,ts}      # an owned tool script
dashboards/<name>/        # a dashboard — frontend/index.ts, backend/index.ts, same as v1's app
llm/<name>.toml           # an LLM backend
embeddings/<name>.toml    # an embedding backend
services/<name>.yaml      # one hive service
triggers/<name>.toml      # a trigger definition
project/config.toml       # the bundle's own project scaffold — at most one per bundle
```

Each file, other than `project/config.toml`, is written in **exactly the shape the real store
already reads** — an `agents/<name>.toml` is a `[[backends]]`/`bin_tools`/`secrets`-shaped
`AgentManifest` (`crates/adi-agents/src/agent.rs`), an `llm/<name>.toml` is a
`docs/llm-backends.md`-shaped backend file, a `services/<name>.yaml` is one `ServiceSpec`
(`crates/adi-hive/src/config.rs`) — because the guides are the contract, and the store's own
schema is the one a publisher can already read documentation for. This is a deliberate departure
from inventing a bundle-specific schema: a publisher who already knows how to hand-write an agent
definition already knows how to write one for a bundle.

**Store-owned fields are never the publisher's to set**, the same rule v1 applies to
`config.toml`/`.adi/` in a dashboard repository, generalized from whole files to specific fields
within a shared-schema file: `created_at`, `updated_at`, `version`/manifest-stamp fields, and the
`project` field on every kind that carries one are stripped from what the repository ships and
written fresh at install time. A repository that versions them anyway installs, with a note, the
same as v1's `config.toml` case.

**A legacy v1 app repository — `frontend/index.ts` and `backend/index.ts` at the repository
root, no kind directories at all — is still a valid bundle:** it is read as a bundle whose one
element is a dashboard, filed under a synthetic `dashboards/<slug>/` position, and it keeps every
property v1 gave it, including a dedicated `.git` clone of its own (below). Nothing published
under v1 needs to move.

### Addressing one element

`<marketplace>/<slug>` still names the whole bundle, and installing it with nothing further
installs everything it offers. A single element adds a **third coordinate**, `<kind>/<name>`,
taken verbatim from the repository layout:

```
adi/crm-suite                          # the whole bundle
adi/crm-suite/agents/sales-bot         # just the agent
adi/crm-suite/dashboards/crm           # just the dashboard
adi/crm-suite/project                  # the scaffold — a singleton, no name needed
```

`kind` is part of the address, not just of the layout, because the same published name can appear
under two kinds (`agents/reviewer` and `tools/reviewer` are not the same element) and the address
has to disambiguate that without reading the repository first.

## No Rust in an item — and what actually enforces it

**An item may never carry Rust source or a compiled binary.** The reason is the same one v1 gives
for a pinned commit being the whole security story, one step further: `guides/*.md` describe eight
surfaces the *store* already knows how to run safely — a script under a size and shape it
understands, a declarative config file, a container it merely supervises. A `.rs` file or a
prebuilt executable checked into a bundle is a ninth thing, one this machine would have to compile
or execute blind, and the platform has deliberately never grown that capability for anything
untrusted.

**What enforces it:** the same place v1's `NotAnApp` check already lives — a static scan of the
staged clone, before it is moved out of staging, refusing an install whose tree contains a
`Cargo.toml`, a `.rs` file, or a file whose leading bytes are a compiled-binary magic number (ELF,
Mach-O, PE) anywhere under a kind directory. This is a **publishing-time content rule**, not a
sandbox, and it is worth being exactly as honest about its limits as v1 is about the pin's:

- **A hive service's `runner.docker` is a named hole.** An `image:` field names an arbitrary
  container, and nothing about "no `.rs` file in the repository" says anything about what that
  image was built from. A container that *is* a compiled Rust program is one `docker pull` away,
  fully within this rule.
- **A tool's `.sh` script, a trigger's `sh`/`ts` code block, and a hive service's `runner.script`
  are the other named hole.** All three are, definitionally, arbitrary programs — a shell command
  can `curl` a binary and execute it at runtime just as easily as a repository could have shipped
  one. The static scan catches what is *checked in*; it cannot and does not catch what a script
  *fetches*.

So the rule buys exactly one thing: a bundle cannot **publish** Rust as part of its own content.
It does not, and cannot, make every element's runtime behavior safe — that was never the pin's job
either. A future version that wants to carry Rust honestly (a compiled agent tool, say) is a
named, separate extension — not a relaxation of this rule, a new one with its own review story.

## Installing part of a bundle

This is the problem the whole design turns on. Two questions have to be answered together: what id
does an element land as, and what happens when it names a sibling that either was not installed or
landed under a different id than the repository wrote.

### Two collision regimes, decided by whether an id is read back out of a structured field

Every kind mints an id the way every store registry already does — `adi_config::mint`, a slug of
a name made unique by a numbered suffix (`crates/adi-config/src/ids.rs`), the exact mechanism v1
already uses for dashboards. What differs is **what a fresh install is allowed to do on a
collision**, and the answer depends on one question: is this id read back out of some *other*
element's structured field?

An agent's own manifest names three things by id: `bin_tools` (tool ids), `backends[].backend`
(LLM backend ids), and `project` (a project id) — and a dashboard's, tool's, and trigger's manifest
each carry the same `project` field. Nothing else in the store reads an agent, a dashboard, a
trigger, or a hive service's key back out of another element's structured field — a shell command
might *say* `adi-mono agents run sales-bot`, but that is free text nobody parses, not a checked
reference.

That split is the whole rule:

| kind | on a name collision |
| --- | --- |
| tools | **land verbatim or refuse.** A tool becomes a `tools/.bin/<name>` shim and is named by exact id from `bin_tools`; minting `csv-import-2` would leave a sibling agent's `bin_tools: ["csv-import"]` silently pointing at somebody else's pre-existing tool of that name — the sharpest failure mode in this whole design, so it is refused rather than risked. |
| LLM backends, embedding backends | **land verbatim or refuse**, for the same reason: `backends[].backend` names one by id, and reusing an existing backend's id by accident would attach the bundle's model or embedding configuration to the wrong login. |
| projects | **land verbatim or refuse**: every other kind's `project` field names one by id. |
| agents, dashboards, triggers, hive services | **mint freely**, exactly like v1's dashboards — a numbered suffix on a collision is ordinary, because nothing structurally depends on any of their ids. |

A refused element is not a refused install: the install's plan names every element, the id it
would land as, and which ones are blocked by a collision. The operator resolves each block
explicitly — rename it in the request, or leave it out of this install — before anything is
written. Nothing lands under a silently-suffixed id for a kind where that would break a sibling's
reference.

**Consequence worth stating plainly: installing the same bundle twice is not the free "crm/crm-2"
that a second dashboard install is**, the moment that bundle carries a tool, a backend, or a
project. Every such element collides with the first copy's and has to be renamed by hand, one at a
time. This is real, new friction v1 never had, and it is the direct cost of refusing to let a
suffix silently desync a bundle's own wiring. The escape hatch is the same one hive services
already need for their own state (see below): a project-scoped second copy, filed under a
different project, so its tools and backends are wanted under different names by design rather
than by luck.

### Cross-element dependencies

A **structured** reference — `bin_tools`, `backends[].backend`, `project` — is read straight off
the landed ids, because every element in the same bundle install lands under its published name
whenever that name is free (the whole point of the verbatim-or-refuse rule above). A publisher who
writes `bin_tools = ["csv-import"]` in `agents/sales-bot.toml` gets exactly that, unrewritten, the
overwhelming majority of the time; it only needs attention on the renamed-to-resolve-a-collision
path, where the install's own report names exactly which reference no longer resolves and to what
it used to name.

A reference the operator did not install at all — an agent naming a tool the operator excluded
from a partial install — is **not refused**. It is offered and left unresolved, exactly the
existing behavior for a hand-authored agent: "an agent with no tools enabled has an empty bin,
however many tools the store holds" (`guides/tools.md`). Refusing to install `sales-bot` because
`csv-import` was left out would turn "install only the agent" into a lie about what that phrase
means; the agent lands, runs, and simply does not have that tool until the operator installs it
too (`adi/crm-suite/tools/csv-import`) or ticks an existing one of the same job by hand.

An **unstructured** reference — a dashboard's backend code calling a hive service by hostname, a
trigger's shell block invoking another tool's shim by name, an agent's system prompt mentioning a
capability by name — is not something this design can rewrite, because nothing here parses
TypeScript or shell. It is called out once, here, rather than pretended away: **a bundle's own
code is expected to name its own siblings by their published id**, and a renamed-to-resolve id
breaks that reference the same way it would break a hand-edited one, with no automatic fix-up
possible. The install's report says which ids were renamed; fixing the code that assumed the old
one is the operator's or the publisher's job, not this design's.

## Secrets

**A bundle never carries a secret's value, and nothing here tries to make it.** What it can and
should carry is exactly what the store's own schema already lets an element *declare* a need for,
because both mechanisms already exist and neither needed a new field:

- An `agents/<name>.toml` may carry `secrets = [{ name = "OPENAI_API_KEY" }]` — a real
  `SecretAttachment`, naming a secret this agent wants attached, whether or not that secret exists
  yet on the machine it lands on.
- An `llm/<name>.toml` or `embeddings/<name>.toml` may carry `api_key_env = "OPENAI_API_KEY"` — the
  existing convention both backend kinds already use for a hosted credential.

Install does not create a placeholder secret, does not prompt for a value mid-install, and does
not refuse for one being absent — a secret is encrypted, per-machine, and the operator's alone to
set (`guides/secrets.md`). What it does is **read every secret name named anywhere in the bundle**
(every `SecretAttachment.name`, every `api_key_env`) and diff it against what this machine actually
has, global and project scope both. The result is a **report, not a gate**: `Installed.notes`
gains one line per missing name ("names OPENAI_API_KEY, which is not set — sales-bot and the `gpt5`
backend will not work until you set it"), and the bundle's row in the listing keeps showing which
names are still missing for as long as they are — computed live off the current secrets store on
every read, the same way `outdated` is computed live off the current cache, never stored and
frozen at install time.

**What the element does in the meantime is exactly what an agent with a dangling secret attachment
already does today: nothing breaks, nothing is injected.** "A secret attachment that names a dead
scope silently injects nothing" (`crates/adi-agents/src/lib.rs`) is existing, verified behavior —
an agent whose `OPENAI_API_KEY` is unset simply runs without it, and an LLM backend whose
`api_key_env` names nothing readable fails the way any misconfigured backend already fails
(`docs/llm-backends.md`'s own `auth`-class hold and ask). Nothing new is invented for the bundle
case; the report exists so the operator finds out from a listing rather than from a failed run.

## Inert on arrival, per kind

v1's rule — installed is not started — has a different shape for each kind, because "started"
means something different for each one.

| kind | what "inert on arrival" means, specifically |
| --- | --- |
| **dashboard** | unchanged from v1: `archived_at` stamped, hive file written parked (`.adi/hive.yaml.archived`), nothing executes until `start`. |
| **agent** | inert by construction — nothing runs an agent definition by existing. What needs stating is that its `bin_tools`/`backends`/`secrets` may name things this install did not bring, and none of that is an error (above). |
| **tool** | inert by construction, same as a hand-created tool — "creating a tool gives it to nobody" (`guides/tools.md`). Landing the script and regenerating the *global* `.bin` shim is not the same as any agent being able to reach it; that still needs `bin_tools`, whether pre-wired by the publisher on a bundled agent or ticked by hand later. |
| **LLM backend, embedding backend** | inert by construction — a backend is a file nothing runs until some agent's `backends` list (LLM) or `embeddings/settings.toml` assignment (embedding) names it. Landing one changes nothing about what any existing agent or consumer resolves to. |
| **project scaffold** | inert by construction — a `config.toml` and nothing else; nothing supervises a project directly. |
| **trigger** | **must arrive `enabled = false`, forced, regardless of what the repository's own `triggers/<name>.toml` says.** This is the sharp case the operator's brief calls out by name: a `TriggerManifest` defaults to `enabled = true` when the field is absent (`crates/adi-triggers/src/trigger.rs`), and a `background` or `event` trigger *launches on its own* the moment it is enabled — a webhook trigger becomes a live, reachable endpoint the instant it lands. Install writes the file with `enabled` forced to `false` no matter what value the repository carries, and the operator's own `enabled` toggle is the one deliberate act that lets it run — exactly analogous to a dashboard's `start`. |
| **hive service** | **has no on/off switch to force — and that is a real gap this design has to work around, not paper over.** A `ServiceSpec` in `hive.yaml` (`crates/adi-hive/src/config.rs`) carries a `start:` policy, `always` or `on-demand`, and nothing that means "known to the supervisor but never launched." Worse: a service with no `proxy.host` — a typical backing service, a database, a queue — **defaults to `always`**, meaning the moment its entry is merged into a live `hive.yaml`, it launches at the supervisor's next read, with no request required. So a bundled service is **not** written into the live `hive.yaml` (global, or the bundle's project's) at install time at all. It lands as a standalone file the supervisor never reads — `marketplace/services/<bundle-id>/<name>.yaml`, holding exactly that one `ServiceSpec` — and *starting* it is the act of copying that block into the real `hive.yaml` under its landed key, at which point the supervisor's own periodic re-read picks it up and its `start:` policy governs it from there on, same as any hand-written service. This reuses no new mechanism in `adi-hive` itself; it only decides *when* a service's YAML is allowed to reach a file the supervisor actually scans. |

The last row is worth flagging on its own: it is the one place this design proposes behavior the
store does not already have a slot for (every other "arrives parked" mechanism above is either v1
unchanged, or an existing field used exactly as documented). Stopping a `runner.docker` service
afterward is also only half-automatic through this route — removing its key from `hive.yaml` stops
the supervisor's own `docker wait` loop, but the container itself is documented to survive that
("a supervisor restart leaves the container running… to actually stop it, `docker stop <name>`",
`crates/adi-hive/src/config.rs`) — so uninstalling a docker-backed service element says so, rather
than claiming a clean stop it cannot deliver.

## Provenance, without one directory per element

v1's `.adi/marketplace.json` lives beside the one thing it describes, because a v1 app *is* one
directory. A bundle's elements are a `.toml` file, a `.sh` script, a `.yaml` fragment, and at most
one real directory (a dashboard) — most of them have nowhere to put a sidecar file without writing
a foreign field into a schema that is supposed to be exactly the real store's own.

So provenance moves up one level, to the marketplace module itself, mirroring how `sources.toml`
and `cache/<name>.json` already live there rather than beside a dashboard:

- **The bundle keeps one permanent git clone of its own**, `marketplace/bundles/<bundle-id>/` —
  not staged-and-discarded the way v1's staging directory is, kept, because it is now the one
  place `git log` answers "what installed this" the way v1's per-app `.git` used to answer it for
  every element at once. The operator is not expected to edit it; it is the publisher's copy, and
  it only moves under `update`.
- **One install ledger per bundle**, `marketplace/installs/<bundle-id>.json`, mapping every
  installed element to the id it actually landed as, its kind, and — for every kind other than a
  dashboard's own clone (below) — a content fingerprint of what was last written there. This is
  what a listing reads to answer "what does this bundle have installed, and is any of it edited,"
  the same role `.adi/marketplace.json` played for one directory.
- **The legacy single-dashboard bundle is the one exception, and keeps v1's mechanism verbatim**:
  its clone *is* `dashboards/<id>/`, `.adi/marketplace.json` lives there exactly as today, and
  `git -C dashboards/<id> pull` still works unmediated by anything described in this document. A
  bundle of one dashboard costs nothing extra, which is the property decision #2 asks for.

For everything else, "what does `git status` say" is not a question that can be asked of a plain
file copied out of somebody else's repository — **an operator who edits an installed agent's
prompt in the panel has edited a store file, not a working tree**, and there is no `.git` there to
notice. Drift is therefore tracked by **content fingerprint, not by git**: the ledger records the
fingerprint of what was written at install (and at each successful update); before writing over an
element on update, the live file's current fingerprint is compared to the recorded one, and only
a match is safe to overwrite.

## Update

`marketplace update <marketplace>/<slug>` fetches the manifest, moves the bundle's own internal
clone onto the new pin (a fast-forward, exactly `git::move_to` today), and then re-applies every
element from that clone into its live location — **per element**, not as one all-or-nothing act:

- **A flat-file or script element** (agent, tool, trigger, LLM backend, embedding backend) whose
  live fingerprint still matches what was last written is fast-forwarded to the new pin's content.
  One whose fingerprint has moved — the operator edited it — is **left alone and reported**,
  exactly v1's `Dirty` refusal, scoped to that one element rather than the whole bundle. `--force`
  on that element overwrites it and loses the edit, same semantics as v1, finer grain.
- **A dashboard element**, in a multi-element bundle, gets the same fingerprint treatment over its
  tracked files — with the same carve-out v1 already solved for its generated entry points
  (`frontend/index.ts`, `frontend/index.html`, `backend/index.ts`): those three are the panel's to
  rewrite, excluded from the drift check exactly as they are today, reused unchanged.
- **A project scaffold** updates its `config.toml` the same way as any flat file — in practice
  almost never, since a publisher revising a bundle rarely changes the project's own name.
- **A hive service element** updates the parked fragment file if it was never started, or the live
  entry in the target `hive.yaml` if it was — fingerprinted the same way, since a started service's
  entry is a plain YAML block an operator could also have hand-edited.
- **The legacy single-dashboard bundle** updates exactly as v1 does today: nothing above applies to
  it, and it is not worth a special code path to make it consistent with the general case, only a
  special case to leave alone.

**A bundle update can therefore partially succeed** — three elements fast-forwarded, two blocked
on local edits — where v1's single-app update is all-or-nothing. That is a genuine improvement the
new shape buys almost for free, and it is called out because it means "did the update work" is a
per-element question for a multi-element bundle, not a single yes/no the way it is for an app.

## What "installed" means for a partially-installed bundle

The listing shows a bundle the way v1 shows an app — cached entry, every install of it — with one
addition: **installed** is now a fraction. A bundle's row names how many of its `elements` are
installed here (2 of 4, say), which ones, under what ids, whether each is outdated against the
current pin, and whether any named secret is still missing. There is no "fully installed" state
that means anything more than "every element the manifest currently lists happens to be here" —
installing a fifth element the publisher adds later is exactly as ordinary as installing the first
four were, addressed the same way (`adi/crm-suite/agents/new-thing`), against the same install
ledger, minting or refusing an id by the same per-kind rule above.

**Installing one more element later** reads the ledger, lands the new element under its published
id (refusing on a collision exactly as a first install would), and adds it to the same ledger entry
— it is not a second install of the bundle, it is the existing one growing. **Removing one element**
is the mirror: it leaves the ledger's other entries untouched, uninstalls that one element by its
own kind's ordinary path (below), and the bundle's row simply shows one fewer element installed —
never a reason to touch anything else the bundle brought in.

## Uninstall

There is no marketplace-wide uninstall verb, the same as v1 delegates uninstall to the Dashboards
page. A bundle spread across the store delegates the same way, kind by kind, to whichever page
already owns deletion for that kind:

| kind | how it goes |
| --- | --- |
| dashboard | Dashboards page: Archive → Delete, unchanged from v1. |
| agent | `POST /api/agents/delete`. |
| tool | `POST /api/tools/archive` then `/remove` (archive drops the `.bin` shim; remove is permanent). |
| trigger | `POST /api/triggers/delete`. |
| LLM backend | `POST /api/llm/backends/delete`. |
| embedding backend | `POST /api/embeddings/backends/delete` — refused (409) while a consumer is still assigned to it, exactly as today; reassign first. |
| project | `POST /api/projects/remove`. |
| hive service | **no delete endpoint exists today** — `GET /api/hive` / `POST /api/hive/start` / `/stop` / `/create` are the whole surface (`guides/services.md`). Removing one is editing the target `hive.yaml` and dropping its key, which is what this uninstall path does by hand; a `runner.docker` element additionally needs `docker stop`/`docker rm`, said plainly rather than claimed automatic (see "Inert on arrival," above). |

Every one of these removes that element from the bundle's ledger entry, never the ledger entry
itself while any element remains; the ledger and the internal clone are only removed once nothing
from the bundle is installed anywhere.

## What is deliberately not here

- **A bundle-wide install/uninstall transaction.** Installing five elements is five per-kind
  writes; a failure partway leaves whatever landed already landed, reported, not rolled back — the
  same posture v1 takes toward "nothing partial in the dashboards tree" for *one* app, deliberately
  not extended to "nothing partial across a whole bundle," because partial is the point of decision
  #2.
- **Nested projects, or more than one project scaffold per bundle.** A bundle registers at most
  one project; a publisher wanting a sub-project hierarchy nests it under a project the operator
  already has, by hand, after install.
- **Rewriting an unstructured (code-level) reference between elements** — see "Cross-element
  dependencies," above. This is a hole this design names rather than closes.
- **Rust-carrying items.** Named in the brief as a future extension, and it is one: a signed,
  reviewed path for a compiled tool or agent runtime is a different trust model from "the pin is
  the whole security story," and deserves its own document rather than a loosened rule here.
- **Sharing an installed bundle's state across machines over the mesh** — out of scope the same
  way `docs/llm-backends.md` leaves holds unshared across machines; each machine's install is its
  own.
- **Version comparison beyond the pin**, and **counts** — unchanged from v1's own list, for the
  same reasons v1 gives.

## Decisions taken

The calls made in this document that the operator would most reasonably want to argue with, and
why they were made this way:

1. **The wire field is renamed `bundles`, with `apps` read as an alias for the same array.** The
   alternative — keep calling it `apps` forever — costs nothing to implement and never breaks a
   published manifest, but reads increasingly wrong as a manifest that lists a tool-only bundle
   under a JSON key spelled `apps`. The alias makes the rename free for every existing publisher;
   only new documentation has to say the new name.
2. **`elements[].kind` is advisory, checked against the real repository only at install time, not
   at manifest-sync time.** The alternative — validate it at sync, the way v1 validates `repo` and
   `commit` — would mean a manifest can never describe a bundle without also promising exactly what
   its tree contains today, coupling a listing's correctness to a git fetch on every sync. The cost
   is a bundle whose preview lies until somebody tries to install the element it named wrong.
3. **A collision on a tool, an LLM/embedding backend, or a project id is refused, never
   auto-suffixed — unlike v1's dashboards, and unlike this document's own agents/dashboards/
   triggers/services.** This is the sharpest asymmetry in the whole design, and it makes a second
   install of the same bundle real friction where v1 had none. The alternative (mint a suffix
   everywhere, exactly like v1) is simpler to explain and was seriously considered; it was rejected
   because a silently-renamed tool id is not a cosmetic collision, it is an agent's `bin_tools`
   quietly starting to name a stranger's script.
4. **A bundle's own project, if it carries one, is the only way to get project-scoped elements —
   there is no per-element opt-out.** A publisher cannot predict what project ids exist on someone
   else's machine, so an element cannot name one; the only honest project a bundle can hand its
   siblings is one it brought itself.
5. **A hive service is never written directly into a live `hive.yaml` at install time**, because
   `hive.yaml` has no field that means "known, not running," and a service with no `proxy.host`
   defaults to launching immediately on being read. The parked-fragment-file mechanism above is new
   machinery this design adds specifically to make "inert on arrival" true for this one kind — it
   is the one place this document proposes behavior beyond what `adi-hive` already does, and it is
   worth the operator's attention for exactly that reason.
6. **The bundle keeps one permanent internal git clone, rather than discarding staging the way v1
   does.** It costs disk (one clone per bundle, held indefinitely) that v1's transient staging
   never paid. It was kept because it is the only way "what installed this" stays a `git log` away
   for the majority of elements that cannot otherwise carry their own `.git`.

## What v1's code assumes that this design changes

- `docs/marketplace.md`'s own prediction — "the manifest is expected to grow a `kind`" — is only
  half right, as covered above: the field exists, but on the element, not the entry, because the
  entry is no longer one kind.
- `crates/adi-marketplace/src/install.rs`'s whole `stage`/`land` pipeline assumes **one clone
  becomes one directory** (`dashboards/<id>`) via a single `std::fs::rename`. That assumption does
  not survive a bundle: the clone becomes zero, one, or many store locations, of at least three
  different shapes (a directory, a single file, a merge into an existing map). None of the existing
  functions generalize as written; a bundle installer is new code alongside them, not a
  parameterization of them.
- `STRIPPED_ON_ARRIVAL` (`.adi`, `hive.yaml`) and `git::STORE_OWNED` (`config.toml`, `.adi/`,
  `node_modules/`) are dashboard-specific lists. The generalized rule in this document — store-owned
  fields are stripped per-kind, not per-path — needs its own list per kind, not a reuse of these
  two.
- Nothing in `crates/adi-marketplace` today reads a repository's tree beyond the two required
  dashboard entry points. A bundle installer has to walk up to eight directories and dispatch on
  what it finds, which is genuinely new surface, not an extension of `REQUIRED_FILES`.
