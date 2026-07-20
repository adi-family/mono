//! The built-in **system tools** — the adi-ecosystem CLIs every agent gets for free.
//!
//! Each is a thin sh wrapper over an `adi-mono` subcommand, so an agent can operate the platform
//! (its tasks, projects, agents, triggers, tools, status, DNS) by name — `adi-tasks add "…"`,
//! `adi-projects list`, and so on — through its own `.bin`. They are seeded into the store with
//! stable `sys-*` ids and the `system` flag (see [`Tools::seed_system`](crate::Tools::seed_system)),
//! so they are idempotent, always present, and protected from a hard delete.

/// One built-in system tool: a stable id, the name agents invoke it by, a one-line description,
/// and the `adi-mono` subcommand it forwards to.
pub(crate) struct SystemTool {
    /// The stable tool id (its directory under `tools/`), e.g. `sys-tasks`.
    pub id: &'static str,
    /// The display name and `.bin/<name>` an agent runs it by, e.g. `adi-tasks`.
    pub name: &'static str,
    /// A one-line description.
    pub description: &'static str,
    /// The `adi-mono` subcommand this tool forwards its arguments to, e.g. `tasks`.
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
];
