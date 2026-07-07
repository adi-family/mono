//! adi-projects — register and track adi projects: a pure library (no CLI, no daemon) over
//! the shared [`adi_config`] store. Each project is a directory under `~/.adi/mono/projects/`
//! whose `config.toml` is a metadata [`Manifest`] (name, description, timestamps, archive
//! state). A project's *runtime* config (services, proxy hosts, ports) lives separately in
//! the project's own `.adi/hive.yaml`, owned by adi-hive — this crate only owns the manifest.
//!
//! ```
//! # let tmp = std::env::temp_dir().join(format!("adi-projects-doctest-{}", std::process::id()));
//! # let _ = std::fs::remove_dir_all(&tmp);
//! use adi_projects::Projects;
//!
//! # let store = Projects::with_config(adi_config::Config::with_root(&tmp));
//! // In real code: let store = Projects::open();
//! let created = store.create("demo", Some("Demo".into()), None)?;
//! assert_eq!(created.id, "demo");
//! assert!(!created.is_archived());
//!
//! store.archive("demo")?;
//! assert!(store.get("demo")?.unwrap().is_archived());
//! # std::fs::remove_dir_all(&tmp).ok();
//! # Ok::<(), adi_projects::Error>(())
//! ```

mod error;
mod project;

use std::path::PathBuf;

use adi_config::{Config, ConfigFile};

pub use error::{Error, Result};
pub use project::{Manifest, Project};

use project::{now_unix, validate_id};

/// The store module projects live under, and the manifest file within each project dir.
const PROJECTS_MODULE: &str = "projects";
const MANIFEST_FILE: &str = "config.toml";

/// The projects registry: lists, reads, and mutates the per-project manifests under the
/// `projects` module dir. Cheap to clone; all state is on disk.
#[derive(Debug, Clone)]
pub struct Projects {
    config: Config,
}

impl Default for Projects {
    fn default() -> Self {
        Self::open()
    }
}

impl Projects {
    /// Open the registry backed by the standard store (`~/.adi/mono`, honoring `$ADI_DIR`).
    #[must_use]
    pub fn open() -> Self {
        Self {
            config: Config::open(),
        }
    }

    /// Open the registry backed by a caller-supplied [`Config`] — for tests or alternate installs.
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self { config }
    }

    /// The store this registry reads from.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The `projects` directory: `~/.adi/mono/projects`.
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.config.module(PROJECTS_MODULE).dir().to_path_buf()
    }

    /// Where a project's runtime hive config lives: `projects/<id>/.adi/hive.yaml`. This crate
    /// owns the project *layout* (so callers don't re-derive it) but not the YAML format —
    /// adi-hive does. Returns the path even if the file doesn't exist.
    ///
    /// # Errors
    /// [`Error::InvalidId`] for an unsafe id.
    pub fn hive_path(&self, id: &str) -> Result<PathBuf> {
        validate_id(id)?;
        Ok(self.dir().join(id).join(".adi").join("hive.yaml"))
    }

    /// The manifest file handle for `id`, at `projects/<id>/config.toml` (touches no disk).
    fn manifest_file(&self, id: &str) -> ConfigFile<Manifest> {
        self.config
            .module(PROJECTS_MODULE)
            .file(&format!("{id}/{MANIFEST_FILE}"))
    }

    /// Every registered project, sorted by id. A project dir without a `config.toml` isn't
    /// registered yet and is skipped; a missing `projects/` dir yields an empty list.
    ///
    /// # Errors
    /// [`Error::Io`] on a directory read failure, or [`Error::Config`] if a manifest is invalid TOML.
    pub fn list(&self) -> Result<Vec<Project>> {
        let entries = match std::fs::read_dir(self.dir()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::Io(e)),
        };

        let mut projects = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            // A non-UTF-8 or non-safe directory name can't be a valid id; skip it.
            let Ok(id) = entry.file_name().into_string() else {
                continue;
            };
            if validate_id(&id).is_err() {
                continue;
            }
            let file = self.manifest_file(&id);
            if !file.exists() {
                continue;
            }
            projects.push(Project {
                id,
                manifest: file.load()?,
            });
        }
        projects.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(projects)
    }

    /// The project with this id, or `None` if it isn't registered.
    ///
    /// # Errors
    /// [`Error::InvalidId`] for an unsafe id, or [`Error::Config`] if the manifest is invalid TOML.
    pub fn get(&self, id: &str) -> Result<Option<Project>> {
        validate_id(id)?;
        let file = self.manifest_file(id);
        if !file.exists() {
            return Ok(None);
        }
        Ok(Some(Project {
            id: id.to_string(),
            manifest: file.load()?,
        }))
    }

    /// Register a new project, writing its `config.toml`. `name` defaults to the id when
    /// omitted or blank; a blank `description` is dropped.
    ///
    /// # Errors
    /// [`Error::InvalidId`] for an unsafe id, [`Error::Exists`] if one is already registered,
    /// or [`Error::Config`] on a write failure.
    pub fn create(
        &self,
        id: &str,
        name: Option<String>,
        description: Option<String>,
    ) -> Result<Project> {
        validate_id(id)?;
        let file = self.manifest_file(id);
        if file.exists() {
            return Err(Error::Exists(id.to_string()));
        }
        let manifest = Manifest {
            name: clean(name).unwrap_or_else(|| id.to_string()),
            description: clean(description),
            created_at: now_unix(),
            archived_at: None,
        };
        file.save(&manifest)?;
        Ok(Project {
            id: id.to_string(),
            manifest,
        })
    }

    /// Archive a project (soft delete), stamping `archived_at` if it isn't already archived.
    /// Idempotent: re-archiving keeps the original timestamp.
    ///
    /// # Errors
    /// [`Error::NotFound`] if unregistered, plus the usual id/config errors.
    pub fn archive(&self, id: &str) -> Result<Project> {
        let mut project = self.require(id)?;
        if project.manifest.archived_at.is_none() {
            project.manifest.archived_at = Some(now_unix());
            self.manifest_file(id).save(&project.manifest)?;
        }
        Ok(project)
    }

    /// Restore an archived project, clearing `archived_at`. Idempotent for an active project.
    ///
    /// # Errors
    /// [`Error::NotFound`] if unregistered, plus the usual id/config errors.
    pub fn unarchive(&self, id: &str) -> Result<Project> {
        let mut project = self.require(id)?;
        if project.manifest.archived_at.is_some() {
            project.manifest.archived_at = None;
            self.manifest_file(id).save(&project.manifest)?;
        }
        Ok(project)
    }

    /// Permanently delete a project's directory and everything in it. Returns `false` if it
    /// wasn't there. This is a hard delete — prefer [`archive`](Self::archive) for reversible removal.
    ///
    /// # Errors
    /// [`Error::InvalidId`] for an unsafe id, or [`Error::Io`] on a removal failure.
    pub fn remove(&self, id: &str) -> Result<bool> {
        validate_id(id)?;
        let dir = self.dir().join(id);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(Error::Io(e)),
        }
    }

    /// Load a project, turning absence into [`Error::NotFound`].
    fn require(&self, id: &str) -> Result<Project> {
        self.get(id)?.ok_or_else(|| Error::NotFound(id.to_string()))
    }
}

