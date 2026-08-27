# Changelog

What changed in each release of adi, written for the person who will read it — not for a
release script.

**This file is the release notes.** At tag time `scripts/changelog.sh <version>` lifts a
version's section out of here and it becomes three things: the `notes` field of the published
`manifest.json`, the body of the GitHub release, and the *What's new* the control panel shows
before it installs anything (`docs/adi-update.md`). A version with no section here does not
ship — the release workflow stops rather than publish a placeholder.

So write the entry when the work lands, and write it for someone who is about to decide
whether to restart their machine for it. Say what is different now, not which files moved.
Internal refactors, comment passes and CI plumbing belong in the git log, which keeps them
perfectly well already.

Keep `## Unreleased` at the top; cutting a release renames it to `## <version> — <date>`
(ISO date, an em dash) and opens a fresh empty one. The version heading is the only line the
extraction script cares about.

## Unreleased

### Added

- **The chat's right rail says who you are talking to, and where.** Chat Analytics counted what a
  conversation cost and what went wrong in it, and never named the agent having it — the name was
  on the session row you clicked to get here and nowhere on the screen you landed on. Above it
  there is now an **Agent** block of its own: the agent's name and what it is doing right now
  (waiting on you, answering, running, awaiting a wake, idle), the backend and model behind it and
  the project it is filed under; the conversation's **working directory**, when it started and
  when it last said something; and the settings that explain the behaviour in front of you — its
  permission mode, how many tools, knowledge bases and secrets it carries, whether it keeps a
  memory, and whether it runs unattended. It appears before the first turn lands, which is when
  the counts below it have nothing to show yet.

- **Analytics opens with what is running right now.** The page led with a fortnight of history,
  so it could tell you what this machine had done and nothing about what it was doing. A panel
  above the totals now names every run in flight — the agent, the project it is filed under, the
  task it was given, how long it has been at it and when it last said something — and the live
  sessions of interactive agents beside them, since those keep no run history to appear in.
  The times climb while you watch, and when nothing is running the panel says so rather than
  vanishing, which reads the same as a page that hasn't loaded.

## 0.3.3 — 2026-08-26

### Fixed

- **Linux and Windows have builds again** — 0.3.2 shipped for macOS alone. `domain`,
  `frontdoor_addr` and `frontdoor_label` became accessors in an earlier commit and only the
  macOS paths were updated with them; the Linux and Windows ones sit behind `#[cfg]`, which a
  Mac never compiles, so the tree looked green everywhere except the release builder. 0.3.2's
  fleet fix reaches a node with this release.

## 0.3.2 — 2026-08-26

### Fixed

- **A node's deeper hostnames are reachable over the fleet.** `<service>.<node>.n.adi` used to
  mean exactly four labels, so only a service whose name was one label had an address from
  another machine: `nosh.zomro-de1.n.adi` worked and `app.nosh.zomro-de1.n.adi` — the very same
  machine's `app.nosh.adi` — was answered with *not a fleet hostname*. The node label is now
  simply the one before `n.adi`, and everything to its left is the service, which is exactly
  that machine's own hostname with its `.adi` taken off. Grants name the whole thing
  (`http:app.nosh`), and the control panel's links, transfers and dashboard rows follow.
  Both machines need this version: a node running an older one refuses the name on the wire.
  Plain `http://` works as soon as it is granted; `https://` to a name that deep needs a
  dotted `proxy.mesh_nodes` entry (`nosh.<node>`) on the viewer's front door, because one
  wildcard label covers one level.

## 0.3.1 — 2026-08-21

### Fixed

- **Linux and Windows have builds again.** 0.3.0 published no Windows package at all, and a
  Linux one cut by hand off a laptop — the release builder had been unable to produce either
  since the code index arrived, because that brought the first C++ into a tree whose build
  hosts only ever had a C compiler. Nothing about the software itself is different here: this
  is 0.3.0, built where it was supposed to be built.

## 0.3.0 — 2026-08-21

### Added

- **The version, and the way to the next one, live in the top bar.** Every screen says what
  this machine is on. When a newer release is published *for this platform*, an Update button
  appears beside it, shows what is in it, and installs it — download, checksum, signature,
  swap, restart, and an automatic roll-back if the stack does not come back. macOS, Linux and
  Windows alike.
- **This changelog**, and the release pipeline that carries it all the way to that button.
- **An agent can stop and ask you something.** A run that needs a decision leaves a question
  in the conversation and waits on your answer instead of guessing; the chat says it is
  waiting, and the sessions rail marks it in blue so you can find it among forty others.
- **A run can be told something while it is still working** — the next message queues instead
  of bouncing, and a turn that runs out of rounds wraps up rather than throwing the work away.
- **A run can ask to be woken**: by an event, by the clock, or by a script that decides.
- **Messages carry images**, and every engine that can be shown one is.
- **You can speak a message into the composer**, in whichever transcription engine you pick.
- **Knowledge outlives the run that worked it out.** An agent can write what it learned into a
  knowledge base, search it by meaning later, and keep some of it private to itself.
- **A conversation can be reviewed by another conversation**, so what a run got wrong is fed
  back rather than read once and forgotten.
- **Chat analytics**: what every agent has actually run, which have never been launched at
  all, where a conversation went wrong, what was said twice, and what a given day cost.
- **A conversation can be starred**, and a starred one is never aged out by the session cap.
- **Conversations keep one shell**, so a directory is named once and every command after it
  lands in the same place.
- **The adi loop speaks to z.ai's GLM models**, and its own tools — Read, Write, Edit, Bash,
  Glob, Grep — are available whichever provider is running the loop.
