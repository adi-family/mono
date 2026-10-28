# Bring your own credentials — spec

A user supplies their own value for an environment variable the server allows, and the runs they
start on that server, and every run those runs start, get that value in place of the server's.
Read this before touching `crates/adi-secrets`, the run environment in `crates/adi-agents`, or the
launch paths in `adi-cli` and `adi-webapp-api`.

Status: **design, not built.** The design is task ADI-MONO-173. This revision follows the scope
revision of 2026-10-28 and the third round of decisions on the same day, both at the top of that
task, which replace the original design wherever they conflict with it. Three consequences: there
is no "who pays" concept anywhere in this document; values reach runs as plain environment
variables, not through the LLM gateway; and the user is a random id minted per paired peer, so one
peer is one user. Line numbers are as of `main` at 82ff81b (the code is unchanged from 63ebf74).
Wherever the code contradicts the design, or the design leaves something undecided, the point is
listed under §15 "Open questions" and **not** settled here.

**Two fixes land before any of this:** ADI-MONO-174 (the member gate misses two secrets routes,
§7) and ADI-MONO-175 (a CLI-spawned child inherits its parent's whole environment, §11). Both are
security fixes that stand on their own, and phase 1 assumes both.

## 1. The problem

A server's runs use the server's credentials. The credential an agent runs on is a secret an
operator set, for example `CLAUDE_CODE_OAUTH_TOKEN` in the global scope, and an agent's `[[secrets]]`
row attaches it. That works on a machine one person uses. It stops working once a fleet puts other
people on the machine (`docs/fleet.md` §8a, the member role) and they start running agents there:

- **Everyone spends the operator's subscription.** On the partner sandbox, which is ADI-MONO-171's
  reason for existing, every partner's run uses ADI's Anthropic key. A partner who has their own
  subscription can't use it.
- **There is no way to say "my runs, my key".** The secrets store has two scopes, global and project
  (`adi-secrets/src/lib.rs:438-459`). Neither belongs to a person.
- **A quota is recorded against the server's login, for everyone.** Holds are keyed on the
  credential's *name* (§6). If a user's own token hit a quota under the name the server's token
  uses, the server's login would be held for every agent on the machine.

## 2. The principle

**A user overrides a secret the server already uses. They don't get a backend, and they don't get
new access.**

The server keeps `llm/backends/*.toml` exactly as `docs/llm-backends.md` defines them. What a user
brings is the *value* behind an env-var name a run already receives, such as
`CLAUDE_CODE_OAUTH_TOKEN`, `ANTHROPIC_API_KEY` or `OPENAI_API_KEY`. Three things have to agree
before a user value reaches a run:

1. **The server allows the name.** It is in the server's `overridable` list (§4.1).
2. **The agent already attaches the name.** A user value replaces a secret in the agent's own
   `[[secrets]]` allowlist and never adds one (§5).
3. **The user's device has unlocked it on this server.** The value is held in the server's memory
   as a *lease*, for as long as the user picked in that device's settings for this server
   (§4.3, §9).

**The settings are the feature.** One policy file on the server, one small file per user, and one
table per server connection on the user's device decide everything. The CLI verbs in §4.4
edit them. Buttons and presets are a later layer on top of them (§14) and add no behaviour of their
own.

**Automations always use the server's values.** Triggers, schedules, workflows, task dispatch and
channels launch with no user attached and never receive a user value. That may be revisited later.

## 3. Identity: which name is the user

The user's identity comes from the headers the mesh gateway attaches on the node side
(`adi-mesh/src/gateway.rs:808-820`). Any copy the caller sent is stripped first. `adi-app` reads
them in `fleet_sender` (`adi-app/src/main.rs:582`). That trust holds for the panel because `adi-app`
serves exactly one request per connection (`adi-webapp-api/src/http.rs:2`, `Connection: close`): the
headers are attached to the first head on a stream, and on the panel there is no second head.
Anything that moves an identity-bearing endpoint behind a keep-alive server has to re-check this.

**The user is the credential username, and it is a random id minted for each paired peer.**
`<u>` in this document is `X-Adi-Fleet-User`, which is the server's pairing record's `auth.user`
(`gateway.rs:816-819`). It is registry data, not something the peer sends with a request. It is
also what `launched_by = human:<u>` and ADI-MONO-171's ownership checks already use
(`asking_user`, `adi-webapp-api/src/handlers/agents.rs:1384`).

**One peer is one user in v1.** A person's Mac and their browser client are two pairings, so they
are two users, each with its own values and its own lease. Grouping several devices under one
person is later work. Everything below is simpler for it: a user has exactly one device, so a lease
belongs to one device, revoking it is revoking that device, and §8 needs one wrap per value.

- **Never `X-Adi-Fleet-Node`.** That is the peer's self-declared nickname, and `docs/fleet.md` §2
  rule 3 lets a clash rename it.
- **Never the petname as the key.** The petname is what the server calls the peer, and an operator
  can rename it (`FleetRegistry::rename`). A user scope keyed on it would move with the rename.
  The panel shows the petname as the user's label, because a random id means nothing to a reader.
- **A local caller is the owner and has no user layer.** A request with no `FleetSender` (the panel
  on loopback, or a terminal on the server) is the machine's own operator, whose secrets are the
  global ones.

### 3.1 Where the username comes from today

The username is decided by whichever side **minted the invite**, and the other side copies it:

1. `join::decide` runs on the minter. It takes `spec.username` from the invite, or falls back to
   `PAIR_USER = "adi"` (`adi-mesh/src/join.rs:619`, the constant at `:112`), and stores it with the
   password in the minter's record of the dialler (`record.set_password(username, password)`,
   `:631`).
2. The reply carries it back: `JoinReply::Accepted(Accepted { username, .. })` (`:472-476`,
   built at `:640-646`).
3. The dialler files the minter with the same credential, in `record_viewer`
   (`record.set_password(&accepted.username, ..)`, `:740`). The browser client stores the same
   field on its own `NodeRecord` (`adi-mesh-client/src/invite.rs:104`, `store.rs:50-51`) and sends
   it as Basic auth (`bridge.rs:204`).
4. On every request the receiving side's gateway attaches its record's `auth.user` as
   `X-Adi-Fleet-User` (`gateway.rs:816-819`). `Credential::user` is stored in the clear in
   `fleet.toml` (`auth.rs:117`).

The no-body invite, which is what the webapp's "mint a code" button sends
(`adi-webapp-api/src/handlers/fleet.rs:127-130`), therefore pairs every peer as `adi`. Two members
paired that way are the same user to everything that reads the header today, ADI-MONO-171's
ownership checks included: each can stop or delete the other's runs as "their own". The doc on
`PAIR_USER` says the username "carries no security weight". That stopped being true when
ADI-MONO-171's ownership checks started reading it.

### 3.2 The change: a random user id, minted by the server

**`decide` mints a random id where it used to write `PAIR_USER`,** and it is returned in
`Accepted.username` as the username is today. Neither the wire format nor any dialler changes:
the Mac's `record_viewer` and the browser client already store whatever `Accepted.username` says.

```
u-<26 characters, lowercase Crockford base32>      130 random bits, e.g. u-7f3kq9…
```

