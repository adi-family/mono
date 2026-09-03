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

### Changed

- **The chat rail opens on the sessions you started.** The filter box in the Sessions head has
  been there since 1.1.0, but it opened on **All sessions** — and on a machine whose agents launch
  each other, that is a rail where the four conversations you had are somewhere among the four
  hundred the machine spawned for itself. **Only started by me** is now the default, so the rail
  answers "what was I just doing" first and the rest is one press of the funnel away. Nothing is
  hidden quietly: the funnel is lit from the first draw because the list is narrowed, whatever is
  on screen stays listed whoever started it, a live terminal session stays listed because nobody
  records who opened one, and **All sessions** still shows everything. Two things to know if the
  rail looks short: a conversation from before 1.1.0 is attributed to nobody and so is not counted
  as yours, and a machine with no chats at all still reads "No chats yet — press New to start
  one." rather than blaming the filter.

## 1.4.2 — 2026-09-03

### Fixed

- **The Windows package is built again.** 1.4.1 shipped without one, so no Windows machine could
  take that release at all: a diagnostic section written for the macOS front door read a file's
  permission bits through `std::os::unix`, which Windows does not have — and although that code
  can never run there, it still had to compile there. This is 1.4.1, for Windows.

## 1.4.1 — 2026-09-03

### Fixed

- **The front door can be installed on a Mac again.** Since 1.0.0 it could not, on any machine,
  and nothing said so. A root daemon is worth no more than the file it runs, so ADI refuses to
  point one at a binary an ordinary user could replace — correctly, because with the daemon's
  self-watch that is a way to become root without a prompt. But the binary it was naming lived
  inside the app bundle, and an app dragged into `/Applications` belongs to whoever dragged it:
  the refusal fired on every install, `.adi` route and repair, every time, and it fired *before*
  the password prompt, printing its reason where only a subprocess could see it. Machines whose
  front door predated the check kept working and hid it; anyone else got a `.adi` that resolved
  and then hung, with three green ticks in the setup panel above it. The daemon now runs a
  root-owned copy of `adi-hive` that the privileged install puts in `/Library/Application
  Support/ADI/`, which is exactly what the rule was asking for. One consequence worth knowing:
  a copy in root's keeping cannot be refreshed by an auto-update, so after one the front door
  goes on proxying with the build it was installed with — the services list says so and offers
  **Update the front door to this build**, and everything else about the update lands as usual.
- **The repair now reaches the machines it was written for.** 1.4.0 taught ADI to notice a front
  door that was installed and answering nothing, but it only offered to fix a daemon definition
  it recognised — and it recognised them by a filename that daemons installed before 1.0 do not
  carry. The one machine the check existed for was therefore the one it skipped. A definition is
  ours if it runs a program we install, whatever generation wrote it; one repointed at somebody
  else's build is still never rewritten, only started. A dead front door is also now repaired
  ahead of a merely stale one, since the repair fixes both on its way past.
- **A diagnostic report describes the root daemon.** It detailed every per-user service and said
  nothing about the only one that answers `.adi`, so an archive from a broken machine looked
  exactly like an archive from a working one. It now carries that daemon's definition verbatim,
  whether the program it names is present, whether that program is the build the app shipped,
  and whether the automatic repair has ever run here.

## 1.4.0 — 2026-09-03

### Fixed

- **A front door that stopped is put back when you open ADI again.** `.adi` names are answered by
  a root daemon, and the only question ever asked about it was whether its file was on disk. So a
  machine where the file had been copied but launchd had never loaded it — a password prompt
  cancelled halfway through the first install, a background item switched off later — reported
  itself perfectly set up while every `.adi` name resolved and then went nowhere. It does not even
  look like a failure: the daemon is also what puts its address onto `lo0`, and macOS drops packets
  to an address no interface owns, so the browser never gets an error page. It just loads forever,
  in every browser, and reopening the app fixed nothing because the file was still exactly where it
  was supposed to be. Opening ADI now asks the address rather than the file, and offers to put the
  daemon back — at most once every few minutes, so a prompt you dismiss does not follow you around,
  and never for a front door you repointed at your own build. The services list grows a **Repair the
  front door** button for as long as it is silent, `adi-mono dns grant-network` is the same repair
  from a terminal, and a diagnostic report now prints `front door answering  NO` and says which
  command fixes it, instead of showing three green gates above a machine that does not work.

