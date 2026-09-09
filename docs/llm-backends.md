# LLM backends — spec

One agent, many backends. Read this before touching `crates/adi-agents` or the agent form.

Status: **requirements confirmed 2026-09-09, not implemented.** Nothing in this document is
built. The operator restated the requirements after the first draft; this document has been
reconciled to that statement, and the differences are recorded under "Decisions taken".

## The problem

A model choice is currently welded to an agent definition. `adi-agent`, `adi-agent-glm` and
`adi-agent-kimi` are three agents that differ in `settings` / `provider` and in nothing
else — same prompt, same tools, same working dir — because there is no way to say "this
agent, on that model". The consequences:

- **Configuration is copied, so it drifts.** A tool enabled on one, a prompt fixed on
  another. Three definitions, three chances to be the odd one out.
- **A usage limit ends the run.** Anthropic's five-hour cap arrives mid-conversation and
  the turn simply fails. The human notices, picks a different agent by hand, and loses the
  thread.
- **Every run finds out the hard way.** A limit is discovered per-run: sixteen agents in
  flight discover it sixteen times, because nothing on this machine records that the
  credential is spent.
- **Recovery costs a turn too.** Nothing knows when the limit lifts, so the way back is a
  human trying again and seeing whether it works.

## The principle

**An agent is an identity. A backend is one complete way to answer a turn.**

There is **one** object, and it is called a backend: a login, a model, and the dials that
model runs with, under a name a human chose. Not a login *plus* a configuration on top of
it — one flat thing, `anthropic-smart`, that can be used as it stands.

The prompt, the tools, the working dir, the memory and the conversation belong to the
agent, are written once, and are re-applied unchanged onto whichever backend ends up
serving the turn. Nobody configures tools per backend, and no backend knows an agent's name.

## One object, one list

```
LAUNCH     start_at: "codex-deep"   ·   overrides                 per run
   │
AGENT      prompt · tools · cwd · env · memory                    identity
           backends = [ { anthropic        · effort high      } , the order, ON THE AGENT
                        { anthropic-sonnet                    } , base + optional override
                        { codex            · reasoning high   } ,
                        { glm                                 } ]
   │
BACKEND    runtime · login · model · dials · limit rules · probe  one flat object
             anthropic        = claude subscription · opus-5   · effort high
             anthropic-sonnet = claude subscription · sonnet-5
             codex            = chatgpt login       · default
             glm              = z.ai key            · glm-5.3
```

**There is no second object.** No preset, no chain, no login record. A backend is the whole
answer, the agent's list is the order, and a launch may start anywhere in that list.

**There is no `default_backend` field either** — the default is first in the list.

## The backend

One file per backend under `llm/backends/<id>.toml` in the mono store (`adi_config::Module`,
the pattern `sessions/settings.toml` already uses in `limits.rs`).

```toml
# llm/backends/anthropic.toml — the id is the FILENAME; there is no `id` field to drift from it
label = "Anthropic — my Claude subscription"

runtime  = "pty:claude"                          # the existing Backend enum — backend.rs
settings = "~/.claude/settings.anthropic.json"   # the credential
model    = "claude-opus-5"
context_tokens = 200000

params = { effort = "high" }        # the dials: how deep it thinks, and its neighbours

# How this backend says "I am out", and what that means.
[[limit_rules]]
match  = "(?i)usage limit reached|5-hour limit"
class  = "quota"            # quota | rate | auth | transient | unknown
scope  = "model"            # model | login  — how wide the hold spreads
resume = "from_message"     # from_message | retry_after | fixed
fixed  = "4h"               # used when nothing is parseable

[[limit_rules]]
match = "(?i)invalid api key|authentication_error"
class = "auth"
scope = "login"

[probe]
model  = "claude-haiku-4-5-20251001"   # a few tokens on something cheap
prompt = "ok"
```

