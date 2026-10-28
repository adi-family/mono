# Bring your own credentials — spec

A user brings their own LLM credential to a server, and everything they start there runs on their
subscription. Read this before touching `crates/adi-secrets`, the credential path in
`crates/adi-agents`, or `crates/adi-llm-gateway`.

Status: **design, not built.** The design was agreed with the operator on 2026-10-28 and is recorded
in task ADI-MONO-173. This document turns it into a spec against the code as it stands on `main`
(63ebf74). Wherever the code contradicts the design, or the design leaves something undecided, the
point is listed under §16 "Open questions" and **not** settled here. The body is written so that
either answer to an open question still fits. Line numbers are as of that commit.

## 1. The problem

A server pays for every model call made on it. The credential an agent runs on is a secret an
operator set, `CLAUDE_CODE_OAUTH_TOKEN` in the global scope, say, and an agent's `[[secrets]]` row
attaches it. That is fine on a machine one person uses. It stops being fine once a fleet puts
other people on it (`docs/fleet.md` §8a, the member role) and they start running agents there:

- **The operator pays for strangers.** On the partner sandbox, which is ADI-MONO-171's reason for
  existing, every partner's run spends ADI's Anthropic workspace key. The sandbox plan's answer is
  a hard spend cap and a key "any partner can read". That bounds the bill. It doesn't attribute it,
  and it can't let a partner who has their own subscription use it.
- **A credential in a run's environment belongs to every reader of that run.** An agent can
  `echo $CLAUDE_CODE_OAUTH_TOKEN`. The output lands in a transcript, and §8a lets every peer read
  every transcript. On Linux the same value can also be read from `/proc/<pid>/environ` by any
  other process running under the same Unix user (checked on this host: readable, while
  `/proc/<pid>/mem` is refused under yama `ptrace_scope = 1`). On a shared box every agent runs as
  that one user.
- **A quota is recorded against the server's login, for everyone.** Holds are keyed on the
  credential's *name* (§6). If a user's own token tripped a quota under the name the server's
  token uses, the server's login would be held for every agent on the machine.

## 2. The principle

**A user overrides a secret. They don't get a backend.**

The server keeps `llm/backends/*.toml` exactly as `docs/llm-backends.md` defines them: one flat
object per way of answering a turn, and a chain on each agent. What a user brings is the *value*
behind an env-var name the backend already reads, such as `CLAUDE_CODE_OAUTH_TOKEN`,
`ANTHROPIC_API_KEY` or `OPENAI_API_KEY`. When their run resolves that name, their value wins.
Nothing about models, chains, failover or limit rules changes per user, and no backend learns a
user's name.

**A credential is held where it is spent, and only for as long as its owner allows.** A user's
secret is wrapped to keys only their own devices hold (§9). The server sees plaintext only through
a *lease*: the device unlocks the secret over the mesh, and the gateway holds it in memory and
injects it into outbound requests (§10–§12). The run never sees the key. Its environment carries a
per-session token and a base URL. The lease lasts as long as the user picked for this server, in
their device's settings for that connection (§11).

## 3. Who pays: `paid_by`

Every session records who pays for it, separately from who launched it.

```
SessionRecord.launched_by   who asked        human | human:<u> | agent:<name> | automation | ""
SessionRecord.paid_by       whose credential user:<u> | server
```

They are separate because they answer different questions and diverge in ordinary use. An agent
that a person's conversation spawned is `launched_by = agent:<name>`, but the person pays for it. A
trigger is `automation`, but its creator pays.

**`paid_by` is its own column, never part of `RunOverrides`.** `overrides.rs:34` says what an
override is: "a *setting* … and nothing is a capability". Spending somebody's subscription is a
capability. The column is added the way `launched_by` was (`store/db.rs:193`,
`ALTER TABLE sessions ADD COLUMN paid_by TEXT NOT NULL DEFAULT ''`). It sits on `SessionRecord`
beside `launched_by` (`store/record.rs:80`). Empty means `server`, which is true of every session
before this existed.

**It is decided once, at session creation, and never re-derived.** That is the same reason
`overrides` (`record.rs:87`) and `chain` (`record.rs:100`) are pinned. Every later turn of a
conversation is paid for by whoever its first turn was.

| how the session starts | `paid_by` | read from |
|---|---|---|
| a person, through the panel over the mesh | `user:<u>` | the gateway-attached identity headers (§4). **Never the request body** |
| a person, locally (panel on loopback, or a terminal) | `server` | no mesh sender, so this is the machine's own operator |
| an agent's run, through a tool on its `PATH` | the parent session's `paid_by` | the parent session, looked up by `awaits::caller()`'s `(agent, conv)` (`awaits.rs:689`) |
| a later turn: reply, queued message, await wake, goal nudge | the session's own `paid_by` | the record |
| a trigger, schedule or workflow step | its creator's payer; `server` for a system one | the definition's `created_by` (missing today, see §16 Q5) |
| a channel message | the linked ADI user, else `server` | the channel connection (no such link today, see §16 Q5) |
| task dispatch (`dispatch.rs:295`) | not named by the design | §16 Q5 |

Three properties the code has to be changed to hold:

- **The body never names a payer.** `run_agent` takes `launched_by` from the request body
  (`adi-webapp-api/src/handlers/agents.rs:347`). `attribute_human` (`:1394`) enriches only the bare
  word `human` and passes anything else through untouched. So today a peer can send
  `"launched_by": "human:someone-else"` and the store records it. That is harmless for a listing
  filter and fatal for billing. `paid_by` is computed from `FleetSender` alone, and
  `LaunchOptions` gains a `paid_by` that no HTTP handler fills from JSON.
- **An agent launch inherits; it does not choose.** The CLI attributes a launch from inside a run
  by reading `ADI_AGENT` and `ADI_RUN_ID` (`launcher::by_caller`, `launcher.rs:46`, via
  `adi-cli/src/agents.rs:1442`). The parent's `paid_by` is read off the parent session the
  `(agent, conv)` pair names. The parent must exist and be live. If it doesn't, the payer is
  `server` under `byob = allowed` and the launch is refused under `byob = required` (§14). On a
  single-Unix-user box this is attribution rather than authentication, because a run can set those
  variables itself (§15).
