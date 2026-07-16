//! The project workspace registry (`<project>/.adi/workspaces.toml`) and the create
//! orchestration that ties it to the lifecycle hooks: a project's FIRST workspace is
//! created by its `init` hook (e.g. `git clone`), every ADDITIONAL one by its `workspace`
//! hook (e.g. `git worktree add`), and a "local" workspace just links an existing
//! directory — no hook runs.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::hook::{ADI_DIR, HOOK_INIT, HOOK_WORKSPACE, HookRun, Hooks, pid_alive, validate_name};

/// The registry file, relative to the project dir.
const REGISTRY_FILE: &str = "workspaces.toml";
/// The default parent for new workspaces, relative to the project dir.
pub const WORKSPACES_DIR: &str = "workspaces";

/// The workspaces of one project. Constructed from the project's directory, mirroring
/// [`Hooks`]; all state lives in `.adi/workspaces.toml`.
#[derive(Debug, Clone)]
pub struct Workspaces {
    project_dir: PathBuf,
}

/// The registry document: a flat list of entries, tolerant of unknown future fields.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Registry {
    #[serde(default)]
    workspaces: Vec<WorkspaceEntry>,
}

/// One registered workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    /// The workspace's name (a safe single path segment).
    pub name: String,
    /// Its directory — always absolute.
    pub path: PathBuf,
    /// How it came to be: created by the init hook, by the workspace hook, or linked local.
    pub kind: WorkspaceKind,
    /// The hook that created it (`None` for local links).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook: Option<String>,
    /// The creating hook run's pid (`None` for local links); used to show `creating` while
    /// the detached run is still alive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// When the workspace was registered, as Unix epoch seconds.
    #[serde(default)]
    pub created_at: u64,
}

/// How a workspace was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceKind {
    /// The project's first working copy, created by the `init` hook.
    Init,
    /// An additional working copy, created by the `workspace` hook.
    Workspace,
    /// An existing directory linked by absolute path; no hook ran.
    Local,
}

impl WorkspaceKind {
    /// The kind as the wire/UI string: `init` | `workspace` | `local`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Init => "init",
            Self::Workspace => "workspace",
            Self::Local => "local",
        }
    }
}

/// A workspace's live status, derived from its creating run and its directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceStatus {
    /// A linked local directory (registered as-is).
    Local,
    /// The creating hook run is still alive.
    Creating,
    /// The directory exists on disk.
    Ready,
    /// The creating run is gone and the directory never appeared.
    Failed,
}

impl WorkspaceStatus {
    /// The status as the wire/UI string: `local` | `creating` | `ready` | `failed`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Creating => "creating",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

impl Workspaces {
    /// Workspaces over `project_dir`'s `.adi/workspaces.toml`.
    pub fn new(project_dir: impl Into<PathBuf>) -> Self {
        Self {
            project_dir: project_dir.into(),
        }
    }

    /// The registry file: `<project>/.adi/workspaces.toml`.
    #[must_use]
    pub fn registry_path(&self) -> PathBuf {
        self.project_dir.join(ADI_DIR).join(REGISTRY_FILE)
    }

    /// The default directory for a new workspace: `<project>/workspaces/<name>`.
    ///
    /// # Errors
    /// [`Error::InvalidName`] when the name isn't a safe single segment.
    pub fn default_path(&self, name: &str) -> Result<PathBuf> {
        validate_name(name)?;
        Ok(self.project_dir.join(WORKSPACES_DIR).join(name))
    }

    /// Every registered workspace, in registration order. A missing registry is empty.
    ///
    /// # Errors
    /// [`Error::Registry`] when the file doesn't parse, [`Error::Io`] otherwise.
    pub fn list(&self) -> Result<Vec<WorkspaceEntry>> {
        Ok(self.load()?.workspaces)
    }

    /// The primary workspace — the first hook-created (non-local) entry, which later
    /// `workspace`-hook runs use as their working directory.
    ///
    /// # Errors
    /// Propagates [`Self::list`] errors.
    pub fn primary(&self) -> Result<Option<WorkspaceEntry>> {
        Ok(self
            .load()?
            .workspaces
            .into_iter()
            .find(|w| w.kind != WorkspaceKind::Local))
    }