- **The form is chosen for where it goes.** It is a Basic-auth username (no `:`), a path segment
  under `secrets/users/` (it passes `validate_project`'s filesystem-name rule, `secret.rs:136-138`),
  part of an AAD, and the tail of `launched_by = human:<u>`. Lowercase base32 has no ambiguous
  glyphs, which matters because `mesh join` prints it for a person to retype into a browser prompt
  (`adi-cli/src/mesh.rs:417`). That is the cost of the change: the old prompt took `adi` from memory.
- **Random, not derived.** Not from the peer's key, which would make the id the same on every server
  the peer pairs with and so link the user across fleets. Not from the petname, which can be renamed.
- **The record remembers that this machine minted it.** `NodeRecord` gains
  `minted_user: bool` (`fleet.rs:392`, a plain value, so it goes before `auth` per the field-order
  note). Only `decide` sets it, and only for an id it generated itself. `#[serde(default)]` makes
  every existing record `false`.
- **A user scope needs `minted_user`.** `adi-app` gains `user_scope(projects, sender)` beside
  `peer_role` (`adi-app/src/main.rs:596`). It loads the registry, finds the sender's petname, and
  answers `Some(auth.user)` only when `minted_user` is set and the header's user matches the record.
  Anything else answers `None`, which means "no user layer: the server's values". On doubt it
  trusts the peer with less, as `peer_role` does when it defaults to `Member`.

That rule gives the decided answers directly:

| peer | `auth.user` | user scope |
|---|---|---|
| paired after this ships, no username in the invite | `u-…`, minted by `decide` | yes |
| paired before this ships | `adi` | **none until re-paired** |
| filed by `record_viewer`, because the server dialled the peer's invite | whatever the peer's side minted | none (§15 Q14) |
| an invite that pins `username` | the pinned name | none in v1 (§15 Q13) |
| a local caller | — | none: the owner, on global secrets |

**A re-pair keeps a minted id.** `decide` rotates the username on a re-pair today (`:628-631`).
Re-inviting a peer to rotate a lost password must not orphan the values stored under its id, so a
re-pair of a record with `minted_user` keeps `auth.user` and rotates only the password. A re-pair
of an `adi` record mints a fresh id, which is how a legacy pairing gets a user scope.

**Unpairing ends the user.** The id is never reissued, so unpairing the peer revokes its lease
(§10.1) and deletes `secrets/users/<u>/`. Nothing else could ever open those wraps.

## 4. The settings layer

### 4.1 Server policy: `secrets/settings.toml`

```toml
# ~/.adi/mono/secrets/settings.toml
[user_secrets]
enabled     = true                                   # default false
overridable = ["CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_API_KEY"]   # default []
missing     = "server"                               # server | wait | fail; default "server"
max_lease   = "72h"                                  # optional; absent = no cap
```

| key | what it decides |
|---|---|
| `enabled` | Whether this server takes user values at all. `false` ignores the user layer, refuses `secrets/mine/set` and unlocks, and keeps stored user secrets on disk unused. |
| `overridable` | The env-var names a user may supply. Empty by default, so turning `enabled` on changes nothing until the operator names something. A name not on the list can't be set, can't be shared, and is never applied. |
| `missing` | What a run does when it would use a user's value for a name and no live lease holds one (§10). `server`: use the server's value for that name. `wait`: park the turn until the device unlocks. `fail`: refuse the turn with a message saying so. |
| `max_lease` | The longest lease a device may ask for. A device's `lease` is clamped to it, and `"forever"` becomes `max_lease`. |

No such file exists today. `adi-secrets` has no settings at all: the crate holds `global/`,
`projects/` and the master key (`crypto.rs:8-11`). The file sits at the root of `secrets/`, beside
`global/`, so `list(None)` never sees it as a secret (`lib.rs:220-247` reads `global/` only).

**Read on each launch and each turn, not cached.** The same reasoning `docs/llm-backends.md` gives
for `llm/settings.toml`: an operator who narrows the list expects the next turn to obey it.

**Only an admin can write it.** It is edited with `adi-mono secrets policy` locally, or through a new
`POST /api/secrets/policy`, which joins the member gate's list (`member_blocked`,
`adi-app/src/main.rs:622-635`).

**An unreadable `[user_secrets]` fails closed: the feature is off.** A file that doesn't parse
reads as `enabled = false`. No user value reaches any run, unlocks are refused, and every run uses
the server's values. That matches `LlmSettings::load`, which reads a corrupt file as its defaults
(`llm/settings.rs:49`), and it is the decided reading of "closed": when the server can't tell
which names a user may replace, it replaces none. The cost is stated plainly: a server whose policy
said `missing = "fail"` puts every member back on its own credentials until the file is fixed. So
the parse error is logged on every read and shown on the panel's Secrets page, and
`adi-mono secrets policy` prints it instead of the defaults.

### 4.2 Per user: `secrets/users/<u>/`

```
~/.adi/mono/secrets/
  settings.toml                          §4.1
  global/<NAME>.toml                     today
  projects/<project-id>/<NAME>.toml      today
  users/<u>/<NAME>.toml                  new: one user value, wrapped to their device (§8)
  users/<u>/settings.toml                new: that user's own policy
```

```toml
# secrets/users/<u>/settings.toml
missing = "wait"     # may only be stricter than the server's
```

- **The per-user `missing` may only tighten.** The order is `server` < `wait` < `fail`, and the
  effective value is the stricter of the server's and the user's. A user can say "never put my runs
  on the server's key" (`wait` or `fail` where the server says `server`). A user on `wait` under a
  server on `fail` gets `fail`, so they can't loosen a server either. `fail` ranks above `wait`
  because it is the one that also stops work from piling up behind a missing value.
- **A user value can only exist for an `overridable` name.** `set` refuses any other name. If the
  operator later removes a name from the list, the stored value stays on disk and is never applied.
- **`settings.toml` collides with a secret name.** Secret names are env-var identifiers, and
  `validate_name` accepts lowercase (`secret.rs:124-133`), so a secret called `settings` would be
  stored as `users/<u>/settings.toml`. §15 Q1.
- **How an unreadable `users/<u>/settings.toml` reads is open.** §15 Q15.

**The scope is a real third variant, not a reserved project id.** `adi-app` keeps node passwords in
a pseudo-project called `fleet-nodes` (`adi-app/src/viewer.rs:27`). That works because nothing
resolves that scope into a run. A user scope exists only to be resolved into runs, and `<u>` must
never be mistaken for a project id. `validate_project` (`secret.rs:138`) would accept a username
as a project id. So `adi-secrets` gains `enum Scope { Global, Project(&str), User(&str) }`. The
existing `project: Option<&str>` methods become thin wrappers over it, and `manifest_file` /
`scope_dir` / `ensure_scope_dir` (`lib.rs:438-476`) gain the `users/` arm. A user secret's AAD is
`user:<u>/<NAME>`. It can't collide with `global/<NAME>` or `projects/<id>/<NAME>` (`aad`,
`lib.rs:481-486`), so a ciphertext copied between scopes won't open. `list_all` does **not** walk
`users/`: it backs the panel's secrets page, and a user scope is write-only (§7).

### 4.3 On the device: one table per server connection

How long a user's values keep working on a server is **the user's decision, made on their own
device, separately for each server connection.** It is applied every time the device connects.
Nobody is prompted per run.

```toml
# ~/.adi/mono/mesh/connections.toml, on the user's device
[connections.<server>.secrets]
share       = ["CLAUDE_CODE_OAUTH_TOKEN"]   # which of my values this server may lease; default []
lease       = "24h"                         # 0 | "<n>h" | "forever"; default 0
auto_unlock = true                          # unlock on connect without asking; default false
```

| key | meaning |
|---|---|
| `share` | The names this device releases to this server. A name the server doesn't list as `overridable` is skipped on unlock. Empty means this device never unlocks anything here. |
| `lease` | How long the lease outlives the device. `0`: only while connected, ending a short grace after the last renew. `"24h"`: 24 hours after the device was last seen. `"forever"`: until revoked, unpaired, or the server restarts. Clamped to the server's `max_lease`. |
| `auto_unlock` | Unlock as soon as the device connects. Without it the user unlocks by hand (`mesh unlock`). |

- **The TTL counts from last seen, not from the unlock.** Every renew moves `last_seen` forward. A
  laptop that is open all week keeps its lease all week, and the clock starts when it goes away.
- **The device decides and the server enforces.** The policy travels with the unlock (§9 step 4).
  The device can't be relied on to say "stop" after it has gone offline, which is exactly when the
  clock matters, so the server runs the clock.
- **It lives in its own file, not in `fleet.toml`.** On a Mac (or any `adi-mono` device) it is
  `connections.toml` in the `mesh` module directory, beside `fleet.toml` (`MODULE = "mesh"`,
  `adi-mesh/src/config.rs:15`; `FLEET_FILE`, `fleet.rs:49`), keyed by the server's petname. The
  pairing record (`[nodes.<petname>]`, `NodeRecord`, `fleet.rs:392`) stays what it is: the
  registry, which the mesh rewrites on every pair, rename and grant. A user's choice about their
  own values has no business in a file the mesh owns, and a separate file means no code that saves
  the registry can lose it. In the browser client it is a separate IndexedDB object store keyed by
  petname, not a field on the client's `NodeRecord` (`adi-mesh-client/src/store.rs`), for the same
  reason.
- **A rename moves it, an unpair deletes it.** The key is the petname, so `FleetRegistry::rename`
  (`fleet.rs:896`) and unpair each touch `connections.toml` too. That is the one coupling between
  the two files.

### 4.4 The verbs

Every verb edits one of the three settings above, or acts on a lease.

```sh
# on the server, as its operator
adi-mono secrets policy                                   # show [user_secrets]
adi-mono secrets policy --enable --overridable CLAUDE_CODE_OAUTH_TOKEN,ANTHROPIC_API_KEY \
                        --missing server --max-lease 72h
adi-mono secrets leases                                   # live leases: user, device, names, expiry

# on a user's device, against a server
adi-mono secrets set CLAUDE_CODE_OAUTH_TOKEN --user --node <server>   # value from stdin, wrapped here
adi-mono secrets rm  CLAUDE_CODE_OAUTH_TOKEN --user --node <server>
adi-mono secrets policy --user --node <server> --missing wait          # my own settings.toml
adi-mono secrets leases --node <server>                               # my leases there
adi-mono mesh share  <server> CLAUDE_CODE_OAUTH_TOKEN --lease 24h --auto-unlock
adi-mono mesh unlock <server>                                         # unlock now
adi-mono mesh lock   <server>                                         # revoke my lease there now (§10.1)
```