## 1.3.0 — 2026-09-03

### Changed

- **The whole product is drawn to one design system now**, written down in `design/DESIGN.md`
  and valued in one file, `design/tokens.css`. The control panel is dark — there is no theme
  toggle any more — and quieter: sidebars and bars recede, the transcript sits on the lightest
  surface at 15.5px, tables lost the cards around them, labels lost their capitals, and one
  orange per screen marks the one action or live state that matters. Type is Geist, with Geist
  Mono only for what a machine wrote or will read — paths, ids, commands, model names. Every
  icon is Lucide, at one stroke. The mark is flat: three hexagons in the ink around them, no
  gloss, no motion. The same rules reach the front door's error pages, the pages the mesh
  gateway serves, the shell every dashboard is generated into, the mesh client, the disk image
  and the landing at withadi.dev. Nothing about what the app does changed; a dashboard you
  already have picks up the new shell the next time it is listed.

### Added

- **You can now join another machine's fleet from the control panel.** Settings → Fleet has a
  *Join a fleet* panel: paste the invite the other machine minted, press Join, and this machine
  dials out and pairs. It then shows the password that pairing minted — once, because neither
  machine stores it — and the link to the other machine's panel at `app.<name>.n.adi`. Before this
  the page could only mint invites, so the machine doing the *dialling* needed somebody at a
  terminal to run `adi-mono mesh join`; that is precisely the machine most likely to have nobody
  who wants one.

### Fixed

- `adi-mono mesh join` printed the wrong address to open after pairing — the name the *other*
  machine files you under, which resolves nowhere on yours. It now prints the name you file it
  under, which is the one that works.

## 1.2.1 — 2026-09-02

### Fixed

- **ADI starts on a Windows machine that has never had a compiler on it.** The installer put
  everything in place and then Windows refused to run any of it: *"the code execution cannot
  proceed because libstdc++-6.dll was not found"*, over an offer to reinstall that could not
  help. The released binaries were linked against a GCC runtime library that is not part of
  Windows and that nothing installs; they now carry it inside them. Install this version over the
  broken one — an ADI that cannot start cannot update itself.

## 1.2.0 — 2026-09-02

### Added

