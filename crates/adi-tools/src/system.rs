//! The built-in **system tools** — the adi-ecosystem CLIs every agent gets for free.
//!
//! Each is a thin sh wrapper over an `adi-mono` subcommand, so an agent can operate the platform
//! (its tasks, projects, agents, triggers, tools, status, DNS) by name — `adi-tasks add "…"`,
//! `adi-projects list`, and so on — through its own `.bin`. They are seeded into the store with
//! stable `sys-*` ids and the `system` flag (see [`Tools::seed_system`](crate::Tools::seed_system)),
//! so they are idempotent, always present, and protected from a hard delete.

/// The knowledge CLI's stable id.
///
/// Exported because the agent launch path adds this one shim on its own: an agent configured with
/// a memory or a knowledge base has *asked* for knowledge, and a setting it cannot reach would be
/// a setting that does nothing. Named here so the two crates cannot drift apart on a string.
pub const SYS_KNOWLEDGE: &str = "sys-knowledge";

/// The **root** knowledge CLI's stable id: the same command group, run as the owner of the store.
///
/// Its whole reason to exist is the one thing plain `adi-knowledge` will not do — write into
/// another agent's memory. An agent's own isolation makes `agent:<somebody-else>/…` read-only, and
/// that is right for ordinary work: a memory nobody else can rewrite is what makes it *theirs*.
/// But a reviewer that has just worked out how an agent should be run has learned something for
/// *that* agent, and the only shelf it belongs on is the one that agent reads first.
///
/// **This is a seatbelt, not a lock.** Every shim already forwards to `adi-mono`, so `adi-mono` is
/// on every agent's `PATH` and any agent with a shell can pass `--root` for itself. What giving an
/// agent this tool buys is intent: it is named in the tool list, it is legible in the manifest, and
/// there is one place to say what it is for. Enforcement was never available at this layer — see
/// the note on isolation in `docs/knowledge.md`.
pub const SYS_KNOWLEDGE_ROOT: &str = "sys-knowledge-root";

/// One built-in system tool: a stable id, the name agents invoke it by, a one-line description,
/// and the `adi-mono` subcommand it forwards to.
pub(crate) struct SystemTool {
    /// The stable tool id (its directory under `tools/`), e.g. `sys-tasks`.
    pub id: &'static str,
    /// The display name and `.bin/<name>` an agent runs it by, e.g. `adi-tasks`.
    pub name: &'static str,
    /// A one-line description.
    pub description: &'static str,
    /// The `adi-mono` subcommand this tool forwards its arguments to, e.g. `tasks` — with any
    /// fixed arguments that always precede the caller's own, as `knowledge --root` does. Those
    /// belong here rather than in a field of their own: they are part of *which command this tool
    /// is*, and two tools over one subcommand that differ only by a flag is exactly the case.
    pub subcommand: &'static str,
}

impl SystemTool {
    /// The sh script body: forward every argument to `adi-mono <subcommand>`. `exec` replaces the
    /// wrapper process so the subcommand owns stdio and the exit code passes straight through.
    pub(crate) fn script(&self) -> String {
        format!(
            "#!/bin/sh\n\
             # {name} — a built-in adi system tool. Forwards to `adi-mono {sub}`.\n\
             # Managed by the platform; edits are overwritten when system tools are re-seeded.\n\
             exec adi-mono {sub} \"$@\"\n",
            name = self.name,
            sub = self.subcommand,
        )
    }
}

/// The catalog seeded into every store. Each entry maps a short agent-facing name to an `adi-mono`
/// subcommand, giving agents a curated CLI surface over the whole adi ecosystem.
pub(crate) const SYSTEM_TOOLS: &[SystemTool] = &[
    SystemTool {
        id: "sys-status",
        name: "adi-status",
        description: "Show live status across all adi services (add --json).",
        subcommand: "status",
    },
    SystemTool {
        id: "sys-projects",
        name: "adi-projects",
        description: "Register and manage adi projects (list/add/show/archive/…).",
        subcommand: "projects",
    },
    SystemTool {
        id: "sys-tasks",
        name: "adi-tasks",
        description: "Work the task tree (list/add/show/edit/archive/…).",
        subcommand: "tasks",
    },
    SystemTool {
        id: "sys-agents",
        name: "adi-agents",
        description: "Manage agent definitions and runs (list/add/run/…).",
        subcommand: "agents",
    },
    SystemTool {
        id: "sys-triggers",
        name: "adi-triggers",
        description: "Manage triggers — webhook/background code blocks (list/add/fire/…).",
        subcommand: "triggers",
    },
    SystemTool {
        id: "sys-tools",
        name: "adi-tools",
        description: "Manage tools themselves (list/add/link/run/…).",
        subcommand: "tools",
    },
    SystemTool {
        id: "sys-dns",
        name: "adi-dns",
        description: "Control the adi DNS resolver (status/enable/…).",
        subcommand: "dns",
    },
    SystemTool {
        id: "sys-db",
        name: "adi-db",
        description: "Run SQL against the shared SQLite store (query/exec/tables/schema/list). `--project P` for a project's database.",
        subcommand: "db",
    },
    SystemTool {
        id: "sys-secrets",
        name: "adi-secrets",
        description: "Read and manage secrets (list/read/set/rm). `adi-secrets read <NAME>` prints the value.",
        subcommand: "secrets",
    },
    SystemTool {
        id: SYS_KNOWLEDGE,
        name: "adi-knowledge",
        description: "Search and write knowledge bases — text notes ranked by meaning (search/add/list/get/edit/rm/bases). A base is `global/<name>`, `project:<id>/<name>`, or `agent:<name>/<base>`; `--as-agent <you>` applies your isolation.",
        subcommand: "knowledge",
    },
    SystemTool {
        id: SYS_KNOWLEDGE_ROOT,
        name: "adi-knowledge-root",
        description: "The knowledge CLI as the owner of the store: same verbs, no isolation — the one way to write into another agent's memory (`agent:<them>/memory`). Use it to leave an agent what it should have known; use plain `adi-knowledge` for everything else.",
        subcommand: "knowledge --root",
    },
];
