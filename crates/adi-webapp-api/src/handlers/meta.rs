//! The `/api/meta` surface: the state of the Meta page, which manages a single well-known global
//! agent named `adi-agent` — the default ADI agent. The page reuses the agents endpoints
//! (`/api/agents/save`, `/run`, `/peek`) to create and run it; this endpoint only reports whether
//! it exists, its current definition, and the defaults to seed a new one with (its system prompt
//! and the tools to enable on it).

use adi_agents::Agents;
use adi_config::Config;
use adi_tools::Tools;

use crate::types::MetaState;

use super::agents::agents_state;
use super::guides::{STORE_SHORTHAND, ensure_guides, prompt_section, store_root_display};
use super::response::{Response, ok_json};

/// The well-known name of the default ADI agent the Meta page manages. Creating the agent is an
/// ordinary `/api/agents/save` under this name; this handler just decides which stored agent is
/// "the" meta-agent.
pub const ADI_AGENT_NAME: &str = "adi-agent";

/// `GET /api/meta` — report the Meta page's state: the `adi-agent` definition (if it has been set
/// up), the defaults to seed a new one with (system prompt + enabled tools), and the agent form
/// schema (whose backend list drives the setup picker).
#[must_use]
pub fn meta(store: &Agents, tools: &Tools) -> Response {
    // Scaffold the built-in guides the agent's prompt points at, so they exist on disk by the
    // time the agent is created. Idempotent and non-destructive — it never overwrites edits.
    ensure_guides(store.config());
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
        default_prompt: default_prompt(store.config()),
        default_bin_tools: default_bin_tools(tools),
        agent,
        form: state.form,
    })
}

/// The tools a saved `adi-agent` gets enabled: **every active tool** in the store — the seeded
/// system CLIs (`adi-tasks`, `adi-projects`, …) and every user tool, project-scoped ones included.
/// The meta-agent operates the whole environment, so it is the one agent that should never be
/// missing a capability someone registered; the setup form unions this with whatever is already
/// enabled, so a save keeps up with tools created since.
///
/// Archived tools are left out — an archive is a deliberate "stop offering this". A store read
/// failure degrades to "no defaults" rather than failing the page.
fn default_bin_tools(tools: &Tools) -> Vec<String> {
    tools.list().map_or_else(
        |_| Vec::new(),
        |list| {
            list.into_iter()
                .filter(|t| !t.is_archived())
                .map(|t| t.id)
                .collect()
        },
    )
}