- **Windows installs like an app now.** Download `ADI-Setup-x64.exe`, click through it, and what
  you get is one entry in the Start menu called ADI. Opening it starts the platform, opens the
  control panel, and leaves an icon by the clock you can start, stop and reach ADI from. Before
  this, the download was a zip of four `.exe` files and four `.cmd` files in one folder, and the
  first thing it asked a new person was which of them to run — a question with no good answer,
  since all four are the platform and none of them is the app. The four are still there, because
  the platform is genuinely four supervised services; they are in a `bin\` folder now, exactly as
  they have always been hidden inside `ADI.app` on a Mac. The install goes into your own user
  account and needs no administrator, `adi` lands on your PATH, ADI appears in Installed apps, and
  uninstalling gives back the `.adi` domain and leaves `%USERPROFILE%\.adi` — your projects,
  secrets and database — untouched. The one administrator prompt, for the `.adi` domain, is now
  asked during the install where a prompt is expected, instead of arriving unexplained the first
  time you opened something.

- **The Mac app updates itself, from the app.** The window now shows which version you are on and
  a button that fetches the next one. This was only ever on the control panel before, and the
  control panel is `app.adi` — a page reached through a name ADI itself has to resolve. So the one
  fault that most needs a new version, a `.adi` route that has stopped working, was also the fault
  that hid the way to get one: nothing loaded, and the only remaining advice was to download the
  disk image again by hand. The button runs the copy of the CLI inside the app bundle and talks to
  GitHub over your Mac's own DNS, so it works when nothing of ADI's does. It says what it is doing
  — including that ADI closes and reopens itself to finish — and a version that turns out not to
  work is still rolled back, exactly as a background update is.
- **"Something not working?" makes one file to send.** A second button at the foot of the window
  collects everything that could explain a fault — the versions, the two permissions, every
  service and what its supervisor last did with it, the `.adi` route, what is listening, whether
  the panel and the front door actually answer, the tail of every log, any crash reports — into a
  single archive, and shows it in Finder ready to attach to a message. It reads only; nothing is
  started or stopped, so it is safe to press while something is broken. It also tells you in the
  window what it already thinks is wrong, which for a stopped service or a missing route is the
  whole answer and needs sending to nobody. Secrets, your database and agent transcripts are never
  opened, and any credential-looking value in the config it does copy is blanked out first. The
  same thing is `adi-mono diagnose` in a terminal, for a Mac where the app will not open at all.
- **…and a second button next to it opens the issue for you.** *Open an Issue* goes straight to
  GitHub with the report already written up: which build this is, which macOS, whether each of the
  three setup steps is done, what every service is doing, and anything the report flagged — with a
  blank space at the top for what actually happened. Those are the details that decide whether a
  bug can be looked at, and every one of them used to cost a message asking for it. Drag the
  archive into the box and it is a complete report.

## 1.1.0 — 2026-09-01

### Added

- **A Mac installs `bun` for itself.** A dashboard is a pair of bun servers, and macOS only ever
  *assumed* bun was there: a Mac that had never installed it by hand still scaffolded a dashboard,
  still listed it, still gave it a hostname — and then served a dead host, with nothing on screen
  saying why. Starting the stack now fetches the pinned build into `~/.bun/bin`, checked against
  its published SHA-256 before it is ever made executable, exactly as the Linux node installer has
  done since 1.0.0. A bun you already have is reported and left exactly as it is — we do not
  upgrade one out from under your other projects — and a Mac with no route to GitHub still comes
  up, because only dashboards need it. `adi-mono bun` asks for the step on its own and says in one
  line what happened.
- **A phone pairs by being shown a code.** Scanning is the primary pairing action on iPhone and
  iPad now: an invite is over nine hundred characters, so a QR code is how it gets onto a phone,
  and the text field is the fallback for a camera that is refused, absent, or pointed at nothing.
  A machine paired this way is filed under its own name instead of a key-derived
  `viewer-25f6795fa6`.

### Fixed

- **Opening a dashboard your phone holds no grant for no longer answers "The node refused this
  service."** Tapping one asks the machine to share it and then opened the page immediately — but
  a node's gateway serves from a snapshot it re-reads every few seconds, so the request was judged
  against a registry that had never heard of the grant. The phone waits that window out now. It
  read as a flake rather than a bug, because iPhone usually won the race and iPad usually lost it.

## 1.0.1 — 2026-08-27

### Added

- **The chat rail can be narrowed to the conversations you started.** A fleet starts most of its
  own work — an agent launches a helper, a trigger fires one on an event, a script runs one on a
  schedule — so a rail of four hundred conversations mixed the handful a person actually had in
  with everything the machine had spawned for itself, and nothing recorded the difference. Every
  run now writes down who asked for it at the one moment that is known, the launch: a person,
  another agent by name, or automation with nobody watching. The Sessions head gains a filter box
  offering **All sessions**, **Only starred** and **Only started by me** — the first two being the
  starred-only toggle that was there before, now one option among three. Two cases worth knowing:
  a conversation opened before this release is attributed to nobody and deliberately does *not*
  count as yours, because a filter that read every unattributed session as a person's would show
  a year of agent-spawned runs under your name; and whatever is on screen stays visible whichever
  filter is on, since a filter must never hide the conversation you are reading.

- **One run can be launched on a different model, or with its permissions loosened, without
  editing the agent.** An agent definition is a template and editing it is usually the right way
  to change what a run does — the exception is the launch that is deliberately unlike the others:
  try this task on the big model, run this one under `bypassPermissions` because it is a scratch
  checkout. Doing that by editing the agent means remembering to edit it back, and forgetting is
  how an agent ends up permanently on settings somebody chose for one afternoon. The composer
  gains a run-settings panel, and `adi-mono agents run` takes `--set model=opus`,
  `--set permission_mode=bypassPermissions`, `--set unattended=true`, repeatable. `--set <key>=`
  with nothing after the `=` *unsets* what the agent pins for this run, back to the engine's own
  default, which is the only way to say "the agent fixes this and this run should not". The
  override travels with the launch and is re-applied on every later turn of that conversation, so
  a chat cannot answer its second message as a different agent than its first. Settings only: a
  run cannot grant itself a tool, a secret or a knowledge base its agent was not given, because
  those are the agent's identity rather than its dials. The panel remembers what you set per
  agent in this browser, beside the working directory it already kept.

## 1.0.0 — 2026-08-27

The first release under a stable version number. Two things are in it: a browser tab can now be
paired with a machine and render its control panel with nothing listening in between, and five
security fixes — one of which closed a path from any web page the operator happened to visit to
code running as root on their machine.

**One thing to do after updating.** This release replaces the local certificate authority (see
below), so `https://app.adi` will warn until you trust the new one. On macOS:
`sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain
~/.adi/mono/hive/tls/ca.pem`. The old root is inert from here and worth removing from your trust
store. Plain `http://` is unaffected, and the front door logs the instruction on every start
until it is done.