    /// Which lifecycle hook the next hook-backed create will run: `init` while no
    /// hook-created workspace exists, `workspace` afterwards.
    ///
    /// # Errors
    /// Propagates [`Self::list`] errors.
    pub fn next_hook(&self) -> Result<&'static str> {
        Ok(if self.primary()?.is_none() {
            HOOK_INIT
        } else {
            HOOK_WORKSPACE
        })
    }

    /// A workspace's live status (see [`WorkspaceStatus`]).
    #[must_use]
    pub fn status(&self, entry: &WorkspaceEntry) -> WorkspaceStatus {
        if entry.kind == WorkspaceKind::Local {
            return WorkspaceStatus::Local;
        }
        // pid-alive first: git clone creates the target dir early, so dir-exists alone
        // can't distinguish "done" from "half way through".
        if entry.pid.is_some_and(pid_alive) {
            return WorkspaceStatus::Creating;
        }
        if entry.path.is_dir() {
            WorkspaceStatus::Ready
        } else {
            WorkspaceStatus::Failed
        }
    }

    /// Create a workspace.
    ///
    /// With `local`, `explicit` (or the default path) must be an existing directory, which
    /// is registered as-is — no hook runs and the returned run is `None`. Otherwise the
    /// target must NOT exist yet (the hook creates it — `git clone`/`git worktree add` both
    /// insist on that): the parent is pre-created, [`Self::next_hook`] picks `init` or
    /// `workspace`, and the hook is spawned detached with the workspace env contract —
    /// `project_env` pairs (`ADI_PROJECT_ID`/`ADI_PROJECT_NAME` from the caller) plus
    /// `ADI_PROJECT_DIR`, `ADI_WORKSPACE_NAME`, `ADI_WORKSPACE_DIR`, `ADI_WORKSPACE_COUNT`
    /// (hook-created workspaces before this one), and `ADI_PRIMARY_WORKSPACE_DIR` when a
    /// primary exists. cwd is the target's parent for `init` and the primary workspace's
    /// directory for `workspace` (so `git worktree add "$ADI_WORKSPACE_DIR"` just works).
    ///
    /// # Errors
    /// [`Error::InvalidName`], [`Error::Exists`] (name taken or target dir present),
    /// [`Error::NotAbsolute`] (explicit path must be absolute), [`Error::NotADir`] (local
    /// link target missing), [`Error::NoHook`] (no `init`/`workspace` hook file),
    /// [`Error::PrimaryMissing`] (primary workspace dir not on disk yet), or the run/IO
    /// errors of [`Hooks::run`].
    pub fn create(
        &self,
        name: &str,
        explicit: Option<&Path>,
        local: bool,
        project_env: &[(String, String)],
    ) -> Result<(WorkspaceEntry, Option<HookRun>)> {
        validate_name(name)?;
        let mut registry = self.load()?;
        if registry.workspaces.iter().any(|w| w.name == name) {
            return Err(Error::Exists(format!("workspace {name}")));
        }

        let target = match explicit {
            Some(p) if !p.is_absolute() => return Err(Error::NotAbsolute(p.to_path_buf())),
            Some(p) => p.to_path_buf(),
            None => self.default_path(name)?,
        };

        if local {
            if !target.is_dir() {
                return Err(Error::NotADir(target));
            }
            let entry = WorkspaceEntry {
                name: name.to_string(),
                path: target,
                kind: WorkspaceKind::Local,
                hook: None,
                pid: None,
                created_at: now_unix(),
            };
            registry.workspaces.push(entry.clone());
            self.save(&registry)?;
            return Ok((entry, None));
        }

        if target.exists() {
            return Err(Error::Exists(target.display().to_string()));
        }

        let hooks = Hooks::new(&self.project_dir);
        let primary = registry
            .workspaces
            .iter()
            .find(|w| w.kind != WorkspaceKind::Local)
            .cloned();
        let count = registry
            .workspaces
            .iter()
            .filter(|w| w.kind != WorkspaceKind::Local)
            .count();
        let (hook, kind, cwd) = match &primary {
            None => {
                let parent = target
                    .parent()
                    .ok_or_else(|| Error::NotADir(target.clone()))?
                    .to_path_buf();
                (HOOK_INIT, WorkspaceKind::Init, parent)
            }
            Some(p) => {
                if !p.path.is_dir() {
                    return Err(Error::PrimaryMissing);
                }
                (HOOK_WORKSPACE, WorkspaceKind::Workspace, p.path.clone())
            }
        };
        if !hooks.exists(hook) {
            return Err(Error::NoHook(hook.to_string()));
        }

        fs::create_dir_all(target.parent().ok_or_else(|| Error::NotADir(target.clone()))?)?;

        let mut env: Vec<(String, String)> = project_env.to_vec();
        env.push((
            "ADI_PROJECT_DIR".to_string(),
            self.project_dir.display().to_string(),
        ));
        env.push(("ADI_WORKSPACE_NAME".to_string(), name.to_string()));
        env.push((
            "ADI_WORKSPACE_DIR".to_string(),
            target.display().to_string(),
        ));
        env.push(("ADI_WORKSPACE_COUNT".to_string(), count.to_string()));
        if let Some(p) = &primary
            && p.path.is_dir()
        {
            env.push((
                "ADI_PRIMARY_WORKSPACE_DIR".to_string(),
                p.path.display().to_string(),
            ));
        }

        let run = hooks.run(hook, &env, &cwd)?;
        let entry = WorkspaceEntry {
            name: name.to_string(),
            path: target,
            kind,
            hook: Some(hook.to_string()),
            pid: Some(run.pid),
            created_at: now_unix(),
        };
        registry.workspaces.push(entry.clone());
        self.save(&registry)?;
        Ok((entry, Some(run)))
    }

