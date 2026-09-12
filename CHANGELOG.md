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

- **An operator can now see and change which embedding backend the code index, knowledge base and
  fact base each use, instead of only ever reading whatever an environment variable happened to
  say at startup.** New `adi-mono embeddings` command group (`backends`, `show`, `save`, `delete`,
  `settings`, `seed`), a matching `/api/embeddings/backends`/`/api/embeddings/settings` API, and a
  panel page at *Settings → Embedding backends* — modelled on the LLM backend registry, and
  deliberately smaller: no holds, no prober, nothing to migrate. Each backend states its runtime
  (`candle`, `ollama`, an OpenAI-compatible endpoint, or the deterministic `hash` stub), the model
  and width it produces, and any same-model fallback; a `candle` backend a build lacks the feature
  for is shown unavailable rather than silently broken. An API key is always named by the
  environment variable it is read from — this surface never displays or accepts a raw key.

### Fixed

- **Knowledge search could rank a note as current when its vectors were made by a different
  embedding model than the one doing the searching.** Two of the models in this tree happen to
  produce vectors of the same width, which is exactly the case a length check cannot catch — a
  search now checks the model by name, not just the width, so a stale vector is treated as
  absent, as the crate's own documentation always said it would be.

- **Swapping the embedding model the code index uses left every untouched file's old vectors
  mixed permanently into search results.** Re-indexing only re-embeds a file whose *content*
  changed, so a model swap with nothing else edited silently combined two different vector
  spaces in the same search. A model change is now detected and forces the same full re-index a
  format upgrade already does, so search never mixes vectors from two different models again.

- **The knowledge base, fact base, and code indexer now share one place to configure which
  embedding model each uses**, instead of each reading its own hardcoded default or environment
  variable. Existing installs keep exactly the model they already had — this only changes where
  the choice comes from, not what it resolves to, until an operator edits it.

- **The live graph hung work on a conversation that had not started it.** One card could wear
  dozens of children it never had — on a real machine, forty-five of them — and the fan of edges
  leaving it was most of what the page drew.

  An edge left the launching agent's conversation nearest in time, and where none of them had begun
  yet it reached *forwards* and took whichever began soonest afterwards: a cause that started after
  its effect. That is the ordinary case rather than a corner, because the graph keeps only an
  agent's newest six conversations, and an agent that spawns work all day has drawn conversations
  newer than nearly everything it launched.

  The rule only looks back now, and it asks the whole listing rather than the handful of cards
  already on the canvas: the conversation that was open when the work began is drawn even if it is
  far too old to have been picked, which is usually the one you were looking for. The page had the
  answer and was not showing it — on this machine the work hung off a bare agent pill while
  `Work BUGBOUNTY-809`, the conversation that really started it, sat undrawn in the same listing.

  Where the agent genuinely had nothing open — because the conversation is no longer in the store at
  all — the card still hangs off a pill bearing the agent's name, which is all the record supports:
  that agent set the work off, and which of its conversations did is written down nowhere.

- **An agent stood on the canvas as though it had started itself.** That pill was drawn as a root,
  level with *You*, which is the one thing on the picture that cannot be true: an agent cannot have
  launched anything unless something launched it first.

  It now answers to whatever that agent's own conversations answer to — usually you — so the chain
  reads *You → adi-agent → the work* instead of beginning in mid-air. Conversations with no launcher
  recorded still answer to *Unrecorded* rather than to you: most of a machine's history predates the
  record, and putting it under a person's name would be a different lie.

- **A conversation stopped and waiting on you could vanish from the chat rail with no way back —
  stuck for good.** A run a subagent launched for itself dropped out under the default "Only started
  by me" filter, and one belonging to an agent you had not starred dropped out under ★, even while it
  sat there asking a question only a person could answer. Hiding a chat could strand one the same
  way.

  All three now step aside for a run that is waiting on you: it stays in the rail's "Waiting on you"
  band regardless of the filter, the ★, or whether it was hidden, and shows there once rather than
  also in the Hidden list — the question is what needs seeing, not who started the conversation or
  where it was filed.

