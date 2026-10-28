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

**The user is a server-minted id, never the username (decided 2026-10-28, ADI-MONO-179).** `<u>`
in this document is that id: `users/<u>/` secret storage, the AAD, `LeaseRef`, and the hold key
`#user:<u>` (§6) are all keyed on it, and it is what `launched_by = human:<u>` and
ADI-MONO-171's ownership checks already use (`asking_user`,
`adi-webapp-api/src/handlers/agents.rs:1384`). It is **immutable** once minted and **always
unique** on this server, and it is never derived from, or equal to, a username — see §3.2.

The **username** is the human label and the credential username a peer authenticates with: the
server's pairing record's `auth.user`, attached as `X-Adi-Fleet-User`
(`gateway.rs:816-819`). It is registry data, not something the peer sends with a request. An
invite may pin it (`InviteSpec.username`); unpinned, it defaults to the id minted for it, which is
what makes an unpinned peer its own user.

**A pinned username now counts as a user too.** The earlier v1 reading — that a pin got no user
scope at all, so the partner sandbox's several devices pinned to one `p<uid>` had none — is
withdrawn (settles §15 Q13, see §3.2). What "one peer is one user" meant before this decision —
that a lease belongs to exactly one device, so revoking it is revoking that device (§8) — now
has an exception: several peers pinned to the same username share one user, and so share one
lease once phase 2 ships it. That is unchanged work for phase 1 (nothing in it wraps a value to a
device yet) and an open question for phase 2, not answered here.

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

### 3.2 The change: separating the id from the username

**ADI-MONO-177 first (`decide` mints a random id where it used to write `PAIR_USER`); ADI-MONO-179
then separates that id from the username it is returned under.** Two things happen now, in order:

1. **The username is resolved.** `spec.username` — an invite's pin — if there is one; otherwise
   there is no username to resolve yet, because an unpinned peer's username is decided by the next
   step, not chosen ahead of it.
2. **The username is resolved to an id**, through `FleetRegistry::resolve_user` (`fleet.rs`): look
   it up in `self.users`; if it is already there — because another peer pinned the same name
   before — reuse that id; otherwise mint a fresh one and record the mapping. An unpinned peer has
   no username to look up, so this mints a fresh id and uses it as the username too — the `u-…` a
   person retypes into a browser prompt is both.

```
u-<26 characters, lowercase Crockford base32>      130 random bits, e.g. u-7f3kq9…
```

- **The form is chosen for where it goes.** It is a Basic-auth username (no `:`), a path segment
  under `secrets/users/` (it passes `validate_project`'s filesystem-name rule, `secret.rs:136-138`),
  part of an AAD, and the tail of `launched_by = human:<u>`. Lowercase base32 has no ambiguous
  glyphs, which matters because `mesh join` prints it for a person to retype into a browser prompt
  (`adi-cli/src/mesh.rs:417`).
- **Random, not derived.** Not from the peer's key, which would make the id the same on every server
  the peer pairs with and so link the user across fleets. Not from the petname or the username,
  either of which can be renamed or re-pinned without the id moving.
- **`FleetRegistry.users` is the username → id mapping**, kept beside `nodes` (`fleet.rs`), the only
  place a username is ever bound to an id. It is what lets several peers pinned to the same
  username — the partner sandbox's several devices, each pinned to one `p<uid>` — share the id the
  first one got, and what lets the id survive a re-pair or a later rename of the label untouched.
- **`NodeRecord` carries the id itself, not a flag.** The earlier `minted_user: bool`
  (ADI-MONO-177) is gone; `NodeRecord.user: Option<String>` (`fleet.rs:392`, a plain value, so it
  goes before `auth` per the field-order note) holds the id. `None` for a peer this server has
  never minted or pinned one for. `#[serde(default)]` reads every record filed before this field
  existed as `None` — including a record ADI-MONO-177 had already set `minted_user = true` on,
  which is a deliberate loss, not a migration bug: re-pairing is how such a record gets a user
  again, the same as a legacy `adi` record always has.
- **A user scope needs `NodeRecord.user`.** `adi-app`'s `user_scope(projects, sender)` (beside
  `peer_role`, `adi-app/src/main.rs:596`) loads the registry, finds the sender's petname, and
  answers `Some(user.clone())` — the **id**, never the username — only when `user` is `Some` and
  the header's user still matches `auth.user`. Anything else answers `None`, which means "no user
  layer: the server's values". On doubt it trusts the peer with less, as `peer_role` does when it
  defaults to `Member`.