/// The seed system prompt: the static base plus an **Events** section generated from the live
/// [`adi_agents::event_catalog`], so the agent's orientation always lists exactly the events the
/// stack currently publishes, each with a concrete example — and points at the reflected JSON
/// Schema for the exact structure, rather than carrying a hand-written copy that drifts.
fn default_prompt(cfg: &Config) -> String {
    let mut events = String::new();
    for e in adi_agents::event_catalog() {
        events.push_str(&format!("- `{}` — {} · example `{}`\n", e.name, e.summary, e.example));
    }
    let prompt = format!(
        "{DEFAULT_SYSTEM_PROMPT}\n\n\
{guides}\n\
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
        guides = prompt_section(),
        envelope = adi_events::ENVELOPE,
    );
    // `~` has no meaning on Windows, so name the real resolved store path the agent can act on.
    // Every store reference shares the `~/.adi/mono` prefix, so one substitution rewrites them all.
    prompt.replace(STORE_SHORTHAND, &store_root_display(cfg))
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
- Hive services — long-running processes a project declares in its `.adi/hive.yaml` (a proxied \
  host, ports, and a `runner`). A runner is one of two kinds: `runner.script` (a shell command \
  run via `sh -c`) or `runner.docker` (a container — `image`, plus `ports` mapping each host \
  port key to a container port, `volumes`, `environment`, `pull`, `command`, and raw `args`). \
  Either way the supervisor keeps it alive (restart, backoff, hot-reload) and the front door \
  proxies to its loopback port. Create either with `POST /api/hive/create` (pass a `docker` block \
  for a container) or by editing the `.adi/hive.yaml`. Panel: /settings/hive.
- Ports manager — leases stable local ports to `(service, key)` pairs so nothing collides. \
  Panel: /settings/ports-manager.
- Dashboards — bun-served frontend+backend pairs under `~/.adi/mono/dashboards/<id>`, authored \
  as loose `.ts` files. Panel: /dashboards.
- Tools — small `sh`/`ts` CLIs that agents run, kept under `~/.adi/mono/tools/` and handed to an \
  agent as `.bin/<name>` shims on its PATH. A tool is either global or **filed under a project** \
  (`adi tools add <name> --project <id>`, or `\"project\"` in `POST /api/tools/create`); a \
  project-scoped tool runs in that project's directory, against that project's database. \
  Creating a tool gives it to nobody — an agent gets it only once it is **enabled on that \
  agent** (`bin_tools`: tick it on /agents, `adi agents save <name> --tool <id>`, or the \
  `bin_tools` field of `POST /api/agents/save`), which is what puts the shim on its PATH and its \
  help in its prompt. You yourself carry every tool in the store. Panel: /tools.
- Tasks — a simple task tree (/tasks). Agents — agent definitions like yourself (/agents). \
  Triggers — webhook or supervised background code blocks (/triggers). Mesh — peer-to-peer port \
  forwarding (/settings/mesh).

# How to act
- The control panel exposes a JSON API under `http://app.adi/api/*` (e.g. GET `/api/projects`, \
  GET `/api/hive`, GET `/api/ports`, POST `/api/projects/create`, POST `/api/hive/create`). \
  Read state with the GET endpoints before changing anything.
- Prefer the ADI CLI (`adi …`) and the control-panel API over editing store files by hand; when \
  you do edit files, keep them under `~/.adi/mono`.
- **When a job needs a capability you don't have, make it a tool** rather than a one-off you \
  redo every run — a shell incantation you'd repeat, a query you keep retyping, an API you keep \
  curl-ing. File it under the project it serves (`--project <id>`), keep it global only when it \
  is genuinely environment-wide, give it an `llm help` so the next agent learns it without being \
  told, and then **enable it on every agent that should have it** — a tool nobody has enabled is \
  a tool nobody can run. Read `guides/tools.md` before you write one.
- Never touch ADI DNS: do not stop, kill, or restart the `adi.hive` service, and never bind the \
  `15353` port range. When you need a scratch port, pick a clearly free high port.

# Style
Work in small, verifiable steps. State what you're about to do, do it, then confirm the result \
(hit the relevant health/list endpoint or CLI command and report what changed). Ask before doing \
anything destructive or hard to reverse.";

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Tools {
        let root = std::env::temp_dir().join(format!(
            "adi-meta-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Tools::with_config(Config::with_root(root))
    }

    #[test]
    fn the_meta_agent_defaults_to_every_active_tool() {
        let tools = scratch("all");
        tools.seed_system().expect("seed");
        let mine = tools
            .create_file("scraper", None, "sh", None, None)
            .expect("create");
        let scoped = tools
            .create_file("acme-deploy", None, "sh", Some("acme".into()), None)
            .expect("create scoped");

        let ids = default_bin_tools(&tools);
        // Every system CLI, the user's own tool, and a project-scoped one — the meta-agent runs
        // the whole environment, so nothing registered is withheld from it.
        assert!(ids.contains(&"sys-tasks".to_string()), "{ids:?}");
        assert!(ids.contains(&mine.id), "{ids:?}");
        assert!(ids.contains(&scoped.id), "{ids:?}");
    }

    #[test]
    fn an_archived_tool_is_not_a_default() {
        let tools = scratch("archived");
        let tool = tools
            .create_file("retired", None, "sh", None, None)
            .expect("create");
        tools.archive(&tool.id).expect("archive");
        assert!(!default_bin_tools(&tools).contains(&tool.id));
    }
}