## 1.13.3 — 2026-09-12

### Added

- **A live graph of what set what off.** New page in the panel, and a door to it in the chat home's
  right rail under Marketplace: one canvas holding you, the conversations you opened, the ones
  those set off, and on down the chain. Left to right is causation, not time — a card further right
  was set off by the one before it.

  One card per conversation, with the agent it belongs to written under its title, so an agent is
  never a step of its own. Scroll to zoom, drag to pan, click a card to open the conversation.
  Pointing at one puts the rest in the bar: whose it is, where that agent is filed, who asked for
  it, and where it got to. A running conversation wears the orange dot, one waiting on an answer an
  amber one, one that ended badly a red one.

  A machine with a thousand conversations is still a graph you can read: at most six of each
  agent's are drawn and at most eighty in all, newest first, and the bar says how many there are
  altogether. Whoever started one of those is drawn as well, however long ago their own conversation
  was, so nothing on the canvas hangs from nothing.

  It costs no new endpoint: the picture is the two listings the panel already carries, read as a
  graph instead of as a table. One thing the store cannot tell it, and it does not pretend
  otherwise — a run records which *agent* asked for it, never which of that agent's conversations
  did. So an edge says that *that agent* set this off, and it leaves whichever of its conversations
  this one started nearest to.

## 1.13.2 — 2026-09-12

### Added

- **The tab strip answers the two gestures a browser has already taught.** In the macOS app's panel
  window, a wheel-click on a tab closes it, and a right-click opens the menu of what there is no room
  for on a chip: **Rename…**, **Pin Tab**, and, behind a divider, **Close Tab**. The app's own tab
  has no menu — it cannot be closed, the brand is not renamed, and it is already where pinning would
  put a tab.

  **Pinning** moves a tab up behind the app's and keeps it there, so its ⌘-number stops moving as
  dashboards open and close beside it. A pinned tab wears a pin where its ⨯ was and declines the two
  gestures that close a tab under the pointer; ⌘W and its own menu still close it, because both say
  what they are about to do.

  **Renaming** happens in the chip, which becomes a box holding the name it is wearing: ↩ keeps it,
  ⎋ leaves the tab alone, clicking away keeps what was typed, and an emptied box puts the page's own
  title back. ⇧⌘T now brings a closed tab back with its name and its pin as well as its address.

### Fixed

- **The ⨯ on a tab could not be clicked.** It was hittable only *on* its two thin diagonals — about
  0.9pt of stroke — and every other pixel of the glyph fell through to the tab underneath. On a tab
  that was not chosen, that selected it, which looks like a ⨯ doing the wrong thing; on the tab
  already in front it did nothing at all, which is how it was reported: "I can't close the current
  tab". The close button now has a target of its own, 22pt across the whole glyph and its slop, and
  the glyph sits on exactly the pixels it did before — the strip is unchanged to look at, and the
  target still stops short of where a long title truncates. ⌘W and a chip's own **Close Tab** were
  never affected.

## 1.13.1 — 2026-09-11

### Fixed

- **Settings → Shared assets said "Loading…" and never stopped.** The setting the last release added
  was unreachable from the panel it shipped in: the live channel keeps a list of the reads it is
  willing to watch, `/api/settings/shared-assets` was never added to it, and so the page's one
  request was refused every time. Opening the URL directly happened to work — the panel asks for
  everything once at startup — but clicking through to the page from anywhere else left it on
  "Loading…" for as long as the tab stayed open. The apps marketplace was quietly missing from that
  same list, which is why an install made in another tab didn't appear in one already open.

  A page that can't load its read now says what went wrong instead of loading for ever, and the list
  is checked against the panel's own source at test time, so the next page to be left off it fails a
  build rather than reaching somebody's screen.

## 1.13.0 — 2026-09-11

### Added