- **The `--node` verbs go over the mesh.** `adi-mono secrets` acts on the local store through the
  `adi-core` facade (`adi-cli/src/secrets.rs`), so today nothing sets a secret on another machine
  from a shell. These call the node's `/api/secrets/mine/*` and `/api/leases/*` through the local
  mesh gateway, addressed the way a dashboard transfer is (`docs/fleet.md` §10):
  `127.0.0.1:<gateway port>` with `Host: app.<node>.n.adi`. `--user` without `--node` is refused,
  because there is no user scope on your own machine.
- **`secrets set --user` refuses until phase 2.** Phase 1 ships the scope and the API, but a value
  can only be stored wrapped to the device (§8), and the wraps land in phase 2. Until then the verb
  answers "user values need device wraps, which this build doesn't have" and sends nothing.
  `secrets rm --user` and `secrets policy --user` work from phase 1.
- **No reveal, ever.** There is no `secrets get --reveal --user` and no `secrets read --user`.
  `secrets read` is "the value-returning primitive an agent uses" (`adi-cli/src/secrets.rs:55`), and
  a user scope must never be readable that way.
- **The lease verb is `mesh lock`, not `mesh revoke`.** `adi-mono mesh revoke <petname> <grant>`
  already removes a grant from a peer (`adi-cli/src/mesh.rs:173-178`), and overloading it would
  make one typo the difference between "stop my runs" and "cut this peer off". `lock` is the
  opposite of `unlock`, which is the pair a user reads them as. It leaves the device's table alone,
  so under `auto_unlock` the next connect unlocks again. To stop that, set `share = []` or
  `auto_unlock = false` with `mesh share`.
- **`mesh share` writes the device's own `connections.toml` (§4.3)** and nothing else. It sends
  nothing until the next unlock.

## 5. Where a user value reaches a run

**The layer goes into the attached-secret allowlist, not into `Secrets::resolve`.** On `main`,
`Secrets::resolve` (`adi-secrets/src/lib.rs:398`) isn't on an agent run's path. Triggers call it:
`Triggers::run_env` (`adi-triggers/src/lib.rs:287`) puts every global and project secret into a
trigger's shell. The marketplace calls it too (`bundle.rs:1448`). Agent runs use
`attached_secret_env` (`adi-agents/src/lib.rs:3776-3792`), called from `spec_in`
(`lib.rs:2421`). It decrypts only the `[[secrets]]` rows the agent names, globals first with
projects over them. Adding a user layer to `resolve` would put user values into trigger shells,
and automations always use the server's values (§2). So `resolve` doesn't change.

**The order a run's environment is built in, with the new layer:**

```
global attachment  <  project attachment  <  USER VALUE (attached ∩ overridable ∩ shared, live lease)
  <  ADI_DB / workspace / knowledge vars  <  the agent's own [env]  <  ADI_RUN_ID, ADI_LEASE, …
```

That order is `spec_in`'s today (`lib.rs:2421-2440`) with one layer inserted. The user value
replaces an attached secret where the attached secret would have gone. An agent whose own `[env]`
names the same variable keeps winning, exactly as it beats a secret today. That way a user value
replaces a *secret* and never a setting the agent's author pinned.

**Names, per turn:**

```
applied = session.lease.names                  pinned at creation (§11)
        ∩ agent's current [[secrets]] names    re-read every turn (as_run, lib.rs:194)
        ∩ server's current overridable
        ∩ the live lease's shared names
```

Each of the four can only narrow. A name the agent stopped attaching mid-conversation stops getting
the user's value, and nothing can add a name.

**Which runs get a lease reference at all.** A session records `lease = { user: <u>, names }` at
creation (§11) when:

- it was launched by a person over the mesh: `run_agent` with a `FleetSender` for which
  `user_scope` answers `Some(<u>)` (§3.2, `adi-webapp-api/src/handlers/agents.rs:322`), or by a run
  that already holds one (§11);
- `[user_secrets] enabled` is set; and
- the agent attaches at least one `overridable` name.

It is recorded whether or not the lease is live at that moment. Liveness is decided per turn, and
`missing` says what happens without a live lease (§10). The panel's local launches, the Review
button (`handlers/agents.rs:822`, launched as bare `human`), and every automation record no lease
and use the server's values. That covers task dispatch (`dispatch.rs:289-296`), channels
(`adi-channels/src/dispatch.rs:167-174`), triggers, schedules and workflows.

**`<u>` comes from the header, never from the body.** `run_agent` takes `launched_by` from the
request body (`handlers/agents.rs:347-353`), and `attribute_human` (`:1394`) enriches only the bare
word `human`. Anything else passes through untouched. A peer can send
`"launched_by": "human:someone-else"` today and the store records it. That is harmless for a listing
filter and would be fatal here. So the lease's user is computed from `FleetSender` and the
registry alone (`user_scope`), and `LaunchOptions` gains a `lease` field that no HTTP handler fills
from JSON. The `launched_by` forgery itself is §15 Q6.

**The panel's Test button tests the asker's value.** `ondemand::resolve_credential`
(`llm/ondemand.rs:292`) reaches for the *global* secret of the runtime's ambient env var
(`ambient_credential_env`, `:268-276`). A test asked over the mesh by `<u>` uses `<u>`'s live
lease value for that name when there is one, under the same `missing` rule.

**Settings-file backends are out of reach.** A backend that names `settings =
"~/.claude/settings.glm.json"` carries its credential in a file, and its hold key is that file
(`llm/backend.rs:267-268`). The layer overrides env vars by name. It doesn't rewrite files. §15 Q7.

## 6. Holds: the key fix

`LlmBackendManifest::credential()` (`llm/backend.rs:266-284`) answers "whose subscription is this":
the settings file, else `provider|base_url|api_key_env`, else `runtime:<runtime>`. Every
`harness:claude-sdk` backend that names nothing is `runtime:harness:claude-sdk`. A hold is
`(credential, model | *)` (`llm/holds.rs:84`), shared by every agent on the machine, which is the
whole point of `docs/llm-backends.md` §Holds.

With a user layer that sharing becomes a bug. Igor's token hits a five-hour cap, the turn is
classified `quota`, and the hold lands on `runtime:harness:claude-sdk`. Now every agent on the
server skips the server's login until Igor's quota resets.

**The rule: when a user value supplied the credential for a turn, the hold key carries the user.**

```
server value   runtime:harness:claude-sdk              claude-opus-5
user value     runtime:harness:claude-sdk#user:<u>     claude-opus-5
```

"Supplied the credential" means the user layer provided the backend's `api_key_env`, or, for a
backend naming nothing, the runtime's ambient var (`ambient_credential_env`). `#user:<u>` is
proposed because `#` appears in no `credential()` form today, and `holds.db` stores the key as
text, so no schema change is needed.

**Computed per turn, not pinned.** `ResolvedBackend.credential` is set by `resolve_row`
(`llm/chain.rs:408`) and pinned into the session's chain on the first turn. `hold_key()`
(`chain.rs:109`) and `RunSpec.credential` (set at `lib.rs:1386` and `:2206`) both read it from
there. Whether a turn ran on the user's value or the server's can change mid-conversation: the
lease lapses under `missing = "server"`, or the user removes their secret. So the pinned field stays
the backend's own credential, and the qualified key is computed for each turn from the pinned row
and which layer supplied the value. The same qualified key goes into `RunSpec.credential`, so a
Codex thread created on a user's key is never resumed on the server's login, or the other way
round. `codex_credential` (`runner/detached.rs:668-672`) already refuses exactly that between two
logins.

**The prober can't reach a user's hold, and that's acceptable for now.** `prober::check`
(`llm/prober.rs:160`) looks for a backend whose `credential()` equals the hold's. A qualified key
matches none, so the hold is reported `Unreachable` and waits out its own deadline, which is what
every vendor-CLI hold already does. A probe is a billed request, so probing a user's hold would
spend the user's quota and need their lease. §15 Q8.

**The panel's hold list shows a qualified key as "a user's subscription"**, and shows which user
(by petname, §3) only to that user. "Igor's quota is spent until 18:40" isn't the operator's to publish to every
member.

## 7. The own-scope API: write-only

A user may **set, replace and remove** their own values, set their own `settings.toml`, and list
their own names and metadata. No API ever reveals a user value. That holds for an admin and for the
user themselves. With device wraps the server couldn't reveal a value at rest anyway (§8).

```
GET  /api/secrets/mine                 → { names: [{ name, description, updated_at }], missing }
POST /api/secrets/mine/set             { name, description?, ciphertext, nonce, wraps[] }
POST /api/secrets/mine/remove          { name }
POST /api/secrets/mine/settings        { missing }
GET  /api/leases                       → my live leases (an admin: all of them, names only)
```