- **Usage events don't carry it.** `adi-usage/src/dispatch.rs:268` already reports the *kind* of
  `launched_by` and not the name. `paid_by` names a person, so it never goes into a usage event.

## 4. Identity: which name is the user

The payer's identity comes from the headers the mesh gateway attaches on the node side
(`adi-mesh/src/gateway.rs:808-820`). Any copy the caller sent is stripped first, and the gateway's
own are added only after both of §5's gates pass. `adi-app` reads them in `fleet_sender`
(`adi-app/src/main.rs:582`). That trust holds for the panel because `adi-app` serves exactly one
request per connection (`adi-webapp-api/src/http.rs:2`, `Connection: close`): the headers are
attached to the first head on a stream, and on the panel there is no second head. Anything that
moves an identity-bearing endpoint behind a keep-alive server has to re-check this.

**Never `X-Adi-Fleet-Node`.** That is the peer's self-declared nickname, and `docs/fleet.md` §2 rule
3 lets a clash rename it.

Which of the two remaining headers names *the user* is the design's own open seam. The design says
"fleet registry user via `X-Adi-Fleet-Petname`". The code complicates that:

- **A petname names a device, not a person.** It is one `NodeRecord` in `fleet.toml`, one key, one
  pairing. §9 needs "one wrap per device", so one user has several petnames. The partner-sandbox
  plan pairs every device as `p<uid>-<4hex>`.
- **The credential username is what groups a person's devices**, and it is what `launched_by =
  human:<u>` and ADI-MONO-171's ownership checks already use (`asking_user`, `agents.rs:1384`;
  `member_run_refusal`, `:2238`). It is registry data (`record.auth.user`, attached at
  `gateway.rs:817`), not the peer's say-so. But it defaults to `PAIR_USER = "adi"` for every
  pairing (`adi-mesh/src/join.rs:112`), whose doc says it "carries no security weight". Two members
  paired without an explicit `username` are, by this name, one person.

This spec writes `<u>` for the user throughout, and uses it in paths, AAD strings and hold keys.
§16 Q1 is the decision of what `<u>` is.

## 5. The user scope and the third layer

### Storage

```
~/.adi/mono/secrets/
  global/<NAME>.toml                     today
  projects/<project-id>/<NAME>.toml      today
  users/<u>/<NAME>.toml                  new
```

The AAD binds a ciphertext to its location (`adi-secrets/src/lib.rs:481`), so a value copied into
another scope won't decrypt. A user secret's AAD is `user:<u>/<NAME>`. It can't collide with
`global/<NAME>` or `projects/<id>/<NAME>`, because neither of those starts with `user:`.

**The scope is a real third variant, not a reserved project id.** `adi-app` keeps node passwords
in a pseudo-project called `fleet-nodes` (`adi-app/src/viewer.rs:27`). That works because nothing
resolves that scope into a run. A user scope is the opposite: resolving it into runs is its whole
purpose, and `<u>` must never be mistaken for a project id. `validate_project` (`secret.rs:138`)
would accept a username as a project id, and `resolve(Some(id))` would then inject it as a
project. So `adi-secrets` gains `enum Scope { Global, Project(&str), User(&str) }`. The existing
`project: Option<&str>` methods become thin wrappers over it, and `manifest_file` / `scope_dir` /
`ensure_scope_dir` (`lib.rs:438-476`) gain the `users/` arm. `list_all` does **not** walk `users/`:
it is what the panel's global secrets page lists, and a user scope is write-only (§7).

### Where the layer goes: not where the design says

The design adds the third layer to `Secrets::resolve` (`lib.rs:398`). **On `main` that function is
not the agent run's path.** It is called by triggers: `Triggers::run_env`
(`adi-triggers/src/lib.rs:287`) puts every global and project secret into a trigger's shell. It is
also called by the marketplace (`bundle.rs:1448`) to check whether a secret is set. Agent runs
stopped using it when attachments became an allowlist. `spec_in` (`adi-agents/src/lib.rs:2410`)
calls `attached_secret_env` (`lib.rs:3776`), which decrypts exactly the `[[secrets]]` rows the agent
names, globals first and projects over them. `docs/fleet.md` §11's line that `Secrets::resolve`
"injects every global secret into every agent run's environment" predates that change.

Adding a user layer to `resolve` as it stands would therefore put a user's key into every trigger's
environment, which is the opposite of the design's intent. The layer belongs in the two places a
*run's credential* is actually resolved:

1. **`attached_secret_env`.** For each attached name, a `users/<payer>/<NAME>` secret, when the
   session's payer is a user and has one, overrides the global/project value. That is the design's
   "global → project → user, later wins by env-var name", applied to the allowlist. It never adds a
   name the agent didn't attach: a user can't push an arbitrary variable into somebody's agent.
2. **`ondemand::resolve_credential`** (`llm/ondemand.rs:292`). This is the panel's **Test** button,
   which today reaches for the *global* secret of the runtime's ambient env var (`:268`). A test
   asked by a member tests *their* credential when they have one. Under `byob = required` it never
   falls back to the global one (§14).

Whether `Secrets::resolve` should grow the layer too, for a user's own triggers in `always` mode
(§8), is §16 Q2.

**What the layer yields depends on the phase.** In phase 2 it yields the decrypted value into the
run's environment, exactly as a global attachment does today. That carries every exposure §1
lists, so phase 2 user secrets are effectively `always` mode (§16 Q12). From phase 3 it yields a
**lease reference**, not a value. The env var the run receives holds the per-session gateway token,
and the base URL points at the gateway (§12).

**Settings-file backends are out of reach of the layer.** A backend that names `settings =
"~/.claude/settings.glm.json"` carries its credential in a file, and its hold key is that file
(`backend.rs:268`). The layer overrides env vars by name. It doesn't rewrite files. Whether a
user's env value should beat a settings file's own `env` block is §16 Q11.

## 6. Holds: the key fix

`LlmBackendManifest::credential()` (`llm/backend.rs:266`) is "who is paying": the settings file,
else `provider|base_url|api_key_env`, else `runtime:<runtime>`. Every `harness:claude-sdk` backend
naming nothing is `runtime:harness:claude-sdk`. A hold is `(credential, model | *)`
(`llm/holds.rs:84`), shared by every agent on the machine, which is the whole point of
`docs/llm-backends.md` §Holds.

With a user layer that point turns into a bug. Igor's token hits a five-hour cap, the turn is
classified `quota`, and the hold lands on `runtime:harness:claude-sdk`. Now every agent on the
server skips the server's login until Igor's quota resets.

**The rule: when the user layer supplied the credential for a turn, the hold key carries the user.**

```
server credential   runtime:harness:claude-sdk                 claude-opus-5
user credential     runtime:harness:claude-sdk#user:<u>        claude-opus-5
```

The design leaves the encoding open. `#user:<u>` is shown because `#` appears in no `credential()`
form today, and `holds.db` stores the key as text, so no schema change is needed.

