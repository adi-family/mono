//! Where a run starts — and telling the run so.
//!
//! Every executor launches its process in exactly one directory, and each relative path the agent
//! reads or writes lands there. Nothing used to *state* which directory that was. The default was
//! the store root; an agent whose work lives elsewhere had to be handed an absolute `working_dir`
//! by hand; and one definition reused across many targets (a recon pass, a per-repo reviewer)
//! could not be handed one at all, because the right directory differs per launch. So runs
//! re-derived it from their opening message and prefixed `cd <absolute path> &&` onto command
//! after command — which is only ever as correct as the run's memory, and silently writes to the
//! wrong place the first time a command forgets.
//!
//! This module is the single answer to "where does this run start". [`resolve`] is the one
//! precedence chain, applied before any backend builds a command; [`env`] exports the result so a
//! script can find it without being told; [`block`] states it in the system prompt so the agent has
//! no reason to guess, and an explicit reason not to `cd`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use adi_config::Config;

use crate::agent::RawAgentArguments;
use crate::launch::expand_home;
use crate::{StoredAgent, StoredAgentManifest};

/// The key every backend's arguments spell their static working directory under. One name across
/// `pty:codex`, `process:*`, and `harness:claude-sdk`, which is what lets this read the raw
/// arguments instead of decoding each backend's own struct to ask the same question.
const WORKING_DIR: &str = "working_dir";

/// The directory the run starts in, exported so scripts and later turns can find it.
const WORKDIR_ENV: &str = "ADI_WORKDIR";
/// The agent's own name — the run had no way to know what it was called.
const AGENT_ENV: &str = "ADI_AGENT";
/// The agent's project scope, and that project's directory, when it has one.
const PROJECT_ENV: &str = "ADI_PROJECT";
const PROJECT_DIR_ENV: &str = "ADI_PROJECT_DIR";

/// Where a run of this agent starts, most specific answer first:
///
/// 1. `run_dir` — this *launch's* directory, when the caller has one. The answer for an agent
///    pointed at a different target every run, which no stored field can express.
/// 2. the manifest's `working_dir` — the agent's declared home, whatever the backend.
/// 3. the directory of the agent's `project`, when it names one that is registered. A
///    project-scoped agent belongs in its project, the same way a project-scoped tool already runs
///    there (see `adi-cli`'s `tools::working_dir`).
/// 4. the store root — the right home for an agent that operates the environment itself, and the
///    fallback that can always be spawned into.
///
/// Blank entries count as unset at every level, so clearing a field in the UI falls through rather
/// than launching into `""`. A named-but-unregistered project falls through too: a manifest can
/// outlive the project it names, and a launch must not fail over it.
pub(crate) fn resolve(
    config: &Config,
    manifest: &StoredAgentManifest,
    run_dir: Option<&str>,
) -> PathBuf {
    run_dir
        .and_then(expand_home)
        .or_else(|| declared(&manifest.arguments).and_then(|d| expand_home(&d)))
        .or_else(|| project_dir(config, manifest))
        .unwrap_or_else(|| config.root().to_path_buf())
}

/// The directory a run is *already* in, for code that executes inside the spawned child (the `adi`
/// loop's `harness-turn`). The child was launched in the resolved directory and carries it in its
/// environment, so reading it back is exact — including a per-run directory that the manifest alone
/// could not reproduce. Falls back to resolving from the manifest when the variable is absent.
pub(crate) fn current(config: &Config, manifest: &StoredAgentManifest) -> PathBuf {
    std::env::var_os(WORKDIR_ENV)
        .map(PathBuf::from)
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or_else(|| resolve(config, manifest, None))
}

/// The manifest's own `working_dir`, read straight off the raw arguments — see [`WORKING_DIR`].
fn declared(arguments: &RawAgentArguments) -> Option<String> {
    arguments
        .get(WORKING_DIR)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(ToString::to_string)
}