    /// Unregister a workspace. Never touches its files — a clone/worktree on disk stays
    /// where it is. `Ok(false)` when no such workspace is registered.
    ///
    /// # Errors
    /// Propagates registry load/save errors.
    pub fn remove(&self, name: &str) -> Result<bool> {
        let mut registry = self.load()?;
        let before = registry.workspaces.len();
        registry.workspaces.retain(|w| w.name != name);
        if registry.workspaces.len() == before {
            return Ok(false);
        }
        self.save(&registry)?;
        Ok(true)
    }

    fn load(&self) -> Result<Registry> {
        let path = self.registry_path();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Registry::default());
            }
            Err(e) => return Err(e.into()),
        };
        toml::from_str(&text).map_err(|e| Error::Registry(e.to_string()))
    }

    /// Atomic save: write a tmp sibling, then rename over the registry. Two simultaneous
    /// creates are last-write-wins — acceptable for a single-user tool.
    fn save(&self, registry: &Registry) -> Result<()> {
        let path = self.registry_path();
        fs::create_dir_all(path.parent().expect("registry path has the .adi dir as parent"))?;
        let text = toml::to_string_pretty(registry).map_err(|e| Error::Registry(e.to_string()))?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }
}

/// The current time as Unix epoch seconds (0 if the clock predates the epoch).
#[must_use]
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook::tests::{scratch_dir, wait_until};

    fn env() -> Vec<(String, String)> {
        vec![
            ("ADI_PROJECT_ID".to_string(), "test-id".to_string()),
            ("ADI_PROJECT_NAME".to_string(), "Test".to_string()),
        ]
    }

    #[test]
    fn list_is_empty_without_a_registry() {
        let ws = Workspaces::new(scratch_dir("ws-empty"));
        assert!(ws.list().unwrap().is_empty());
        assert_eq!(ws.next_hook().unwrap(), HOOK_INIT);
    }

    #[test]
    fn local_link_registers_and_remove_leaves_the_dir() {
        let dir = scratch_dir("ws-local");
        let ws = Workspaces::new(&dir);
        let target = dir.join("elsewhere");
        fs::create_dir_all(&target).unwrap();

        let (entry, run) = ws.create("home", Some(&target), true, &env()).unwrap();
        assert!(run.is_none());
        assert_eq!(entry.kind, WorkspaceKind::Local);
        assert_eq!(ws.status(&entry), WorkspaceStatus::Local);
        // A local link never counts as the primary, so the next create still inits.
        assert_eq!(ws.next_hook().unwrap(), HOOK_INIT);

        assert!(ws.remove("home").unwrap());
        assert!(!ws.remove("home").unwrap());
        assert!(target.is_dir(), "remove must never delete files");
    }

    #[test]
    fn local_link_requires_an_existing_dir_and_abs_path() {
        let dir = scratch_dir("ws-local-bad");
        let ws = Workspaces::new(&dir);
        assert!(matches!(
            ws.create("gone", Some(&dir.join("missing")), true, &env()),
            Err(Error::NotADir(_))
        ));
        assert!(matches!(
            ws.create("rel", Some(Path::new("relative/path")), true, &env()),
            Err(Error::NotAbsolute(_))
        ));
    }

    #[test]
    fn create_without_an_init_hook_is_refused() {
        let ws = Workspaces::new(scratch_dir("ws-nohook"));
        assert!(matches!(
            ws.create("main", None, false, &env()),
            Err(Error::NoHook(_))
        ));
    }

    #[test]
    fn first_create_runs_init_then_workspace_runs_with_primary() {
        let dir = scratch_dir("ws-lifecycle");
        let ws = Workspaces::new(&dir);
        let hooks = Hooks::new(&dir);
        // The init hook proves the env contract: count 0, dir created by the script.
        hooks
            .create(
                HOOK_INIT,
                "[ \"$ADI_WORKSPACE_COUNT\" = 0 ] || exit 9\nmkdir \"$ADI_WORKSPACE_DIR\"\npwd > \"$ADI_WORKSPACE_DIR/cwd\"",
            )
            .unwrap();
        hooks
            .create(
                HOOK_WORKSPACE,
                "[ \"$ADI_WORKSPACE_COUNT\" = 1 ] || exit 9\n[ -n \"$ADI_PRIMARY_WORKSPACE_DIR\" ] || exit 8\nmkdir \"$ADI_WORKSPACE_DIR\"\npwd > \"$ADI_WORKSPACE_DIR/cwd\"",
            )
            .unwrap();

        let (first, run) = ws.create("main", None, false, &env()).unwrap();
        assert_eq!(first.kind, WorkspaceKind::Init);
        assert_eq!(first.hook.as_deref(), Some(HOOK_INIT));
        assert!(run.is_some());
        assert!(wait_until(|| first.path.join("cwd").is_file()));
        assert!(wait_until(|| ws.status(&first) == WorkspaceStatus::Ready));
        // init's cwd is the workspaces/ parent.
        let cwd = fs::read_to_string(first.path.join("cwd")).unwrap();
        assert!(
            cwd.trim().ends_with(WORKSPACES_DIR),
            "init cwd was {cwd:?}"
        );

        let (second, _) = ws.create("feature", None, false, &env()).unwrap();
        assert_eq!(second.kind, WorkspaceKind::Workspace);
        assert!(wait_until(|| second.path.join("cwd").is_file()));
        // The workspace hook's cwd is the primary workspace's directory.
        let cwd = fs::read_to_string(second.path.join("cwd")).unwrap();
        let primary = first.path.canonicalize().unwrap();
        assert_eq!(cwd.trim(), primary.to_str().unwrap());

        assert_eq!(ws.next_hook().unwrap(), HOOK_WORKSPACE);
        assert!(matches!(
            ws.create("main", None, false, &env()),
            Err(Error::Exists(_))
        ));
    }

    #[test]
    fn hook_create_refuses_an_existing_target_dir() {
        let dir = scratch_dir("ws-target-exists");
        let ws = Workspaces::new(&dir);
        Hooks::new(&dir).create(HOOK_INIT, "true").unwrap();
        let target = ws.default_path("main").unwrap();
        fs::create_dir_all(&target).unwrap();
        assert!(matches!(
            ws.create("main", None, false, &env()),
            Err(Error::Exists(_))
        ));
    }

    #[test]
    fn second_create_while_primary_dir_is_missing_is_refused() {
        let dir = scratch_dir("ws-primary-missing");
        let ws = Workspaces::new(&dir);
        let hooks = Hooks::new(&dir);
        // An init hook that never creates the target: the clone "failed".
        hooks.create(HOOK_INIT, "true").unwrap();
        hooks.create(HOOK_WORKSPACE, "true").unwrap();
        let (first, _) = ws.create("main", None, false, &env()).unwrap();
        assert!(wait_until(|| ws.status(&first) == WorkspaceStatus::Failed));
        assert!(matches!(
            ws.create("second", None, false, &env()),
            Err(Error::PrimaryMissing)
        ));
    }

    #[test]
    fn registry_round_trips_through_disk() {
        let dir = scratch_dir("ws-roundtrip");
        let ws = Workspaces::new(&dir);
        let target = dir.join("linked");
        fs::create_dir_all(&target).unwrap();
        ws.create("linked", Some(&target), true, &env()).unwrap();

        let again = Workspaces::new(&dir).list().unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].name, "linked");
        assert_eq!(again[0].kind, WorkspaceKind::Local);
        assert!(again[0].created_at > 0);
    }
}
