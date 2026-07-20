use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use adi_agents::AgentManifest;
use adi_agents::Agents;
use adi_agents::Backend;
use adi_agents::Error as AgentStoreError;
use adi_agents::StoredAgent;
use adi_agents::arguments::WasmArguments;
use adi_agents::contains_json_null;

use crate::types::{
    AgentBackendOption, AgentBuildResult, AgentCode, AgentDto, AgentFormField, AgentFormFieldKind,
    AgentFormOption, AgentFormSpec, AgentKeys, AgentPeek, AgentRef, AgentRunInfo, AgentRunResult,
    AgentRuns, AgentsState, RunAgent, RunRef, SaveAgent, SaveAgentCode,
};

use super::files::MAX_TEXT_BYTES;
use super::response::{Response, clean, error, ok_json};

/// `GET /api/agents` — every registered agent definition. Each mutation endpoint below returns a
/// fresh [`AgentsState`], so the client refreshes from one round-trip.
#[must_use]
pub fn agents(store: &Agents) -> Response {
    match agents_state(store) {
        Ok(state) => ok_json(&state),
        Err(e) => Response::from(&e),
    }
}

/// The full [`AgentsState`]: the stored definitions decorated with live run state, plus the form
/// schema. Tmux sessions are listed once; process agents consult their recorded PID. Shared with
/// the meta handler, which reuses it to find the well-known `adi-agent` and reads back the schema.
pub(crate) fn agents_state(store: &Agents) -> Result<AgentsState, AgentStoreError> {
    let sessions = adi_agents::running_sessions();
    Ok(AgentsState {
        agents: store
            .list()?
            .into_iter()
            .map(|a| agent_dto(store, a, &sessions))
            .collect(),
        form: agent_form_spec(),
    })
}

