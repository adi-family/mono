# adi-family — project instructions

please prefer working on the main unless asked to checkout. we value speed over stability now.

## Deploying `app.adi`: restart, don't just warn

When a change needs a service restart to take effect, **land the deploy yourself** —
don't stop at warning that the running copy is stale.

**How `app.adi` is wired (know this before touching it):**

- The control panel is `adi-app`, run by an **unprivileged per-user `LaunchAgent`**
  — label `family.adi.app.control-panel`, plist at
  `~/Library/LaunchAgents/family.adi.app.control-panel.plist` (user-owned, editable,
  `KeepAlive` + `RunAtLoad`). It runs `adi-app 8000` **as the user** on
  `127.0.0.1:8000`. Restarting it needs **no sudo**.
- A separate **root** front door (`adi-hive`, from `hive-frontdoor.yaml`) binds `:80`
  and *proxies* `app.adi` → `127.0.0.1:8000`. Never confuse the two; never restart
  the front door for an app deploy.

**Build:** `scripts/build-app.sh` → `trunk` builds the Leptos UI, then
`cargo build --release -p adi-app` embeds `dist/` → `target/release/adi-app`.

**⚠️ You (probably) cannot write into `/Applications/ADI.app`.** It's a signed,
notarized bundle, so macOS **App Management** protection blocks modifying
`…/Contents/Resources/adi-app` — you get `Operation not permitted` **even under
`sudo`** (it's a TCC check on the *terminal*, which root doesn't override). A bundle
swap only works if the user first grants their terminal *App Management* (System
Settings → Privacy & Security → App Management), then re-runs the `! sudo cp … && sudo
mv …` swap. Offer that, but don't assume it.

**Working local deploy (no sudo, no bundle write) — repoint the LaunchAgent at the
fresh binary:**

1. Back up the plist:
   `cp ~/Library/LaunchAgents/family.adi.app.control-panel.plist <scratch>/cp.plist.bak`
2. Edit `ProgramArguments[0]` in that plist from the bundle path to
   `/Users/<you>/adi-family/target/release/adi-app` (keep the `8000` arg).
3. Reload: `launchctl bootout gui/$(id -u)/family.adi.app.control-panel 2>/dev/null;
   launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/family.adi.app.control-panel.plist`
   — `bootout` also kills the old process; `RunAtLoad` starts the new one.
4. **Verify through the front door**, not just the port:
   `curl -s http://app.adi/api/health` and confirm a *new* endpoint your change added
   returns `200` (e.g. `curl -o /dev/null -w '%{http_code}' http://app.adi/api/<new>`).

