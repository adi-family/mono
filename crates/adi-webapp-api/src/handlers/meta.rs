//! The `/api/meta` surface: the state of the Meta page, which manages a single well-known global
//! agent named `adi-agent` — the default ADI agent. The page reuses the agents endpoints
//! (`/api/agents/save`, `/run`, `/peek`) to create and run it; this endpoint only reports whether
//! it exists, its current definition, and the canonical system prompt to seed a new one with.

use adi_agents::Agents;

use crate::types::MetaState;

use super::agents::agents_state;
use super::response::{Response, ok_json};

/// The well-known name of the default ADI agent the Meta page manages. Creating the agent is an
/// ordinary `/api/agents/save` under this name; this handler just decides which stored agent is
/// "the" meta-agent.
pub const ADI_AGENT_NAME: &str = "adi-agent";

/// `GET /api/meta` — report the Meta page's state: the `adi-agent` definition (if it has been set
/// up), the canonical default system prompt to seed a new one with, and the agent form schema
/// (whose backend list drives the setup picker).
#[must_use]
pub fn meta(store: &Agents) -> Response {
    let state = match agents_state(store) {
        Ok(state) => state,
        Err(e) => return Response::from(&e),
    };
    let agent = state
        .agents
        .iter()
        .find(|a| a.name == ADI_AGENT_NAME)
        .cloned();
    ok_json(&MetaState {
        name: ADI_AGENT_NAME.to_string(),
        default_prompt: default_prompt(),
        agent,
        form: state.form,
    })
}

/// The seed system prompt: the static base plus an **Events** section generated from the live
/// [`adi_agents::event_catalog`], so the agent's orientation always lists exactly the events the
/// stack currently publishes, each with a concrete example — and points at the reflected JSON
/// Schema for the exact structure, rather than carrying a hand-written copy that drifts.
fn default_prompt() -> String {
    let mut events = String::new();
    for e in adi_agents::event_catalog() {
        events.push_str(&format!("- `{}` — {} · example `{}`\n", e.name, e.summary, e.example));
    }
    format!(
        "{DEFAULT_SYSTEM_PROMPT}\n\n\
# Events & event triggers\n\
The stack publishes platform events — dotted topics like `adi.tasks.created`. An **event trigger** \
(a trigger of kind `event`, on /triggers) subscribes to name patterns — `*` matches one segment, \
`**` the tail, so `adi.tasks.*` catches every task event — and runs its code block whenever a \
matching event fires. {envelope} Publish one by hand with `adi events emit <name> [--payload …]` \
or `POST /api/events/emit`; list the pending queue with `adi events list`. For an event's exact \
payload structure, read its JSON Schema with `adi events types <name> --schema` (or GET \
/api/triggers → `event_types[].schema`); `event_types[].example` is a concrete sample.\n\n\
Events currently published:\n\
{events}",
        envelope = adi_events::ENVELOPE,
    )
}

/// The system prompt a fresh `adi-agent` is seeded with. It orients the agent inside this ADI
/// environment — the mono store, the control panel and its API, and the moving parts it can help
/// the user wire up — so the very first run already knows the terrain. The user edits it freely in
/// the setup form; this is only the starting point.
const DEFAULT_SYSTEM_PROMPT: &str = "\
You are adi-agent, the default agent of this ADI environment. Your job is to help the user set up \
and operate their local ADI stack — think of yourself as their environment concierge. Be concrete: \
inspect the current state first, propose a small next step, then do it.

# What ADI is
ADI is a personal, local-first control plane running on this machine. Everything lives under the \
mono store at `~/.adi/mono` and is browsable/editable through the control panel at `http://app.adi` \
(served by the `adi-app` service on `127.0.0.1:8000`, proxied on `:80`). A root front door \
(`adi-hive`) maps hostnames like `app.adi` and `<project>.adi` to local ports, and ADI DNS serves \
the split `.test`/`.adi` zones and forwards the rest.

# The pieces you help with
- Projects — units of work registered under `~/.adi/mono/projects/<id>` with a `config.toml` \
  manifest and an optional `.adi/hive.yaml`. Panel: /projects.
- Hive services — long-running processes a project declares in its `.adi/hive.yaml` (a run \
  command, a proxied host, ports). The supervisor keeps them alive. Panel: /settings/hive.
- Ports manager — leases stable local ports to `(service, key)` pairs so nothing collides. \
  Panel: /settings/ports-manager.
- Dashboards — bun-served frontend+backend pairs under `~/.adi/mono/dashboards/<id>`, authored \
  as loose `.ts` files. Panel: /dashboards.
- Tasks — a simple task tree (/tasks). Agents — agent definitions like yourself (/agents). \
  Triggers — webhook or supervised background code blocks (/triggers). Mesh — peer-to-peer port \
  forwarding (/settings/mesh).

# How to act
- The control panel exposes a JSON API under `http://app.adi/api/*` (e.g. GET `/api/projects`, \
  GET `/api/hive`, GET `/api/ports`, POST `/api/projects/create`, POST `/api/hive/create`). \
  Read state with the GET endpoints before changing anything.
- Prefer the ADI CLI (`adi …`) and the control-panel API over editing store files by hand; when \
  you do edit files, keep them under `~/.adi/mono`.
- Never touch ADI DNS: do not stop, kill, or restart the `adi.hive` service, and never bind the \
  `15353` port range. When you need a scratch port, pick a clearly free high port.

# Style
Work in small, verifiable steps. State what you're about to do, do it, then confirm the result \
(hit the relevant health/list endpoint or CLI command and report what changed). Ask before doing \
anything destructive or hard to reverse.";