/// `POST /api/agents/run` — launch an agent in its backend. Tmux engines start an interactive
/// session you type into, so the `message` is optional there. Headless engines (`process` /
/// `harness`) get one shot: they run a single `--print` turn on `message` as the prompt and exit,
/// so a task is **required** — launching one with no message would just have it act on a placeholder
/// and do nothing, so that is rejected (400) rather than silently run.
#[must_use]
pub fn run_agent(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_run_agent(body) else {
        return bad_agent_ref();
    };
    let name = req.name.trim();
    let message = req.message.trim();
    let agent = match get_agent(store, name) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    let interactive = agent.manifest.executor() == "tmux";
    if !interactive && message.is_empty() {
        return error(
            400,
            "This backend runs headless (one --print turn), so it needs an initial task — enter what it should do before running.",
        );
    }
    let launch = if message.is_empty() {
        store.run(name)
    } else {
        store.run_with_message(name, message)
    };
    let launch = match launch {
        Ok(launch) => launch,
        Err(e) => return Response::from(&e),
    };
    let (message, run_id) = match launch {
        adi_agents::Launch::Tmux { session, .. } => (
            format!("Started “{name}” — attach: tmux attach -t {session}"),
            String::new(),
        ),
        adi_agents::Launch::Process {
            pid, log, run_id, ..
        } => (
            format!(
                "Started “{name}” as process {pid} — output: {}",
                log.display()
            ),
            run_id,
        ),
    };
    match agents_state(store) {
        Ok(state) => ok_json(&AgentRunResult {
            message,
            run_id,
            state,
        }),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/agents/runs` — a headless agent's run history, newest first (each Run is an independent
/// run of the agent's settings). Interactive (tmux) agents keep no history and answer `runs: []`.
#[must_use]
pub fn agent_runs(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_agent_ref(body) else {
        return bad_agent_ref();
    };
    match get_agent(store, req.name.trim()) {
        Ok(agent) => ok_json(&runs_response(store, &agent)),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/agents/run/peek` — a read-only snapshot of one specific run's log (or the tmux pane
/// for an interactive backend). A run that has produced nothing answers with empty output, not 404.
#[must_use]
pub fn peek_run(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_run_ref(body) else {
        return bad_run_ref();
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    let run_id = req.run_id.trim();
    let peek = store.peek_run(&agent, run_id);
    ok_json(&AgentPeek {
        name: agent.name.clone(),
        running: peek.running,
        output: peek.output,
        attach: peek.attach,
        interactive: peek.interactive,
        run_id: run_id.to_string(),
    })
}

/// `POST /api/agents/run/stop` — stop one specific run, then report the fresh run history. Idempotent
/// for an already-finished run; only an unknown agent is a 404.
#[must_use]
pub fn stop_run(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_run_ref(body) else {
        return bad_run_ref();
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    if let Err(e) = store.stop_run(&agent.name, req.run_id.trim()) {
        return Response::from(&e);
    }
    ok_json(&runs_response(store, &agent))
}

/// Build the [`AgentRuns`] history answer for an agent.
fn runs_response(store: &Agents, agent: &StoredAgent) -> AgentRuns {
    AgentRuns {
        name: agent.name.clone(),
        interactive: agent.manifest.executor() == "tmux",
        runs: store
            .runs(agent)
            .into_iter()
            .map(|r| AgentRunInfo {
                run_id: r.run_id,
                started_at: r.started_at,
                message: r.message,
                running: r.running,
            })
            .collect(),
    }
}

/// `POST /api/agents/save` — create or update an agent definition (an upsert keyed by `name`),
/// then report the fresh list. `name` and `backend` are required. Passing `rename_from` renames an
/// existing agent to `name` before applying the edit, instead of leaving the old manifest behind.
#[must_use]
pub fn save_agent(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_save_agent(body) else {
        return bad_save_agent();
    };
    if req.arguments.values().any(contains_json_null) {
        return error(
            400,
            "agent arguments cannot contain null (the manifest store is TOML)",
        );
    }
    let name = req.name.trim().to_string();
    // Move the manifest first, so the save below is an ordinary edit of an existing file — that is
    // what preserves `created_at`. A failed rename must abort the save, or the edit would land on
    // a fresh agent and strand the original.
    if let Some(from) = clean(req.rename_from).filter(|from| *from != name) {
        if let Err(e) = store.rename(&from, &name) {
            return Response::from(&e);
        }
    }
    let manifest = AgentManifest {
        backend: Backend::from(req.backend.trim()),
        arguments: clean_arguments(req.arguments),
        tags: req
            .tags
            .into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        starred: req.starred,
        project: clean(req.project),
        // The store owns the timestamps.
        created_at: 0,
        updated_at: 0,
    };
    match store.save(&name, manifest) {
        Ok(_) => agents(store),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/agents/delete` — delete an agent definition, then report the fresh list.
#[must_use]
pub fn delete_agent(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_agent_ref(body) else {
        return bad_agent_ref();
    };
    match store.delete(req.name.trim()) {
        Ok(_) => agents(store),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/agents/peek` — a read-only snapshot of a running agent's tmux pane, for the live
/// view. A registered agent without a live session answers `running: false` (200, not an error);
/// only an unknown name is a 404.
#[must_use]
pub fn peek_agent(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_agent_ref(body) else {
        return bad_agent_ref();
    };
    match get_agent(store, req.name.trim()) {
        Ok(agent) => peek_response(store, &agent),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/agents/send-keys` — type into a running agent's tmux session (the interactive
/// half of the live view): `text` is sent literally, then `key` is pressed. Replies with a
/// fresh pane snapshot after a short settle delay, so the sender sees the effect immediately.
#[must_use]
pub fn send_agent_keys(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_agent_keys(body) else {
        return bad_agent_keys();
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    if let Err(e) = adi_agents::send_keys(&agent.name, &req.text, &req.key) {
        return Response::from(&e);
    }
    // Give the TUI a beat to redraw, so the response snapshot already shows the keystrokes.
    std::thread::sleep(std::time::Duration::from_millis(120));
    peek_response(store, &agent)
}

/// `POST /api/agents/stop` — stop a live tmux session or detached process, then report the fresh
/// list. Idempotent for an already-stopped agent; only an unknown definition is a 404.
#[must_use]
pub fn stop_agent(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_agent_ref(body) else {
        return bad_agent_ref();
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    match store.stop(&agent.name) {
        Ok(_) => agents(store),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/agents/code` — read the employee source file a wasm agent's `src` argument points
/// at, for the code editor on the Agents page.
#[must_use]
pub fn agent_code(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_agent_ref(body) else {
        return bad_agent_ref();
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    let src = match agent_src(&agent) {
        Ok(src) => src,
        Err(resp) => return resp,
    };
    match std::fs::metadata(&src) {
        Ok(meta) if meta.len() > MAX_TEXT_BYTES => {
            return error(
                400,
                &format!(
                    "{src} is too large to edit ({} bytes, max {MAX_TEXT_BYTES})",
                    meta.len()
                ),
            );
        }
        _ => {}
    }
    match std::fs::read_to_string(&src) {
        Ok(code) => ok_json(&AgentCode {
            name: agent.name,
            path: src,
            code,
        }),
        Err(e) => error(400, &format!("couldn't read {src}: {e}")),
    }
}

/// `POST /api/agents/code/save` — write the code editor's buffer back to the wasm agent's
/// `src` file, replying with the fresh [`AgentCode`].
#[must_use]
pub fn save_agent_code(store: &Agents, body: &[u8]) -> Response {
    let Ok(req) = serde_json::from_slice::<SaveAgentCode>(body) else {
        return error(
            400,
            "expected JSON body { \"name\": \"…\", \"code\": \"…\" }",
        );
    };
    if req.name.trim().is_empty() {
        return error(
            400,
            "expected JSON body { \"name\": \"…\", \"code\": \"…\" }",
        );
    }
    if req.code.len() as u64 > MAX_TEXT_BYTES {
        return error(
            400,
            &format!("source too large to save (max {MAX_TEXT_BYTES} bytes)"),
        );
    }
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    let src = match agent_src(&agent) {
        Ok(src) => src,
        Err(resp) => return resp,
    };
    match std::fs::write(&src, req.code.as_bytes()) {
        Ok(()) => ok_json(&AgentCode {
            name: agent.name,
            path: src,
            code: req.code,
        }),
        Err(e) => error(500, &format!("couldn't write {src}: {e}")),
    }
}

/// `POST /api/agents/build` — compile a wasm agent's `src` TypeScript into its component:
/// `node <src dir>/node_modules/@adi-family/workforce-sdk/build.mjs <src> -o <src dir>/build`.
/// Blocks for the build (a few seconds), replies with its combined output. A successful build
/// fills in an empty `wasm` argument with the compiled path, making the agent dispatchable.
#[must_use]
pub fn build_agent(store: &Agents, body: &[u8]) -> Response {
    let Some(req) = parse_agent_ref(body) else {
        return bad_agent_ref();
    };
    let agent = match get_agent(store, req.name.trim()) {
        Ok(agent) => agent,
        Err(e) => return Response::from(&e),
    };
    let src = match agent_src(&agent) {
        Ok(src) => PathBuf::from(src),
        Err(resp) => return resp,
    };
    let Some(dir) = src.parent().map(Path::to_path_buf) else {
        return error(400, "the src argument has no parent directory");
    };
    let build_mjs = dir.join("node_modules/@adi-family/workforce-sdk/build.mjs");
    if !build_mjs.exists() {
        return error(
            400,
            &format!(
                "no workforce SDK next to the source ({} missing) — run `npm install` in {} first",
                build_mjs.display(),
                dir.display()
            ),
        );
    }
    let Some(node) = node_bin() else {
        return error(
            500,
            "no node binary found (tried $ADI_NODE, PATH, /opt/homebrew/bin, /usr/local/bin)",
        );
    };
    let out_dir = dir.join("build");

    // jco runs via a `#!/usr/bin/env node` shebang, so the child's PATH must reach node even
    // when this server was launched with a minimal LaunchAgent environment.
    let mut path_env = std::env::var("PATH").unwrap_or_default();
    if let Some(node_dir) = Path::new(&node)
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
    {
        path_env = format!("{}:{path_env}", node_dir.display());
    }

    let output = std::process::Command::new(&node)
        .arg(&build_mjs)
        .arg(&src)
        .arg("-o")
        .arg(&out_dir)
        .current_dir(&dir)
        .env("PATH", path_env)
        .output();
    let out = match output {
        Ok(out) => out,
        Err(e) => return error(500, &format!("couldn't spawn {node}: {e}")),
    };

    let mut text = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !stderr.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(stderr.trim_end());
    }
    let ok = out.status.success();

    let stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let wasm = out_dir.join(format!("{stem}.wasm")).display().to_string();
    // First successful build wires the component up; an explicit `wasm` argument is respected.
    let typed_manifest = agent.manifest.clone().into_typed::<WasmArguments>();
    if ok
        && typed_manifest
            .as_ref()
            .is_ok_and(|manifest| manifest.arguments.wasm.as_deref().is_none_or(str::is_empty))
    {
        let mut manifest = match typed_manifest {
            Ok(manifest) => manifest,
            Err(error) => return Response::from(&error),
        };
        manifest.arguments.wasm = Some(wasm.clone());
        if let Err(e) = store.save(&agent.name, manifest) {
            return Response::from(&e);
        }
    }

    match agents_state(store) {
        Ok(state) => ok_json(&AgentBuildResult {
            ok,
            output: text,
            wasm,
            state,
        }),
        Err(e) => Response::from(&e),
    }
}

/// The employee source path from an agent's `src` argument, or the 400 explaining how to set it.
fn agent_src(agent: &StoredAgent) -> Result<String, Response> {
    let arguments = agent
        .manifest
        .typed_arguments::<WasmArguments>()
        .map_err(|error| Response::from(&error))?;
    arguments.src.filter(|s| !s.is_empty()).ok_or_else(|| {
        error(
            400,
            &format!(
                "agent {} has no `src` argument pointing at its TypeScript source — \
                     set the Source path in the form (or --argument src=/path/to/employee.ts)",
                agent.name
            ),
        )
    })
}

/// The node binary the build runs with: `$ADI_NODE`, then PATH, then the usual install spots.
fn node_bin() -> Option<String> {
    if let Ok(node) = std::env::var("ADI_NODE")
        && !node.is_empty()
    {
        return Some(node);
    }
    if std::process::Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        return Some("node".to_string());
    }
    ["/opt/homebrew/bin/node", "/usr/local/bin/node"]
        .into_iter()
        .find(|p| Path::new(p).exists())
        .map(ToString::to_string)
}

/// Look an agent up, folding "not registered" into [`AgentStoreError::NotFound`] (→ 404).
fn get_agent(store: &Agents, name: &str) -> Result<StoredAgent, AgentStoreError> {
    store
        .get(name)?
        .ok_or_else(|| AgentStoreError::NotFound(name.to_string()))
}

/// The [`AgentPeek`] answer for an agent: a tmux pane capture for interactive backends, or the tail
/// of the detached run's log for the headless backends (which persists after the run ends). A
/// registered agent with nothing to show answers `running: false` with empty output, not an error.
fn peek_response(store: &Agents, agent: &StoredAgent) -> Response {
    let peek = store.peek(agent);
    ok_json(&AgentPeek {
        name: agent.name.clone(),
        running: peek.running,
        output: peek.output,
        attach: peek.attach,
        interactive: peek.interactive,
        run_id: String::new(),
    })
}

/// Flatten a stored agent into its wire [`AgentDto`], computing adapter and live run state.
fn agent_dto(
    store: &Agents,
    agent: StoredAgent,
    sessions: &std::collections::BTreeSet<String>,
) -> AgentDto {
    let executor = agent.manifest.executor().to_string();
    let runnable = adi_agents::is_runnable(&agent.manifest);
    let running = if executor == "tmux" {
        sessions.contains(&adi_agents::session_name(&agent.name))
    } else {
        store.is_running(&agent)
    };
    let m = agent.manifest;
    AgentDto {
        name: agent.name,
        backend: m.backend.to_string(),
        arguments: m.arguments,
        executor,
        tags: m.tags,
        starred: m.starred,
        project: m.project,
        created_at: m.created_at,
        updated_at: m.updated_at,
        runnable,
        running,
    }
}

/// The agentic-loop backend that picks its model provider at definition time (the `provider`
/// argument); every other backend has its engine baked into the `executor:what` id.
const ADI_HARNESS: &str = "harness:adi";

/// The adi-workforce employee backend: a compiled WASM component (TS → jco) the bundled engine
/// dispatches messages into. The component is named by the `wasm` argument.
const WASM_LOOP: &str = "wasm:loop-script";

/// The backends whose engine is the Claude CLI/SDK, whatever the executor.
const CLAUDE_BACKENDS: &[&str] = &["tmux:claude", "process:claude", "harness:claude-sdk"];

/// The backends whose engine is the Codex CLI.
const CODEX_BACKENDS: &[&str] = &["tmux:codex", "process:codex"];

/// The built-in Claude Code tools offered as one-tap toggles on the allow/deny tool pickers.
/// These are the bare tool names; a scoped specifier (e.g. `Bash(git *)`) is still typed by hand
/// into the same field. Kept in the order they read best in the picker, not alphabetically.
const CLAUDE_TOOLS: &[&str] = &[
    "Read",
    "Edit",
    "Write",
    "Bash",
    "Glob",
    "Grep",
    "Task",
    "TodoWrite",
    "NotebookEdit",
    "WebFetch",
    "WebSearch",
    "BashOutput",
    "KillShell",
    "ExitPlanMode",
    "SlashCommand",
];

/// Suggested models per backend, offered as one-tap chips on the Model picker. These mirror each
/// backend's `model_placeholder` — the canonical aliases/ids for that engine — while the field
/// stays free text for anything else (a full id, a provider-specific or local model).
const CLAUDE_CLI_MODELS: &[&str] = &["opus", "sonnet", "haiku", "fable"];
const CLAUDE_SDK_MODELS: &[&str] = &["claude-opus-4-8", "claude-sonnet-5", "claude-haiku-4-5"];
const CODEX_MODELS: &[&str] = &["gpt-5-codex"];
const ADI_MODELS: &[&str] = &["kimi-k2.6", "gemini-2.5-pro"];

/// Static backend/form metadata for the Agents page. This lives server-side so the API defines
/// both the selectable backends and the field shape the client renders. Backends are
/// `executor:what` pairs — the executor (`tmux` / `process` / `harness` / `wasm`) is the run
/// mechanism, the suffix is what it runs.
#[allow(clippy::too_many_lines)]
fn agent_form_spec() -> AgentFormSpec {
    let mut fields = Vec::new();

    let mut name = agent_field("name", "Name", AgentFormFieldKind::Text);
    name.placeholder = "athz-solver".into();
    name.hint = "a task tagged this name auto-starts it".into();
    name.mono = true;
    name.required = true;
    fields.push(name);

    let mut backend = agent_field("backend", "Backend", AgentFormFieldKind::Select);
    backend.required = true;
    fields.push(backend);

    // The project the agent is filed under (or global). The options are the registered
    // projects, which only the client knows live — it special-cases this field by name and
    // fills the select from its projects state, like the Triggers form does.
    let mut project = agent_field("project", "Project", AgentFormFieldKind::Select);
    project.hint = "shows on that project's page".into();
    fields.push(project);

    // The adi harness runs its own agentic loop and needs to know which provider API to call;
    // provider-specific knobs below are scoped to this choice via `providers`.
    let mut provider = field_ids(
        "provider",
        "Provider",
        AgentFormFieldKind::Select,
        &[ADI_HARNESS],
    );
    provider.options = opts(&[
        ("", "— pick a provider —"),
        ("anthropic", "Anthropic"),
        ("openai", "OpenAI"),
        ("gemini", "Gemini"),
        ("monshoot", "Monshoot"),
        ("ollama", "Ollama (local)"),
    ]);
    provider.hint = "model provider the adi loop calls".into();
    fields.push(provider);

    let mut model = agent_field("model", "Model", AgentFormFieldKind::ModelPicker);
    model.placeholder = "model alias".into();
    model.hint = "tap a suggestion for the chosen backend, or type any model".into();
    model.mono = true;
    model.wide = true;
    fields.push(model);

    // ---- wasm employees (adi-workforce) ----
    let mut src = txt_field(
        "src",
        "Source path",
        &[WASM_LOOP],
        "/path/to/employee.ts",
        "TypeScript source the Code editor edits and builds",
    );
    src.wide = true;
    fields.push(src);

    let mut wasm = txt_field(
        "wasm",
        "Component path",
        &[WASM_LOOP],
        "/path/to/agent.wasm",
        "compiled component; a successful Build fills this in",
    );
    wasm.wide = true;
    fields.push(wasm);

    // ---- claude engines (any executor) ----
    let mut permission = field_ids(
        "permission_mode",
        "Permission mode",
        AgentFormFieldKind::Select,
        CLAUDE_BACKENDS,
    );
    permission.options = opts(&[
        ("", "— default —"),
        ("acceptEdits", "acceptEdits"),
        ("auto", "auto"),
        ("bypassPermissions", "bypassPermissions"),
        ("manual", "manual"),
        ("dontAsk", "dontAsk"),
        ("plan", "plan"),
    ]);
    fields.push(permission);

    fields.push(for_providers(
        sel_field(
            "effort",
            "Effort",
            CLAUDE_BACKENDS,
            opts(&[
                ("", "— default —"),
                ("low", "low"),
                ("medium", "medium"),
                ("high", "high"),
                ("xhigh", "xhigh"),
                ("max", "max"),
            ]),
            "thinking / reasoning depth",
        ),
        &["anthropic"],
    ));

    fields.push(sel_field(
        "output_format",
        "Output format",
        &["process:claude"],
        opts(&[
            ("", "text (default)"),
            ("json", "json"),
            ("stream-json", "stream-json"),
        ]),
        "how the run result is emitted",
    ));

    fields.push(tools_field(
        "allowed_tools",
        "Allowed tools",
        "Bash(git *) Edit Read",
        "built-in tools to allow — tap to toggle, or type a scoped rule like Bash(git *)",
    ));

    fields.push(tools_field(
        "disallowed_tools",
        "Disallowed tools",
        "Bash(rm *) WebFetch",
        "built-in tools to deny — tap to toggle, or type a scoped rule like Bash(rm *)",
    ));

    fields.push(num_field(
        "max_budget_usd",
        "Max budget (USD)",
        &["process:claude"],
        "e.g. 5",
        "hard spend cap (print mode)",
    ));

    fields.push(txt_field(
        "fallback_model",
        "Fallback model",
        &["process:claude", "harness:claude-sdk"],
        "sonnet",
        "used when the primary model is overloaded",
    ));

    let mut append = field_ids(
        "append_system_prompt",
        "Append system prompt",
        AgentFormFieldKind::Textarea,
        CLAUDE_BACKENDS,
    );
    append.placeholder = "Appended after the default system prompt…".into();
    append.wide = true;
    fields.push(append);

    // ---- codex engines (any executor) ----
    fields.push(sel_field(
        "sandbox",
        "Sandbox",
        CODEX_BACKENDS,
        opts(&[
            ("", "— default —"),
            ("read-only", "read-only"),
            ("workspace-write", "workspace-write"),
            ("danger-full-access", "danger-full-access"),
        ]),
        "filesystem / exec sandbox policy",
    ));

    fields.push(sel_field(
        "approval",
        "Approval",
        CODEX_BACKENDS,
        opts(&[
            ("", "— default —"),
            ("untrusted", "untrusted"),
            ("on-request", "on-request"),
            ("never", "never"),
        ]),
        "when to ask before running a command",
    ));

    fields.push(for_providers(
        sel_field(
            "reasoning_effort",
            "Reasoning effort",
            CODEX_BACKENDS,
            opts(&[
                ("", "— default —"),
                ("low", "low"),
                ("medium", "medium"),
                ("high", "high"),
            ]),
            "reasoning depth",
        ),
        &["openai"],
    ));

    fields.push(txt_field(
        "working_dir",
        "Working dir",
        CODEX_BACKENDS,
        "/path/to/repo",
        "agent working root (-C)",
    ));

    fields.push(chk_field(
        "skip_git_repo_check",
        "Skip git-repo check",
        &["process:codex"],
    ));
    fields.push(chk_field("web_search", "Web search", CODEX_BACKENDS));
    fields.push(chk_field("json_events", "JSONL events", &["process:codex"]));

    // ---- tmux/process shared (a vendor CLI runs either way) ----
    let mut add_dir = field_executors(
        "add_dir",
        "Add dir",
        AgentFormFieldKind::Text,
        &["tmux", "process"],
    );
    add_dir.placeholder = "/extra/writable/dir".into();
    add_dir.hint = "additional writable directory".into();
    add_dir.mono = true;
    add_dir.wide = true;
    fields.push(add_dir);

    // ---- harness:adi provider knobs (scoped to the `provider` argument) ----
    fields.push(for_providers(
        sel_field(
            "thinking",
            "Thinking",
            &[],
            opts(&[
                ("", "— default —"),
                ("adaptive", "adaptive"),
                ("disabled", "disabled"),
            ]),
            "extended-thinking mode",
        ),
        &["anthropic"],
    ));

    fields.push(for_providers(
        num_field(
            "frequency_penalty",
            "Frequency penalty",
            &[],
            "-2.0 – 2.0",
            "",
        ),
        &["openai"],
    ));
    fields.push(for_providers(
        num_field(
            "presence_penalty",
            "Presence penalty",
            &[],
            "-2.0 – 2.0",
            "",
        ),
        &["openai", "monshoot"],
    ));
    fields.push(for_providers(
        sel_field(
            "response_format",
            "Response format",
            &[],
            opts(&[
                ("", "— default —"),
                ("text", "text"),
                ("json_object", "json_object"),
                ("json_schema", "json_schema"),
            ]),
            "structured output",
        ),
        &["openai", "monshoot"],
    ));

    fields.push(for_providers(
        num_field(
            "thinking_budget",
            "Thinking budget",
            &[],
            "tokens",
            "thinkingConfig budget",
        ),
        &["gemini"],
    ));

    fields.push(for_providers(
        num_field(
            "num_ctx",
            "Context size",
            &[],
            "e.g. 8192",
            "context window (num_ctx)",
        ),
        &["ollama"],
    ));
    fields.push(for_providers(
        num_field("repeat_penalty", "Repeat penalty", &[], "e.g. 1.1", ""),
        &["ollama"],
    ));
    fields.push(for_providers(
        num_field("min_p", "Min-p", &[], "0.0 – 1.0", ""),
        &["ollama"],
    ));
    fields.push(for_providers(
        txt_field(
            "keep_alive",
            "Keep alive",
            &[],
            "5m / -1",
            "how long to keep the model loaded",
        ),
        &["ollama"],
    ));
    fields.push(for_providers(
        chk_field("think", "Thinking", &[]),
        &["ollama"],
    ));
    fields.push(for_providers(
        sel_field(
            "format",
            "Response format",
            &[],
            opts(&[("", "— default —"), ("json", "json")]),
            "structured output",
        ),
        &["ollama"],
    ));

    // ---- harness:adi sampling (provider-scoped) ----
    // temperature is left OFF the providers where a non-default value 400s: Anthropic current
    // models, OpenAI o-series/gpt-5, and Monshoot kimi-k2.6 (verified). It stays only where it's
    // a normal knob — Gemini and Ollama.
    fields.push(for_providers(
        num_field("temperature", "Temperature", &[], "0.0 – 2.0", ""),
        &["gemini", "ollama"],
    ));
    fields.push(for_providers(
        num_field("top_p", "Top-p", &[], "0.0 – 1.0", ""),
        &["openai", "gemini", "monshoot", "ollama"],
    ));
    fields.push(for_providers(
        num_field("top_k", "Top-k", &[], "e.g. 40", ""),
        &["gemini", "ollama"],
    ));
    fields.push(for_providers(
        num_field("seed", "Seed", &[], "e.g. 42", "deterministic sampling"),
        &["openai", "gemini", "ollama"],
    ));

    // ---- harness:adi shared (whatever the provider) ----
    let mut max_tokens = field_ids(
        "max_tokens",
        "Max output tokens",
        AgentFormFieldKind::Number,
        &[ADI_HARNESS],
    );
    max_tokens.placeholder = "e.g. 4096".into();
    max_tokens.hint = "maps to each provider's output-cap field".into();
    max_tokens.numeric = true;
    fields.push(max_tokens);

    let mut stop = field_ids(
        "stop",
        "Stop sequences",
        AgentFormFieldKind::Text,
        &[ADI_HARNESS],
    );
    stop.placeholder = "comma-separated".into();
    stop.hint = "stop generation on these strings".into();
    stop.mono = true;
    stop.wide = true;
    fields.push(stop);

    let mut max_turns = field_ids(
        "max_turns",
        "Max turns",
        AgentFormFieldKind::Number,
        &[ADI_HARNESS, "harness:claude-sdk", WASM_LOOP],
    );
    max_turns.placeholder = "optional".into();
    max_turns.hint = "harness cap on agent turns per run".into();
    max_turns.numeric = true;
    fields.push(max_turns);

    let mut api_key_env = field_ids(
        "api_key_env",
        "API key env",
        AgentFormFieldKind::Text,
        &[ADI_HARNESS],
    );
    api_key_env.placeholder = "OPENAI_API_KEY".into();
    api_key_env.hint = "environment variable read for the chosen provider".into();
    api_key_env.mono = true;
    fields.push(api_key_env);

    let mut base_url = field_ids(
        "base_url",
        "Base URL",
        AgentFormFieldKind::Text,
        &[ADI_HARNESS],
    );
    base_url.placeholder = "provider endpoint override".into();
    base_url.hint = "e.g. https://api.moonshot.ai/v1 · http://localhost:11434".into();
    base_url.mono = true;
    base_url.wide = true;
    fields.push(base_url);

    // ---- always shown ----
    fields.push(agent_field(
        "starred",
        "Starred",
        AgentFormFieldKind::Checkbox,
    ));

    let mut tags = agent_field("tags", "Tags", AgentFormFieldKind::Text);
    tags.placeholder = "comma-separated (dispatch / filtering)".into();
    tags.wide = true;
    fields.push(tags);

    let mut tools = field_ids(
        "tools",
        "CLI commands",
        AgentFormFieldKind::Text,
        &[ADI_HARNESS, "harness:claude-sdk", WASM_LOOP],
    );
    tools.placeholder = "tasks,projects,agents".into();
    tools.hint = "which adi-mono command groups this agent may use".into();
    tools.mono = true;
    tools.wide = true;
    fields.push(tools);

    let mut prompt = agent_field(
        "system_prompt",
        "System prompt",
        AgentFormFieldKind::Textarea,
    );
    prompt.placeholder = "The system prompt that seeds this agent...".into();
    prompt.wide = true;
    fields.push(prompt);

    AgentFormSpec {
        backends: vec![
            agent_backend(
                "tmux:claude",
                "tmux · Claude CLI",
                "tmux",
                "opus / sonnet / fable / haiku",
                CLAUDE_CLI_MODELS,
            ),
            agent_backend(
                "tmux:codex",
                "tmux · Codex CLI",
                "tmux",
                "gpt-5-codex",
                CODEX_MODELS,
            ),
            agent_backend(
                "process:claude",
                "process · Claude CLI",
                "process",
                "opus / sonnet / fable / haiku",
                CLAUDE_CLI_MODELS,
            ),
            agent_backend(
                "process:codex",
                "process · Codex CLI",
                "process",
                "gpt-5-codex",
                CODEX_MODELS,
            ),
            agent_backend(
                "harness:claude-sdk",
                "harness · Claude SDK",
                "harness",
                "claude-opus-4-8 / claude-sonnet-5",
                CLAUDE_SDK_MODELS,
            ),
            agent_backend(
                ADI_HARNESS,
                "harness · ADI loop",
                "harness",
                "provider model, e.g. kimi-k2.6 / gemini-2.5-pro",
                ADI_MODELS,
            ),
            agent_backend(
                WASM_LOOP,
                "wasm · Workforce employee",
                "wasm",
                "set by the employee's loop config",
                &[],
            ),
        ],
        fields,
    }
}

fn agent_backend(
    id: &str,
    label: &str,
    executor: &str,
    model_placeholder: &str,
    model_suggestions: &[&str],
) -> AgentBackendOption {
    AgentBackendOption {
        id: id.into(),
        label: label.into(),
        executor: executor.into(),
        model_placeholder: model_placeholder.into(),
        model_suggestions: strings(model_suggestions),
    }
}

fn agent_field(name: &str, label: &str, kind: AgentFormFieldKind) -> AgentFormField {
    AgentFormField {
        name: name.into(),
        label: label.into(),
        kind,
        placeholder: String::new(),
        hint: String::new(),
        options: Vec::new(),
        backend_ids: Vec::new(),
        executors: Vec::new(),
        providers: Vec::new(),
        mono: false,
        wide: false,
        numeric: false,
        required: false,
    }
}

fn agent_option(value: &str, label: &str) -> AgentFormOption {
    AgentFormOption {
        value: value.into(),
        label: label.into(),
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_string()).collect()
}

/// A field visible only for specific backend ids (e.g. `tmux:claude`).
fn field_ids(name: &str, label: &str, kind: AgentFormFieldKind, ids: &[&str]) -> AgentFormField {
    let mut f = agent_field(name, label, kind);
    f.backend_ids = strings(ids);
    f
}

/// A field visible for whole executors (`tmux` / `process` / `harness`).
fn field_executors(
    name: &str,
    label: &str,
    kind: AgentFormFieldKind,
    executors: &[&str],
) -> AgentFormField {
    let mut f = agent_field(name, label, kind);
    f.executors = strings(executors);
    f
}

/// Also show a field when `harness:adi` targets one of these providers (on top of whatever
/// backend-id scoping the field already carries).
fn for_providers(mut f: AgentFormField, providers: &[&str]) -> AgentFormField {
    f.providers = strings(providers);
    f
}

/// A select field scoped to backend ids, with a hint.
fn sel_field(
    name: &str,
    label: &str,
    ids: &[&str],
    options: Vec<AgentFormOption>,
    hint: &str,
) -> AgentFormField {
    let mut f = field_ids(name, label, AgentFormFieldKind::Select, ids);
    f.options = options;
    f.hint = hint.into();
    f
}

/// A numeric field scoped to backend ids.
fn num_field(
    name: &str,
    label: &str,
    ids: &[&str],
    placeholder: &str,
    hint: &str,
) -> AgentFormField {
    let mut f = field_ids(name, label, AgentFormFieldKind::Number, ids);
    f.placeholder = placeholder.into();
    f.hint = hint.into();
    f.numeric = true;
    f
}

/// A monospace text field scoped to backend ids.
fn txt_field(
    name: &str,
    label: &str,
    ids: &[&str],
    placeholder: &str,
    hint: &str,
) -> AgentFormField {
    let mut f = field_ids(name, label, AgentFormFieldKind::Text, ids);
    f.placeholder = placeholder.into();
    f.hint = hint.into();
    f.mono = true;
    f
}

/// A checkbox scoped to backend ids (stored as a boolean backend argument).
fn chk_field(name: &str, label: &str, ids: &[&str]) -> AgentFormField {
    field_ids(name, label, AgentFormFieldKind::Checkbox, ids)
}

/// A tool-picker for the Claude backends: toggle chips for [`CLAUDE_TOOLS`] over a free-text
/// input, both editing the one space-separated tool spec (`--allowed-tools` / `--disallowed-tools`).
fn tools_field(name: &str, label: &str, placeholder: &str, hint: &str) -> AgentFormField {
    let mut f = field_ids(name, label, AgentFormFieldKind::ToolPicker, CLAUDE_BACKENDS);
    f.options = CLAUDE_TOOLS.iter().map(|&t| agent_option(t, t)).collect();
    f.placeholder = placeholder.into();
    f.hint = hint.into();
    f.mono = true;
    f.wide = true;
    f
}

/// Build a select-option list from `(value, label)` pairs.
fn opts(pairs: &[(&str, &str)]) -> Vec<AgentFormOption> {
    pairs.iter().map(|&(v, l)| agent_option(v, l)).collect()
}

// Map an agent-store error to an HTTP status: bad name / unrunnable backend / bad key → 400,
// missing → 404, wrong run state (already / not running) → 409, else 500.
impl From<&AgentStoreError> for Response {
    fn from(e: &AgentStoreError) -> Self {
        let status = match e {
            AgentStoreError::Arguments(_)
            | AgentStoreError::InvalidName(_)
            | AgentStoreError::NotRunnable(_)
            | AgentStoreError::InvalidKey(_) => 400,
            AgentStoreError::NotFound(_) => 404,
            AgentStoreError::Exists(_)
            | AgentStoreError::AlreadyRunning(_)
            | AgentStoreError::NotRunning(_) => 409,
            AgentStoreError::Config(_)
            | AgentStoreError::Io(_)
            | AgentStoreError::Launch(_)
            | AgentStoreError::Tmux(_)
            | AgentStoreError::Process(_) => 500,
        };
        error(status, &e.to_string())
    }
}

fn parse_save_agent(body: &[u8]) -> Option<SaveAgent> {
    let req: SaveAgent = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty() && !req.backend.trim().is_empty()).then_some(req)
}

fn bad_save_agent() -> Response {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"backend\": \"…\", … } with a non-empty name and backend",
    )
}

fn parse_agent_ref(body: &[u8]) -> Option<AgentRef> {
    let req: AgentRef = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty()).then_some(req)
}

fn parse_run_agent(body: &[u8]) -> Option<RunAgent> {
    let req: RunAgent = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty()).then_some(req)
}

fn parse_run_ref(body: &[u8]) -> Option<RunRef> {
    let req: RunRef = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty() && !req.run_id.trim().is_empty()).then_some(req)
}

fn bad_run_ref() -> Response {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"run_id\": \"…\" } with a non-empty name and run_id",
    )
}

fn bad_agent_ref() -> Response {
    error(400, "expected JSON body { \"name\": \"…\" }")
}

fn parse_agent_keys(body: &[u8]) -> Option<AgentKeys> {
    let req: AgentKeys = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty() && (!req.text.is_empty() || !req.key.is_empty())).then_some(req)
}

fn bad_agent_keys() -> Response {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"text\": \"…\", \"key\": \"…\" } with a non-empty name and at least one of text/key",
    )
}

/// Normalize only the key at the shared top-level boundary. Argument values and nested manifests
/// are preserved exactly because their shape belongs to the selected backend.
fn clean_arguments(
    arguments: BTreeMap<String, serde_json::Value>,
) -> BTreeMap<String, serde_json::Value> {
    arguments
        .into_iter()
        .filter_map(|(key, value)| {
            let key = key.trim().to_string();
            if key.is_empty() {
                return None;
            }
            Some((key, value))
        })
        .collect()
}