- **`<u>` comes from the gateway's identity headers, never from the body.** There is no `user`
  field to send. A local caller has no user scope.
- **Members may reach these, and only these.** `member_blocked` (`adi-app/src/main.rs:622-635`)
  refuses a member on exactly `/api/secrets/{reveal,set,remove}`. The `/mine` routes are separate
  paths, so they need no exception carved into that list. `POST /api/secrets/policy` is added to the
  list (§4.1).
- **`set` refuses names not in `overridable`**, refuses everything while `enabled = false`, and
  refuses a caller with no user scope (§3.2).
- **In phase 1 `set` refuses everything.** It accepts only wrapped values (§8), and the wraps are
  phase 2. The route, its checks and its tests ship in phase 1, and the answer is
  `501 user values need device wraps`. `remove`, `settings` and `GET /mine` work.
- **`GET /api/secrets` never shows a user scope.** It is `list_all`, which doesn't walk `users/`.

**ADI-MONO-174 lands before phase 1.** Reading the member gate for this spec turned up two routes
missing from it: `POST /api/secrets/set-oauth` (`main.rs:835`) and `POST /api/secrets/refresh`
(`main.rs:464`). A member can therefore overwrite or refresh a *global* OAuth secret today,
including the very `CLAUDE_CODE_OAUTH_TOKEN` this spec is about. A user scope that members can
write to would be pointless beside a global scope they can also write to. That task also refreshes
the stale `docs/fleet.md` §8a/§11 wording.

## 8. Device wraps

A user value is stored on the server in a form only the user's device can open:

```toml
# secrets/users/<u>/CLAUDE_CODE_OAUTH_TOKEN.toml
description = "my Max subscription"
nonce       = "…"           # the value, sealed under a fresh per-secret data key (DEK),
ciphertext  = "…"           # XChaCha20-Poly1305, AAD "user:<u>/CLAUDE_CODE_OAUTH_TOKEN"
created_at  = 1793000000
updated_at  = 1793000000

[[wraps]]                   # the DEK, sealed to the device's key; exactly one in v1
kind    = "keychain-v1"     # | "webauthn-prf-v1"
nonce   = "…"
wrapped = "…"
```

- **One DEK per secret, one wrap.** A user is one peer (§3), so there is one device to wrap to.
  `wraps` is still an array, so grouping several devices under one person later adds entries
  rather than changing the format. Setting a new value mints a new DEK and a new wrap. Plaintext
  never reaches the server on the way in: the device seals before it sends.
- **A new device is a new user.** It pairs, gets its own id, and enters its own values. There is
  no enrolment of a second device into an existing user in v1, so the question of how a new device
  gets a DEK it can't unwrap doesn't arise yet.
- **The server's master key is not involved.** Today's crypto seals with a master key kept in a
  `0600` file beside the ciphertext (`crypto.rs:8-11`), readable by the Unix user every agent run
  on the box runs as. A user value sealed that way would be readable by any run, whenever it liked.
  There is no `always` mode in v1 for that reason. A server restart drops every lease, and those
  runs wait until the device reconnects (§10).
- **The device key is not the mesh identity key.** The mesh key authenticates a connection. It sits
  as a raw file at `~/.adi/mono/mesh/identity.key` (`adi-mesh/src/identity.rs:19`) and in browser
  IndexedDB (`adi-mesh-client/src/store.rs`), both readable by anything running as that user.
  Reusing it would make "can pair as you" and "can read your subscription" one capability.
- **macOS: a key in the Keychain.** A random 256-bit key-wrapping key, stored as a Keychain item
  with device-only accessibility. **No macOS Keychain code exists in the tree today.** Only
  `apps/ios/AdiFleet/Keychain.swift` does, so it is added to `adi-app`, which is the device on a Mac.
  Whether the separate `adi-mono` binary can use that item is §15 Q12.
- **Browser client: WebAuthn PRF.** A passkey with the `prf` extension, its output put through HKDF
  into the key-wrapping key. Nothing of the kind exists in `crates/adi-mesh-client` today.
- **The unwrap runs in device-owned code, never in the server's panel.** A node's panel is HTML and
  wasm *served by the node*, and a server can change a page it serves to keep the plaintext it was
  trusted to unwrap. So unlocking happens in the Mac's own `adi-app` and in the browser client's own
  shell, never inside the iframe rendering `app.<node>.n.adi`.

## 9. Unlock and leases

**Whoever has no open port dials, and here that is always the device** (`docs/fleet.md` §8).
Nothing can dial a browser, and a Mac behind NAT is no better. So every step is a request *from*
the device *to* the server's panel, over the connection the fleet already has. "Seen online" can
only ever mean "the device asked something recently".

```
device                                         server: adi-app (in memory)
──────                                         ───────────────────────────
1  GET  /api/leases/session                    → adi-app's session public key + epoch
2  GET  /api/secrets/mine                      → this device's wraps for the shared names
3  unwrap each DEK locally (Keychain / PRF), open the value,
   seal it to the session key: X25519 + XChaCha20-Poly1305,
   AAD = user:<u>/<NAME> | epoch | device
4  POST /api/leases/unlock { lease, sealed[] } → checks AAD user == header user, names ⊆ overridable,
                                                 clamps lease to max_lease, opens, keeps a Lease
5  POST /api/leases/renew  every H seconds     → last_seen := now
6  POST /api/leases/revoke (mesh lock, unpair) → lease dropped, capabilities void, runs stopped (§10.1)
```

- **The routes are `/api/leases/*`, not `/api/mesh/*`.** `member_blocked` refuses a member every
  `/api/mesh*` path (`main.rs:626`), and unlocking is exactly what a member does.
- **"Re-encrypted to the server session" is read as: to an ephemeral key `adi-app` generates at boot
  and never writes down.** A restart makes every earlier unlock undecryptable on its own. §15 Q9.
- **QUIC already encrypts the hop.** `adi/mesh/http/1` runs inside a QUIC connection between the
  device's key and the node's key, and the relay sees only ciphertext (`docs/fleet.md` §9). The seal
  in step 3 keeps plaintext out of anything that logs request bodies on the way in.

The lease, in `adi-app`'s memory only:

```rust
struct Lease {
    user: String,                                   // <u>; the table is keyed by it, one lease per user
    petname: String,                                // the peer that unlocked, for display and the unpair hook
    values: BTreeMap<String, Zeroizing<String>>,    // NAME → value, NAME ∈ shared ∩ overridable
    lease: LeaseLength,                             // 0 | hours | forever, after the max_lease clamp
    last_seen: Instant,                             // steps 1, 4, 5
}
```

- **One lease per user.** A user is one peer (§3), so a second unlock from the same peer replaces
  the lease in place rather than adding one beside it.
- **Leases live in `adi-app` because `adi-app` starts the runs.** Every in-app launch and turn
  (§11) reads them in-process. Nothing else on the box can read them except through §11's
  capability.
- **Never on disk in server-decryptable form.** The lease isn't journalled or persisted. The values
  are zeroed on drop. A server restart ends every lease, the device's next renew gets
  "unknown epoch, re-unlock", and with `auto_unlock` it redoes steps 1–4 by itself.
- **The renewing side.** On a Mac, `adi-app` runs as a LaunchAgent whether or not anybody is looking,
  so `lease = 0` means *while this Mac is awake and online*. The renew loop is a background worker in
  the device's `adi-app`. In the browser it means *while a tab of the client is open*.
- **Server-side presence is a different clock.** `fleet_activity` (`adi-mesh/src/activity.rs`) is
  keyed on the *nickname*, uses a 60-second window (`ACTIVE_WINDOW_SECS`, `activity.rs:25`), and
  sits in a database every agent can write. None of that fits a lease, whose `last_seen` lives in
  `adi-app`'s memory and is keyed by the petname that renewed.
- **The heartbeat `H` and the grace for `lease = 0`** are §15 Q10.

## 10. Missing values: waiting on unlock, and auto-resume

Before any turn of a session with a lease reference starts, the launcher works out `applied` (§5)
for each name in `session.lease.names`. Where the user has a live value, it is used. Where they
don't, the effective `missing` (the stricter of the server's and the user's) decides:

| `missing` | a name with no live value |
|---|---|
| `server` | uses the server's attached value for that name, and the hold key for the turn is unqualified (§6) |
| `wait` | the turn doesn't start: the session parks, waiting on unlock |
| `fail` | the turn doesn't start, and the run says why: "CLAUDE_CODE_OAUTH_TOKEN isn't unlocked on this server", written the way `send_or_note_failure` (`lib.rs:2252`) writes a failed launch |

**Waiting on unlock** reuses what the store already has:

- the message stays in the session's `queue`, the table every mid-turn message already waits in;
- the session reads as `RunLifecycle::Waiting` (`run.rs:121-128`, "the conversation is going to move
  again on its own"), which a queued message already produces (`Pending`, `lib.rs:328-335`);
- `adi.agents.run.waiting_on_unlock { agent, run_id, user, names }` is published, and the rail shows
  "waiting for <u> to unlock" in place of a spinner;
- it is **not** `Decision::Ask`. `failover.rs:45` defines that as "a line in the conversation", and
  nothing here needs anyone to answer anything. The device needs to come back.

**The check has to come before the dequeue.** `advance_queue` drops the message from the queue
*before* starting it, on purpose: "a message that fails to launch has still had its turn"
(`lib.rs:2158-2162`). A parked turn must not be one of those. So the unlock check runs ahead of
`dequeue` there, and ahead of the start/queue decision in `deliver_with` (`lib.rs:1863-1870`). A
session waiting on unlock answers "not startable", exactly like one that is still busy.

**A fresh launch can wait too.** Under `wait`, a launch whose first turn has no live value creates
the session and queues its opening message instead of sending it. `Launch` has no variant for
"created, not started" (`run.rs:22-34`). §15 Q2.

**It resumes by itself.** When `adi-app` accepts an unlock or a renew that brings `<u>`'s values
back, it publishes `adi.secrets.lease.granted { user, names }`. The sessions whose `lease.user` is
`<u>` become startable again, and the app's own listing, which runs twice a second
(`lib.rs:2589-2590`), drains their queues through the ordinary `advance_queue`. That is the same
delivery an await wake uses. `adi.secrets.lease.revoked` and `adi.secrets.lease.expired` exist for
the rail and anyone watching.

**A lease that *expires* never cuts a turn in half.** Expiry is the device going quiet, not the
user asking for anything, so the turn in flight finishes on what it was started with and the check
applies from the next turn on. The same holds for a server restart, which ends every lease. A
*revoke* is different, and is the next section.

### 10.1 Revoke stops everything that uses the lease

A revoke is the user saying "stop using my key now". A value already in a running process's
environment can't be withdrawn from it, so the only way to honour that is to stop the process. **A
revoke invalidates the lease's capabilities and stops the running turn of every session provisioned
from the lease, subagents included.** It doesn't merely park the next turn.

It is triggered by `POST /api/leases/revoke` (`mesh lock`, §4.4) and by unpairing the peer on the
server (§3.2). It runs in full even when no lease is in the table: after an `adi-app` restart the
lease is gone, but turns started before the restart may still be running with the values in their
environment, and those are what the user wants stopped. `adi-app` does this, in order:

1. **Drop the lease** from the table and zero its values. From here no new turn can get them.
2. **Void the capabilities.** Every session whose `lease.user` is `<u>` has `sessions.lease_cap`
   set to NULL (§11). A run still alive that forwards `adi-mono agents run` in the next few
   milliseconds gets `403`, so a dying parent can't start a fresh child on the lease. Steps 1 and 2
   hold the lease table's write lock. The launch path holds its read lock from the capability
   check until the child's row is written at `create_as`, so a launch either finished before the
   revoke, and step 3 finds it, or fails.
3. **Find the sessions.** The store gains `SessionStore::with_lease_user(<u>)`, one query across
   every agent: `SELECT agent, id FROM sessions WHERE json_extract(lease, '$.user') = ?1`. It is
   the same shape as `agents()` (`store/mod.rs:401-412`), a walk of `sessions` that includes agents
   whose definition has since been deleted. Children are in it by construction: each child is its
   own session with `lease.user = <u>` (§11), so the tree needs no walking.
4. **Stop the live ones, through the existing stop path.** For each session that
   `session_is_alive` reports alive (`lib.rs:1088-1091`), `adi-app` calls the same runner stop
   `Agents::stop_run` uses (`lib.rs:2922-2953`): `runner.stop(.., STOP_GRACE)` (5 s, `lib.rs:133`),
   then `clear_queue`, then the `AgentRunStopped` event, so the rail shows it the way it shows a
   press of Stop. `stop_run` itself returns `false` early for an agent that no longer exists
   (`lib.rs:2926-2928`), and a deleted agent's live run is exactly the run nobody else will stop, so
   revoke goes through a variant without that gate. The runner already comes from the record, not
   the agent (`:2929-2932`).
5. **Publish `adi.secrets.lease.revoked { user, stopped: [run_id] }`.**

What that means in practice:

- **It stops a turn that ran on the server's value too.** Under `missing = "server"` a session can
  be lease-referenced and running on the server's value. The store doesn't record which source a
  running turn got, and after an `adi-app` restart the app couldn't know anyway, since detached runs
  outlive it. The user asked for everything provisioned from their lease to stop, and stopping one
  turn too many is the safe side of that.
- **A parked session stays parked.** A session waiting on unlock isn't running anything and holds
  no value, so step 4 leaves it alone and its queue survives. It resumes on the next unlock like
  any other parked session. A session whose turn step 4 stopped has its queue cleared, as a manual
  Stop does.
- **The next turn gets a new capability.** A session whose `lease_cap` was voided gets a fresh one
  minted the next time a turn of it starts on a live lease. For Codex that changes the child
  environment's hash, so the conversation continues in a new thread (§15 Q5).
- **Expiry isn't revoke.** A lease that runs out of `lease` time, or a server restart, doesn't stop
  anything (above). Only an explicit act does.

## 11. Subagent inheritance

**Every run started by a run that holds a lease gets that lease's values, transitively. The same
goes for every later turn of each of those sessions.** That is enforced in `adi-agents` and
`adi-app`, never left to an agent's prompt. This section traces each path that starts a run or a
turn, and says where the lease reference is recorded and where a child picks it up.

### Two funnels, many processes

Everything that spawns an engine process goes through one of two functions in `adi-agents`:

| funnel | what calls it | which process it runs in |
|---|---|---|
| `Agents::launch_run` (`lib.rs:1306`), a **new session** | `run_agent` (`handlers/agents.rs:382`), the Review button (`:822`), task dispatch (`dispatch.rs:289`), channels (`adi-channels/src/dispatch.rs:167`), `adi-mono agents run` (`adi-cli/src/agents.rs:744-757`) | `adi-app` for the first four; **the CLI's own process** for the last |
| `Agents::start_turn` (`lib.rs:2181`), a **later turn** | `deliver_with` (`lib.rs:1892`), reached from a panel reply (`reply_run`, `handlers/agents.rs:857`), an answer, an await wake (`awaits.rs:1063-1080`), a question default, a goal nudge; and `advance_queue` (`lib.rs:2163`), reached from any transcript read or run listing, which is also how failover re-asks (`lib.rs:2706-2714`) | `adi-app` for the panel and its worker loop (`adi-app/src/awaits.rs:71-104`); **any process holding an `Agents`** for the rest, including the CLI's `agents answer`, `agents runs` and `agents run --wait` (`adi-cli/src/agents.rs:838`, `:942`, `:1689`) |

Two facts about the code decide the design:

1. **A run's child is launched by the CLI, inside the run.** The `adi-agents` tool an agent gets for
   `can_spawn` (`with_spawn_tool`, `lib.rs:314-320`) is `adi-mono agents run`. That calls
   `Agents::launch` in its own process, and the child's caller is read from `ADI_AGENT` /
   `ADI_RUN_ID` (`launcher::by_caller`, `launcher.rs:46`; `awaits::caller`, `awaits.rs:689-700`).
   Leases live in `adi-app`'s memory (§9), so a process outside `adi-app` can't provision them.
2. **On `main` nothing scrubs the inherited environment.** `spawn_child` sets the run's env over
   whatever the spawning process already has, with no `env_clear` (`backends/detached.rs:82-91`).
   The pty runner does the same (`runner/pty.rs:122-128`). `effective_env` says so outright: "the
   last explicit run value wins over the app server's inherited environment"
   (`runner/detached.rs:1431-1433`). So a child the CLI spawns inside a run inherits the *parent's
   whole environment*, including secrets the child's agent doesn't attach. That already defeats the
   per-agent allowlist for server secrets, before any user value exists. It is ADI-MONO-175, which
   strips the parent's attached-secret names and `ADI_*` run identity before the spawn and lands
   **before phase 1**. This design assumes it: once it lands, the only way a child gets a user
   value is to be provisioned with one.

So: **leased work is started only by `adi-app`, and every other process either forwards to it or
leaves the work queued for it.**

### The lease reference, on the session

```
SessionRecord.lease        Option<LeaseRef>   JSON column `lease`, NULL = server values only
LeaseRef { user: String, names: Vec<String> } user = whose lease; names = attached ∩ overridable at creation
sessions.lease_cap         TEXT               SHA-256 of this session's capability, NULL when lease is NULL
```

- It is added the way `launched_by` and `overrides` were (`store/db.rs:192-197`,
  `ALTER TABLE sessions ADD COLUMN …`) and sits on `SessionRecord` beside them
  (`store/record.rs:80-87`). NULL on every existing session, and that is the right answer for them.
- **It is a lease reference, not a payer.** It names whose lease a session's turns look for. It says
  nothing about billing, and it never goes into a usage event.
- **Written once, at `create_as`** (`store/mod.rs:302`), and never re-derived. That is the same
  reason `overrides` and `chain` are pinned (`lib.rs:1418-1430`). Every later turn of a conversation
  looks for the lease its first turn was given.
- **`names` is computed from the agent being launched**, not copied from a parent. A child's
  `LeaseRef` is `{ user: parent.user, names: child.attached ∩ overridable }`. The attached-secret
  allowlist of *each* agent bounds what a user value can replace in *that* agent's runs.

### The capability: `ADI_LEASE`

`ADI_AGENT` and `ADI_RUN_ID` are plain environment variables. Any process can set them, and a run
id appears in every listing. Caller identity for a leased launch therefore can't come from them.

- **Minted per session.** When a session gets a `LeaseRef`, `adi-app` generates 32 random bytes,
  stores their SHA-256 in `sessions.lease_cap`, and puts the capability in the run's environment:

  ```
  ADI_LEASE      = <base64url, 32 bytes>
  ADI_LEASE_URL  = http://127.0.0.1:<adi-app's port>
  ```

  They are pushed after `name_conversation` (`lib.rs:3435-3437`), with the same precedence as
  `ADI_RUN_ID`.
- **Stable for the life of the session, not per turn.** Codex decides whether a thread may be
  resumed by hashing the child's whole environment (`hash_child_environment`,
  `runner/detached.rs:1402-1418`, called at `:765`). A capability that changed every turn would
  make every turn a new thread. Persisting only the hash means an `adi-app` restart doesn't change
  it.