Caveat: this runs the **repo dev binary**, not the bundle. It survives reboots and
app relaunches (`adi up` won't rewrite the plist while the service is loaded), but an
explicit `adi enable` / disable→enable (or `cargo clean` removing the path) reverts to
the old bundle binary. To revert deliberately: restore the backed-up plist + reload.

**Surgical restart pattern.** To kill only the app-service so launchd respawns it:
`pkill -9 -f 'Resources/adi-app '` (or `'target/release/adi-app 8000'` after a
repoint). Use a pattern that includes the trailing arg — the old
`'…/adi-app$'` anchor **never matches**, because the live command ends in ` 8000`.
`pgrep -af '<pattern>'` first to confirm it hits exactly the app-service and never
`adi-hive`.

**The one exception is ADI DNS (`adi.hive`)** — never stop, kill, or restart it; if a
task seems to require that, ask first. Everything here is about the `app` /
front-door services.

## Finding duplicated code: `adi-mono indexer clones`

This repo carries its own code index (`crates/adi-indexer`, full docs in `docs/indexer.md`).
Besides symbol/full-text/semantic search it fingerprints the **shape** of every symbol — the
parse tree with identifiers and literal values stripped out — so copy-paste is findable however
thoroughly it was renamed, and across languages.

**Reach for this before hand-rolling a duplication hunt with grep.** Index once, then queries are
instant.

```bash
adi-mono indexer index                                # build/refresh; rebuilds itself if stale
adi-mono indexer clones --min-nodes 40                # groups with identical shape
adi-mono indexer clones --min-nodes 60 --distance 8   # + copies that have since drifted
adi-mono indexer similar <symbol>                     # what *means* something like this
adi-mono indexer similar <symbol> --structural        # what *looks* like this
```

Every subcommand takes `--json` and `--path`. As of this writing the tree has **70 exact groups
(169 symbols)** at `--min-nodes 40`, and 237 groups (792 symbols) at `--min-nodes 60
--distance 8`.

Reading the output:

- `--min-nodes` is a floor on symbol size in parse-tree nodes, and it is load-bearing: below ~40
  every codebase has thousands of accessors that share a shape and mean nothing by it. Raise it
  to cut noise.
- `--distance` is bits of a 64-bit simhash — 0 is identical, unrelated code sits near 32, so only
  single digits are useful. Distance is meaningless on small symbols; pair it with a higher
  `--min-nodes`.
- Containers (modules, classes, traits) are excluded by default — a container's shape is just its
  children's concatenated. `--all-kinds` if you really want them.
- A bare method name works (`row_to_symbol` finds `SqliteStorage::row_to_symbol`).

**A confirmed clone is not automatically a refactor — read the pair before touching it.** The
~37 Leptos `*_view` functions that cluster together share a shape only because that is what
`view!` expands to, and merging them would be actively wrong. Nor does the per-language
`is_primitive_type` / `is_builtin` family in `adi-indexer/src/lang/*` mean anything by its shared
shape: each is a `matches!` over a different language's keywords. The tool finds candidates;
judgment is still yours.

**And a merged clone often still groups, smaller.** `get_transitive_callers` /
`get_transitive_callees` in `graph.rs` were identical 31-line BFS twins; they now share one
`transitive` walker and are three-line wrappers around it — still a group, at 47 nodes instead of
189. Two named entry points into one implementation is the floor, not a finding. Read the node
count, not just the group.

### The Rust-only counterpart: `adi-clone-lint`

`lints/adi-clone-lint` (full docs in `docs/clone-lint.md`) asks the same question as a **rustc
lint**, over HIR instead of tree-sitter. Warn-only, never denies.

```bash
cargo dylint --lib adi_clone_lint --workspace
cargo dylint --lib adi_clone_lint -- -p adi-indexer
```

Use it, rather than the indexer, when you want the *extent* of a duplication and a list of what
differs between the copies. It reports runs of statements rather than whole symbols — so the
shared stretch of two otherwise-different functions is findable — and because rustc has already
resolved every identifier, the renaming it reports is proved from `Res::Local` rather than
guessed: `sum` stands where `acc` stands, `100` becomes `200`.

Use the indexer instead when you need the whole tree at once or a language other than Rust: a
rustc lint runs per crate, so duplication whose halves live in two crates is invisible to it.

It is pinned to `nightly-2025-12-04` and is **its own workspace root** — never add it to
`members`, and never run it expecting the tree's stable toolchain. Tune it in `dylint.toml` at
the repo root (`min_nodes` is the knob that matters); `ADI_CLONE_LINT_DEBUG=1` dumps every
candidate fragment and group, which is the only way to tell a finding that was suppressed from
one that was never a candidate. As of this writing the tree reports **339 findings** — 113
identical, 53 renamed, 173 near — a third of them in `adi-webapp`, where Leptos `view!` shape is
duplication the lint is right about and you should leave alone.

If you change *what* gets indexed or *how* the embedded text is built, bump
`cache::SCHEMA_VERSION` **and** `indexer::PIPELINE_VERSION`. Both incremental layers key on file
content, so a change to the crate is invisible to them: skip the bump and a full reindex returns
in seconds with the old data and no error at all.

## Comments: `docs/comments.md`

The standard every comment in this tree is held to — nine rules, worked through on
`crates/adi-agents`, which was audited against all of them.

The one-line version: **a comment earns its place by saying what the code cannot.** Not what
the line does (the line says that, and never goes stale) — the reason, the alternative that
was tried, the constraint somebody else's API imposes, the bug the line exists to prevent.

For inline `//` comments the bar is higher still: keep one only where a reader would
otherwise get the line **wrong** — another system's behaviour, a past bug, an ordering or
concurrency hazard, a deliberate non-idiom, a magic value. Explanation that merely helps
belongs in the module header or the item's own doc. In `crates/adi-agents` that works out
at one inline comment per ~36 lines of code.

Two checks worth running after any refactor, because a comment left behind by a moved
function is a trap no compiler catches:

```bash
cargo doc -p <crate> --no-deps      # unresolved intra-doc links = dead references
```

and a grep for backticked identifiers in comments that match no definition in the tree.

## The design system: `design/DESIGN.md`

Every UI surface — the control panel, the mesh client, the landing, the native apps, the pages
the front door renders — follows `design/DESIGN.md`, drawn from one token file,
`design/tokens.css`. Read the rulebook fully before touching a screen; its §7 checklist is what
"done" means and its §8 Never/Always list is law (one orange per screen, sentence case, mono
only for machine strings, no cards around tables, no shadows, Lucide only).

- **Tokens** live once, in `design/tokens.css`; adi-css `@use`s it, adi-ui inlines it through
  Tailwind, the mesh client links it, the landing keeps a copy. Never restate a hex.
- **Icons**: `scripts/lucide.sh add <name>` fetches a Lucide SVG into `crates/adi-ui/icons/`;
  `adi-ui`'s build script turns the directory into `adi_ui::Lucide`; draw it with
  `adi_ui::Icon` (stroke 1.5, sizes 14/16/20/24). No inline SVG paths, no Unicode glyph icons.
- **Fonts**: `scripts/fonts.sh` refreshes the self-hosted Geist / Geist Mono / Bricolage subsets
  in `crates/adi-ui/fonts/`.
- `docs/design.md` maps where each surface implements the system.