- **The control panel can fetch itself from a CDN near your browser, instead of over the link to the
  machine running ADI.** The panel is a large wasm bundle — plus its JS glue, two stylesheets and
  fonts — and every instance has always served that off its own disk. Where the browser is somewhere
  else, that is the worst possible place to serve it from: a paired fleet node over `n.adi`, a phone,
  a laptop reaching a machine on a bad connection, all of it crossing the slow link on every cold
  load. **Settings → Shared assets** now offers three answers: *This machine, always* — the default,
  and exactly what every release before this did; *The CDN, only when the browser isn't on this
  machine*, which leaves loopback and this instance's own `.adi` name alone and helps only the case
  that needs helping; and *The CDN, always*.

  The URL carries the version and **nothing** about the instance — no hostname, no node id, no
  per-install token — so every instance on the same version asks for the identical URL, and a browser
  that loaded the bundle for one of them has it cached for the rest. Only the very first load,
  anywhere, costs anything.

  **Choosing a CDN mode cannot leave you looking at a page that will not load.** Each import and each
  stylesheet carries a fallback to the same local path it would have used anyway, and it fires on any
  failure at all: offline, bucket down, or a version nobody ever published. At worst it costs one
  failed request per asset. `index.html` itself never moves — the instance you are talking to always
  serves that — and the stylesheets keep their integrity hashes, so a byte out of place is refused.

  This is also the first release that **publishes** to `cdn.withadi.dev`: the build that ships a
  version now uploads that version's assets before it builds a single binary. Until now the setting
  existed with nothing on the other end of it, which is the quiet way it could have been on and doing
  nothing. If you have never touched this setting, nothing about this release changes for you — and
  if it matters to you that a panel load never leaves the machine, that is what the default already
  is, and nothing else in ADI depends on a CDN mode.