- **Validated only by `adi-app`, only from loopback.** A request carrying `X-Adi-Lease` is honoured
  only when it has no `FleetSender` (`fleet_sender`, `main.rs:582`). A capability that leaked into a
  transcript can't be replayed by a member over the mesh. A capability that matches no session is
  refused with `403`. It never silently falls back to the server's values, because a wrong
  capability is either a bug or a forgery.
- **It is the lease's token for that session.** It stays valid while its session exists, whether or
  not the lease is live at that moment, because liveness is §10's per-turn question and an expired
  lease must not orphan a tree that will resume. Two things end it: deleting the session, and a
  revoke, which voids the capability of every session on the lease at once (§10.1). Children keep
  their own capabilities, and a revoke voids those too, because they are on the same lease.

### The paths, one by one

**A person launches over the mesh** (`run_agent`, `handlers/agents.rs:322`). `adi-app` computes
`LeaseRef` from `FleetSender.user` (§5), mints the capability, and launches in-process. The first
turn's environment comes from §5's layering, then the capability goes on top.

**A run launches a child: `adi-mono agents run` inside a run** (`adi-cli/src/agents.rs:744-757`).
When `ADI_LEASE` is set, the CLI doesn't launch in-process. It sends the same launch to
`POST $ADI_LEASE_URL/api/agents/run` with `X-Adi-Lease: $ADI_LEASE`. In `adi-app`, `run_agent`
does the following:

1. hashes the capability and looks up the session it names;
2. ignores the body's `launched_by`, records `agent:<that session's agent>`, and weighs `can_spawn`
   against that agent. This also hardens ADI-MONO-113's `spawn_policy`, which today believes
   whatever `ADI_AGENT` says (`lib.rs:1356-1376`);
3. builds the child's `LeaseRef` from the parent's user and the child's own attachments, and mints
   the child's own capability;
4. launches in-process, so the child is spawned from `adi-app`'s environment and not the parent's.

The CLI prints the answer exactly as it prints a local launch today, and `follow_the_run`
(`agents.rs:1485`) registers the wake locally as before. A grandchild repeats this with the child's
capability, so inheritance is transitive by construction. If the app can't be reached, the CLI
refuses rather than launching in-process on different credentials (§15 Q3).

**A run launches a child with `ADI_LEASE` removed** (`env -u ADI_LEASE adi-mono agents run …`). The
CLI launches in-process, as any lease-less launch does today, and the child gets no `LeaseRef`.
With ADI-MONO-175 in place the spawn strips the parent's attached-secret names, which include every
name a user value replaced, and `ADI_*`, which includes `ADI_LEASE` and `ADI_LEASE_URL`. Phase 3
adds a test that pins this down for the user layer. `hash_child_environment` has to apply the same
removal, so the fingerprint matches what the child sees. A run that opts out of inheritance gets the
server's values, never the parent's by accident.

**A later turn: reply, answer, await wake, question default, goal nudge, failover re-ask, queued
message.** All of them reach `start_turn` through `deliver_with` or `advance_queue`. `start_turn`
reads `record.lease`, resolves values from the live lease (§10), and re-applies the session's own
stored capability (minting a fresh one if a revoke voided it, §10.1).

**A reply runs on the conversation's lease, whoever sends it.** `docs/fleet.md` §8a lets any peer
reply into any conversation. `reply_run` (`handlers/agents.rs:857`) already carries the replier
into `Agents::reply_as` only as a conversation marker (`sender_marker`, `:864`), and that stays
so: the replier's own `user_scope` is never consulted for the lease. The turn looks for the
session's `lease.user` and nobody else's. There is no per-replier switching. A
conversation keeps one source of values for its life, and a turn never changes whose key it runs on
because of who typed. The cost is in §12: member B can spend member A's lease by talking in A's
chat. The `Agents` that `adi-app` builds (`adi-app/src/main.rs:225`) carries a lease
source. Every other `Agents` has none, and for a session whose `lease` is set it **doesn't start the
turn**. `deliver_with` enqueues and answers `Sent::Queued`, and `advance_queue` returns `false`
before dequeuing. The app's listing drains it within a second. So `adi-mono agents answer` typed
into a leased conversation from a terminal, or a CLI listing that would have advanced its queue,
can never start a leased turn on the wrong environment.

**Automation launches** (dispatch, channels, triggers, schedules, workflows) never carry a
capability and never set `LeaseRef`. A leased run that files a task which dispatch later picks up
has handed work to the server's values. That is the revision's rule, not a gap.

### When the lease expires under a child

- **The running turn of every session in the tree finishes** on the environment it started with
  (§10).
- **The next turn of any of them**, parent, child or grandchild, is checked on its own and gets the
  same `missing` answer. All of them share one `user`, so under `wait` the whole tree parks together
  and resumes together on `adi.secrets.lease.granted`.
- **A child launched after the lease expired** still inherits the `LeaseRef`, because the parent's
  capability is still valid. Its first turn meets `missing` like any other: on server values under
  `server`, created and parked under `wait`, refused with a message under `fail`.

### When the lease is revoked under a child

- **Every running turn in the tree is stopped**, the parent's and every descendant's, because
  every one of them is a session with `lease.user = <u>` (§10.1 step 3). Nothing in the tree goes
  on running on the revoked value.
- **No new child can start on it.** Every capability in the tree is void, so a forwarded launch from
  a process that hasn't died yet is refused with `403`.
- **The tree isn't orphaned.** The sessions keep their `LeaseRef`. A later unlock by the same user
  makes their *next* turns startable again, with fresh capabilities. Stopped turns aren't re-run:
  as with a manual Stop, the conversation moves when somebody sends it something.

## 12. Threat model and honest limits

Values reach runs as environment variables. Within that mechanism, this is what each party can do:

| who | can | cannot |
|---|---|---|
| **another member of the server** | read your transcripts (unchanged, `docs/fleet.md` §8a), including any value an agent of yours printed; reply into your conversation, and its next turn runs on your lease, by design (§11) | reveal your values through any API; set, replace or remove yours; start a *new* conversation on your lease (the user comes from the registry, and `X-Adi-Lease` is ignored over the mesh) |
| **a member paired before this ships** (username `adi`) | run on the server's values, as today | get a user scope, or be mistaken for another `adi` member's scope, until re-paired (§3.2) |
| **any agent run on the server** (same Unix user) | read every leased run's `/proc/<pid>/environ` (readable on this host), so your values and that run's `ADI_LEASE`, for as long as those runs live; use a stolen capability on loopback to launch runs on your lease; read the master key, so every global | read a user value at rest (it is wrapped to the device); obtain your value while none of your leased runs is alive |
| **the server's operator, or root** | everything above, plus `adi-app`'s memory while a lease is live, and change the code so the next unlock is kept | read your value while no lease is live and no code has been changed |
| **the network and the relay** | see that two keys talk | read anything: QUIC end to end, plus the seal to the session key |
| **a thief with your device** | unlock on your behalf, since in v1 the device *is* the user | — the server's operator unpairs its petname: the lease is revoked and its runs stopped (§10.1), and `users/<u>/` is deleted (§3.2). There is no "revoke from your other device" in v1, because your other device is another user |
| **an agent run on your own device** | edit your device's `connections.toml` (readable and writable by that Unix user), or ask your local `adi-app` to unlock | read the device key out of the Keychain without the item's access rule |

The limits, said plainly:

- **While a lease is live, your value is as exposed as any global secret an agent attaches
  today.** It sits in the environment of every leased run, readable by every process running as
  the server's Unix user, and printable into a transcript by the agent itself. The lease bounds
  *when new runs get it*. It doesn't bound what a run that had it did with it. Revoking on the
  device stops every run that had it (§10.1), but it can't un-leak a value a run already copied or
  printed. Revoke at the provider.
- **Replying spends the conversation owner's lease.** Any member who can reply into your
  conversation can make it take another turn on your key. That is the decided trade (§11): one
  source of values per conversation, and the member gate's collaborative read-and-reply model
  unchanged. A member who must not do that shouldn't be on a server where you lease.
- **Inheritance is attribution with a capability, not isolation.** The capability stops a run from
  *guessing* its way onto someone's lease by setting `ADI_RUN_ID`. It doesn't stop a co-resident
  process from *reading* a capability, because the Unix user that can read it can read the values
  next to it too. Running agents under a different Unix user from `adi-app` would change that, and
  is out of scope here.
- **The allowlist bounds what the platform provisions, not what a run passes on.** A leased run can
  hand its own environment to anything it starts itself. What §11 guarantees is that the platform
  never adds a user value to a run whose agent doesn't attach that name.
- **Prefer keys you can revoke and cap.** An Anthropic workspace key with a spend limit, an OpenAI
  project key, or a dedicated `claude setup-token`, rather than the login you use everywhere. Every
  limit above matters less with a key that is cheap to lose.

## 13. Later: the gateway

The original design had the LLM gateway hold the lease and inject the key into outbound requests,
with runs getting only a per-session token and a base URL. The key would then never be in a run's
environment, which removes the first limit in §12. It is out of v1. Shipping it means installing and
supervising `adi-llm-gateway` on every node, which today isn't packaged or pointed at by any run.
It also means reversing the gateway's "adds no key" rule (`adi-llm-gateway/src/lib.rs:21-23`) and
proving that the `claude` and `codex` CLIs accept a swapped bearer for subscription logins. The
settings and leases in this document are what the gateway would read if it is built, so nothing
here has to change for it.

## 14. Later: presets

Buttons are sugar over §4's settings. Each one writes settings and adds no behaviour of its own.

| button | where | what it writes |
|---|---|---|
| **Allow users' own values** | server, Secrets page | `[user_secrets] enabled = true`, `overridable = [<names picked>]`, `missing = "server"` |
| **Require users' values** | server | as above with `missing = "fail"` (or `"wait"`: park rather than refuse) |
| **Connect a token** | device, the server's connection row | `secrets set <NAME> --user --node <server>` plus `share += [<NAME>]` |
| **Lease: off** | device | `share = []` (or `auto_unlock = false` and never unlock) |
| **Lease: while connected** | device | `lease = 0`, `auto_unlock = true` |
| **Lease: N hours** | device | `lease = "<N>h"`, `auto_unlock = true` |
| **Lease: until revoked** | device | `lease = "forever"`, `auto_unlock = true` (clamped to `max_lease`) |
| **Revoke now** | device | `mesh lock <server>` (§4.4, §10.1): stops every run on the lease, and leaves the table alone, so the next connect re-unlocks under `auto_unlock` |
| **Never use the server's key for me** | device or "My keys" | `users/<u>/settings.toml`, `missing = "wait"` |

## 15. Open questions

Each is either a place the code contradicts the design, or a decision the design leaves open. None
is settled by this document. The three rounds of decisions on ADI-MONO-173 settled the earlier list's
questions on the default username, the revoke verb's name, where the device setting lives, the
strictness order, a corrupt policy, revoke stopping in-flight turns, replies from someone else,
phase 1 without wraps, enrolling a second device, and the inherited environment (now
ADI-MONO-175). Those are in the body, and the rest are renumbered here.

**Q1 — `users/<u>/settings.toml` collides with a secret named `settings`.** `validate_name` accepts
lowercase (`secret.rs:124-133`). Options: refuse `settings` as an `overridable` name (a real env
var is almost never lowercase), or move the file out of the scope directory
(`users/<u>.settings.toml`, or `users/<u>/.settings.toml`, which `validate_name` rejects as a
secret name).

**Q2 — A launch that can't start yet.** Under `wait`, a fresh launch with no live value creates the
session and queues its opening message (§10). `Launch` is `Pty | Process { pid, log, run_id }`
(`run.rs:22-34`). There is no "created, waiting" shape, and `adi-mono agents run --wait`,
`follow_the_run` and the panel all expect a process. Add a variant, or refuse such a launch with
"unlock first"?

**Q3 — The CLI when the app is unreachable.** A leased run's `agents run` forwards to `adi-app`
(§11). If the app is down or restarting, this spec refuses the launch. The alternative is to apply
the user's `missing` policy locally. That means the CLI reading `sessions.lease_cap` and both
settings files itself, and under `server` it would launch in-process with the server's values. That
works, but it moves credentials without the app ever seeing the launch.

**Q4 — Does `missing` cover a user who never stored a value?** §5 records a `LeaseRef` for every
member launch of an agent attaching an `overridable` name, so a member with no stored value at all
meets `missing` and, under `fail`, can't run those agents on this server. That is what makes `fail`
the partner sandbox's "the server never pays for a member". The other reading records a `LeaseRef`
only when the user has stored a value for at least one of the names.

**Q5 — Changing credentials mid-conversation breaks Codex threads.** Under `missing = "server"`,
a conversation alternates between the user's value and the server's as the lease comes and goes.
`hash_child_environment` (`runner/detached.rs:1402`) covers every value, so each switch starts a
new Codex thread, and the qualified hold key (§6) makes `codex_credential` refuse a resume across
them anyway. A revoke has the same effect through the re-minted capability (§10.1). Accept that, or
pin a session to whichever source its first turn used?

**Q6 — The `launched_by` body field.** `run_agent` records whatever `launched_by` the body sends
(`handlers/agents.rs:347-353`). This spec keeps the lease off that path, but the field still lets a
member stamp `human:<someone>` on a run, which defeats ADI-MONO-171's `member_run_refusal`
(`:2235`) the other way round. With random user ids a forger has to learn the victim's id first,
but ids appear on run cards. Fix it here or separately?

**Q7 — Settings-file backends.** A user value can't override a credential that lives in a settings
file (§5). Is BYO limited to env-named credentials (`api_key_env`, or the runtime's ambient var),
or should a user value beat a settings file's own `env` block?

**Q8 — Probing user-scoped holds.** The prober can't match a qualified key (`prober.rs:160`), so a
user's hold waits out its deadline. Probing would spend the user's quota and need their lease.
Leave it `Unreachable`?

**Q9 — "Re-encrypted to the server session".** Read here as: sealed by the device to an ephemeral
key `adi-app` generates at boot (§9). If the mesh connection's own keys were meant, §9's flow
changes.

**Q10 — Numbers.** The renew interval `H`, the grace after the last renew for `lease = 0`, and
whether an absent `max_lease` really means "no cap".

**Q11 — Codex and a user key.** `ambient_credential_env` assumes `OPENAI_API_KEY`
(`ondemand.rs:273`). The Codex runner knows `CODEX_API_KEY` and `CODEX_ACCESS_TOKEN`
(`runner/detached.rs:773-776`) and binds the host's `auth.json` login whenever one exists. Whether
a user's key actually takes precedence over the host's `codex login`, and whether user-keyed Codex
runs need their own `CODEX_HOME`, has to be shown before Codex is BYO.

**Q12 — The Keychain key and the `adi-mono` binary.** The device key is a Keychain item `adi-app`
creates (§8). On a Mac, the separate `adi-mono` binary asking for that item may get a prompt or a
refusal, so `secrets set --user` and `mesh unlock` from a terminal may have to go through the local
`adi-app` instead of opening the Keychain themselves. To be shown on a signed build before phase 2.

**Q13 — An invite that pins `username`.** `InviteSpec.username` (`join.rs:265-267`) lets the minter
choose the username, and the partner-sandbox plan uses it to give one person (`p<uid>`) several
devices. That is exactly the device grouping "one peer = one user" puts off, and a pinned name can
be given to two peers. This spec gives a pinned username **no user scope** in v1 (§3.2), which
leaves the partner sandbox without BYO until it stops pinning or grouping is built. The
alternatives: honour a pin as a user id and have `decide` refuse a pin another record already
carries, or have the sandbox stop pinning and keep `p<uid>` in `labels` instead.

**Q14 — The server dialling a device's invite.** When the server spends an invite the *device*
minted, the server files the device through `record_viewer`, and the username is whatever the
device's side chose (`join.rs:740`). The design says the id is "minted at pairing and returned in
the join response", which describes the server minting. A device-chosen id could equal another
user's, so this spec gives such a record no user scope (`minted_user` stays `false`, §3.2). Is that
enough, or should the server refuse to be the dialler for a peer that wants a user scope?

**Q15 — An unreadable `users/<u>/settings.toml`.** The server policy fails closed by turning the
feature off (§4.1). The per-user file can only tighten, so the analogous readings are "as if it said
nothing" (the server's `missing` applies, and the user's runs may land on the server's key, which is
what they had asked not to happen) or "as if it said `fail`" (the user's leased work stops until
they fix it).

**Doc drift noticed, not fixed here (ADI-MONO-174 covers it):** `docs/fleet.md` §11 says
`Secrets::resolve` "injects every global secret into every agent run's environment". Runs use the
attachment allowlist now, and only triggers get the whole set (§5). §8a describes the member gate
as covering "the global" secrets actions, but the code covers every scope.

---

## Checklist

Two prerequisites, then four phases. Each item ships with unit tests in the same file, as in
`docs/fleet.md`.

### Before phase 1

- [ ] P0.1 ADI-MONO-174: `set-oauth` and `refresh` on the member gate, and the `docs/fleet.md`
      §8a/§11 wording.
- [ ] P0.2 ADI-MONO-175: a CLI-spawned child no longer inherits its parent's attached secrets or
      `ADI_*` run identity.

### Phase 1 — plumbing only: policy, storage, the hold-key fix, the own-scope API and CLI, the random pairing id

Nothing in phase 1 lets a user store a value. Every piece is exercised through tests and an
in-memory lease source.

- [ ] P1.1 `adi-mesh`: `decide` mints `u-<26 base32>` where it used `PAIR_USER`, returned in
      `Accepted.username`. `NodeRecord.minted_user`, set only by `decide` for an id it minted. A
      re-pair keeps a minted id and replaces an `adi` one. Tests: two no-body pairings get distinct
      ids; a re-pair keeps the id and rotates the password; a pinned username and a `record_viewer`
      record have `minted_user = false`; a legacy record reads `false`.
- [ ] P1.2 `adi-app`: `user_scope(projects, sender)` beside `peer_role`, `Some(<u>)` only for a
      `minted_user` record whose `auth.user` matches the header. `None` for `adi`, a local caller,
      an unknown petname or an unreadable registry.
- [ ] P1.3 `adi-secrets`: `Scope::User`, `users/<u>/` paths, AAD `user:<u>/<NAME>`. `list_all`
      skips `users/`. A user ciphertext copied into global or another user's scope fails to open.
- [ ] P1.4 `secrets/settings.toml` `[user_secrets]` (`enabled`, `overridable`, `missing`,
      `max_lease`), read per launch and per turn. A corrupt file reads as `enabled = false`, logged
      and shown.
- [ ] P1.5 `users/<u>/settings.toml` with `missing` clamped stricter-only, `server < wait < fail`
      (settle Q1, Q15).
- [ ] P1.6 `SessionRecord::lease` (`LeaseRef`) and `sessions.lease_cap`, with their `ALTER TABLE`s.
      Written once at `create_as`, never re-derived. NULL reads as "server values only".
- [ ] P1.7 `run_agent` computes `LeaseRef` from `user_scope` only (settle Q4). Test: a body naming
      any user or lease field is ignored.
- [ ] P1.8 The layer: `spec_in` takes the session's resolved lease values and overlays them in
      `attached_secret_env` for `pinned ∩ attached ∩ overridable ∩ shared` only, under the agent's
      `[env]`. Exercised through an in-memory lease source.
- [ ] P1.9 The hold fix: per-turn qualified key `#user:<u>` when a user value supplied the
      credential. The same key in `RunSpec.credential`. Test: a user quota doesn't hold the
      server's login.