### Security

- **A web page you visit can no longer drive this machine's control panel.** The panel has no
  login: it listens on loopback and treats everything that reaches it as the operator. But `.adi`
  resolves machine-wide, so `http://app.adi/api/*` was an address any page open in the operator's
  browser could post to — and because the panel reads JSON out of a body whatever its type claims,
  a `text/plain` POST was a CORS *simple* request: no preflight, nothing to consent to, the side
  effect just happens. The end of that chain was root, `POST /api/fs/write` being jailed to a
  directory that holds the `hive.yaml` the root front door re-reads every three seconds and runs.
  Every `/api/*` request is now refused unless its `Origin` is absent or names the host it was
  sent to. This is not authentication and does not pretend to be — it stops a web page driving the
  panel; a process already on this machine can still send whatever it likes.

- **The root front door launches nothing at all.** The rule that strips service runners from a
  hive running as root was applied only to *imported* hives, leaving the top-level file's own
  `run:` exactly where it was — and the file that got the exemption is the one that needed the
  rule, since the root daemon is pointed at a store file that is user-owned by design. A `run:`
  written there was executing as root within three seconds, no restart needed. The decision moved
  to the accessor the supervisor actually calls, which as root now returns nothing and names the
  services it dropped.

- **A root daemon will not run a program an ordinary user can rewrite.** Installing the front door
  resolved its program as a sibling of whichever binary was doing the installing, so `adi enable`
  from a repo build was enough to put a `target/release` path into a root plist — after which a
  plain `cargo build --release` became root within about a minute, with no prompt. Installation
  now refuses if that program, or any directory above it, is owned by a non-root user or is
  group- or other-writable, and names the component at fault before anything is written.

- **The local certificate authority may only vouch for this machine's own names.** The CA you are
  asked to install as a system trust root was bounded only in path length — nothing constrained
  *names*, so it could sign a certificate for `google.com`, for a bank, or for your own SSO, and
  every browser on the machine would have accepted it. It is now constrained to the `adi` DNS
  subtree, `localhost` and `127.0.0.0/8`, which is exactly what the front door serves. Because the
  copy in your trust store is not the copy on disk, an existing CA is replaced outright rather
  than quietly rebuilt — hence the re-trust above.

- **The store is written owner-only.** Every file it held landed at whatever the umask said, which
  is `0644` on a stock macOS account. That included this node's long-term mesh identity key, and
  the invite nonce and ticket that are together a *complete* invitation to join. macOS puts every
  local account in `staff`, so a second account on the machine could read all of it, and every
  backup and sync carried the same bytes. New files are opened `0600` and directories `0700`;
  existing ones are repaired as they are read, keeping the owner's own bits so a tool script stays
  executable.

### Added

