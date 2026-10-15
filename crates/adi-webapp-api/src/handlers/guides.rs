//! The built-in ADI guides: per-area Markdown docs seeded into `~/.adi/mono/guides/` and
//! referenced from the default agent system prompt. The agent reads the relevant guide before
//! working in an area, so it follows the current store paths and conventions rather than guessing.
//!
//! Seeding also *refreshes* a guide when the app ships a newer version of it, without clobbering
//! a user's own edits: a sidecar file, `guides/.shipped.json`, remembers the hash of what ADI
//! itself last wrote for each guide. A copy that still hashes to that (or, for an install that
//! predates the sidecar, to any hash ADI has ever shipped for that file — see
//! [`historical_hashes`]) is unedited and safe to replace; anything else is a person's own text,
//! left untouched with the new version written beside it as `<file>.new`. See [`ensure_guides`]
//! for the exact rule.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use adi_config::Config;
use sha2::{Digest, Sha256};

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
    Guide {
        file: "channels.md",
        summary: "connecting Telegram/Slack to an agent, and posting back mid-run",
        body: include_str!("../../templates/guides/channels.md"),
    },
    Guide {
        file: "workflows.md",
        summary: "TypeScript workflows: checkpoints, schedules, events, rewind",
        body: include_str!("../../templates/guides/workflows.md"),
    },
];

/// The directory guides live in: `~/.adi/mono/guides`.
#[must_use]
pub fn guides_dir(cfg: &Config) -> PathBuf {
    cfg.module(GUIDES_MODULE).dir().to_path_buf()
}

/// The sidecar that remembers the hash of what ADI itself last wrote for each shipped guide,
/// so a later refresh can tell "nobody touched this" from "a person edited it" without diffing
/// content against every template version by hand. Lives beside the guides themselves; an agent
/// has no reason to open it and it isn't one of the guides, so it's dotfiled.
const SHIPPED_SIDECAR: &str = ".shipped.json";