**Computed per turn, not pinned.** `ResolvedBackend.credential` is set by `resolve_row`
(`llm/chain.rs:408`) and pinned into the session's chain on its first turn. `hold_key()`
(`chain.rs:109`) and `RunSpec.credential` (set at `lib.rs:1386` and `:2206`) both read it from
there. Whether a given turn ran on the user's value or the server's can change mid-conversation:
a lease expires and fallback is allowed (§13), or the user removes their secret. So the pinned
field stays the backend's own credential, and the qualified key is computed for each turn from
`(pinned row, which layer supplied it)`. The same qualified key goes into `RunSpec.credential`. A
Codex thread created on a user's key is then never resumed on the server's login, or the other way
round. `codex_credential` (`runner/detached.rs:668`) already refuses exactly that between two logins.

**The prober can't reach a user's hold, and that's acceptable for now.** `prober::check`
(`llm/prober.rs:160`) looks for a backend whose `credential()` equals the hold's. A qualified key
matches none, so the hold is reported `Unreachable` and waits out its own deadline. That is the
same honest outcome every vendor-CLI hold already gets. A probe is a billed request, so probing a
user's hold would bill the user, and in phase 3 it would need their lease. §16 Q13.

**The panel's hold list shows a qualified key as "your subscription", not as the username**, to
anyone but its owner. "Igor's quota is spent until 18:40" isn't the operator's to publish to
every member.

## 7. The own-scope API: write-only

A user may **set, replace and remove** secrets in their own scope, and list their own names and
metadata. No API ever reveals a user secret's value. That includes an admin, and the user
themselves. In `leased` mode the server couldn't reveal it anyway (§9). In `always` mode it could,
and the API must still refuse.

```
GET  /api/secrets/mine                 → [{ name, description, mode, devices, updated_at }]
POST /api/secrets/mine/set             { name, mode, description?, value? | wraps? }
POST /api/secrets/mine/remove          { name }
```

- **`<u>` comes from the gateway's identity headers, never from the body.** There is no `user`
  field to send. A local caller (no `FleetSender`) has no user scope: the machine's operator *is*
  the server, and their secrets are global.
- **Members may reach these, and only these.** `member_blocked` (`adi-app/src/main.rs:622`) refuses
  a member on `/api/secrets/{reveal,set,remove}` regardless of project. The `/mine` routes are a
  separate prefix, so they need no exception carved into that list. `docs/fleet.md` §8a's
  description of the gate ("the global `/api/secrets/{reveal,set,remove}`") understates it: the
  code refuses every scope.
- **`GET /api/secrets` stops showing user scopes to anyone.** It is `list_all`, which won't walk
  `users/` (§5).
- **`set` takes a value or wraps, by mode.** An `always` secret is sent as a value and sealed under
  the master key, the same as today's `set`. A `leased` secret is sent already wrapped by the
  device (§9), so plaintext never reaches the server on the way in either.

**Two gaps in today's member gate that this work touches, found while reading it.**
`POST /api/secrets/set-oauth` (`main.rs:835`) and `POST /api/secrets/refresh` (`main.rs:464`)
aren't in `member_blocked`'s list, so a member can overwrite a *global* OAuth secret today. That
includes the very `CLAUDE_CODE_OAUTH_TOKEN` this spec is about. §16 Q14.

### The CLI over the mesh

`adi-mono secrets` acts on the local store only. It goes through the `adi-core` facade
(`adi-cli/src/secrets.rs`), so there is no way to set a secret on another machine from a shell.
Phase 2 adds:

```sh
adi-mono secrets set CLAUDE_CODE_OAUTH_TOKEN --user --node <petname>      # value from stdin
adi-mono secrets rm  CLAUDE_CODE_OAUTH_TOKEN --user --node <petname>
adi-mono secrets list --user --node <petname>
```

These call the node's `/api/secrets/mine/*` through the local mesh gateway, addressed exactly the
way a dashboard transfer is (`docs/fleet.md` §10): `127.0.0.1:<gateway port>` with
`Host: app.<node>.n.adi`, so the system resolver and the front door aren't on the path. The node's
password comes from the credential this machine already keeps (`docs/fleet.md` §11). `--user`
without `--node` is refused: there is no user scope on your own machine. There is deliberately no
`secrets get --reveal --user` and no `secrets read --user`. `secrets read` is documented as "the
value-returning primitive an agent uses" (`adi-cli/src/secrets.rs:55`), and a user scope must never
be readable that way.

## 8. Secret modes

Each user secret carries one mode, stored in its manifest:

| mode | where the plaintext can come from | when to use it | phase |
|---|---|---|---|
| **`leased`** (default) | only a live lease (§10), held in the gateway's memory | everything a person starts | 3 |
| **`always`** | the server's master key, today's crypto (`crypto.rs`) with AAD `user:<u>/<NAME>` | only the user's own unattended triggers, with a provider-scoped key | 2 |
| **`custodian`** | an external unlock service, on a signed grant | the partner sandbox, where nobody trusts the box | 5 |