`runtime` is the existing `Backend` enum value (`pty:claude`, `process:codex`,
`harness:adi`, …), so this sits **above** the runner registry and changes nothing under it.
Everything the argument structs in `arguments.rs` carry splits cleanly in two: the
*connection* facts (`settings`, `provider`, `base_url`, `api_key_env`) are fields, and the
*dials* (`effort`, `reasoning_effort`, `thinking`, `max_tokens`, `sandbox`) are `params`.

**There is no inheritance between backends.** A backend is never built on another one: that
is programming, not configuration. Every definition stands alone, and reuse happens in the
agent's list below, by overriding a base.

## The agent

The manifest loses `settings` / `provider` / `base_url` / `model` and gains one field — an
**ordered array of objects**, each a base plus an optional override:

```toml
backends = [
  { backend = "anthropic", overrides = { thinking = "high" } },
  { backend = "codex" },
  { backend = "glm" },
]
```

- **`backend`** — the id of the backend used as the *base*. Every entry has one.
- **`overrides`** — what this agent changes about it: `model` and the dials. Optional; an
  entry with none is the backend exactly as defined. Overrides never touch the runtime or
  the credential — wanting those different means wanting a different backend.
- **The order matters.** First is what a new chat starts on, then the second, then the
  third.

**One row per backend.** Falling from Opus to Sonnet on one subscription is *two backends*
— `anthropic` and `anthropic-sonnet`, each flat, each naming the same `settings` file —
listed one after the other. It is not one backend listed twice under different model
overrides.

The config file does not forbid repeating a base, and the resolver must not break on a
hand-written manifest that does. But it is **not offered in the interface and is not
documented as a way of working**: the row editor picks each backend at most once, and an
operator who wants a second model defines a second backend.

Precedence, low to high: **the backend's own fields → the agent entry's `overrides` → the
launch's overrides.** Nothing else about models appears on an agent.

## The launch

```jsonc
POST /api/agents/run
{
  "agent": "adi-agent",
  "message": "…",
  "start_at": "codex",                   // begin here; the rest still follows
  "overrides": { "effort": "low" }
}
```

`start_at` **rotates the named entry to the front and keeps the rest in their configured
order** — a run started on the third row runs row 3, then row 1, then row 2. It names a
backend id and resolves to the entry using it; a position (`"start_at": 2`) is also
accepted, which is what disambiguates a hand-written manifest that repeats a base. The
picker offers backend names, not positions. Pinning to one entry with no failover is a
separate, explicit `"only"`.

This rides the existing `RunOverrides` mechanism (`overrides.rs`), which already travels
with the launch, is written onto the session, and is re-applied on every later turn — the
same reason a chat cannot answer its first message on one model and its second on another
by accident.

## Resolution

```
first turn of a session:
    chain := agent.backends, rotated by start_at (or replaced by `only`)
    per entry: config := base backend <- entry overrides <- launch overrides
    PIN the resolved chain onto the session record

every turn:
    for entry in chain, best-first:
        if held(entry): record why, continue
        run it
    if every entry is held: ask the human (always — this one ignores the toggle)
```

The chain is resolved once and pinned so that an agent or a backend edited mid-conversation
does not change what a live chat is talking to.

## Holds

A hold is `(login, model | *) → until T`, with the reason, the evidence line, and who set
it, in **`llm/holds.db`** — its own database beside the backend definitions, not the shared
`db/global.db`. That is a deviation from the first draft of this spec, taken deliberately:
the shared database is the *user's* data store, the thing dashboards and agents query, and a
platform-internal table keyed on somebody's credential does not belong in it.

**The key is read off the resolved entry — there is no login object.** Two backends naming
the same `settings` file are the same subscription, and an override that changes the model
changes the key with it, so a quota that stops `anthropic` on Opus is recorded against that
credential and *that* model — the Sonnet entry beside it is untouched, and every agent
listing either one knows immediately. That is what stops sixteen runs discovering the same
limit sixteen times.

`scope` on the matching rule decides how wide it spreads: an Opus quota holds one model on
that login, a dead token or an unreachable endpoint holds the login and every entry that
names it.

`T` comes from the provider's own words where possible (`resume = "from_message"` parses
the reset time out of the error, `retry_after` reads the header), and from `fixed` with
exponential backoff where not.

