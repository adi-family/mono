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

/// The real, resolved store root as a display string, with forward slashes so it reads and pastes
/// cleanly everywhere — Windows accepts `/` in paths and has no `~`, and the API takes `/` too.
#[must_use]
pub fn store_root_display(cfg: &Config) -> String {
    cfg.root().display().to_string().replace('\\', "/")
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
    // Bake this machine's real store path into the seeded copy, so the guide names a directory
    // that exists here rather than the unexpandable `~` shorthand.
    let root = store_root_display(cfg);
    for g in GUIDES {
        let path = dir.join(g.file);
        if !path.exists() {
            let _ = std::fs::write(&path, g.body.replace(STORE_SHORTHAND, &root));
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