- **A browser tab is its own peer, and it pairs by dialling.** `mono-mesh-client.withadi.dev` is a
  page that holds its own key, dials the machines it has been paired with, and renders each one's
  control panel — no server, no open port, nothing listening behind it. Everything on the screen
  came over QUIC from a machine that is not reachable from the internet. Long-lived streams work
  both ways, so the panel's live channel, its event streams and its websockets all run through it.
  Pair by running `adi-mono mesh invite` **on the machine you want to reach** and giving the token
  to the page.

- **Pairing a phone is pointing it at a code.** An invite is around a thousand characters, and
  getting that onto a phone was the whole friction. `adi-mono mesh invite` now draws it as a QR at
  a terminal; the Fleet page has a **Show pairing QR** button that mints one and counts down its
  ten minutes; and the browser client has a **Scan** button that reads one. The code carries the
  token rather than a URL, so a phone's own camera app will only offer to copy it — the page says
  so, because that failure is silent and looks like a broken code. Redirected output is unchanged,
  so scripts that read this command keep working.

- **A phone sees what each machine runs, not just its panel.** Under every paired machine are the
  dashboards that machine is running, read from the node itself. A row your pairing did not grant
  says **Allow**, and the first tap asks the node for it.

- **⌘K answers on the page it just took you to.** The palette was mounted only inside one shell,
  so using it, jumping somewhere, and pressing it again did nothing — it stopped working exactly
  where you had started trusting it. Both shells now take their rows from one list, so they cannot
  come to disagree about what the app can do, and **Pair new device** is one of those rows: it
  lands on Fleet and raises the QR.

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

- **An agent is edited on its own page**, at `/agents/new` and `/agents/<name>/edit`, rather than
  in a form that opened under the list. The page paints filled, survives a refresh, a deep link
  and Back, and a rename leaves the URL pointing at what is in the form. Settings in a chat's
  Agent panel opens that agent's editor rather than the list you were already past.

### Changed

- **The `tcp:` and `ctl:` grant families are withdrawn from the fleet page and the CLI.** Both were
  offered as examples and neither was ever enforced: what actually gates the raw-forward path is a
  different list entirely, and nothing anywhere consumed `ctl:`. This failed *closed*, so it was
  never exploitable — but an operator who added `tcp:127.0.0.1:22` believed they had opened
  something and had not, and one who removed it believed they had closed something and had not.
  Both beliefs are worth correcting. They still parse and load, so existing `fleet.toml` files are
  unaffected; `ctl:` is explicitly reserved.

### Fixed

- **A Linux node stops taking its whole session down with one runner.** `kill` on procps-ng keeps
  only the *first digit* of a bare negative pid, so stopping one hive runner was a `SIGTERM`
  broadcast to every process the caller owned — control panel, dashboards, DNS and the session
  manager itself. Linux pids reach seven figures and a node's all begin with `1`, so this fired
  every time. It read for weeks as "the node keeps losing its linger and dying"; losing the linger
  was the aftermath, not the cause. macOS parses the same argument correctly, which is why it only
  ever surfaced on Linux.

- **A fronted app keeps its own `Authorization` header.** The stored mesh password rode
  `Authorization`, attached whenever the client had sent none — so a page sending its own bearer
  token to its own API suppressed the password, drew a challenge from the node, and popped the
  browser's native password prompt on an ordinary `fetch`. The document itself carried no such
  header, so the page always loaded and only its AJAX calls prompted, which looked like a site
  asking for a password at random. The password now rides `X-Adi-Authorization` and is stripped at
  the gate, so it never reaches the service and the app's own header arrives untouched. A node
  from before the split still works.

- **A release will not ship a panel with no layout.** The dev server writes the same directory the
  release build embeds, so its partial output could land between the two and ship an `index.html`
  from one build beside the assets of another. Nothing failed — the missing stylesheet hit the
  single-page fallback and was served as HTML with a `200` — and the panel came up with its markup
  intact and no styling at all. Every asset the page asks for is now checked to be present, after
  the UI build and again after the binary has embedded it.

### Performance

- **The store no longer flushes every write to disk.** Making the store owner-only had replaced a
  plain write with one that also fsynced, which was a durability change smuggled in under a
  permissions one and paid on every task, manifest and run record the app saved.

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