- [ ] P1.10 `ondemand::resolve_credential` uses the asker's live value for the Test button.
- [ ] P1.11 `GET /api/secrets/mine`, `POST /api/secrets/mine/{set,remove,settings}`,
      `POST /api/secrets/policy` (member-blocked). Never reveals. `set` refuses names outside
      `overridable`, callers without a user scope, and, in phase 1, everything (`501`).
- [ ] P1.12 CLI: `secrets policy` (local, and `--user --node`), `secrets rm --user --node`, and
      `secrets set --user --node`, which refuses until phase 2. No `--reveal` or `read` for `--user`.

### Phase 2 — device wraps, the per-connection setting, unlock, leases, revoke, waiting on unlock

- [ ] P2.1 The wrapped manifest: DEK and one `[[wraps]]` entry. `set` accepts wraps only, and stops
      refusing. Plaintext never reaches the server.
- [ ] P2.2 macOS device key in the Keychain (`adi-app`), WebAuthn PRF device key in
      `adi-mesh-client`. The unwrap runs in device-owned code only (settle Q12).
- [ ] P2.3 `adi-app`'s ephemeral session key, `/api/leases/{session,unlock,renew,revoke}` and
      `GET /api/leases`, the in-memory `Lease`, one per user, with zeroize. `max_lease` clamp.
      Restart answers "unknown epoch" (settle Q9, Q10).
- [ ] P2.4 The device's `connections.toml` in the `mesh` module dir (and its own object store in the
      browser client), moved by a rename and deleted by an unpair. The renew worker in the device's
      `adi-app`, TTL from last seen.
- [ ] P2.5 CLI: `mesh share`, `mesh unlock`, `mesh lock`, `secrets leases`.
- [ ] P2.6 Revoke (§10.1): drop the lease, void every `lease_cap` of `<u>`,
      `SessionStore::with_lease_user`, stop each live session through the runner stop without the
      agent-exists gate, `adi.secrets.lease.revoked`. Also on unpair, which deletes `users/<u>/`.
      Runs with no lease in the table. Tests: a running turn and a running child are both stopped; a
      parked session keeps its queue; a forwarded launch after the revoke gets `403`.
- [ ] P2.7 `missing` per turn: `server` / `wait` / `fail`. The unlock check runs before `dequeue` in
      `advance_queue` and before the start decision in `deliver_with`.
- [ ] P2.8 Waiting on unlock: queue-held turn, `RunLifecycle::Waiting`,
      `adi.agents.run.waiting_on_unlock`. Resume on `adi.secrets.lease.granted`. A fresh launch that
      waits (settle Q2).

### Phase 3 — subagent and turn inheritance of the lease

- [ ] P3.1 Capability: minted per leased session, SHA-256 in `sessions.lease_cap`, `ADI_LEASE` and
      `ADI_LEASE_URL` pushed after `ADI_RUN_ID`, stable across turns and app restarts, re-minted
      after a revoke (settle Q5).
- [ ] P3.2 `run_agent` honours `X-Adi-Lease` only without a `FleetSender`. It derives `launched_by`
      and the `can_spawn` caller from the capability's session and builds the child's `LeaseRef`
      from the child's own attachments. An unknown or voided capability is `403`.
- [ ] P3.3 `adi-mono agents run` forwards to `$ADI_LEASE_URL` when `ADI_LEASE` is set, and refuses
      when the app is unreachable (settle Q3).
- [ ] P3.4 Only `adi-app`'s `Agents` carries a lease source. Elsewhere a leased session's turn is
      queued, never started (`deliver_with` → `Sent::Queued`; `advance_queue` → `false` before
      dequeue).
- [ ] P3.5 A reply, from anyone, runs on the session's `lease.user`. Test: member B's reply into A's
      leased conversation gets A's value, and B's own lease is never consulted.
- [ ] P3.6 Tests: a grandchild of a leased run gets the user's value for a name it attaches; a child
      that doesn't attach the name gets nothing; a forged `ADI_RUN_ID` without the capability gets
      server values; an `env -u ADI_LEASE` launch inherits nothing (ADI-MONO-175's strip, pinned for
      the user layer, with `hash_child_environment` matching); expiry under a child parks the whole
      tree under `wait` and resumes it on renew; a revoke stops the whole tree; an await wake, a
      queued message and a failover re-ask of a leased session all run on the lease.

### Phase 4 — the panel for the settings

- [ ] P4.1 Server: the `[user_secrets]` policy on the Secrets page (enabled, overridable names,
      missing, max lease, and the parse error when there is one). Live leases listed by user
      (shown by petname), names and expiry.
- [ ] P4.2 "My keys" (own scope): names, last updated, remove, my `missing`. Never shows a value.
- [ ] P4.3 Device: the per-connection setting on the Mac's Fleet row for that server and in the
      browser client's node settings: share, lease (off / while connected / N hours / until
      revoked), auto-unlock, revoke now.
- [ ] P4.4 "Waiting for <petname> to unlock" in the rail, "stopped: key revoked" on revoked runs, and
      a user's held subscription shown anonymised to everyone but its owner.
- [ ] P4.5 The presets of §14, once the settings above have shipped.
