# Goals — what a conversation is for

A turn ends when the model stops calling tools. That is a fact about the turn, not about the work:
the run may have finished, or it may have paused at the end of a thought. A **goal** is the missing
sentence — what *done* means for this conversation — kept beside it and put back to it every time it
falls quiet.

```
  create ──▶ OPEN ──idle──▶ nudge ──▶ turn ──▶ idle ──▶ nudge ──▶ …
               │
               ├── goals met <id>               ──▶ MET
               └── goals knowingly-give-up <id> ──▶ GIVEN_UP
```

## The rule that shapes everything else

**Nothing in the platform closes a goal.** There is no attempt cap, no timeout, and no sweep that
decides a run has been at it long enough. The two verbs are the only exits and both are somebody's
judgement, written down with a reason.

The consequence is deliberate: a run that neither finishes nor gives up is asked again for as long
as its conversation exists. That is the price of the alternative being worse — "exceeded 10
attempts" is a sentence nobody can act on later, while `knowingly-give-up --why "no route to the
staging host from here"` is one somebody can.

The give-up verb is spelled out in full for the same reason. It is the one way work stops without
being finished, and it should read as the admission it is.

## Five verbs, and none of them can be refused

```bash
adi-mono goals create  "every flaky test is fixed or quarantined"
adi-mono goals edit    <id> --text "…"
adi-mono goals show    [<id>] [--all]
adi-mono goals met     <id> --evidence "what shows it"
adi-mono goals knowingly-give-up <id> --why "what stopped you"
```

Closing a goal that is already closed, or closing it the other way after the fact, reports what
actually happened and exits 0 — it is not an error. The caller is usually a model reading this as a
tool result, and a *failed* tool result is something it retries or argues with rather than accepts.
The store settles the race with a conditional `UPDATE`, so the first ending is the one that stands
([`store::goals::close`](../crates/adi-agents/src/store/goals.rs)).

Two things do fail, because in both cases there is nothing to accept: an id that matches no goal
(a typo — silence would let a run believe it had finished), and `create` naming a conversation that
does not exist.

## A run can set its own goal

`create` with no `--agent` / `--session` reads `ADI_AGENT` and `ADI_RUN_ID`, both of which are in
the environment of every turn. So a run writes down what it is doing and is then held to it:

```bash
adi-mono goals create "the migration runs clean on a fresh database"
```

`ADI_RUN_ID` is exported by `name_conversation` (`crates/adi-agents/src/lib.rs`) rather than by
`workspace::env`, because a `RunSpec` is assembled *before* `store.create` mints the id it would
carry. It is set on all three launch paths — a fresh run, a later turn, and a simulated one.

A goal is stamped `set_by: agent` or `human` depending on which answered, and the two are worth
telling apart afterward: a run that set its own goal has decided what it is doing. The HTTP endpoint
always records `human` — it *is* the UI, and the field is never taken from the request body.

**The loop this closes, and the loop it opens.** An agent that sets itself a goal mid-turn is nudged
the moment that turn ends, and keeps itself going with nobody watching. It is also the one way a run
can put itself in a cycle that only its own give-up breaks. Both follow from the same rule.

## When a nudge fires

[`goals::tick`](../crates/adi-agents/src/goals.rs) rides the once-a-second worker that already
carries awaits and question deadlines (`crates/adi-app/src/awaits.rs`). Goals go **last** in that
sweep: the other two can each un-quiet a conversation, and a goal check running before them would
ask a run about its goal in the same second something else woke it.

A conversation is asked only when all of these hold:

| Clause | Why asking through it would be wrong |
|---|---|
| no turn in flight | the run is answering; the goal is what it is answering about |
| queue empty | somebody is already mid-sentence with this chat, and a nudge would jump the line |
| no pending question | the run stopped to ask a *person*; it has already said what it is waiting on |
| no registered await | the run has said in its own words what it is waiting for, and arranged its own wake |
| `NUDGE_FLOOR_MS` elapsed | see below |

The floor (30s) is the only pacing in the design, and it is a limit on the *sweep*, not on the run.
A nudge becomes a turn, and a turn that fails to start — a misconfigured engine, a binary that is
not there — leaves the conversation idle again within milliseconds; without a floor the sweep would
re-ask at its own tick rate for as long as that lasted. It is stamped *before* the message goes out
for the same reason, so a conversation nothing can be delivered into is retried on the floor rather
than every second.

Delivery is [`Agents::deliver`](../crates/adi-agents/src/lib.rs), **never `reply`**. A reply is a
person speaking, and a person speaking into a conversation that has stopped to ask them something
settles that question — a nudge sent that way would answer the run's own question with the goal
check. This is the same trap [awaits](../crates/adi-agents/src/awaits.rs) documents.

All of a conversation's open goals travel in one message: a run asked about them one at a time would
answer each without the others in front of it. With exactly one goal, its id is substituted straight
into both commands, so what the run reads can be run as printed.

## Where it lives

`goals` is the fourth table in `sessions.db`, beside `sessions`, `turns` and `queue`, cascading off
the session (delete the chat and its goals go with it). `goals_by_id` is unique, which is what lets a
goal be found by id alone — the id is quoted back in every nudge and typed by a shell that may not
know which conversation it is standing in. `goals_open` is partial, because the sweep runs every
second and finds nothing in nearly every pass, so the *empty* answer is the one that has to be cheap.

| Layer | File |
|---|---|
| table, states, the never-refuse rule | `crates/adi-agents/src/store/goals.rs` |
| sweep, idle predicate, nudge text, verbs | `crates/adi-agents/src/goals.rs` |
| `ADI_RUN_ID` | `crates/adi-agents/src/workspace.rs`, `lib.rs` (`name_conversation`) |
| the clock | `crates/adi-app/src/awaits.rs` |
| CLI | `crates/adi-cli/src/goals.rs` |
| HTTP | `crates/adi-webapp-api/src/handlers/agents.rs`, routes in `crates/adi-app/src/main.rs` |
| UI | `crates/adi-webapp/src/pages/agents/actions.rs` (`goal_bar`) |

`POST /api/agents/goals` (read), `/api/agents/goal/set`, `/api/agents/goal/close`. The read is on the
live-channel allowlist at the `FAST` cadence.

## Events

`adi.agents.goal.set`, `.nudged`, `.met`, `.given_up` — all four in the catalog, so a trigger can
subscribe. `.nudged` carries the running count deliberately: nothing closes a goal on the run's
behalf, so a number that keeps climbing is the only signal that a run is circling one rather than
converging on it.