That rule gives the decided answers directly:

| peer | `auth.user` (the username) | `NodeRecord.user` (the id) | user scope |
|---|---|---|---|
| paired after ADI-MONO-177, no username in the invite | `u-…`, minted by `decide` | the same `u-…` | yes |
| an invite that pins `username` | the pinned name | resolved by `resolve_user`: shared with any other record already pinned to it, else freshly minted | yes (ADI-MONO-179, settles §15 Q13 — was none in v1) |
| paired before ADI-MONO-177 | `adi` | `None` | **none until re-paired** |
| filed by `record_viewer`, because the server dialled the peer's invite | whatever the peer's side minted | `None` | none (§15 Q14) |
| a local caller | — | — | none: the owner, on global secrets |

**A re-pair keeps a record's id and username together, or resolves both together — never one
without the other.** A record that already carries a `user` keeps both `auth.user` and `user`
across a re-pair, rotating only the password: re-inviting a peer to recover a lost password must
not orphan the values stored under its id, and must not let a differently-pinned re-invite
quietly move which user it is. A record with no `user` yet — a legacy `adi` record, or one
`record_viewer` filed — resolves a username and an id exactly as a fresh pairing does, which is
how such a record gets a user at all.

**Unpairing ends the user, once nothing else needs it.** The id is never reissued, so unpairing
the **last** peer that carries a given id (`FleetRegistry::has_user`, checked before the delete)
revokes its lease (§10.1) and deletes `secrets/users/<u>/`
(`adi_secrets::Secrets::forget_user`, wired into `/api/fleet/unpair` and the two drop endpoints).
A second peer still sharing the id — the partner sandbox's several devices pinned to one
`p<uid>` — keeps the storage alive until it, too, is unpaired.

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
kind    = "device-key-v1"   # | "keychain-v1" | "webauthn-prf-v1" — later hardening, §15 Q12
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
- **The device key is a `0600` key file, on every platform (decided 2026-10-28, Igor, revising
  this section's original Keychain/WebAuthn plan for v1).** A random 256-bit key-wrapping key,
  persisted as 32 raw bytes at `~/.adi/mono/mesh/wrap.key` (`adi-mesh/src/devicekey.rs`) — beside
  [identity.key](adi-mesh/src/identity.rs), in the same module, but never the same file or key
  (previous bullet). `kind = "device-key-v1"` is the only wrap this build produces or opens. A
  macOS Keychain item and a browser `adi-mesh-client` WebAuthn PRF key are **later hardening**,
  not v1: §15 Q12 (whether the separate `adi-mono` binary can use a Keychain item `adi-app`
  created) is moot for exactly that reason — nothing here calls the Keychain at all, so there is
  no prompt to worry a second binary with.
- **The unwrap runs in device-owned code, never in the server's panel.** A node's panel is HTML and
  wasm *served by the node*, and a server can change a page it serves to keep the plaintext it was
  trusted to unwrap. So unlocking happens in the Mac's own `adi-app` (the auto-unlock case, §4.3) or
  the device's own `adi-mono mesh unlock` (the CLI verb, §4.4, ADI-MONO-180 P2.5 — the only one
  built so far; see that item's own note), and in the browser client's own shell — never inside the
  iframe rendering `app.<node>.n.adi`.

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
3  unwrap each DEK locally (the device key file, §8), open the value,
   seal it to the session key: X25519 + XChaCha20-Poly1305,
   AAD = user:<u>/<NAME> | epoch | device
4  POST /api/leases/unlock { lease, sealed[] } → checks AAD user == header user, names ⊆ overridable,
                                                 clamps lease to max_lease, opens, keeps a Lease
5  POST /api/leases/renew  every H seconds     → last_seen := now
6  POST /api/leases/revoke (mesh lock, unpair) → lease dropped, capabilities void, runs stopped (§10.1)
```

- **The routes are `/api/leases/*`, not `/api/mesh/*`.** `member_blocked` refuses a member every
  `/api/mesh*` path (`main.rs:626`), and unlocking is exactly what a member does.
- **"Re-encrypted to the server session" is read as: to an ephemeral key `adi-app` generates at
  boot and never writes down.** A restart makes every earlier unlock undecryptable on its own.
  **Settled by ADI-MONO-180 (§15 Q9):** exactly this reading, built as `adi_mesh::lease_seal` —
  `adi-app` mints an X25519 keypair once per boot (`SessionKey::generate`, never persisted) plus a
  random epoch token; `GET /api/leases/session` hands back the public key, the epoch, **and the
  caller's own minted id** — settling, in passing, how a device whose username is a *pinned* one
  (§3.2) ever learns the real id its AAD in step 3 has to name: it asks, right here, rather than
  being expected to know it already. Step 3's seal is a fresh ephemeral X25519 keypair *per call*
  (never reused), HKDF-SHA256 over the raw ECDH output, XChaCha20-Poly1305 with the stated AAD; the
  `device` component is the caller's own `EndpointId`, read off the gateway's pairing record on
  each side independently, never sent in the body.
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
  so `lease = 0` means *while this Mac is awake and online*. The renew loop is meant to be a
  background worker in the device's `adi-app`; **ADI-MONO-180 ships only the manual half** —
  `adi-mono mesh unlock <server>` does steps 1-4 by hand, and nothing yet calls step 5
  automatically or acts on `auto_unlock` on its own. The server side of every step (session,
  unlock, renew, revoke, the TTL math) is built and tested either way, so the worker itself —
  reading `connections.toml` for `auto_unlock`/`lease` and calling steps 1-5 on a timer from inside
  `adi-app` — is what's left, and it was left out because unattended, autonomous calls need the
  device's own credential available without a human retyping a password each cycle, which collides
  with §3.1's "neither side stores it" the moment nobody is at the keyboard; `adi-app` already has
  a precedent for holding a node password at the user's own request
  (`adi-app/src/viewer.rs`'s `HeldCredentials`, used for dashboard transfer's outbound calls) and
  reusing that mechanism here, rather than inventing a second one, is the shape the worker should
  take. In the browser `lease = 0` means *while a tab of the client is open*.
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
| **an agent run on your own device** | edit your device's `connections.toml` (readable and writable by that Unix user), or ask your local `adi-app` to unlock; **and, since the 2026-10-28 decision at §8, read the device key file directly** — it is a plain `0600` file, the same exposure the mesh identity key and the server's master key already have to a co-resident process | fetch a value the server no longer serves (`secrets rm --user`, unpair, a later rename of the AAD label) — the device key alone opens nothing without the wrapped blob the server still has to hand back |

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
is settled by this document, except Q1, Q4 and Q15 below, settled during ADI-MONO-177's phase 1
implementation and so written down here rather than left open underneath it. The three rounds of
decisions on ADI-MONO-173 settled the earlier list's questions on the default username, the revoke
verb's name, where the device setting lives, the strictness order, a corrupt policy, revoke
stopping in-flight turns, replies from someone else, phase 1 without wraps, enrolling a second
device, and the inherited environment (now ADI-MONO-175). Those are in the body, and the rest are
renumbered here.

**Q1 — `users/<u>/settings.toml` collides with a secret named `settings`.** `validate_name` accepts
lowercase (`secret.rs:124-133`). Options: refuse `settings` as an `overridable` name (a real env
var is almost never lowercase), or move the file out of the scope directory
(`users/<u>.settings.toml`, or `users/<u>/.settings.toml`, which `validate_name` rejects as a
secret name).

**Settled by ADI-MONO-177: the second option.** The file is `users/<u>/.settings.toml` — inside the
scope directory (so it moves and is deleted with the rest of `<u>`'s own files, by rename and
unpair alike), but under a name `validate_name` can never accept: the leading `.` is neither an
ASCII letter nor `_`, so no secret a user could ever `set --user` can land on this path, however the
operator's `overridable` list is written. That beats refusing `settings` as an `overridable` name,
which would depend on an operator remembering to keep it off the list rather than making the
collision impossible by construction.

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

**Settled by ADI-MONO-177: the first reading — §5's, as already written.** `run_agent` computes a
`LeaseRef` from the launch's user scope, the server's `[user_secrets]` policy, and the agent's own
`attached ∩ overridable` names alone; whether that user has ever stored a value for one of those
names never enters the computation, and phase 1 ships no way to check that even if it wanted to
(nothing in phase 1 lets a user store a value at all). A lease names what a conversation is
*allowed* to look for, not what it already has — which is the only reading under which `fail`
means "the server never pays for this member" rather than "unless they happen to have brought
nothing yet". See `lease_for` in `adi-webapp-api`.

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

**Settled by ADI-MONO-180: the ephemeral-boot-key reading, built as written.** `adi_mesh::lease_seal::SessionKey`
is an X25519 keypair generated in `App::leases`/`App::lease_session`
(`adi-app/src/main.rs`) once at startup and never serialized; a restart mints a new one and a new
epoch, which is exactly what makes "unknown epoch, re-unlock" (§9) a real answer rather than a
placeholder. The mesh connection's own keys (iroh/QUIC) are not reused for this — they authenticate
the *connection*, same reasoning as §8's "the device key is not the mesh identity key".

**Q10 — Numbers.** The renew interval `H`, the grace after the last renew for `lease = 0`, and
whether an absent `max_lease` really means "no cap".

**Settled by ADI-MONO-180.** `lease = 0`'s grace is 90 seconds (`CONNECTED_GRACE`,
`adi-app/src/leases.rs`) — long enough that a renew loop ticking every few seconds never races it,
short enough that a device that actually went away reads as gone within about a minute and a half.
`H` itself (how often the renew loop ticks) is left to the renewing side, as the body already says;
no number is fixed here because nothing calls it automatically yet (§9's renewing-side note). An
absent `max_lease` means no cap, confirmed by `LeaseLength::clamp(self, max: Option<LeaseLength>)`:
`None` is a no-op, `Some` always shortens — `Forever` becomes `max`, anything longer than `max` is
cut to it, anything shorter is untouched.

**Q11 — Codex and a user key.** `ambient_credential_env` assumes `OPENAI_API_KEY`
(`ondemand.rs:273`). The Codex runner knows `CODEX_API_KEY` and `CODEX_ACCESS_TOKEN`
(`runner/detached.rs:773-776`) and binds the host's `auth.json` login whenever one exists. Whether
a user's key actually takes precedence over the host's `codex login`, and whether user-keyed Codex
runs need their own `CODEX_HOME`, has to be shown before Codex is BYO.

**Q12 — The Keychain key and the `adi-mono` binary.** The device key is a Keychain item `adi-app`
creates (§8). On a Mac, the separate `adi-mono` binary asking for that item may get a prompt or a
refusal, so `secrets set --user` and `mesh unlock` from a terminal may have to go through the local
`adi-app` instead of opening the Keychain themselves. To be shown on a signed build before phase 2.

**Moot, by the decision at §8: there is no Keychain item.** The device key is a `0600` file
(`adi-mesh/src/devicekey.rs`) both `adi-mono` and `adi-app` read the same way any other store file
is read — no OS prompt, no per-binary entitlement, no cross-process ownership question. A Keychain
item (and the browser's WebAuthn PRF key) are later hardening, and whichever of them is built first
will have to answer this question for real; it is reopened then, not here.

**Q13 — An invite that pins `username`.** `InviteSpec.username` (`join.rs:265-267`) lets the minter
choose the username, and the partner-sandbox plan uses it to give one person (`p<uid>`) several
devices.

**Settled 2026-10-28 (ADI-MONO-179): honour it.** A pinned username now resolves to a user id
through `FleetRegistry::resolve_user` (§3.2) — shared with any other record already pinned to the
same name, rather than refused and rather than denied a user scope. That is exactly what the
partner sandbox needs: several devices pinned to one `p<uid>` share a user, and get BYO without
the sandbox changing how it mints invites. It does reopen the device-grouping question §3's "one
peer is one user" was meant to put off — see the note added there — but only for the pinned case,
and only once phase 2 has to decide what a shared lease means for several devices.

**Q14 — The server dialling a device's invite.** When the server spends an invite the *device*
minted, the server files the device through `record_viewer`, and the username is whatever the
device's side chose (`join.rs:740`). The design says the id is "minted at pairing and returned in
the join response", which describes the server minting. A device-chosen id could equal another
user's, so this spec gives such a record no user scope (`NodeRecord.user` stays `None`, §3.2). Is
that enough, or should the server refuse to be the dialler for a peer that wants a user scope?

**Q15 — An unreadable `users/<u>/settings.toml`.** The server policy fails closed by turning the
feature off (§4.1). The per-user file can only tighten, so the analogous readings are "as if it said
nothing" (the server's `missing` applies, and the user's runs may land on the server's key, which is
what they had asked not to happen) or "as if it said `fail`" (the user's leased work stops until
they fix it).

**Settled by ADI-MONO-177: "as if it said nothing", logged.** The per-user file can only ever
*tighten* the server's own `missing`, never loosen it — so an unreadable one defers to the
server's policy exactly the way a missing one does, rather than guessing at the strictest possible
reading on the user's behalf. The server's own policy still governs: a server on `fail` is
unaffected by this either way, and a server on `server` or `wait` is where the difference would
show. The cost is stated plainly, as it is for the server's own corrupt-file reading (§4.1): a typo in a
user's own policy file silently drops them back onto whatever the server already does, rather than
stopping their work outright. It is logged on every read so the condition is visible rather than
merely safe. See `UserSettings::load` in `adi-secrets`.

**Doc drift noticed, not fixed here (ADI-MONO-174 covers it):** `docs/fleet.md` §11 says
`Secrets::resolve` "injects every global secret into every agent run's environment". Runs use the
attachment allowlist now, and only triggers get the whole set (§5). §8a describes the member gate
as covering "the global" secrets actions, but the code covers every scope.

---

## Checklist

Two prerequisites, then four phases. Each item ships with unit tests in the same file, as in
`docs/fleet.md`.

### Before phase 1

- [x] P0.1 ADI-MONO-174: `set-oauth` and `refresh` on the member gate, and the `docs/fleet.md`
      §8a/§11 wording.
- [x] P0.2 ADI-MONO-175: a CLI-spawned child no longer inherits its parent's attached secrets or
      `ADI_*` run identity.

### Phase 1 — plumbing only: policy, storage, the hold-key fix, the own-scope API and CLI, the random pairing id

Nothing in phase 1 lets a user store a value. Every piece is exercised through tests and an
in-memory lease source.

- [x] P1.1 `adi-mesh`: `decide` mints `u-<26 base32>` where it used `PAIR_USER`, returned in
      `Accepted.username`. `NodeRecord.minted_user`, set only by `decide` for an id it minted. A
      re-pair keeps a minted id and replaces an `adi` one. Tests: two no-body pairings get distinct
      ids; a re-pair keeps the id and rotates the password; a pinned username and a `record_viewer`
      record have `minted_user = false`; a legacy record reads `false`.
      **Reworked by ADI-MONO-179 (§3.2):** the id and the username are now separate —
      `NodeRecord.user: Option<String>` replaces `minted_user: bool`, and `FleetRegistry.users`
      maps a username to the id `resolve_user` resolved or minted for it. A pinned username now
      gets a user id too, shared across every record pinned to the same name. Tests added: two
      peers pinned to the same username share one id; a petname rename never moves the id or the
      mapping; unpairing the last peer of a user deletes `secrets/users/<u>/`, unpairing one of two
      keeps it.
- [x] P1.2 `adi-app`: `user_scope(projects, sender)` beside `peer_role`, `Some(<u>)` only for a
      `minted_user` record whose `auth.user` matches the header. `None` for `adi`, a local caller,
      an unknown petname or an unreadable registry.
      **Reworked by ADI-MONO-179:** answers `record.user.clone()` — the id — gated on
      `record.auth.user == header.user`, not on `minted_user`, which no longer exists.
- [x] P1.3 `adi-secrets`: `Scope::User`, `users/<u>/` paths, AAD `user:<u>/<NAME>`. `list_all`
      skips `users/`. A user ciphertext copied into global or another user's scope fails to open.
- [x] P1.4 `secrets/settings.toml` `[user_secrets]` (`enabled`, `overridable`, `missing`,
      `max_lease`), read per launch and per turn. A corrupt file reads as `enabled = false`, logged
      and shown.
- [x] P1.5 `users/<u>/settings.toml` with `missing` clamped stricter-only, `server < wait < fail`
      (settle Q1, Q15).
- [x] P1.6 `SessionRecord::lease` (`LeaseRef`) and `sessions.lease_cap`, with their `ALTER TABLE`s.
      Written once at `create_as`, never re-derived. NULL reads as "server values only".
- [x] P1.7 `run_agent` computes `LeaseRef` from `user_scope` only (settle Q4). Test: a body naming
      any user or lease field is ignored.
- [x] P1.8 The layer: `spec_in` takes the session's resolved lease values and overlays them in
      `attached_secret_env` for `pinned ∩ attached ∩ overridable ∩ shared` only, under the agent's
      `[env]`. Exercised through an in-memory lease source — `lease::LeaseValues`, an
      object-safe trait installed on an `Agents` with `Agents::with_lease_values`; every `Agents`
      opened in production carries `None`, so the layer is a no-op everywhere until phase 2 gives
      `adi-app` a real one. "Shared" (the device's `connections.toml`) doesn't exist yet either;
      a `LeaseValues::value` answering `Some` already implies it, since that is phase 2's own
      `Lease.values`' job to guarantee.
- [x] P1.9 The hold fix: per-turn qualified key `#user:<u>` when a user value supplied the
      credential. The same key in `RunSpec.credential`. Test: a user quota doesn't hold the
      server's login. `ResolvedBackend::credential_env_name` names the one env var that decides a
      row's credential (`None` for a settings-file backend); `hold_key_for`/`login_key_for` take
      the qualifying user. Only the *current* row's key is qualified — a candidate row `fail_over`
      is weighing a *switch to* is checked unqualified, because whether that row's own next turn
      would run on a user's value is that turn's own `spec_in` to decide, not this one's guess.
- [x] P1.10 `ondemand::resolve_credential` uses the asker's live value for the Test button.
      `ondemand::Asker { user, live }`; `resolve_credential` tries it before the secret/ambient
      fallback for both `api_key_env` and ambient-login backends, never for a settings file (§5
      already rules those out). No `missing` handling here — a Test is a one-shot ask, not a
      queued turn, so "no live value" always and only falls through to the server's own credential,
      which is `missing = server`'s own answer; `wait`/`fail` are phase 2's turns to park or
      refuse, not a button's.
- [x] P1.11 `GET /api/secrets/mine`, `POST /api/secrets/mine/{set,remove,settings}`,
      `POST /api/secrets/policy` (member-blocked). Never reveals. `set` refuses names outside
      `overridable`, callers without a user scope, and, in phase 1, everything (`501`) — in that
      order, so the reason given is always the first one that actually applies. `adi-secrets`
      gained `list_mine`/`remove_mine` (`Scope::User`, metadata only, never `list_all`). Decision:
      `GET /api/secrets/mine` is deliberately **not** in `adi-app`'s `SHARED_GETS` coalescing list
      — that key is `method + path + body` with no identity in it, so sharing it would leak one
      user's own names into another user's concurrent poll of the same empty-body route.
- [x] P1.12 CLI: `secrets policy` (local, and `--user --node`), `secrets rm --user --node`, and
      `secrets set --user --node`, which refuses until phase 2. No `--reveal` or `read` for
      `--user`. `--node` dials `127.0.0.1:<gateway port>` with `Host: app.<node>.n.adi` (§4.4's own
      addressing), `reqwest::blocking` + `Authorization: Basic`. The username is this device's own
      `FleetRegistry` record for that petname (`auth.user`, stored in the clear); the password is
      never stored anywhere after pairing (§3.1), so it is prompted for on stderr every call — a
      stored-and-reused credential (the pattern `adi-app`'s own Fleet Dashboards page uses) was
      deliberately not reached for, to avoid duplicating that private cache across a crate
      boundary. `set --user` never makes a request at all: phase 1's answer is fixed, so the CLI
      gives it locally.

### Phase 2 — device wraps, the per-connection setting, unlock, leases, revoke, waiting on unlock

- [x] P2.1 The wrapped manifest: DEK and one `[[wraps]]` entry. `set` accepts wraps only, and stops
      refusing. Plaintext never reaches the server.
      **ADI-MONO-180:** `adi_secrets::Wrap` + `Manifest.wraps` (empty for every global/project
      secret, which still goes through the master key as always); `adi_secrets::wrap` is the pure
      DEK crypto (`generate_dek`/`seal_value`/`open_value`/`wrap_dek`/`unwrap_dek`), independent of
      the master key and callable by device-owned code with no store of its own.
      `Secrets::set_wrapped` is the only writer of a `Scope::User` secret, and never touches the
      master key. `POST /api/secrets/mine/set` stores exactly what it is given — `wraps.len() != 1`
      is the one shape check left (§8: "one DEK per secret, one wrap in v1") — and `GET
      /api/secrets/mine` now also returns the sealed `nonce`/`ciphertext`/`wraps` (still never a
      plaintext) for the owning user's own unlock path (§9 step 2).
- [x] P2.2 macOS device key in the Keychain (`adi-app`), WebAuthn PRF device key in
      `adi-mesh-client`. The unwrap runs in device-owned code only (settle Q12).
      **Reworked by the 2026-10-28 decision at §8: a `0600` key file instead, on every platform.**
      `adi_mesh::devicekey` (`wrap.key`, beside `identity.key`, never the same key). The Keychain
      item and the browser's WebAuthn PRF key are later hardening, not built; Q12 is moot under
      this decision (§15).
- [x] P2.3 `adi-app`'s ephemeral session key, `/api/leases/{session,unlock,renew,revoke}` and
      `GET /api/leases`, the in-memory `Lease`, one per user, with zeroize. `max_lease` clamp.
      Restart answers "unknown epoch" (settle Q9, Q10).
      **ADI-MONO-180:** `adi_mesh::lease_seal::SessionKey` (X25519 + HKDF-SHA256 +
      XChaCha20-Poly1305, fresh ephemeral key per *unlock call* on the device side, one per *boot*
      on the server side) and `adi-app/src/leases.rs`'s `LeaseTable`. `GET /api/leases/session`
      also returns the caller's own id, settling how a device under a *pinned* username — whose
      `Accepted.username` the join protocol never equalled the real id for (§3.2) — learns what to
      put in its own AAD (§9's note on this). `unlock` opens each sealed value, drops any name
      outside `overridable`, clamps the lease, and replaces the user's prior lease in place; `renew`
      evicts rather than revives a lease that has already run out its grace; `revoke` always
      succeeds, even against nothing. `GET /api/leases` is admin-sees-all / member-sees-own, by
      `peer_role`, matching §7.
- [x] P2.4 The device's `connections.toml` in the `mesh` module dir (and its own object store in the
      browser client), moved by a rename and deleted by an unpair. The renew worker in the device's
      `adi-app`, TTL from last seen.
      **ADI-MONO-180, partially.** `adi_mesh::connections` (file, schema, `LeaseLength` parse/
      display/clamp) ships, and both couplings ship too: `adi-cli`'s local `mesh rename`/`unpair`
      and `adi-webapp-api`'s `fleet_rename`/`fleet_unpair` each move or delete the matching
      `connections.toml` entry right after the registry mutation that makes it stale. **The renew
      worker itself — reading `auto_unlock`/`lease` off this file and driving steps 1-5 on a timer
      inside `adi-app` — is not built**; see §9's own note on the renewing side for why (an
      unattended loop needs a credential available without a human retyping a password, which
      `viewer.rs`'s `HeldCredentials` precedent is the shape to reuse, not yet done) and what's left
      to do it. The browser client's own object store is untouched (`adi-mesh-client` has no part
      of this yet).
- [x] P2.5 CLI: `mesh share`, `mesh unlock`, `mesh lock`, `secrets leases`.
      **ADI-MONO-180, with one gap flagged rather than guessed at.** All four ship: `mesh share`
      edits `connections.toml` only; `mesh unlock` does §9 steps 1-4 by hand (one password prompt
      per invocation, reused across all three HTTP calls it makes — `adi_mesh::node_api`, lifted
      out of `adi-cli/src/secrets.rs` where P1.12 left it, now shared by both); `mesh lock` calls
      `/api/leases/revoke`; `secrets leases --node <server>` lists the caller's own. **`secrets
      leases` with no `--node` — the local admin's "every lease on my own server" listing — is
      refused rather than guessed at**: leases live only in a *running* `adi-app`'s memory, and this
      CLI has no established way to reach a local running instance's HTTP API the way it reaches a
      *remote* paired one (through the mesh gateway); that address is left for whoever builds it,
      rather than hard-coding a default port that `PORT`/an installed service could already disagree
      with. `secrets set --user --node` now actually seals and uploads (P2.1/P2.2), closing P1.12's
      own placeholder.
- [ ] P2.6 Revoke (§10.1): drop the lease, void every `lease_cap` of `<u>`,
      `SessionStore::with_lease_user`, stop each live session through the runner stop without the
      agent-exists gate, `adi.secrets.lease.revoked`. Also on unpair, which deletes `users/<u>/`.
      Runs with no lease in the table. Tests: a running turn and a running child are both stopped; a
      parked session keeps its queue; a forwarded launch after the revoke gets `403`.
      **Step 1 already lands with P2.3**: `POST /api/leases/revoke` drops the lease from the table
      and zeroes its values, unconditionally (even against nothing — the §10.1 requirement this
      item's own text names). ADI-MONO-181 only has to add steps 2-5: voiding `lease_cap` (which
      nothing mints yet either — that's phase 3's `ADI_LEASE`, P3.1), `with_lease_user`, and the
      actual runner stop.
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