**`always` is the server holding your key.** The master key is a `0600` file beside the ciphertext
(`crypto.rs:8-11`), owned by the Unix user that every agent run on the box also runs as. This
host's is `-rw------- adi adi`. So an `always` secret is readable by any agent run on the server,
whoever started it, and by any member who can get an agent to read it. The mode exists because an
unattended trigger has no device online to unlock anything. The panel says plainly what it costs,
and recommends a key scoped to the job: an Anthropic workspace key with a spend limit, or an OpenAI
project key.

**`custodian` is outlined here, not specified.** The secret is encrypted to a custodian (a
Cloudflare Worker, AWS KMS) rather than to the user's devices. The user's device signs a *grant*
ahead of time: `{ user, secret, server key, purpose, not_after, budget? }`. When a lease is needed
and no device is online, the gateway presents the grant. The custodian checks it against its own
policy and revocation list, and answers with the plaintext sealed to the gateway's session key
(§10). What this buys over `leased` is a party outside the server that can refuse, rate-limit,
audit and revoke each release. That is the only mode that bounds what a hostile server operator can
do (§15). Phase 5 specifies it.

## 9. Device wraps

A `leased` secret is stored on the server in a form only the user's devices can open:

```toml
# secrets/users/<u>/CLAUDE_CODE_OAUTH_TOKEN.toml
mode        = "leased"
description = "my Max subscription"
nonce       = "…"           # value sealed under a fresh per-secret data key (DEK),
ciphertext  = "…"           # XChaCha20-Poly1305, AAD "user:<u>/CLAUDE_CODE_OAUTH_TOKEN"
created_at  = 1793000000
updated_at  = 1793000000

[[wraps]]                   # one per device; the DEK, sealed to that device's key
device  = "<petname>"
kind    = "keychain-v1"     # | "webauthn-prf-v1"
nonce   = "…"
wrapped = "…"
```

- **One DEK per secret, one wrap per device.** Adding or losing a device touches only the wraps.
  Re-setting the value mints a new DEK and new wraps.
- **The device key is not the mesh identity key.** The mesh key authenticates a connection. It sits
  as a raw file at `~/.adi/mono/mesh/identity.key` (`adi-mesh/src/identity.rs:19`) and in browser
  IndexedDB (`adi-mesh-client/src/store.rs`), both readable by anything running as that user.
  Reusing it would make "can pair as you" and "can read your subscription" the same capability.
- **macOS: a key in the Keychain.** A random 256-bit key-wrapping key stored as a Keychain item with
  device-only accessibility. **No macOS Keychain code exists in the tree today.** Only
  `apps/ios/AdiFleet/Keychain.swift` does, so phase 3 adds it to `adi-app`, which is the "device" on
  a Mac.
- **Browser client: WebAuthn PRF.** A passkey with the `prf` extension, its output put through HKDF
  into the key-wrapping key. Nothing of the kind exists in `crates/adi-mesh-client` today. The PRF
  output is bound to the relying party, which is the client's own origin.
- **The unwrap UI is device-owned code, never the server's panel.** A node's panel is HTML and wasm
  *served by the node*. A page the server serves can be changed by the server to keep the
  plaintext it was trusted to unwrap. So unlocking happens in the Mac's own `adi-app` and in the
  browser client's own shell, never inside the iframe rendering `app.<node>.n.adi`. The panel's
  "My tokens" page (phase 4) can list, remove and change modes, but setting a `leased` value and
  unwrapping one belong to the device.
- **Enrolling a second device** needs the DEK, which only an already-enrolled device can unwrap.
  How that works is §16 Q10.

## 10. Unlock and leases over the mesh

**Whoever has no open port dials, and here that is always the device** (`docs/fleet.md` §8).
Nothing can dial a browser, and a Mac behind NAT is no better. So every step of the unlock is a
request *from* the device *to* the server's panel, over the connection the fleet already has.
"Seen online" can only ever mean "the device asked something recently".

```
device                                        server: adi-app  →  gateway (in memory)
──────                                        ────────────────────────────────────────
1  GET  /api/byo/session                      → gateway's session public key + epoch
2  GET  /api/secrets/mine                     → this device's wraps
3  unwrap each DEK locally (Keychain / PRF), open the value,
   seal it to the session key: X25519 + XChaCha20-Poly1305,
   AAD = user:<u>/<NAME> | epoch | device
4  POST /api/byo/unlock { policy, sealed[] }  → adi-app checks AAD user == header user,
                                                forwards sealed[] untouched;
                                                gateway opens and keeps a Lease
5  POST /api/byo/renew   every H seconds      → last_seen := now
6  POST /api/byo/revoke  (or unpair)          → lease dropped immediately
```

- **"Re-encrypted to the server session" is read here as: to an ephemeral key the gateway generates
  at boot and never writes down.** `adi-app` only relays ciphertext, so neither its memory nor its
  logs ever hold plaintext. A gateway restart makes every earlier unlock undecryptable on its own.
  This is this spec's reading of the design's phrase (§16 Q19).
- **QUIC already encrypts the hop.** Over the mesh, `adi/mesh/http/1` runs inside a QUIC connection
  between the device's key and the node's key, and the relay sees ciphertext (`docs/fleet.md` §9).
  The seal in step 3 protects against the server's own processes, not against the network.
- **The identity check is the panel's.** `adi-app` owns the gateway-attached headers (§4). The
  gateway isn't granted on the mesh at all (it binds loopback only, `adi-llm-gateway/src/main.rs:20`),
  so it is never asked who a request is from by anything except `adi-app`.

The lease, in the gateway's memory only:

```rust
struct Lease {
    user: String,                       // <u>
    device: String,                     // the petname that unlocked it — revoke and expiry are per device
    secrets: BTreeMap<String, Zeroizing<String>>,   // NAME → value
    policy: LeasePolicy,                // §11, as the device sent it
    last_seen: Instant,                 // step 1/4/5
    budget: Option<Budget>,             // §16 Q20
}
```

A user's secrets are live while *any* of their devices holds a lease. Revoking one device's lease
leaves another device's in place.

**Never on disk in server-decryptable form.** The lease isn't journalled, swapped or persisted. The
process locks its pages (`mlock`), marks itself non-dumpable (`PR_SET_DUMPABLE = 0`), and zeroes
values on drop. A server restart therefore always ends every lease. The device's next renew gets
"unknown epoch, re-unlock" back, and redoes steps 1–4 by itself.