Classification decides the response, and this is the part that must not be one regex:

| class       | what happens                                                        |
|-------------|---------------------------------------------------------------------|
| `quota`     | hold until T, move to the next backend, tell the human                |
| `rate`      | short hold, one in-place retry first                                  |
| `auth`      | hold the login and **ask** — a cooldown would be the wrong answer     |
| `transient` | retry in place; no hold                                              |
| `unknown`   | no hold; surface it — an unclassified error must not silently reroute |

The `llm-gateway` journal (`llm_gateway_requests` in `db/global.db`) is the second source
of evidence: it already records every status code and body for traffic routed through
`llm.adi`, so a 429 can be seen there even when the CLI's own message is unhelpful.

## The prober

A background worker **inside `adi-app`** (`crates/adi-app/src/prober.rs`), not the hive
service this spec first called for. The supervisor tracks a service's liveness *by its port*
and a prober binds nothing, so one that died at 03:00 would stay dead silently; a background
trigger has the mirror problem, in that disabling one does not stop the loop already running.
`adi-app` already owns three workers of this shape, so this is the fourth, and it starts and
stops with the panel. `adi-mono llm probe [--watch]` is the hand-run form for a store the
panel is not watching.

At `T` it sends the backend's `probe` (a few tokens on something cheap) and either releases
the hold or extends it with backoff. Probing is **opt-in per backend**: no `[probe]` block
means the backend is never asked, and its hold simply expires. Only `harness:adi` can
actually be probed — a vendor-CLI backend reports "unreachable" and waits out the provider's
own stated deadline, because nothing may be marked recovered on a guess.

**Recovery never costs a chat turn.** A run must never be the thing that discovers a
backend is back, because that discovery is paid for with a failed turn.

## Decisions taken 2026-09-09

Five calls, made by the operator, that the design turns on:

1. **Scope — everything.** Every agent run resolves through a chain: chats, triggers, batch
   solvers. Not chats first.
2. **Auto for limits, ask for the rest — and a global switch.** `quota` and `rate` reroute
   silently with a notice in the transcript; `auth`, `unknown` and all-held stop and ask. A
   global setting (`llm/settings.toml`, `ask_on_switch = false` by default) makes *every*
   switch ask, for an operator who wants nothing changing under them.
   The notice is **one short line**, and it names both ends and the time:
   `switched to codex, anthropic limited until 14:00`.
3. **Failback — finish the conversation first.** A recovered higher backend does not pull a
   live conversation back; it only affects new runs. Within a conversation the chain moves
   **forward**, never backward for improvement. The next new chat starts from row 1 again.
   **Confirmed exception:** when the current backend is itself held and the conversation is
   forced to move anyway, it re-picks **best-first from the whole chain**, not merely the
   next one down — otherwise a thread whose `anthropic-fast` tripped would crawl to GLM
   while a recovered `anthropic-smart` sat idle.
4. **Handoff — full replay.** A vendor switch re-feeds the whole transcript, with tool
   calls translated by the receiving adapter. No summarization in v1.
5. **One object, called a backend, and no inheritance.** A login and its configuration are
   the same thing, not two layers, and a backend is never built on another backend —
   rejected as programming rather than configuration. The order lives on the agent as an
   array of `{ backend, overrides }`, its first entry is the default, and reconfiguration
   happens only there and at launch.
6. **One row per backend in the interface.** Listing the same backend twice under different
   model overrides is not a feature: it stays legal in a hand-edited config file, and the
   resolver tolerates it, but the UI does not offer it and the docs do not teach it. Two
   models means two backends.

## The context-shrink danger

Full replay is only safe while the thread fits. Every backend declares `context_tokens`,
and:

- The agent form **warns on a list that steps down in context size** — putting a
  smaller-window backend after a larger one is a legitimate configuration and a trap, so it
  is a visible danger, not a rejection.
- At switch time, a replay that will not fit **fails loudly and asks**. It must never
  silently truncate a conversation to make it fit.