/// The agent's project directory, when it is scoped to a project that is actually registered.
fn project_dir(config: &Config, manifest: &StoredAgentManifest) -> Option<PathBuf> {
    let project = manifest.project.as_deref().map(str::trim)?;
    config.project_dir(project)
}

/// What a run is told about its own location: the directory it starts in, its own name, and its
/// project scope when it has one. Sits alongside the database pointer in the launch environment for
/// the same reason — a run that has to be *told* where it is in prose will get it wrong, and a
/// script it writes can't be told at all.
pub(crate) fn env(config: &Config, agent: &StoredAgent, workdir: &Path) -> Vec<(String, String)> {
    let mut vars = vec![
        (WORKDIR_ENV.to_string(), workdir.display().to_string()),
        (AGENT_ENV.to_string(), agent.name.clone()),
    ];
    if let Some(project) = agent
        .manifest
        .project
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        vars.push((PROJECT_ENV.to_string(), project.to_string()));
        if let Some(dir) = config.project_dir(project) {
            vars.push((PROJECT_DIR_ENV.to_string(), dir.display().to_string()));
        }
    }
    vars
}

/// The prompt section stating where the run is. Short on purpose: the fix for a run that `cd`s is
/// not more prose about directories, it is one sentence it can trust plus the variable to read.
pub(crate) fn block(config: &Config, agent: &StoredAgent, workdir: &Path) -> String {
    let mut out = format!(
        "# Where you are\n\n\
         This run starts in `{}`, and it is already your working directory — the process was \
         launched there. Every relative path in your instructions resolves against it.\n\n\
         So do not `cd` into it, and do not rebuild it as an absolute prefix on each command. A \
         `cd` you repeat on every command is noise; a `cd` you forget on one command writes that \
         command's output somewhere else. Use relative paths, and reach for an absolute one only \
         to step outside this directory on purpose.\n\n\
         When you do work somewhere else, move there **once**, in a command of its own — your \
         shell keeps its working directory between commands, so `cd <path>` holds for every \
         command after it. Prefixing `cd <path> &&` onto each command re-pays for the same move \
         every time and buys nothing.\n\n\
         It is also `${WORKDIR_ENV}` in your environment, and your own name is `${AGENT_ENV}`, so \
         a script you write can find both without being told.",
        workdir.display(),
    );
    if let Some(project) = agent
        .manifest
        .project
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let _ = write!(
            out,
            "\n\nYou are scoped to the project `{project}` (`${PROJECT_ENV}`)"
        );
        match config.project_dir(project) {
            Some(dir) => {
                let _ = write!(
                    out,
                    ", whose directory is `{}` (`${PROJECT_DIR_ENV}`).",
                    dir.display()
                );
            }
            None => out.push('.'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (Config, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "adi-agents-workspace-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        (Config::with_root(&root), root)
    }

    fn manifest(project: Option<&str>, working_dir: Option<&str>) -> StoredAgentManifest {
        let mut arguments = RawAgentArguments::new();
        if let Some(dir) = working_dir {
            arguments.insert(
                WORKING_DIR.to_string(),
                serde_json::Value::String(dir.to_string()),
            );
        }
        StoredAgentManifest {
            backend: "harness:claude-sdk".into(),
            arguments,
            project: project.map(ToString::to_string),
            ..StoredAgentManifest::default()
        }
    }

    fn agent(manifest: StoredAgentManifest) -> StoredAgent {
        StoredAgent {
            name: "passive".into(),
            manifest,
        }
    }

    #[test]
    fn an_unscoped_agent_starts_in_the_store_root() {
        let (config, root) = store();
        assert_eq!(resolve(&config, &manifest(None, None), None), root);
    }

    /// The whole point of L1: a project agent belongs in its project, with nobody spelling the path.
    #[test]
    fn a_project_agent_starts_in_its_project() {
        let (config, root) = store();
        let project = root.join("projects").join("bugbounty");
        std::fs::create_dir_all(&project).expect("mkdir");
        assert_eq!(
            resolve(&config, &manifest(Some("bugbounty"), None), None),
            project
        );
    }

    /// A manifest can name a project that was archived or removed since. That is not a reason to
    /// refuse to launch — it falls back to the root the run can always be spawned into.
    #[test]
    fn a_project_that_is_not_registered_falls_through() {
        let (config, root) = store();
        assert_eq!(resolve(&config, &manifest(Some("gone"), None), None), root);
        // Nor is a traversal attempt ever joined onto the store path.
        assert_eq!(resolve(&config, &manifest(Some("../.."), None), None), root);
    }

    #[test]
    fn a_declared_working_dir_outranks_the_project() {
        let (config, root) = store();
        std::fs::create_dir_all(root.join("projects").join("bugbounty")).expect("mkdir");
        assert_eq!(
            resolve(
                &config,
                &manifest(Some("bugbounty"), Some("/repo/main")),
                None
            ),
            Path::new("/repo/main")
        );
    }

    /// L2: the launch's own directory is the most specific statement there is, so it wins over the
    /// manifest — that is what makes one definition usable against many targets.
    #[test]
    fn a_run_directory_outranks_everything() {
        let (config, _root) = store();
        assert_eq!(
            resolve(
                &config,
                &manifest(Some("bugbounty"), Some("/repo/main")),
                Some("/targets/crescendo-ai"),
            ),
            Path::new("/targets/crescendo-ai")
        );
    }

    /// Clearing a field in the UI leaves `""`, not a missing key — blank must mean unset at every
    /// level, or a run is spawned into nowhere.
    #[test]
    fn blank_entries_are_unset_at_every_level() {
        let (config, root) = store();
        assert_eq!(
            resolve(&config, &manifest(None, Some("   ")), Some("  ")),
            root
        );
    }

    /// A `working_dir` is written into TOML by hand, where no shell expands `~`.
    #[test]
    fn a_home_prefixed_directory_expands() {
        let (config, _root) = store();
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return;
        };
        assert_eq!(
            resolve(&config, &manifest(None, Some("~/adi-family")), None),
            home.join("adi-family")
        );
    }

    #[test]
    fn the_environment_carries_the_directory_the_name_and_the_scope() {
        let (config, root) = store();
        let project = root.join("projects").join("bugbounty");
        std::fs::create_dir_all(&project).expect("mkdir");
        let agent = agent(manifest(Some("bugbounty"), None));
        let workdir = resolve(&config, &agent.manifest, None);
        let vars = env(&config, &agent, &workdir);
        let get = |key: &str| vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        assert_eq!(get(WORKDIR_ENV), Some(project.display().to_string()));
        assert_eq!(get(AGENT_ENV), Some("passive".to_string()));
        assert_eq!(get(PROJECT_ENV), Some("bugbounty".to_string()));
        assert_eq!(get(PROJECT_DIR_ENV), Some(project.display().to_string()));
    }

    #[test]
    fn an_unscoped_agent_gets_no_project_vars() {
        let (config, _root) = store();
        let agent = agent(manifest(None, None));
        let vars = env(&config, &agent, Path::new("/tmp"));
        assert!(!vars.iter().any(|(k, _)| k == PROJECT_ENV), "{vars:?}");
        assert!(!vars.iter().any(|(k, _)| k == PROJECT_DIR_ENV), "{vars:?}");
    }

    #[test]
    fn the_block_names_the_directory_and_says_not_to_cd() {
        let (config, _root) = store();
        let agent = agent(manifest(None, None));
        let block = block(&config, &agent, Path::new("/targets/crescendo-ai"));
        assert!(block.contains("/targets/crescendo-ai"), "{block}");
        assert!(block.contains("do not `cd`"), "{block}");
        // Stepping outside is allowed — what is ruled out is paying for the same move on every
        // command, which is the habit that actually burns a run's tokens.
        assert!(block.contains("keeps its working directory"), "{block}");
        assert!(block.contains(WORKDIR_ENV), "{block}");
        assert!(!block.contains("project"), "{block}");
    }
}