## 11. The lease setting on the device

How long a user's keys keep working on a server is **the user's call, made on their own device, per
server connection.** There is one setting per paired server. It sits in that connection's settings
on the device, and the device applies it every time it connects. Nobody is prompted per run.

| setting | the lease ends | renewal |
|---|---|---|
| **Off** | never granted; the device never unlocks for this server | — |
| **Only while connected** | `grace` after the last renew (two missed heartbeats) | each heartbeat |
| **N hours after I disconnect** | `N h` after the last time the device was seen | each reconnect and heartbeat |
| **Until I revoke** | on revoke, unpair, or server restart | — (re-unlocked on reconnect after a restart) |

- **The TTL counts from last seen, not from the unlock.** Every renew moves `last_seen` forward. A
  laptop that is open all week keeps its lease all week, and the clock starts when it goes away.
- **The device decides and the server enforces.** The policy travels in the unlock (step 4). The
  device can't be relied on to say "stop" after it has gone offline, which is exactly the moment
  the clock matters. So the gateway runs the clock. Whether the operator may cap it is §16 Q20.
- **Revoke is immediate.** It drops the lease and every per-session token minted against it. A turn
  in flight loses its next request (§13).
- **Where it is kept on the device.** On a Mac it is a new field on the server's `NodeRecord` in that
  Mac's `fleet.toml` (`adi-mesh/src/fleet.rs:392`), which is where that machine already records
  everything about that connection. In the browser client it is a new field on its own
  `NodeRecord` (`adi-mesh-client/src/store.rs:41`).
- **Who renews.** On a Mac, `adi-app` runs as a LaunchAgent whether or not anybody is looking, so
  "only while connected" means *while this Mac is awake and online*. The renew loop is a fourth
  background worker beside the prober (`docs/llm-backends.md` §The prober). In the browser it means
  *while a tab of the client is open*. The heartbeat interval `H` is §16 Q20.
- **Server-side presence is a different clock.** `fleet_activity` in `db/global.db`
  (`adi-mesh/src/activity.rs`) is keyed on the *nickname*, uses a 60-second active window
  (`ACTIVE_WINDOW_SECS`, `activity.rs:25`), and sits in a database every agent can write. None of
  those properties fits a lease. The lease's `last_seen` lives in the gateway and is keyed by the
  petname that renewed.

## 12. Leases in the gateway

**The gateway starts adding keys, on purpose.** Its module doc says it "adds no key and answers
from no cache. Authorization is the client's own header, forwarded as sent"
(`adi-llm-gateway/src/lib.rs:21-23`). That rule is reversed for exactly one case: a request carrying
a per-session token the gateway minted.

```
run env (phase 3)
  CLAUDE_CODE_OAUTH_TOKEN = adi-lt-<opaque>          ← the per-session token, not the key
  ANTHROPIC_BASE_URL      = http://127.0.0.1:<port>/anthropic

gateway, per request
  token  →  (run_id, payer, NAME)  →  lease  →  value
  swap the client's Authorization / x-api-key for the value, forward, journal
```

- **"Per run" means per session.** In this tree a run id *is* the conversation id (`ADI_RUN_ID`), so
  one token lives as long as one conversation. It is not minted per turn: Codex fingerprints the
  child's whole environment to decide whether a thread may be resumed
  (`hash_child_environment`, `runner/detached.rs:765`). A token that changed every turn would make
  every turn a new thread. A token that changes when the gateway restarts does the same, once (§16
  Q8).
- **Loopback address, not `llm.adi`.** `spec_in` hands the run the gateway's leased port directly,
  for the reason `docs/fleet.md` §10 gives for the transfer call: name resolution and the front door
  can be down while the gateway is fine.
- **The token is a bearer capability, and it is meant to be.** Any process under the same Unix user
  can read it out of `/proc/<pid>/environ`. What it buys is spending the lease while the session
  lives, never the key. Deleting the session, revoking the lease, or the lease expiring kills it.
- **An absent lease is not an `auth` failure.** A token whose lease is gone is answered by the
  gateway itself with a distinct status and body (`401` plus `x-adi-lease: missing`). The turn's
  classifier maps that to "waiting on unlock" (§13), never to `LimitClass::Auth`. An `auth` hold
  would hold the *login* and ask, which is the wrong answer to "the user's laptop is closed".
- **The injected value never reaches the journal.** `journal::headers_json` redacts `authorization`
  and `x-api-key` (`journal.rs:22`) and is built from the *client's* request
  (`proxy.rs:137`), before injection. Keep it that way: inject inside `exchange` (`proxy.rs:164`),
  at the point `forward_to_upstream` (`proxy.rs:361`) copies headers.
- **Metering is by payer.** `llm_gateway_requests` (`journal.rs:35`) gains `payer` and `run_id`
  columns, which is the per-payer usage view the design asks for. The journal also stores request
  and response bodies, so every prompt sits in `db/global.db`, the store every agent and dashboard
  queries. Who may read another payer's rows is §16 Q15.

**The gateway isn't on any run's path today, and isn't installed on a node.** Nothing in
`adi-agents` points a run at it. It is reached only by a client someone configured by hand. It
isn't among the services `adi-core` sets up or `apps/linux/build.sh` packages, and on the
development Mac it runs as a hand-wired hive service (`CLAUDE.md`). Phase 3 therefore starts by
shipping it to every install and supervising it like `app` (§16 Q6).

**The control surface needs authenticating.** `adi-app` has to tell the gateway "here is a sealed
unlock", "revoke this device", and "mint a token for session S, payer P". The gateway binds
loopback, and every agent run on the box can reach loopback. If minting were unauthenticated, any
run could mint a token against anybody's lease. How `adi-app` proves itself to the gateway, when
both run as the same Unix user as every agent, is §16 Q16.

## 13. Waiting on unlock, and auto-resume

**A lease that ends never cuts a turn in half. A revoke does.**

- **Expiry, mid-turn.** The turn that is running finishes. A turn is many requests, so the gateway
  keeps serving the session's token until `adi-agents` reports that turn ended, up to the turn's own
  timeout. Then the value is dropped. This needs a "turn open" signal from the launcher to the
  gateway (§16 Q9).