/// Trim a string, dropping it entirely when blank.
fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Projects {
        let root = std::env::temp_dir().join(format!(
            "adi-projects-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Projects::with_config(Config::with_root(root))
    }

    #[test]
    fn create_then_get_and_list_round_trip() {
        let store = scratch("crud");
        assert!(store.list().expect("empty list").is_empty());

        let created = store
            .create("demo", Some("Demo App".into()), Some("a test".into()))
            .expect("create");
        assert_eq!(created.id, "demo");
        assert_eq!(created.manifest.name, "Demo App");
        assert_eq!(created.manifest.description.as_deref(), Some("a test"));
        assert!(created.manifest.created_at > 0);
        assert!(!created.is_archived());

        let got = store.get("demo").expect("get").expect("present");
        assert_eq!(got, created);

        let all = store.list().expect("list");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "demo");
    }

    #[test]
    fn create_defaults_name_to_id_and_drops_blank_description() {
        let store = scratch("defaults");
        let p = store
            .create("bare", None, Some("   ".into()))
            .expect("create");
        assert_eq!(p.manifest.name, "bare");
        assert_eq!(p.manifest.description, None);
    }

    #[test]
    fn duplicate_create_is_an_error() {
        let store = scratch("dup");
        store.create("x", None, None).expect("first");
        assert!(matches!(
            store.create("x", None, None),
            Err(Error::Exists(_))
        ));
    }

    #[test]
    fn archive_and_unarchive_toggle_state() {
        let store = scratch("archive");
        store.create("p", None, None).expect("create");

        let archived = store.archive("p").expect("archive");
        assert!(archived.is_archived());
        let stamp = archived.manifest.archived_at.expect("stamp");

        // Re-archiving keeps the original timestamp.
        let again = store.archive("p").expect("re-archive");
        assert_eq!(again.manifest.archived_at, Some(stamp));

        let restored = store.unarchive("p").expect("unarchive");
        assert!(!restored.is_archived());
        assert!(!store.get("p").expect("get").expect("present").is_archived());
    }

    #[test]
    fn mutating_an_unregistered_project_is_not_found() {
        let store = scratch("missing");
        assert!(matches!(store.archive("ghost"), Err(Error::NotFound(_))));
        assert!(matches!(store.unarchive("ghost"), Err(Error::NotFound(_))));
        assert!(store.get("ghost").expect("get").is_none());
    }

    #[test]
    fn remove_deletes_the_directory() {
        let store = scratch("remove");
        store.create("gone", None, None).expect("create");
        assert!(store.remove("gone").expect("remove"));
        assert!(store.get("gone").expect("get").is_none());
        assert!(!store.remove("gone").expect("remove missing"));
    }

    #[test]
    fn invalid_ids_never_touch_disk() {
        let store = scratch("invalid");
        assert!(matches!(store.get("../escape"), Err(Error::InvalidId(_))));
        assert!(matches!(
            store.create("a/b", None, None),
            Err(Error::InvalidId(_))
        ));
        assert!(matches!(store.remove(".."), Err(Error::InvalidId(_))));
        assert!(matches!(store.hive_path("../x"), Err(Error::InvalidId(_))));
    }

    #[test]
    fn hive_path_points_at_the_projects_dot_adi_hive_yaml() {
        let store = scratch("hive");
        let p = store.hive_path("demo").expect("hive path");
        assert!(
            p.ends_with("projects/demo/.adi/hive.yaml"),
            "got {}",
            p.display()
        );
    }

    #[test]
    fn list_skips_dirs_without_a_manifest() {
        let store = scratch("skip");
        store.create("real", None, None).expect("create");
        // A bare directory (like the demo project's `.adi/hive.yaml`-only dir) isn't registered.
        std::fs::create_dir_all(store.dir().join("bare")).expect("mkdir");
        let all = store.list().expect("list");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "real");
    }
}