Compaction / summarization is the answer here and is deliberately **out of scope for v1** —
this project stops at detecting the overflow and saying so.

## The panel

A new tab, `app.adi ▸ /extended/settings/llm-backends`: one list of backends, each showing its login,
model, dials, **live hold state** ("held until 18:40 — probe in 12m"), and which agents name
it. Set one up, reuse it everywhere.

The existing **LLM traffic** page (`/extended/llm`) is the sibling of this one and should
link both ways: this is where traffic is configured, that is where it is read.

The agent form **shrinks** — `settings`, `provider`, `base_url` and `model` leave it, and an
ordered backend list replaces them (drag to reorder, per-entry `params`, with the
context-shrink warning). A backend already on the list is not offered again, so the form
cannot build a chain that names one twice. The new-chat screen gains the `start_at` picker
and the overrides.

## Migration

- **Model configuration leaves the agent entirely.** `settings`, `provider`, `base_url` and
  `model` are *removed* from the manifest, not left beside the list as a second way to reach
  a model. The upgrade reads each agent's current configuration, writes a backend for each
  distinct one it finds, puts it at the head of that agent's list, and deletes the old
  fields. One path to a model, no compatibility shim.
- `adi-agent`, `adi-agent-glm` and `adi-agent-kimi` then collapse into **one agent whose
  list names all three backends**.

### Running it

```sh
adi-mono llm migrate            # the plan, in full. Writes nothing.
adi-mono llm migrate --apply    # do it
```

`crates/adi-agents/src/llm/migrate.rs`. A dry run by default, because it rewrites every agent
definition in the store and the plan is the only chance to disagree with it.

**The conversion is 1:1** — one backend per *distinct* configuration found, fingerprinted on
runtime, model, the four credential fields and the dials. Two agents set up identically share
one backend; two set up differently get two, even where a person would later merge them.
Collapsing `adi-agent` / `adi-agent-glm` / `adi-agent-kimi` into one agent is a judgement
about which agents are the same agent, and it is made by hand afterwards
(`adi-mono agents save <name> --llm anthropic --llm zai --llm monshoot`), not guessed at here.

What moves is the model (`model`), the login (`settings`, `provider`, `base_url`,
`api_key_env`) and the dials (`effort`, `reasoning_effort`, `thinking`, `thinking_budget`,
`temperature`, `top_p`, `top_k`, `seed`, `max_tokens`, `stop`, `fallback_model`,
`max_budget_usd`). What stays is everything about the *agent* or its *executor*:
`system_prompt`, `append_system_prompt`, `permission_mode`, `allowed_tools`, `tools`,
`max_turns`, `working_dir`, `add_dir`, `sandbox`, `approval`, `output_format`, and the rest.
The runtime is **copied**, not moved: a resolved row sets the runtime it runs on, but an agent
that resolves no row at all still has to run.

Backends are named after the most specific thing that names the login —
`~/.claude/settings.glm.json` becomes `glm`, provider `zai` becomes `zai` — falling back to
the runtime (`harness-claude-sdk`) when nothing names a login at all, and to `-2`, `-3` when
two genuinely different configurations want one name. Nothing carries a `context_tokens`, a
limit rule or a probe: an agent never recorded a context window, and rules and probes are new
behaviour rather than old configuration. A migrated backend therefore never reroutes until
somebody writes a rule for it, which is the safe direction.

Re-running is safe: an agent that already lists a backend is left alone, and a configuration
matching a backend already in the store reuses it instead of writing a second.

**Order of operations.** The binaries must understand `backends` *before* the store is
migrated. Applying this against a stack still running pre-backends code would strip the model
and the login off every agent while nothing yet reads the list that replaced them. Build and
restart first, then `--apply`.

## Out of scope for v1

Inheritance between backends (`extends` — declined: configuration, not programming) ·
named, shareable chains ("order presets" — could exist, declined) · listing one backend
twice with different models as a *UI* feature (legal in a hand-edited file, never offered
or documented) · context compaction on handoff · cost-aware routing (cheapest entry that
can do the job) · per-project chain policy · sharing hold state between machines over the
mesh.