- **A dashboard is edited beside the page it draws**, and you can point at the part you mean.
- **A dashboard moves to another machine in two clicks**, and the dashboards rail shows the
  whole fleet rather than only this machine.
- **The fleet reaches further**: a node can route through a relay of ours when a direct path
  will not form, a node's own panel opens from the rail with the password already held, and
  the Linux installer now brings its own pinned, verified copy of bun.
- **AdiFleet on iPhone and iPad** — a phone that views the fleet and never hosts it: full-screen
  dashboards, a Home Screen icon, and connections that retire themselves when the network
  changes.
- **A project's slug can be edited**, and every store filed under the old one follows it.
- **`adi-mono indexer`** — the code index, in-tree: search by symbol, by text, or by meaning,
  and find copy-paste however thoroughly it was renamed (`docs/indexer.md`). Its Rust-only
  counterpart `adi-clone-lint` proves the renaming from HIR rather than guessing it
  (`docs/clone-lint.md`).
- **Every crate keeps a generated page of the shapes it moves** (`structs.gen.md`).

### Changed

- **The control panel wears a real component library.** `adi-ui` owns the tokens, the tables,
  the trees, the markdown and the code editor, instead of each page carrying its own copy.
- **The sessions rail opens as a shortlist, not the register** — the first nine answer to a
  number, and the rest arrive a page at a time.
- **Onboarding asks for the fields the route you picked actually needs**, and no others.

### Removed

- **The wasm employee engine.** An agent is a process again; the backends that survived do
  everything it did and can be run from a terminal.

### Fixed

- **The updater's checksum step called a tool Linux does not ship**, so no Linux node could
  update itself. It hashes in-process now, on every platform.
- **A dev build reports the version it was built from**, so a control panel run out of a
  checkout no longer reads as a failed auto-update.
- **A run is named by when it started, not by its pid** — the kernel reuses those, and a
  recycled one made a finished run look alive.
- **A run whose engine never started says so** instead of sitting at "unknown", and a call
  nothing is left to answer stops reading as still running.
- **A chat moves in the rail when it speaks, not when it is read.**
- **The box you start a chat in is the box you answer in**, and the box you say it in is the
  box you stop it from.
- **A table in a message renders as a table**, not as a row of pipes.
- **A wrapped bullet is one bullet.** A markdown list hard-wrapped across lines — which is
  every list written to a line limit, and most of what an agent writes in a chat — had the
  tail of each item escape the bullet and land under the list as its own paragraph.
- **Dictation could be started and then never stopped.**
- **The macOS bundle stopped shipping an instruction older Macs do not have**, which had made
  it refuse to launch there.

### Performance

- **The store keeps itself out of Spotlight**, which had been indexing every session file.
- **A session listing is a row read, not a directory walk** — and it asks once per listing
  rather than once per session.
- **A message re-renders its own card**, not the whole transcript.
- **A migration that could never finish stopped running four hundred times per request.**

## 0.2.0 — 2026-08-01

### Added

- **Auto-update.** A pushed tag is the whole release: every platform is built, verified, and
  published, and every installed machine takes it on its own — checksummed, signature-checked
  on macOS, health-checked after the restart, and rolled back if the stack does not come back
  (`docs/adi-update.md`).
- **The app is the chat.** Once the root agent exists the front door is a conversation: every
  session on the left, the agent above the composer, dashboards on the right. A guided setup
  wizard stands in until then.
- **The adi loop** — our own agent loop, answering on any provider you name, with its own
  hands: Read, Write, Edit, Bash, Glob, Grep.
- **The fleet.** Another machine, reachable at a name you type, over a mesh that needs no open
  port: `*.n.adi`. A Linux node installs over ssh; an iPhone views it and never hosts it.
- **Windows.** The whole workspace cross-compiles and runs there — Task Scheduler in place of
  launchd, NRPT in place of the resolver files — with real-Windows CI.
- **Agent-authored dashboards**, filed under projects, with an embedded agent chat that starts
  in the dashboard's own directory.
- **Secrets**: encrypted and scoped, injected into runs through a per-agent allowlist, with an
  OAuth broker for the values that expire.
- **An event bus**, and triggers that fire on what it publishes.
- **A shared SQLite store** every agent, tool, and dashboard can reach.
- **Tools** as a first-class entity — user CLIs, system tools, and the per-agent bin directory
  that puts them on a run's `PATH`.
- **Project hooks and workspaces**: scripted working copies, each with its own terminal.
- **Docker services** in the front door, including attaching to a container that already runs.
- **The control panel became a workbench** — one explorer over every scope, a store browser, a
  file editor with highlighting, and tables you can sort, hide, reorder and have remembered.
- **Install the control panel as an app.**

### Fixed

- **Two services on one host** no longer share a socket that can only carry one request.
- **The front door hot-swaps proxy routes on reload**, so a new domain needs no restart.
- **A hand-repointed front-door daemon is never auto-migrated** out from under you.

## 0.1.0 — 2026-07-16

The first release: the platform that everything since is built on.

### Added

- **ADI DNS** — a local split-DNS resolver serving the `.test` and `.adi` zones and forwarding
  the rest, with a status file and a landing page.
- **The `.adi` front door** (`adi-hive`): one HTTP door for every service on the machine,
  supervising the runners behind it and allocating their ports through a registry.
- **The macOS app** — a menu-bar and windowed shell over `adi-mono`, notarized and stapled.
- **`adi-mono`**, the one CLI: projects, tasks, agents, triggers, services, ports, updates.
- **The control panel** at `app.adi`, and the project registry, task tree, and agent store
  behind it.
- **Agents you can run, watch, and stop**, filed under projects like everything else.
- **Triggers** — background code fired by webhooks and their kin.
- **Auto-update from one published DMG.**