/// `sha256(text)` as lowercase hex — the form both the sidecar and [`historical_hashes`] store.
fn hash_text(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// file name -> hash of the content ADI last wrote there.
type Shipped = HashMap<String, String>;

fn read_shipped(dir: &Path) -> Shipped {
    std::fs::read(dir.join(SHIPPED_SIDECAR))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_shipped(dir: &Path, shipped: &Shipped) {
    if let Ok(bytes) = serde_json::to_vec_pretty(shipped) {
        let _ = std::fs::write(dir.join(SHIPPED_SIDECAR), bytes);
    }
}

/// Undo this machine's resolution of [`STORE_SHORTHAND`] in an on-disk guide, so it can be
/// compared against the machine-independent hashes in [`historical_hashes`] (which are computed
/// over the shorthand, never a resolved path).
fn normalise_for_history(on_disk: &str, cfg: &Config) -> String {
    on_disk.replace(&store_root_display(cfg), STORE_SHORTHAND)
}

/// Every hash a given guide file has ever shipped with, across every released version of its
/// template — the fallback for an install old enough to have no [`SHIPPED_SIDECAR`] yet. Each
/// hash is [`hash_text`] of the template's canonical form: `{{cli}}` replaced with [`CLI`], the
/// store-root shorthand left exactly as [`STORE_SHORTHAND`] (templates are already authored that
/// way — it's the *on-disk* side that needs normalising back to it, via
/// [`normalise_for_history`], since `render` resolves it to this machine's real path). Templates
/// from before the `{{cli}}` placeholder existed are hashed as their own unmodified text, since
/// the replace is then a no-op.
///
/// Regenerate the list for one file after editing its shipped guide, by walking its git history
/// (append the new entry; never remove an old one — the whole point is recognising copies from
/// versions nobody runs anymore):
///
/// ```sh
/// for rev in $(git log --follow --format=%H -- crates/adi-webapp-api/templates/guides/<file>.md); do
///   git show "$rev:crates/adi-webapp-api/templates/guides/<file>.md" \
///     | sed 's/{{cli}}/adi-mono/g' | sha256sum | cut -d' ' -f1
/// done
/// ```
fn historical_hashes(file: &str) -> &'static [&'static str] {
    match file {
        "README.md" => &[
            "246b393fc49433e6f5dc2d5958535e70c809596e9f0779a1316bd83d039abcbe",
            "dc9b0a40fdbd2b03e084a1539b13e3e29dd8069c4ad9ef740115f1155419813f",
            "821ac672597ef84b1cef907035bf5fbe303f1cafda7a44b6566d3c2eff013704",
            "c2c2624ae3947eb51dd78a71afb15ebe5c486a4544fab9a67ac03d80be155ab8",
            "1fa4959ab389f44b51ff621c5ad1f53ffd13530217463ee0e772f35533fe27ff",
            "723bb7a2b1b13f8fd8ad60da1bb884eb1a6feba0f621fbdf85d139f16b3208f9",
        ],
        "projects.md" => &[
            "11916996dec6a910283358e5c2196b0727add0614ea6f90129ee188270c80ae3",
            "46d108464c1f591074cef336487683dd17bc9949e98a6fec28efc11c7709ddcf",
            "4db0fc5f249d4423965d288a8380e5f0f0fdc0118a04cb0976fad49112261b2c",
        ],
        "dashboards.md" => &[
            "2174c121c61687716c9d6f7d37b2cb8e8527d0d07ca067d30bc00b6b37f42e7c",
            "7420a02bb7d93b5b99b2592424eb203209ab4aa97faf0af61ff6a363de5e5f7d",
        ],
        "tasks.md" => &[
            "e6eaa25fac721f1c28609e203f861b4d860a3b4f3aeeef55d397b1bb8942ec1f",
            "68a4cfd180d6c2e5d9e9736c563eb4a470f874ea0dade5124129e120c83942ce",
        ],
        "services.md" => &["bd53402f305c2aad143bef38687dbcfee8f079701acc3c97ae14126cb43a793b"],
        "triggers.md" => &[
            "545740251ae0c5ca8bed855c9eda27d43ab0b15741e3dbde1c0038920515010e",
            "0ad707f4f0dd21ecb2711a5b428e92c96f8f1f340e50208001204b985ae318ac",
            "7505a6b8241345bcad60ceb333fa0fe1bd6a800e47700ea8b52171abea14aea7",
        ],
        "tools.md" => &[
            "4dbb0f5aef449d7d5a5879ba3ad288b5b5c0aa2863beafaaa89e86d4cec2ec45",
            "17e8f7020cda908e47c2e91c255f25527382dea1ef7103c1b1aa5e29c1b00103",
            "eb6fbe9cb4cb457ddcfe3c6d81124a4fb24578f626dca31563e232b2272ce301",
            "fb4b4c5f449f72a3a3cae305019120cfbca031506a55c61b9cda83b5be6aebbe",
        ],
        "agents.md" => &[
            "6e561db13810859cd8007b06c846f638831302cc09a9c8ab403baea8a4097818",
            "89c7107cded3f8af8b998b3d8397c223657e103eff7a61b3e7bde00219feb8f3",
            "6e17ea49a36b9d4992b624929a1ebd40f5b53991555f09669a3aef09d52835da",
            "fe6bccda6bfd3a75b91cc6653162b83379b3054d652898e9a276e56cd566de6b",
            "6ddc61aa246f6eee3b27bdd8d04a3e959231efdda9555500e5af7279ae695a2d",
            "ff41a51d6563253a9193976e84d269fcef6bbd9049e2332bdc527d9182c89f7a",
            "ce85b797ae9746a05bc7d8ff968c245ff1a5218dd298a4b6c6ba7f569abf887f",
            "f0e6ed1732a28369643e487e731ca88f6d165ffde7e3362266502ad40eba8b0f",
            "19106afa5820cdb8ebddc28d53993c6d599e85dfa5dad0ac65e6dd69ee09c2fa",
            "aee29ac6bfe09c87276782d8c8173eeb38290b8b6df3ca3698579567968fdad9",
            "721aa2393a2db05b51375819f0ba77c1a7591de24b2c41a381804e688c845001",
        ],
        "secrets.md" => &[
            "da686585808b0e057219af84b046b56a9dd8457494f5345f847b479c215de697",
            "8500d71d781e9443b7be53196b123d72e31d05cf5672c5c692a4423b80c21b50",
            "7f11380bc83a53d20d362740fd61bab58c48f227cbf880d035d81a20b7750b2e",
        ],
        "db.md" => &["4925929a929fb6d219067d477fc8aa81cf709a082d19f4401f6a7238e26fd91a"],
        "channels.md" => &[
            "aa26257636f42b486cfe22f285fd897e0ff27c7ed763f813f5ffe73b11a3e805",
            "724c57101b6037a39e9b1e881c761cb0e572e38a13041a8bcdbaa70ba461693c",
            "38c2c4f4db56ea58ebfb12dd4c5099467fcce55dbfec30af66244fb1ebea9b67",
            "19fc8eca5ed7b3722db05a8304d3982ca4b5aa457cbc6408af1adbfdb92bd2bf",
        ],
        "workflows.md" => &[
            "5b31e4fa452deff2def14020b3a21cf4e2f365bccff53d841cc074c6aff1bda8",
            "f259b7ccc589fb82a1f09e3b15dc54a85100fec31509f0c78da4fe78dd029847",
            "a6bd61ce07616a355a8293324925db6a84e17fa072a48849f712c2bec316ab7d",
            "1091511cc73c37858033e875bef9ac5b4e694f176817c53d6ec48d4caf4680d2",
            "27d5d3b19de7fedc76de45ee6f01e16198c692df9e2c3737bc669df4cc2cf2f3",
        ],
        _ => &[],
    }
}

/// Ensure every built-in guide exists on disk, creating the directory and writing any missing
/// file. Refreshes a guide that is unchanged from what ADI previously shipped (tracked in
/// [`SHIPPED_SIDECAR`], or — for an install old enough to predate it — matched against
/// [`historical_hashes`]); leaves a guide a person has edited untouched, writing the shipped
/// version beside it as `<file>.new` instead. Best-effort — a filesystem error is swallowed,
/// since a stale or missing guide only degrades the agent's orientation, it never breaks the
/// page that seeds them. Never touches a file it doesn't own, e.g. a `*.bak-*` left by something
/// else in the directory.
pub fn ensure_guides(cfg: &Config) {
    let dir = guides_dir(cfg);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let mut shipped = read_shipped(&dir);
    let mut shipped_changed = false;

    // Bake this machine's real store path and CLI name into the seeded copy, so the guide names a
    // directory and a command that exist here rather than unexpandable shorthands.
    for g in GUIDES {
        let rendered = render(g.body, cfg);
        let path = dir.join(g.file);
        let new_path = dir.join(format!("{}.new", g.file));

        let Ok(on_disk) = std::fs::read_to_string(&path) else {
            // Missing (or unreadable, which we treat the same way best-effort): write it fresh.
            if std::fs::write(&path, &rendered).is_ok() {
                shipped.insert(g.file.to_string(), hash_text(&rendered));
                shipped_changed = true;
                let _ = std::fs::remove_file(&new_path);
            }
            continue;
        };

        if on_disk == rendered {
            let current_hash = hash_text(&rendered);
            if shipped.get(g.file) != Some(&current_hash) {
                shipped.insert(g.file.to_string(), current_hash);
                shipped_changed = true;
            }
            let _ = std::fs::remove_file(&new_path);
            continue;
        }

        let on_disk_hash = hash_text(&on_disk);
        let shipped_this_unedited = shipped.get(g.file).is_some_and(|h| *h == on_disk_hash);
        let legacy_unedited = historical_hashes(g.file)
            .contains(&hash_text(&normalise_for_history(&on_disk, cfg)).as_str());

        if shipped_this_unedited || legacy_unedited {
            if std::fs::write(&path, &rendered).is_ok() {
                shipped.insert(g.file.to_string(), hash_text(&rendered));
                shipped_changed = true;
                let _ = std::fs::remove_file(&new_path);
            }
        } else {
            // A person edited this guide: never touch it. Leave the shipped version beside it
            // instead, so they can merge in their own time.
            let existing_new = std::fs::read_to_string(&new_path).ok();
            if existing_new.as_deref() != Some(rendered.as_str()) {
                let _ = std::fs::write(&new_path, &rendered);
            }
        }
    }

    if shipped_changed {
        write_shipped(&dir, &shipped);
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

    /// `tasks.md`'s template as it shipped at `d20860d`, one version before the one checked in
    /// today — real history, not a synthetic fixture, so the test exercises the same hash
    /// [`historical_hashes`] actually carries for a legacy install. Regenerate alongside
    /// `historical_hashes("tasks.md")` if `tasks.md` ever changes again: `git show
    /// d20860dc05c03d5f72b2f1078f38cf07940b53fc:crates/adi-webapp-api/templates/guides/tasks.md`.
    // Not a `\`-continued literal: that trims leading whitespace off the next line, which would
    // silently eat the two-space continuation indent below and desync the fixture from its hash.
    const OLD_TASKS_TEMPLATE: &str = r#"# Tasks

A simple task tree stored at `~/.adi/mono/tasks/tasks.json`. Use it to track the work you and
the user agree on, so progress survives across runs.

## Do it
- List: `{{cli}} tasks list` or `GET /api/tasks`. Panel: `/tasks`.
- Create: `POST /api/tasks/create` (`{ "title", "project", "parent", "tag", "details" }`).
  `project` and `parent` are optional ids — a task can belong to a project and nest under
  another task.
- Close / reopen / archive / delete: `POST /api/tasks/archive` · `/reopen` · `/delete`.

## Notes
- The explorer's per-project badge counts open tasks — set `project` so work shows up where it
  belongs.
- Prefer small, verifiable tasks; close each as you confirm it rather than batching.
"#;

    fn test_cfg(name: &str) -> Config {
        let root = std::env::temp_dir().join(format!("adi-guides-ensure-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    fn tasks_guide() -> &'static Guide {
        GUIDES.iter().find(|g| g.file == "tasks.md").expect("tasks.md is a built-in guide")
    }

    /// Sanity check for [`OLD_TASKS_TEMPLATE`] itself: if this ever fails, either the fixture
    /// above or `historical_hashes("tasks.md")` has drifted from the real git history and needs
    /// regenerating together.
    #[test]
    fn old_tasks_template_matches_its_historical_hash() {
        let canonical = OLD_TASKS_TEMPLATE.replace(CLI_SHORTHAND, CLI);
        assert!(
            historical_hashes("tasks.md").contains(&hash_text(&canonical).as_str()),
            "fixture no longer matches any hash in historical_hashes(\"tasks.md\")"
        );
    }

    #[test]
    fn missing_guide_is_written_and_recorded() {
        let cfg = test_cfg("missing");
        ensure_guides(&cfg);

        let g = tasks_guide();
        let dir = guides_dir(&cfg);
        let on_disk = std::fs::read_to_string(dir.join(g.file)).unwrap();
        assert_eq!(on_disk, render(g.body, &cfg));

        let shipped = read_shipped(&dir);
        assert_eq!(shipped.get(g.file), Some(&hash_text(&render(g.body, &cfg))));
    }

    #[test]
    fn unedited_previously_shipped_version_is_replaced() {
        let cfg = test_cfg("unedited-previously-shipped");
        let dir = guides_dir(&cfg);
        std::fs::create_dir_all(&dir).unwrap();

        let g = tasks_guide();
        let old_on_disk = render(OLD_TASKS_TEMPLATE, &cfg);
        std::fs::write(dir.join(g.file), &old_on_disk).unwrap();
        // ADI itself wrote exactly this (older) version last time.
        write_shipped(&dir, &HashMap::from([(g.file.to_string(), hash_text(&old_on_disk))]));

        ensure_guides(&cfg);

        let on_disk = std::fs::read_to_string(dir.join(g.file)).unwrap();
        assert_eq!(on_disk, render(g.body, &cfg), "should be refreshed to the current template");
        let shipped = read_shipped(&dir);
        assert_eq!(shipped.get(g.file), Some(&hash_text(&render(g.body, &cfg))));
        assert!(!dir.join(format!("{}.new", g.file)).exists());
    }

    #[test]
    fn edited_guide_is_left_untouched_with_a_dot_new() {
        let cfg = test_cfg("edited");
        let dir = guides_dir(&cfg);
        std::fs::create_dir_all(&dir).unwrap();

        let g = tasks_guide();
        let edited = "# Tasks\n\nMy own notes, nothing like the shipped guide.\n";
        std::fs::write(dir.join(g.file), edited).unwrap();

        ensure_guides(&cfg);

        let on_disk = std::fs::read_to_string(dir.join(g.file)).unwrap();
        assert_eq!(on_disk, edited, "a person's edit must never be overwritten");
        let new_contents = std::fs::read_to_string(dir.join(format!("{}.new", g.file))).unwrap();
        assert_eq!(new_contents, render(g.body, &cfg));
    }

    #[test]
    fn legacy_unedited_without_sidecar_matching_a_historical_version_is_replaced() {
        let cfg = test_cfg("legacy-unedited");
        let dir = guides_dir(&cfg);
        std::fs::create_dir_all(&dir).unwrap();

        let g = tasks_guide();
        // A legacy install: no `.shipped.json` sidecar at all, and the on-disk file is a version
        // ADI shipped before the sidecar existed.
        let old_on_disk = render(OLD_TASKS_TEMPLATE, &cfg);
        std::fs::write(dir.join(g.file), &old_on_disk).unwrap();
        assert!(!dir.join(SHIPPED_SIDECAR).exists());

        ensure_guides(&cfg);

        let on_disk = std::fs::read_to_string(dir.join(g.file)).unwrap();
        assert_eq!(on_disk, render(g.body, &cfg), "a recognised historical version is refreshed");
        assert!(!dir.join(format!("{}.new", g.file)).exists());
        let shipped = read_shipped(&dir);
        assert_eq!(shipped.get(g.file), Some(&hash_text(&render(g.body, &cfg))));
    }

    #[test]
    fn legacy_edited_without_sidecar_gets_a_dot_new() {
        let cfg = test_cfg("legacy-edited");
        let dir = guides_dir(&cfg);
        std::fs::create_dir_all(&dir).unwrap();

        let g = tasks_guide();
        let edited = render(OLD_TASKS_TEMPLATE, &cfg).replace("task tree", "task forest, mine");
        std::fs::write(dir.join(g.file), &edited).unwrap();

        ensure_guides(&cfg);

        let on_disk = std::fs::read_to_string(dir.join(g.file)).unwrap();
        assert_eq!(on_disk, edited, "an edited legacy copy must never be overwritten");
        let new_contents = std::fs::read_to_string(dir.join(format!("{}.new", g.file))).unwrap();
        assert_eq!(new_contents, render(g.body, &cfg));
    }
}