- **There is a documentation site: [docs.withadi.dev](https://docs.withadi.dev).** It opens on what
  ADI actually is, in plain language, with a section for someone evaluating it on security and
  control and another for someone who wants the architecture. The first area written in depth is
  **Hive** — the operator's guide to services and the front door, and how they work underneath —
  joined by **Shared assets** for the setting above. It deploys itself on every change, so it tracks
  the code rather than the last time somebody remembered to publish it.

### Changed

- **A long chat opens at once, and stops re-downloading itself every second.** The control panel used
  to fetch every turn of a conversation — every tool call and every tool result — and then fetch all
  of it again a second later, for as long as the chat was open. On the longest conversation on this
  machine that was **3.4 MB a second**, of which nine tenths was tool input and output sitting behind
  folds nobody had opened. It now asks for the newest twenty turns with the runs of tool calls
  collapsed to the same receipt line the transcript already drew — **43 KB**, and the calls behind a
  run are fetched when you open it. Nothing looks different: the receipt says what it always said,
  and opening it shows the calls it always showed.

  The rest follows from that. **Earlier messages** at the foot of the transcript brings the previous
  twenty — at the foot because the feed reads newest-first, so history arrives below you and nothing
  on screen moves. The numbers in the right-hand rail still count the *whole* conversation rather
  than the page you are looking at, and the failures it lists are still links: clicking one widens
  the transcript, opens the run holding it and scrolls to the call, however far back it was. Sending
  a message no longer downloads the conversation back.

  Two fixes came out of the measuring. A `Write` call's preview — the one line a closed run shows —
  was the entire file it had written, up to 12 KB to fill a line a couple of hundred pixels wide; it
  is cut now. And every read of a chat was loading the whole transcript to look at its *last* turn
  (deciding whether an answer needed committing), ~180ms per read on a long one; it reads one row.

### Fixed

- **A Codex agent answered with its entire log, including the account it was signed in as.** An agent
  on a `process:codex` backend had no reader of its own, so what the chat showed as its answer was
  the whole file the run had written: a hundred-odd lines of startup logging, a banner, a replay of
  the prompt it had just been handed, and a token trailer. Two of those startup lines carry the
  signed-in account's id and **email address** — so a chat anyone could open was publishing them.
  Codex now gets read the way Claude already was, off its own event stream, and its answer is the
  answer. Belt and braces on the other half: the child is started quietly unless the agent asked for
  logging itself, at `error` rather than silence, so a Codex that fails to start can still say why.

  Two things the stream had been swallowing come back with it. A turn that **fails** now passes the
  provider's own error through, keeping the code and status alongside it — which is what lets a rate
  limit still be recognised as one, so an agent with more than one backend fails over to the next
  instead of stopping. And a turn that **completes** now says so: an unreported ending was being read
  as the kind worth classifying, which meant a clean run could be mistaken for a limit and take a
  perfectly good backend out of the chain.

## 1.12.0 — 2026-09-10

### Added

- **The marketplace has a page of its own, and an app can carry a mark and what it is about.**
  Browsing what somebody else published is not a settings act, so the marketplace has left the
  control panel's shell for a page at `/marketplace` — reached from a **Marketplace** band above the
  Apps rail on the chat screen, with the apps as the page rather than one panel among an explorer
  full of them. A manifest entry may now carry an `icon` and `keywords`: the icon is drawn beside
  the entry, and the keywords are tags under its description — and `adi-mono marketplace apps`
  prints them, so the terminal is not the door that knows less. An icon is an `https://` URL, or a
  `data:image/…` URI to carry the image in the manifest itself and have the listing fetch nothing at
  all; `http://` and bare paths are refused. An entry with no icon draws a package tile instead, and
  so does one whose icon will not load — a listing has no business looking damaged because somebody
  else's host is down.

- **An app has a page of its own now, with a gallery on it.** Clicking an app's name in the
  marketplace opens `/marketplace/<marketplace>/<slug>`: its pictures and clips, one at a time with
  thumbnails under them, then the long form its publisher wrote, then the repository and commit an
  install would actually clone. A manifest entry carries them as `readme` — Markdown, rendered
  through the same view layer the rest of the app uses, so markup in it never becomes markup on the
  page — and `gallery`, a list of `https://` (or inline `data:`) pictures and clips, each with an
  optional caption and, for a clip, a poster. Nothing plays by itself: a clip waits to be asked, and
  the page loads its bytes only then. An app that publishes neither still has a page; it is the
  listing row, at reading size, with what installs spelled out under it.

## 1.11.0 — 2026-09-10

### Added

- **The panel window has a browser's keyboard.** The app's window around the control panel answered
  no shortcuts at all: ⌘R did nothing, and ⌘W — the one keystroke a window full of tabs cannot do
  without — was unbound. It now carries the habits every browser has already taught, on the keys
  they are taught on: ⌘R to reload and ⇧⌘R to reload ignoring the cache, which is the one that
  matters after somebody rebuilds a dashboard; ⌘W to close the tab in front and ⇧⌘T to put back the
  last sixteen closed, each at the page it was left on rather than its front page; ⌘[ and ⌘] for
  back and forward; ⌃⇥ and ⌃⇧⇥ along the strip, wrapping; ⌘+, ⌘− and ⌘0 for the size of a page; and
  ⌘F to find in it, with ⌘G and ⇧⌘G stepping through the matches. ⌘1…⌘9 still pick a tab. They are
  real menu items and not bare keystrokes, so the menu bar says what each one does and System
  Settings can rebind any of them — and *File* names what ⌘W is about to do, **Close Tab** on a
  dashboard and **Close Window** on the app's own tab, which is never closed. The find bar says
  *found* or *No matches* and deliberately no count: WebKit publishes no match count, and a number
  counted separately in JavaScript would disagree with the highlighting WebKit itself draws.

### Changed

- **Services are lazy now: one does not run until its page is visited.** A service used to cost the
  same whether anybody was looking at it or not — the hive started everything at boot and kept it
  alive for ever. From this release the hive starts **nothing** that has a `proxy.host` until a
  request arrives for that host, and stops it again once an hour has passed with no request
  (`idle_stop: 30m` in its hive.yaml to say otherwise). While it comes up, the visitor gets a page
  that says the service is starting for them and turns into the service itself as soon as it
  answers — no reload, no 502 that looks like a fault. The stop is a `SIGTERM` first: the process
  gets 30 seconds (`stop_grace`) to finish what it is doing, and a request landing inside that
  window cancels the stop and keeps the service.

  **This changes what your existing hive.yaml means, and it is worth ten minutes before you
  restart.** Anything that has to be up whether or not a browser is pointed at it — a background
  worker, a webhook receiver, a queue consumer, an endpoint something else polls, anything a cron or
  another machine calls — must now say so:

  ```yaml
  services:
    webhooks:
      proxy: { host: hooks.adi }
      start: always          # ← without this it is not running when the call arrives
      runner: { script: { run: bun run hooks } }
  ```

  A service with **no `proxy.host`** is left alone: nothing routable means no request could ever
  wake it, so it keeps being started with the hive exactly as before, without needing a key. That is
  most databases, workers and sidecars, which is why the change is narrower than it sounds — it is
  the services with a `.adi` name, the ones you reach in a browser, that are now lazy.

  What you lose by not editing anything is a first visit that waits a few seconds behind a holding
  page. What you lose by not editing a webhook receiver is the webhook.

  **This works when routing and supervising are two separate processes, which on most machines they
  are.** A front door on `:80` routes what a per-user hive actually runs: the visit lands on the one
  that has no process to start, and the process belongs to the one that saw no visit. The two now
  pass it between them through a pair of small files in the store — the front door leaves the
  request, the supervisor picks it up a fraction of a second later and starts the service, and the
  traffic the front door goes on seeing is what keeps that service from being idle-stopped while
  somebody is still using it. There is nothing to configure, and a machine where one hive both
  routes and supervises never writes the files at all. **Both hives have to be running this
  version.** They share one binary, so an update covers both and each restarts itself once its
  binary is replaced — but until one of them has, a service a visit should have woken stays asleep
  and its host answers the `502` it always did.

  *Settings → Services* shows each service's policy and, instead of a running light, what it is
  actually doing: running, starting, idle-stopped, or stopped. An idle-stopped service is not
  reported as down, because it is not. Start and Stop still work by hand, and starting a service
  that way gives it the same full idle window a visit would. The service form in a project now
  offers **On demand** first, with **Always** beside it, and writes a `start:` key only when the
  choice is not what silence already means.

## 1.10.1 — 2026-09-09

### Added

- **A message can say it wants to be heard now.** Typed while `harness:adi` is still answering, a
  reply used to wait for that answer to finish before it was ever asked — the only way to say
  something new was to interrupt the whole turn with Stop. A new asap button beside Send, shown
  only while the agent is working, sends it to overtake the queue instead: it reaches the model at
  the very next tool-calling round, alongside whatever the turn was already doing, rather than
  after the answer lands. An asap message queued behind a `claude-sdk` or `process:*` run is heard
  when that run ends, the same as before — those engines have no door partway through a turn to
  reach it any sooner. A queued message that asked to overtake says so in the chat.

### Changed

- **The LLM backend form now asks what the runtime it is on actually takes.** Picking a runtime
  used to change nothing: a Codex CLI backend still asked for a provider, a base URL and an API key
  variable — four boxes that runtime never reads — while the knobs it *does* understand had to be
  typed as raw JSON from memory. Choose a runtime now and the rest of the form is that runtime's
  own questions: the login it can be pointed at (a vendor CLI says plainly that it signs in by
  itself, and that every backend on it therefore shares one hold), its suggested models as one-tap
  chips, and its dials as the controls the server declares for them — including the ones that
  depend on the provider, which appear when you pick one. Anything the runtime does not declare is
  still editable as JSON, and a login left over from another runtime is named on screen before the
  save drops it.

### Fixed

- **The LLM backends page loads.** It showed "Loading…" and never anything else: the page watches
  the backend registry over the live channel, and the server's list of reads that channel may watch
  did not include it, so the answer was never sent. Reloading the page directly onto its URL painted
  it once and then let it go stale, which is why it looked intermittent. The agent editor's model
  list came from the same read and was empty for the same reason.
- **A page that cannot load its data says so instead of loading for ever.** Every table read "not
  asked yet" and "asked, and it failed" as the same thing — an empty signal — and showed "Loading…"
  for both. A failed read now names its reason in the table it belongs to, and clears itself when
  the read starts working again. The live channel also answers a read it will not watch, rather than
  going silent on it.

## 1.10.0 — 2026-09-09

### Added

- **An agent can be given more than one way to answer a turn.** Until now an agent was welded to a
  single model: when that model hit its usage limit the run died, and you had to notice, pick a
  different agent and lose the thread. An **LLM backend** is now a thing in its own right — a
  credential, a model, its dials, how that provider says "you are out", and how much history it can
  be handed — kept under one name at *Settings → LLM backends*. An agent keeps its identity (its
  prompt, its tools, its directory, its memory) and holds an ordered **list** of backends instead of
  a model.

  A new conversation starts on the first row. When a backend answers with a quota or a rate limit it
  is held — centrally, keyed on the credential and model, so sixteen runs discover one limit once —
  and the same message is replayed on the next row, with the same prompt, tools and history, under
  one line of notice in the chat. A broken login, an error nobody recognises, or a list with nothing
  left is a stop that asks you, not a silent fallback. Nothing is ever summarised or trimmed to fit:
  a history the next backend cannot hold fails out loud. A held backend is found to be back by a
  prober in the background, never by making your next turn wait.

  `adi-mono llm backends|show|save|delete|settings|holds|release|probe` from the terminal, and
  `adi-mono llm migrate` lifts the model configuration each agent already had into a backend and
  puts it at the head of that agent's list, one for one, dry by default.

- **One endpoint every model client can be pointed at.** The LLM gateway takes the traffic from
  anything that speaks to a model — your own scripts, an agent's engine — and gives it one address,
  one place credentials live, and one record of what went out and what came back. The panel reads
  that record as a page: request by request, with the model, the tokens and the money beside it.

  It can also compress the same literal repeated dozens of times in a prompt into a short name and
  put the literal back before the client sees it. That last part ships **off and marked
  experimental**: measured on this machine it loses more to a broken prompt cache than it saves, and
  a gateway with it switched on says so at startup and warns on every request it rewrites.

- **Every agent definition now says which shape it is written in**, on its first line, and the app
  brings the store forward on the way up. `version = 3` in an agent's file is not decoration: at
  startup adi walks each definition from the shape it claims to the one this build reads — nothing →
  1 → 2 → 3, one step at a time — before the panel, the launcher or a trigger has read anything. A
  step runs once, an edit in the panel is not an upgrade, and a definition written by a *newer* adi
  than the one you are running is reported and left strictly alone rather than guessed at.
  `adi-mono agents migrate` is the same walk asked for by hand, and shows you the plan first.

### Changed

- **An agent that starts on an LLM backend no longer says what it runs on.** The backend already
  names the runtime, and the copy left on the agent was a second answer to the same question — free
  to drift from the one every launch actually used. It is gone from those files: the runtime field
  in the agent form is filled in from the backend at the head of the list and says so, and
  `adi-mono agents save --backend …` on such an agent is refused with the way to change it rather
  than quietly ignored. An agent with no backend list — one that drives a CLI, like `pty:claude` —
  still declares its own, because there is nothing to take one from.

  The migration does not guess: an agent whose stored runtime disagrees with its first backend, or
  whose first backend no longer exists, is held back with the reason and left as it was.

- **An older adi opening a newer store now says so.** It could never write to one — a definition
  stamped above what a build knows has always been left alone — but it came up silent about it and
  then served definitions in a shape it does not understand. It now warns once at startup, with how
  many and how new, and the agents a migration held back are named in that same log.

## 1.9.0 — 2026-09-08

### Added

- **The control panel opens inside ADI now, in a window of its own with tabs.** *Open control
  panel* used to hand `app.adi` to whichever browser the Mac opens `.adi` links with. It now comes
  up in the app: one icon in the Dock, one window that is where you left it, and no tab lost in a
  window of thirty. The panel is the **first tab, always** — it cannot be closed, and nothing can
  navigate it somewhere else, because a link that would opens a tab instead. So there is always
  something to come back to.

  Dashboards and hive services open beside it as tabs, named by the page rather than by their host.
  Clicking a dashboard you already have open brings that tab forward instead of opening a second
  copy of it. ⌘1–⌘9 select a tab, ⌘-click opens one, the ⨯ closes it and drops you back to the tab
  on its left. A link that leaves this machine — an issue tracker, someone's docs — still opens in
  your own browser, where your logins and extensions are.

  There is no address bar and no back, forward or reload button: everything this window can reach is
  a click away on the page it opens on, and the web view keeps all three of those on its right-click
  menu for the times it doesn't. The window's tab strip carries the mark and the wordmark as that
  first tab, so the page stops drawing its own while the app is around it — and ⌘K still opens the
  menu, from the keyboard, exactly as before.

  macOS only. Nothing about the panel itself changed: it is the same page on the same address, and
  opening `app.adi` in a browser works as it always did.

## 1.8.0 — 2026-09-07

### Added

- **Who is around, in the width of a few characters.** The strip of paired machines has moved out
  of the sessions rail and into the head of the right column, beside the Apps list, and it is a
  pile of faces now: one circle per name, active ones first, everyone past the fifth folded into
  a `+N` that names them on hover. An active machine wears one of six solid colours picked from
  its own name; an idle one is a flat grey circle — on a dark rail "brighter" read as switched
  *off*, so brightness no longer carries the meaning. The line beside the pile says it in words
  as well: "2 active now", the machine's name when there is exactly one, or "nobody active now".
  The pile follows the sources ticked in the rail's head, so unticking a machine takes its circle
  away with its rows, and a machine with nothing ticked shows everybody — an unset filter matches
  everything, the way an empty search box does. It used to show nothing at all, which left the
  whole strip invisible until you happened to open a menu you had no reason to open.
- **A message says who sent it.** On a machine more than one person can reach — a paired phone, a
  second laptop, a browser tab somebody left open on theirs — every message in a conversation now
  carries the machine and account it came from, and the reply box says which machine your words
  are about to leave for. On a machine paired with nobody nothing changes: there is one voice,
  and naming it on every message would be noise.

### Changed

- **The platform's own messages stopped pretending to be yours.** An await firing, a question you
  were asked settling, a quiet conversation being nudged about its open goals — each of those
  arrives as a message in the transcript, and each used to wear your bubble under a label reading
  "You", which named the wrong speaker every time. They have a shape of their own now: a rule
  down the left, a line saying what happened — "Woken by `adi.ci.finished` · check passed",
  "Answered by default", "Goal check · 2 open" — the id you would use to look the thing up set
  off at the right, and the message itself under it. The agent reading the conversation is told
  the same facts in the same place, so what you see and what it acts on cannot drift apart.

### Fixed

- **A chat opened from a paired machine is named by what was asked, not by who asked it.** Since
  1.6.0 the sender's name was written into the message text itself, so it led every title the
  rail, the run history tables and Analytics showed for that conversation, and the local model
  that names new chats read it too. The name now travels beside the message rather than inside
  it, and those listings show the words that were typed.

## 1.7.0 — 2026-09-06

### Added

- **Chats can be renamed.** Right-click a conversation in the rail for a Rename… item alongside
  Star and Hide — the same prompt-style rename the Fleet page's nodes already use. A blank answer
  clears the name back to the title the opening message would give it, and the new name is what
  every other listing (the rail, the run history tables, Analytics) shows for that conversation
  from then on.
- **New chats can name themselves.** Once a conversation's opening message is sent, a local model
  (an [ollama](https://ollama.com) `llama3.2:1b` on this machine — nothing leaves it) guesses a
  short title and renames the chat in the background, without holding up the reply. A manual
  rename always wins if it lands first. Turn it off from the Agents page, next to the run limit —
  the toggle is "Auto-name new chats", on by default.

## 1.6.0 — 2026-09-05

### Added

- **The chat home shows which paired machines are online.** A strip above the sessions rail
  lists every paired node, active ones first, each as a name beside the same status dot the
  Fleet page uses — reading the one presence record both draw from, so the two screens can never
  disagree about who's up. Under it: any request a node sends across the mesh now updates a
  shared last-seen record for that node, and a node reads as active for the minute after. The
  strip is absent on a machine paired with nobody.
- **A paired node's agent can be given instructions of its own, and a message it sends now says
  so.** Fleet settings gains an editor: write instructions for one specific paired node, and the
  next conversation *that node* opens here splices them in behind the agent's own system prompt —
  frozen the moment the conversation starts, so editing them afterwards never reaches back into a
  run already underway. Every message arriving from a paired node's own request is now tagged
  with that node's name in the transcript, so a conversation another machine started reads as
  such rather than looking like you had it yourself.

## 1.5.1 — 2026-09-05

### Added

- **The sessions rail can show several sources at once.** Fleet's node menu used to be a pointer
  — pick a paired machine and its sessions replace whatever was showing; pick another and it
  replaces that. It's a checklist now: tick any subset of paired, unlocked nodes, plus this
  machine, and their sessions merge into one rail instead of taking turns. Every row carries its
  own source on the meta line the moment more than one is selected, so which machine a session is
  actually on stays visible at the point you'd act on it — open it, reply, stop it, star it,
  delete it. The selection is remembered across reloads in this browser now, which it never was
  before: safe to do only because that per-row source label exists, so you're never acting on a
  machine you've forgotten you're pointed at.

## 1.5.0 — 2026-09-04

### Added

- **The paperclip takes any file, not just pictures.** Attach a PDF, a CSV, an export somebody sent
  you — it uploads the moment you pick it, exactly as a screenshot does, and the agent is told
  **where it is on this machine** so it can open the file with its own tools. That is the point of
  it: a run happens where the app runs, and until now there was no way to get a document from the
  browser you are typing in onto the machine that would read it. An image still goes to the model as
  a picture; anything else reaches it as a path, and now an image carries its path too — a
  screenshot you want *cropped* is a file, not only pixels. Drop it, paste it, or press the
  paperclip; up to 25 MB a file (5 MB an image, which is a provider's limit, not ours), six per
  message. The tray shows a file by name where a picture shows a thumbnail, and a sent one is a link
  in the transcript rather than a broken image.

### Changed

- **A marketplace app is a git repository now, and you name your copy of it.** Installing one used
  to fetch a packed bundle and land it under whatever the publisher called it — so the app arrived
  as a dashboard you did not name, could not tell apart from a second copy, and had no way to
  update short of reinstalling over it. It is now a `git clone`, checked out at the **exact commit
  the manifest pins**, into a dashboard you name at install time (`Sales CRM` → `sales-crm`,
  `sales-crm.adi`, renameable afterwards like any other). Installing the same app twice is
  ordinary rather than a collision. What lands stays a clone: `.git` intact, the pin on a branch
  tracking `origin`, the working tree clean — so you can read the app, edit it, commit your own
  work on top, and `git pull` it. **Update** moves a copy onto the commit its marketplace pins
  now, as a fast-forward, and refuses rather than walk over changes you made; forcing it is a
  separate ask that says what it costs. Two properties come with the pin: what you read on the
  listing is what installs, whatever the publisher pushes afterwards, and `git log` in the
  directory is the provenance of every byte in it. A repository that ships a hive file of its own
  has it dropped on arrival rather than getting a say in what this machine runs. Installing from
  the panel now starts the app too — pressing Install *is* the deliberate act; untick "Start it
  right away" for the old behaviour, which is still what the CLI does unless you pass `--start`.
- **An app you installed but have not started is on the Dashboards page, not hidden in the
  archive.** It used to arrive marked archived — which is how it stays out of the supervisor's
  reach — and that put it behind the Archived disclosure, where nobody looks for something they
  just installed; the install read as an install that had not happened. Such a row now sits in the
  main list saying **not started**, with its services reading the same instead of "not allocated",
  and both its name and a Start button on the row run it. Nothing changes for a dashboard somebody
  archived on purpose: the two are told apart by whether the app has ever been started, which is
  recorded the first time it is. Publishers: an entry now carries `repo` and a full 40-character
  `commit` instead of `artifact`, and a manifest still written the old way says so on sync
  (`docs/marketplace.md`).
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