- **Revoke, mid-turn.** The next request answers `x-adi-lease: missing`. The turn ends on it, and
  the session parks as below.
- **Server restart, mid-turn.** The lease is gone with the process, so the turn can't finish. The
  next request answers `x-adi-lease: missing`, and the session parks.

**The next turn parks as "waiting on unlock".** Before a turn of a `user:<u>` session goes out,
the launcher checks whether the turn's credential would come from the user layer and whether
`<u>` holds a live lease for that name. If the credential is the user's and there is no lease,
and falling back to the server isn't permitted (§14, §16 Q3), then:

- the message stays in the session's `queue` (the table every mid-turn message already waits in);
- the session is `RunLifecycle::Waiting` (`run.rs:125`), "the conversation is going to move again on
  its own", which is exactly the case;
- `adi.agents.run.waiting_on_unlock { agent, run_id, user, secret }` is published, and the rail
  shows "waiting for <u> to unlock" in place of a spinner;
- it is **not** `Decision::Ask`. `failover.rs:45` defines that as "not a blocking question: it is a
  line in the conversation". Nothing here needs the person to answer anything. Their device needs to
  come back.

**It resumes by itself.** When the gateway accepts an unlock or a renew that brings `<u>`'s lease
back, `adi-app` publishes `adi.llm.lease.granted { user }`. The waiting sessions whose payer is
`<u>` drain their queues, the same delivery an await wake uses (`awaits.rs`, which never touches a
pending question). `adi.llm.lease.revoked` and `adi.llm.lease.expired` exist for the rail and for
anyone watching.

## 14. The server switch: `byob`

```toml
# llm/settings.toml
byob = "allowed"     # off | allowed | required
```

| value | the user layer | a member's run with no user credential for the name |
|---|---|---|
| **`off`** | ignored; `/api/secrets/mine/set` refuses; stored user secrets are kept but unused | runs on the server's credential, as today |
| **`allowed`** | applies | runs on the server's credential, unless the user has said "never fall back" |
| **`required`** | applies | **never** runs on the server's credential: parks waiting on unlock, or is refused at launch if the user has no secret for that name at all |

- **`required` is the partner sandbox's answer** to its open AI-credential question. The sandbox
  plan currently has every partner spending one shared, capped workspace key. `required` means
  the server never pays for a member. The operator's own local and admin launches
  (`paid_by = server`) are unaffected.
