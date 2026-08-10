//! The built-in ADI guides: per-area Markdown docs seeded into `~/.adi/mono/guides/` and
//! referenced from the default agent system prompt. The agent reads the relevant guide before
//! working in an area, so it follows the current store paths and conventions rather than guessing.
//!
//! Seeding is idempotent and non-destructive: a file is written only when it's missing, so a
//! user's edits to a guide survive every later `/api/meta` read.

use std::path::PathBuf;

use adi_config::Config;

/// The store module the guides live under (`~/.adi/mono/guides`).
const GUIDES_MODULE: &str = "guides";

/// The `~/.adi/mono` shorthand the guide templates and the base prompt are written with. `~`
/// never expands on Windows, so every occurrence in agent-facing text is rewritten to the real
/// resolved store root (see [`store_root_display`]) before the agent ever sees it.
pub const STORE_SHORTHAND: &str = "~/.adi/mono";

/// The placeholder every guide template and the base prompt spell the ADI CLI with, rewritten to
/// [`CLI`] before an agent reads it. Agent-facing text must never hardcode the binary name: the
/// machine may also carry an unrelated legacy `adi` binary, and a command the agent types has to
/// be the one that exists *here*.
pub const CLI_SHORTHAND: &str = "{{cli}}";

/// The name of the ADI CLI binary as it is invoked on a PATH. Mirrors `[[bin]] name` in
/// `crates/adi-cli/Cargo.toml` — the two are renamed together, and this is the only place any
/// agent-facing text learns the name.
pub const CLI: &str = "adi-mono";

/// The real, resolved store root as a display string, with forward slashes so it reads and pastes
/// cleanly everywhere — Windows accepts `/` in paths and has no `~`, and the API takes `/` too.
#[must_use]
pub fn store_root_display(cfg: &Config) -> String {
    cfg.root().display().to_string().replace('\\', "/")
}

/// Resolve the shorthands every piece of agent-facing text is authored with — the store root and
/// the CLI name — so what an agent reads names things that exist on this machine.
#[must_use]
pub fn render(text: &str, cfg: &Config) -> String {
    text.replace(STORE_SHORTHAND, &store_root_display(cfg))
        .replace(CLI_SHORTHAND, CLI)
}

/// One built-in guide: the file it seeds, a one-line summary for the prompt index, and its body.
pub struct Guide {
    pub file: &'static str,
    pub summary: &'static str,
    pub body: &'static str,
}

/// The guides shipped with the app, in the order they read as a set. `README.md` is first so a
/// browser landing in the directory meets the index before the topic files.
pub const GUIDES: &[Guide] = &[
    Guide {
        file: "README.md",
        summary: "how these guides work — read this first",
        body: include_str!("../../templates/guides/README.md"),
    },
    Guide {
        file: "projects.md",
        summary: "registering and structuring units of work",
        body: include_str!("../../templates/guides/projects.md"),
    },
    Guide {
        file: "dashboards.md",
        summary: "building & editing dashboards (frontend + backend)",
        body: include_str!("../../templates/guides/dashboards.md"),
    },
    Guide {
        file: "tasks.md",
        summary: "tracking work in the task tree",
        body: include_str!("../../templates/guides/tasks.md"),
    },
    Guide {
        file: "services.md",
        summary: "long-running hive services a project supervises",
        body: include_str!("../../templates/guides/services.md"),
    },
    Guide {
        file: "triggers.md",
        summary: "webhook / background / event code blocks",
        body: include_str!("../../templates/guides/triggers.md"),
    },
    Guide {
        file: "tools.md",
        summary: "small sh/ts CLIs agents run",
        body: include_str!("../../templates/guides/tools.md"),
    },
    Guide {
        file: "agents.md",
        summary: "defining and running agents",
        body: include_str!("../../templates/guides/agents.md"),
    },
    Guide {
        file: "secrets.md",
        summary: "encrypted secrets and Gmail/Google OAuth",
        body: include_str!("../../templates/guides/secrets.md"),
    },
    Guide {
        file: "db.md",
        summary: "the shared SQLite database — storing data agents and dashboards both read",
        body: include_str!("../../templates/guides/db.md"),
    },
];

/// The directory guides live in: `~/.adi/mono/guides`.
#[must_use]
pub fn guides_dir(cfg: &Config) -> PathBuf {
    cfg.module(GUIDES_MODULE).dir().to_path_buf()
}

/// Ensure every built-in guide exists on disk, creating the directory and writing any missing
/// file. Idempotent and non-destructive: an existing file is left exactly as the user last
/// edited it. Best-effort — a filesystem error is swallowed, since a missing guide only degrades
/// the agent's orientation, it never breaks the page that seeds them.
pub fn ensure_guides(cfg: &Config) {
    let dir = guides_dir(cfg);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    // Bake this machine's real store path and CLI name into the seeded copy, so the guide names a
    // directory and a command that exist here rather than unexpandable shorthands.
    for g in GUIDES {
        let path = dir.join(g.file);
        if !path.exists() {
            let _ = std::fs::write(&path, render(g.body, cfg));
        }
    }
}

/// The `# Guides` section for the default system prompt: where the guides live, the standing
/// instruction to consult them, and an index of what each covers — generated from [`GUIDES`] so
/// the prompt never lists a guide the app doesn't ship.
#[must_use]
pub fn prompt_section() -> String {
    let mut index = String::new();
    for g in GUIDES {
        index.push_str(&format!("- `guides/{}` — {}\n", g.file, g.summary));
    }
    format!(
        "# Guides\n\
Task-specific guides live in `~/.adi/mono/guides/` — one Markdown file per area. **Before you \
build or change something in one of these areas, read its guide first** (`cat \
~/.adi/mono/guides/<file>`, or open it in the store file editor): each carries the current store \
paths, the CLI/API to use, and a worked example, so you follow this environment's conventions \
instead of guessing. They're plain Markdown — keep them up to date as the setup evolves.\n\n\
Available guides:\n\
{index}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The failure this guards against was real: the guides told agents to run `adi tasks list`,
    /// which on a machine carrying the older `adi` binary answers `✕ Unknown command`. Agent-facing
    /// text names the CLI through the placeholder, so there is exactly one place to rename.
    #[test]
    fn no_guide_hardcodes_a_cli_name() {
        for g in GUIDES {
            for (n, line) in g.body.lines().enumerate() {
                assert!(
                    !line.contains("`adi "),
                    "{}:{} spells a command with a hardcoded CLI name — use `{CLI_SHORTHAND}`: {line}",
                    g.file,
                    n + 1,
                );
            }
        }
    }

    #[test]
    fn render_resolves_both_shorthands() {
        let cfg = Config::with_root(std::env::temp_dir().join("adi-guides-render"));
        let out = render("run `{{cli}} tasks list` in ~/.adi/mono", &cfg);
        assert!(out.contains("adi-mono tasks list"), "{out}");
        assert!(!out.contains(STORE_SHORTHAND), "{out}");
        assert!(out.contains(&store_root_display(&cfg)), "{out}");
    }
}