- **`required` fails closed.** `LlmSettings::load` reads a corrupt file as the defaults ("a settings
  file must never be the reason nothing can run", `llm/settings.rs:49`). That is right for
  `ask_on_switch` and wrong here: a sandbox whose settings file got mangled must not start paying
  for every partner. A `byob` value that doesn't parse is treated as `required` (§16 Q18).
- **Per user: "never fall back to server credentials".** This is a per-user preference, not a
  secret. It is stored beside the user scope (proposed `secrets/users/<u>/.settings.toml`), set from
  the device or "My tokens", and read at the same point the lease check runs. It makes `allowed`
  behave like `required` for that user alone.

## 15. Threat model and honest limits

What each party can do, with this built:

| who | can | cannot |
|---|---|---|
| **another member of the server** | read your transcripts (unchanged, §8a), see that you have a secret named X (only if an admin lists `secrets/` on disk), reply into your conversation and so spend your lease on its next turn (§16 Q17) | reveal any of your secrets through the API; set, replace or remove yours; make a run of theirs pay with your credential (paid_by isn't in the body) |
| **any agent run on the server** (same Unix user) | read every file: the master key, so every `always` secret and every global; the session store; other runs' `/proc/<pid>/environ`, so their per-session tokens, which lets it spend those leases while those sessions live; reach the gateway's loopback port; forge `ADI_RUN_ID` / `ADI_TRIGGER` to put a launch on someone else's bill | read a `leased` value at rest (it is wrapped to devices); read the gateway's memory on Linux with yama `ptrace_scope ≥ 1` (checked here: `/proc/<pid>/mem` refused) or on macOS without root or entitlements |
| **the server's operator, or root** | dump the gateway's memory while a lease is live; change the gateway or the panel so the next unlock is kept; serve modified panel code | read a `leased` secret while no lease is live; extend a lease past the device's policy *without* modifying code |
| **the network and the relay** | see that two keys talk | read anything: QUIC end to end, plus the seal to the session key |
| **a thief with your device** | unlock on your behalf until you revoke it | — revoke from another device, or unpair the device's petname on the server; its wraps are deleted and its lease ends |

The limits, said plainly:

- **`leased` protects your key from the server's other users and at rest, not from a hostile
  server.** Root on the box can read the key out of memory the moment your device unlocks it. So
  can anyone who can change the code running there. Only `custodian` bounds that, and only to "each
  release is seen, logged and refusable outside the box", not to "never seen".
- **On a single-Unix-user server, `paid_by` for agent and trigger launches is attribution, not
  authentication.** Human launches over the mesh are authenticated by the gateway. Launches made
  from inside the box can claim any parent they like. The partner-sandbox plan already says this
  of the box as a whole: "per-partner identity is attribution, not isolation". Running agents under
  a different Unix user from `adi-app` and the gateway would change that, and is out of scope here.
- **`always` is a server-held key in the full sense.** Every agent run can read it.
- **Prefer keys you can revoke and cap.** An Anthropic workspace key with a spend limit, an OpenAI
  project key, or a dedicated `claude setup-token`, rather than the login you use everywhere. Every
  limit above is smaller with a key that is cheap to lose.

## 16. Open questions

Each one is either a place the code contradicts the design, or a decision the design leaves open.
None of them is settled by this document.

**Q1 — What is `<u>`?** The design says the petname. The petname names a *device* (§4), and §9's
"one wrap per device" means one person has several. The credential username groups devices, is
already what `launched_by = human:<u>` and ADI-MONO-171's member checks use, and is what the sandbox
sets per person (`p<uid>`). But it defaults to `adi` for every pairing (`join.rs:112`).
A suggestion, not a decision: `<u>` = the record's username when it isn't `PAIR_USER`, and a peer
still on `adi` is treated as the server's own (`paid_by = server`, no user scope).

**Q2 — Does `Secrets::resolve` get the layer at all?** It is the trigger path, not the run path
(§5). Adding a user layer there injects user secrets into trigger shells. It is needed only if a
user's own triggers run on their `always` secrets, which also needs Q5.

**Q3 — When a lease is absent under `allowed`, park or fall back?** Design item 7 says the next
turn parks. Item 10 implies `allowed` falls back to the server unless the user opted out. This spec
reads item 7 as applying under `required`, or when the user said "never fall back", and otherwise
falls back with a one-line notice in the transcript. That reading is a guess.

**Q4 — The `launched_by` body field.** `run_agent` records whatever `launched_by` the body sends
(`agents.rs:347-353`). This spec keeps `paid_by` off that path entirely, but the field itself still
lets a member put `human:<someone>` on a run. That defeats ADI-MONO-171's ownership check
(`member_run_refusal`) the other way round, by making a stranger's run look like the member's. Fix
it here, or separately?

**Q5 — The sources that have no creator to read.** Triggers (`adi-triggers`), schedules, workflow
steps and task dispatch (`dispatch.rs:295`) all launch as `automation` and record no `created_by`.
Channels launch as `automation` too (`adi-channels/src/dispatch.rs:172`), and a connection knows an
external `owner_sender_id`, not an ADI user. "Its creator" and "the linked ADI user" don't exist yet.
Each needs a field, and task dispatch isn't in the design's table at all.

**Q6 — Shipping the gateway.** It isn't installed on nodes and no run is pointed at it (§12).
Phase 3 depends on it being present and supervised everywhere `app` is. Is it a separate supervised
binary, or folded into `adi-app` (which would also simplify Q16)?

**Q7 — Which env var Codex honours for a user key.** `ondemand.rs:268` assumes `OPENAI_API_KEY`.
The Codex runner knows `CODEX_API_KEY` and `CODEX_ACCESS_TOKEN` as Codex's direct credentials
(`detached.rs:774-776`), and binds the host's `auth.json` login even when one is present ("Bind a
file login whenever it exists"). Whether a user's key actually takes precedence over the host's
`codex login`, and whether BYO Codex runs need their own `CODEX_HOME`, has to be shown before Codex
can be BYO. This is the design's own item 11.

**Q8 — Token stability across a gateway restart.** Codex resumes a thread only if the child's
environment hashes the same (§12). An in-memory token map loses every token on restart and breaks
every BYO Codex thread once. The options are to persist the token map (hashed), to derive tokens
from something stable, or to exclude the token from the fingerprint.

**Q9 — "The running turn finishes" needs the gateway to know turn boundaries.** It sees requests,
not turns. This spec proposes a turn-open/turn-closed signal from the launcher with the turn timeout
as a hard cap. After a server restart the turn *can't* finish, so "finishes" holds only for expiry.

**Q10 — Enrolling a new device.** A wrap needs the DEK, and only an enrolled device can unwrap it.
Should a new device be wrapped by an existing one (both online), or should the value be re-entered
on the new device? Also, symmetric Keychain or PRF wraps can't be made *to* a device by anyone else.

**Q11 — Settings-file backends.** A user's env value can't override a credential that lives in a
settings file (§5). Is BYO limited to env-named credentials (`api_key_env`, or the runtime's ambient
var), or should a user's value beat a settings file's `env` block?

**Q12 — Phase 2 is `always` in disguise.** Before phase 3's gateway exists, a user secret can only
reach a run as plaintext in its environment, which carries every exposure in §1, including other
runs reading `/proc/<pid>/environ`. Ship phase 2 to members at all, or only behind
`byob = allowed` with that stated in the panel?

**Q13 — Probing user-scoped holds.** The prober can't match a qualified key (`prober.rs:160`), so a
user's hold waits out its deadline. Probing it would bill the user and need their lease. Leave it
`Unreachable`?

**Q14 — Pre-existing member-gate gaps.** `POST /api/secrets/set-oauth` and `/api/secrets/refresh`
aren't in `member_blocked` (`main.rs:622-635`), so a member can overwrite or refresh a global OAuth
secret today. Fix as part of phase 2, which edits that list anyway, or separately?

**Q15 — Who may read another payer's journal rows.** `llm_gateway_requests` keeps full request and
response bodies in `db/global.db`, which agents and every peer's panel can read. Per-payer metering
makes it obvious whose prompts those are.

**Q16 — Authenticating `adi-app` to the gateway's control surface.** Every agent run can reach the
gateway's loopback port and read every file the two share. Candidates: a secret handed over only in
memory at spawn (needs the supervisor's help); a Unix socket plus `SO_PEERCRED` and an `exe` check;
running the gateway inside `adi-app` (Q6); or running agents under a different Unix user.

**Q17 — A reply from someone else spends the session payer's lease.** §8a lets any peer reply into
any conversation, and every later turn pays as the session's `paid_by`. So member B can spend user
A's subscription by talking in A's chat. Is that intended, or does a reply from a different human
into a `user:` session need refusing, or re-billing?

**Q18 — `byob` on a corrupt settings file.** `LlmSettings::load` falls back to defaults
(`settings.rs:49`). This spec proposes `required` as the fail-closed value for an unparsable `byob`,
which differs from every other field in that file.

**Q19 — "Re-encrypted to the server session".** Read here as: sealed by the device to an ephemeral
key the gateway generates at boot (§10). If something else was meant, such as the mesh connection's
own keys, §10's flow changes.

**Q20 — Numbers and units the design leaves open.** The heartbeat interval `H` and the grace for
"only while connected". Whether the operator can cap the device's chosen lease length. Whether the
lease `budget` is counted in tokens or money: the gateway parses no usage today, and the sandbox
plan notes `adi-llm-gateway` has no budget code.

**Q21 — Subscription logins through a swapped bearer.** The design's item 11, still open. Does the
`claude` CLI accept a gateway-issued placeholder in `CLAUDE_CODE_OAUTH_TOKEN` with a custom
`ANTHROPIC_BASE_URL`, and swap correctly for a subscription (OAuth) login, not only an API key? Does
it call anything outside the base URL with that token? The same questions apply to a ChatGPT login
for Codex. Until this is shown, phase 3 is proven for API keys only.

**Doc drift noticed, not fixed here:** `docs/fleet.md` §11 says `Secrets::resolve` "injects every
global secret into every agent run's environment". Runs use the attachment allowlist now, and only
triggers get the whole set (§5). §8a describes the member gate as covering "the global"
secrets actions, but the code covers every scope.

---

## Checklist

The five phases are the task's own. Each item ships with unit tests in the same file, as in
`docs/fleet.md`.

### Phase 1 — `paid_by` and how it propagates

- [ ] P1.1 `SessionRecord::paid_by` (`store/record.rs`) and its `ALTER TABLE` in `store/db.rs`. Empty
      reads as `server`. Not backfilled.
- [ ] P1.2 `LaunchOptions::paid_by`. `Agents::launch` writes it once, at `create_as`. No later turn
      re-derives it.
- [ ] P1.3 `run_agent` computes it from `FleetSender` only. A body field can't set it (test: a body
      sending any payer-ish field is ignored).
- [ ] P1.4 CLI launches from inside a run inherit the parent session's `paid_by` via
      `awaits::caller()`. A missing or dead parent resolves to `server`, or is refused under
      `required`.
- [ ] P1.5 Replies, queued turns, await wakes and goal nudges carry the session's payer (a
      no-change test that pins it).
- [ ] P1.6 `paid_by` on the session DTO (`adi-webapp-api` types). Kept out of `adi-usage` events.
- [ ] P1.7 Settle Q1 (what `<u>` is) and Q5 (creators for triggers, schedules, workflows, channels,
      dispatch) before P1.4's siblings for those sources are built.

### Phase 2 — the user scope, the layer, the hold key, the own-scope API

- [ ] P2.1 `adi-secrets`: `Scope::User`, `users/<u>/` paths, AAD `user:<u>/<NAME>`, a `mode` field on
      the manifest. `list_all` skips `users/`. A copy of a user ciphertext into global or another
      user's scope fails to decrypt.
- [ ] P2.2 `attached_secret_env` gains the user layer over global and project, for attached names
      only.
- [ ] P2.3 `ondemand::resolve_credential` resolves the asker's user layer first.
- [ ] P2.4 The hold fix: the per-turn qualified hold key (`#user:<u>`) when the user layer supplied
      the credential. The same key in `RunSpec.credential`. A user quota doesn't hold the server's
      login (test).
- [ ] P2.5 `GET /api/secrets/mine`, `POST /api/secrets/mine/{set,remove}`. User from the headers
      only. Never reveals. Open to members.
- [ ] P2.6 `adi-mono secrets {set,rm,list} --user --node <petname>` through the local mesh gateway.
      No `--reveal` or `read` for `--user`.
- [ ] P2.7 `byob` in `llm/settings.toml`, `off | allowed | required`. Fail-closed parse (Q18).
      `required` refuses the server's credential for a member payer.
- [ ] P2.8 Settle Q12 (who gets phase 2 before the gateway exists) and Q14 (the member-gate gaps).

### Phase 3 — device wraps, leases in the gateway, the lease setting, waiting on unlock

- [ ] P3.1 Ship and supervise the gateway on every install (Q6). The control-surface authentication
      (Q16).
- [ ] P3.2 The `leased` manifest shape: DEK, per-device `[[wraps]]`. `set` accepts wraps. Plaintext
      never reaches the server.
- [ ] P3.3 macOS device key in the Keychain (`adi-app`). WebAuthn PRF device key in
      `adi-mesh-client`. The unwrap runs in device-owned code only.
- [ ] P3.4 The gateway's ephemeral session key, `/api/byo/{session,unlock,renew,revoke}`, the
      in-memory `Lease` (mlock, non-dumpable, zeroize).
- [ ] P3.5 Per-session tokens (stable per Q8). `spec_in` hands the run the token and the loopback
      base URL instead of the value. Header swap in `exchange`. The `payer`/`run_id` journal columns.
- [ ] P3.6 `x-adi-lease: missing` classified as waiting-on-unlock, never `auth`.
- [ ] P3.7 The lease policy on the device's `NodeRecord` (Mac `fleet.toml`, browser IndexedDB). The
      renew worker in `adi-app`. TTL from last seen. Immediate revoke.
- [ ] P3.8 Waiting on unlock: queue-held turn, `RunLifecycle::Waiting`,
      `adi.agents.run.waiting_on_unlock`. Resume on `adi.llm.lease.granted`. Turn-boundary grace
      for expiry (Q9).
- [ ] P3.9 Prove Q21 (subscription logins through a swapped bearer) and Q7 (Codex precedence)
      before claiming either runtime is BYO.

### Phase 4 — the panel

- [ ] P4.1 "My tokens": own names, mode, devices, last used. Set (`always`), remove, change mode.
      Never shows a value.
- [ ] P4.2 Paid-by on run cards and in the session header. "Waiting for <u> to unlock" in the rail.
- [ ] P4.3 The connection's lease setting in the device UI (the Mac's Fleet page row for that
      server, the browser client's node settings): off, only while connected, N hours, until
      revoked. Plus revoke now.
- [ ] P4.4 The `byob` switch on the LLM backends page. A user's held subscription shown as "your
      subscription" to its owner and anonymised to everyone else.
- [ ] P4.5 Per-payer usage on the LLM traffic page (Q15 decides who sees whose).

### Phase 5 — custodian mode, for the partner sandbox

- [ ] P5.1 Specify `custodian`: the grant format the device signs, the unlock-service contract, and
      the seal to the gateway's session key.
- [ ] P5.2 A reference custodian (Cloudflare Worker or AWS KMS) with per-release audit, rate limit
      and revocation.
- [ ] P5.3 The gateway falls back to the custodian when no device lease is live and the secret's
      mode is `custodian`.
- [ ] P5.4 The partner sandbox runs `byob = required` with partners on `custodian` or `leased`, and
      the shared `ANTHROPIC_API_KEY` global is retired.
