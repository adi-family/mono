//! adi-projects — register and track adi projects: a pure library (no CLI, no daemon) over
//! the shared [`adi_config`] store. Each project is a directory under `~/.adi/mono/projects/`
//! whose `config.toml` is a metadata [`Manifest`] (name, description, timestamps, archive
//! state). A project's *runtime* config (services, proxy hosts, ports) lives separately in
//! the project's own `.adi/hive.yaml`, owned by adi-hive — this crate only owns the manifest.
//!
//! A project's id is minted from its name ([`adi_config::mint`]) and *is* its directory name under
//! `projects/`. Ids minted before that rule are UUIDs and keep working unchanged;
//! [`rename`](Projects::rename) turns one into a name and records the old id in the registry's
//! alias index, so a `.adi/hive.yaml`, an `$ADI_PROJECT`, or a manifest elsewhere in the store that
//! still cites the UUID resolves to the same project it always did.
//!
//! ```
//! # let tmp = std::env::temp_dir().join(format!("adi-projects-doctest-{}", std::process::id()));
//! # let _ = std::fs::remove_dir_all(&tmp);
//! use adi_projects::Projects;
//!
//! # let store = Projects::with_config(adi_config::Config::with_root(&tmp));
//! // In real code: let store = Projects::open();
//! let created = store.create("Demo", None, None)?;
//! assert_eq!(created.manifest.name, "Demo");
//! assert_eq!(created.id, "demo");
//! assert!(!created.is_archived());
//!
//! store.archive(&created.id)?;
//! assert!(store.get(&created.id)?.unwrap().is_archived());
//! # std::fs::remove_dir_all(&tmp).ok();
//! # Ok::<(), adi_projects::Error>(())
//! ```

mod error;
mod project;

use adi_config::{Aliases, clean, optional};
use std::path::PathBuf;

use adi_config::{Config, ConfigFile, Module, now_unix};

pub use error::{Error, Result};
pub use project::{Manifest, Project};

use project::validate_id;

/// The store module projects live under, and the manifest file within each project dir.
const PROJECTS_MODULE: &str = "projects";
const MANIFEST_FILE: &str = "config.toml";
/// The word a project's id falls back to when its name has nothing sluggable in it.
const ID_FALLBACK: &str = "project";

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

    /// A project's own directory: `projects/<id>`. This is the base a file browser is confined
    /// to (see `adi-fs`) — everything the project owns, including its `.adi/hive.yaml`, lives
    /// under it. Returns the path even if the directory doesn't exist yet.
    ///
    /// # Errors
    /// [`Error::InvalidId`] for an unsafe id — the security boundary before the id is joined
    /// onto the store path.
    pub fn project_dir(&self, id: &str) -> Result<PathBuf> {
        Ok(self.dir().join(self.resolve(id)?))
    }

    /// The `projects` module handle — the directory the project dirs and the alias index share.
    fn module(&self) -> Module {
        self.config.module(PROJECTS_MODULE)
    }

    /// The ids projects used to have, each pointing at the id it has now. See [`Aliases`].
    ///
    /// # Errors
    /// [`Error::Config`] if the index exists but can't be read or parsed.
    pub fn aliases(&self) -> Result<Aliases> {
        Ok(Aliases::load(&self.module())?)
    }

    /// The id `id` actually names: itself when a project is registered under it, else what the
    /// alias index says it was renamed to, else `id` unchanged.
    ///
    /// Every read and mutation path goes through here, which is what makes a UUID written into a
    /// `.adi/hive.yaml`, a store document, or somebody's shell history keep working after the
    /// project it names has been given one.
    ///
    /// # Errors
    /// [`Error::InvalidId`] for an unsafe id, or [`Error::Config`] on an unreadable alias index.
    pub fn resolve(&self, id: &str) -> Result<String> {
        validate_id(id)?;
        if self.manifest_file(id).exists() {
            return Ok(id.to_string());
        }
        Ok(self.aliases()?.target(id).unwrap_or(id).to_string())
    }

    /// Whether anything occupies `id`: a project's directory (registered or made by hand), or an
    /// id some project was renamed away from and still answers to.
    fn is_taken(&self, id: &str, aliases: &Aliases) -> bool {
        self.dir().join(id).exists() || aliases.is_alias(id)
    }

    /// Where a project's runtime hive config lives: `projects/<id>/.adi/hive.yaml`. This crate
    /// owns the project *layout* (so callers don't re-derive it) but not the YAML format —
    /// adi-hive does. Returns the path even if the file doesn't exist.
    ///
    /// # Errors
    /// [`Error::InvalidId`] for an unsafe id.
    pub fn hive_path(&self, id: &str) -> Result<PathBuf> {
        Ok(self.project_dir(id)?.join(".adi").join("hive.yaml"))
    }

    /// The manifest file handle for `id`, at `projects/<id>/config.toml` (touches no disk).
    fn manifest_file(&self, id: &str) -> ConfigFile<Manifest> {
        self.module().file(&format!("{id}/{MANIFEST_FILE}"))
    }

    /// Every registered project, sorted by id. A project dir without a `config.toml` isn't
    /// registered yet and is skipped; a missing `projects/` dir yields an empty list.
    ///
    /// # Errors
    /// [`Error::Io`] on a directory read failure, or [`Error::Config`] if a manifest is invalid TOML.
    pub fn list(&self) -> Result<Vec<Project>> {
        let Some(entries) = optional(std::fs::read_dir(self.dir()))? else {
            return Ok(Vec::new());
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

    /// The project with this id, or `None` if it isn't registered. `id` may be an id the project
    /// no longer has (see [`resolve`](Self::resolve)); the returned [`Project`] always carries the
    /// current one.
    ///
    /// # Errors
    /// [`Error::InvalidId`] for an unsafe id, or [`Error::Config`] if the manifest is invalid TOML.
    pub fn get(&self, id: &str) -> Result<Option<Project>> {
        let id = self.resolve(id)?;
        let file = self.manifest_file(&id);
        if !file.exists() {
            return Ok(None);
        }
        Ok(Some(Project {
            id,
            manifest: file.load()?,
        }))
    }

    /// Register a new project under an id minted from its name (its directory name), writing its
    /// `config.toml`. Callers supply only the human-facing `name`; a blank name falls back to the
    /// minted id.
    ///
    /// # Errors
    /// [`Error::NotFound`] for an unregistered parent, or [`Error::Config`] on a write failure.
    pub fn create(
        &self,
        name: &str,
        description: Option<String>,
        parent: Option<String>,
    ) -> Result<Project> {
        let aliases = self.aliases()?;
        let id = adi_config::mint(name, ID_FALLBACK, |candidate| {
            self.is_taken(candidate, &aliases)
        });
        self.create_with_id(&id, Some(name.to_string()), description, parent)
    }

    /// Register a new project under an explicit id, writing its `config.toml`. `name` defaults
    /// to the id when omitted or blank; a blank `description` or `parent` is dropped. A
    /// non-blank `parent` makes this a sub-project and must name a registered project — and
    /// since a parent can only be set here (there is no re-parent operation) a fresh id can
    /// never be its own ancestor, so the links always form a tree.
    ///
    /// Prefer [`create`](Self::create), which generates the id; this is the escape hatch for
    /// callers that must control the directory name (tests, imports of existing dirs).
    ///
    /// # Errors
    /// [`Error::InvalidId`] for an unsafe id, [`Error::Exists`] if one is already registered,
    /// [`Error::NotFound`] for an unregistered parent, or [`Error::Config`] on a write failure.
    pub fn create_with_id(
        &self,
        id: &str,
        name: Option<String>,
        description: Option<String>,
        parent: Option<String>,
    ) -> Result<Project> {
        validate_id(id)?;
        let file = self.manifest_file(id);
        // An id another project was renamed away from is taken too: handing it to a new project
        // would silently re-point every reference still naming the old one.
        if file.exists() || self.aliases()?.is_alias(id) {
            return Err(Error::Exists(id.to_string()));
        }
        let parent = clean(parent);
        if let Some(p) = &parent {
            self.require(p)?;
        }
        let manifest = Manifest {
            name: clean(name).unwrap_or_else(|| id.to_string()),
            description: clean(description),
            parent,
            created_at: now_unix(),
            archived_at: None,
        };
        file.save(&manifest)?;
        Ok(Project {
            id: id.to_string(),
            manifest,
        })
    }

    /// The direct sub-projects of `id` (every project whose `parent` is this id), sorted by id.
    ///
    /// # Errors
    /// [`Error::InvalidId`] for an unsafe id, plus everything [`list`](Self::list) can return.
    pub fn children(&self, id: &str) -> Result<Vec<Project>> {
        validate_id(id)?;
        let mut all = self.list()?;
        all.retain(|p| p.manifest.parent.as_deref() == Some(id));
        Ok(all)
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
            // `project.id`, not `id`: the caller may have named it by an id it no longer has, and
            // writing the manifest back under that one would create a second, half-made project.
            self.manifest_file(&project.id).save(&project.manifest)?;
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
            self.manifest_file(&project.id).save(&project.manifest)?;
        }
        Ok(project)
    }

    /// Permanently delete a project's directory and everything in it. Returns `false` if it
    /// wasn't there. This is a hard delete — prefer [`archive`](Self::archive) for reversible
    /// removal. Sub-projects survive: they re-parent to the removed project's own parent
    /// (top-level when it had none), mirroring how the task tree deletes a node.
    ///
    /// Every id the project answered to goes with it: an alias to a deleted project would hold
    /// that id reserved against a future mint while pointing at nothing.
    ///
    /// # Errors
    /// [`Error::InvalidId`] for an unsafe id, or [`Error::Io`] on a removal failure.
    pub fn remove(&self, id: &str) -> Result<bool> {
        let id = self.resolve(id)?;
        // Capture the parent before the manifest is gone, to hand it down to the children.
        let orphan_parent = self.get(&id)?.and_then(|p| p.manifest.parent);
        let dir = self.dir().join(&id);
        let removed = optional(std::fs::remove_dir_all(&dir))?.is_some();
        if removed {
            for mut child in self.children(&id)? {
                child.manifest.parent.clone_from(&orphan_parent);
                self.manifest_file(&child.id).save(&child.manifest)?;
            }
            Aliases::forget(&self.module(), &id)?;
        }
        Ok(removed)
    }

    /// Rename a project: give it a new id, which *is* the name of its directory under
    /// `projects/`. The manifest travels with the directory, so nothing inside the project has to
    /// be rewritten — but every sub-project's `parent` names the old id, and those are re-pointed
    /// here so the tree survives the move.
    ///
    /// Renaming to the id it already has is a no-op, not an error: the caller asked for a state,
    /// and that state already holds.
    ///
    /// The id it had is recorded in the alias index and keeps resolving — to this project's
    /// directory, its manifest, and everything under it. That is what lets a project move from the
    /// UUID it was created under to its name without breaking a `.adi/hive.yaml`, a store document
    /// or an `$ADI_PROJECT` that names the old one. `from` may itself be such an id.
    ///
    /// This is the *registry* half of a rename. A project id is also written down outside this
    /// store — a tool or an agent filed under it, its scoped secrets, its database, its knowledge
    /// bases — and none of that is this crate's to reach. Callers that own the whole store
    /// (`adi_core::rename_project`) follow the rename into those stores after this returns, which
    /// is what makes the new id the one those actually cost; the alias is for everything nobody
    /// can reach.
    ///
    /// # Errors
    /// [`Error::InvalidId`] for an unsafe id on either side, [`Error::NotFound`] if `from` isn't
    /// registered, [`Error::Exists`] if anything already occupies `to`, or [`Error::Io`] /
    /// [`Error::Config`] if the move or a child rewrite fails.
    pub fn rename(&self, from: &str, to: &str) -> Result<Renamed> {
        validate_id(to)?;
        let project = self.require(from)?;
        let from = project.id.clone();
        if from == to {
            return Ok(Renamed {
                project,
                subprojects: 0,
            });
        }
        // Refuse the move on any occupied directory, not just a registered one: an unregistered
        // `projects/<to>` (a directory somebody made by hand) would otherwise swallow the moved
        // project as a child of itself on platforms where that rename succeeds.
        let target = self.dir().join(to);
        if self.is_taken(to, &self.aliases()?) {
            return Err(Error::Exists(to.to_string()));
        }
        std::fs::rename(self.dir().join(&from), &target)?;
        Aliases::record(&self.module(), &from, to)?;

        let mut subprojects = 0;
        for mut child in self.children(&from)? {
            child.manifest.parent = Some(to.to_string());
            self.manifest_file(&child.id).save(&child.manifest)?;
            subprojects += 1;
        }
        Ok(Renamed {
            project: Project {
                id: to.to_string(),
                manifest: project.manifest,
            },
            subprojects,
        })
    }

    /// Load a project, turning absence into [`Error::NotFound`].
    fn require(&self, id: &str) -> Result<Project> {
        self.get(id)?.ok_or_else(|| Error::NotFound(id.to_string()))
    }
}

/// What [`Projects::rename`] did: the project under its new id, and how many sub-projects had
/// their `parent` re-pointed at it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Renamed {
    /// The renamed project, carrying its new id.
    pub project: Project,
    /// How many sub-projects now name the new id as their parent.
    pub subprojects: usize,
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
            .create_with_id("demo", Some("Demo App".into()), Some("a test".into()), None)
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
    fn create_mints_a_slug_of_the_name() {
        let store = scratch("mint");
        let a = store.create("My App", None, None).expect("create");
        assert_eq!(a.id, "my-app");
        assert!(validate_id(&a.id).is_ok());
        assert_eq!(a.manifest.name, "My App");
        assert_eq!(store.get(&a.id).expect("get").expect("present"), a);

        // A second project of the same name takes the next number rather than the first one's dir.
        let b = store.create("my app", None, None).expect("second create");
        assert_eq!(b.id, "my-app-2");
        assert_eq!(store.list().expect("list").len(), 2);

        // Nothing sluggable: the kind's own word, then the same numbering.
        let cyrillic = store.create("Проект", None, None).expect("cyrillic");
        assert_eq!(cyrillic.id, "project");
        let bare = store.create("   ", None, None).expect("blank name");
        assert_eq!(bare.id, "project-2");
        assert_eq!(bare.manifest.name, bare.id);
    }

    /// The hard constraint: a renamed project still answers to the id it had, so a
    /// `.adi/hive.yaml`, an `$ADI_PROJECT`, or a store document naming the UUID keeps resolving.
    #[test]
    fn a_renamed_project_still_answers_to_the_id_it_had() {
        let store = scratch("alias");
        let uuid = "3352a5eb-5166-4609-b164-3fe6bf1a091c";
        store
            .create_with_id(uuid, Some("Nakit Yok".into()), None, None)
            .expect("create");
        std::fs::write(
            store.project_dir(uuid).expect("dir").join("NOTES.md"),
            "inside",
        )
        .expect("write");

        store.rename(uuid, "nakit-yok").expect("rename");
        let by_old = store.get(uuid).expect("get old").expect("present");
        assert_eq!(by_old.id, "nakit-yok", "the current id is what is reported");
        assert_eq!(by_old.manifest.name, "Nakit Yok");
        // The directory the old id points at is the moved one, files and all.
        let notes = store.project_dir(uuid).expect("dir").join("NOTES.md");
        assert_eq!(std::fs::read_to_string(notes).expect("read"), "inside");
        assert!(
            store
                .hive_path(uuid)
                .expect("hive path")
                .ends_with("projects/nakit-yok/.adi/hive.yaml")
        );
        // `adi_config::Config::project_dir` is the same question asked by the crates that only
        // need to point a run somewhere; it has to answer the old id too.
        assert!(
            store
                .config()
                .project_dir(uuid)
                .expect("config project dir")
                .ends_with("projects/nakit-yok")
        );

        // The id it gave up is not free to be handed to something else.
        assert!(matches!(
            store.create_with_id(uuid, None, None, None),
            Err(Error::Exists(_))
        ));
        // And a rename can be corrected through it.
        store.rename(uuid, "nakit").expect("re-rename by old id");
        assert_eq!(store.get(uuid).expect("get").expect("present").id, "nakit");
        assert_eq!(
            store.get("nakit-yok").expect("get").expect("present").id,
            "nakit"
        );

        // Deleting the project releases every id it answered to.
        assert!(store.remove(uuid).expect("remove by old id"));
        assert!(store.get(uuid).expect("get").is_none());
        assert!(store.aliases().expect("aliases").all().is_empty());
    }

    #[test]
    fn create_defaults_name_to_id_and_drops_blank_description() {
        let store = scratch("defaults");
        let p = store
            .create_with_id("bare", None, Some("   ".into()), Some("  ".into()))
            .expect("create");
        assert_eq!(p.manifest.name, "bare");
        assert_eq!(p.manifest.description, None);
        assert_eq!(p.manifest.parent, None);
    }

    #[test]
    fn subprojects_nest_under_a_registered_parent() {
        let store = scratch("subprojects");
        store
            .create_with_id("root", None, None, None)
            .expect("root");
        let child = store
            .create_with_id("child", None, None, Some("root".into()))
            .expect("child");
        assert_eq!(child.manifest.parent.as_deref(), Some("root"));
        let got = store.get("child").expect("get").expect("present");
        assert_eq!(got.manifest.parent.as_deref(), Some("root"));

        let kids = store.children("root").expect("children");
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].id, "child");
        assert!(store.children("child").expect("no kids").is_empty());
    }

    #[test]
    fn creating_under_an_unregistered_parent_is_not_found() {
        let store = scratch("badparent");
        assert!(matches!(
            store.create_with_id("kid", None, None, Some("ghost".into())),
            Err(Error::NotFound(_))
        ));
        assert!(store.get("kid").expect("get").is_none());
    }

    #[test]
    fn remove_reparents_children_to_the_removed_projects_parent() {
        let store = scratch("reparent");
        store
            .create_with_id("root", None, None, None)
            .expect("root");
        store
            .create_with_id("mid", None, None, Some("root".into()))
            .expect("mid");
        store
            .create_with_id("leaf", None, None, Some("mid".into()))
            .expect("leaf");

        assert!(store.remove("mid").expect("remove"));
        let leaf = store.get("leaf").expect("get").expect("present");
        assert_eq!(leaf.manifest.parent.as_deref(), Some("root"));

        assert!(store.remove("root").expect("remove root"));
        let leaf = store.get("leaf").expect("get").expect("present");
        assert_eq!(leaf.manifest.parent, None);
    }

    #[test]
    fn rename_moves_the_directory_and_repoints_children() {
        let store = scratch("rename");
        store
            .create_with_id("old", Some("Old".into()), Some("keep me".into()), None)
            .expect("root");
        store
            .create_with_id("kid", None, None, Some("old".into()))
            .expect("kid");
        std::fs::write(
            store.project_dir("old").expect("dir").join("NOTES.md"),
            "inside",
        )
        .expect("write");

        let renamed = store.rename("old", "new").expect("rename");
        assert_eq!(renamed.project.id, "new");
        assert_eq!(renamed.subprojects, 1);

        // The old id is an alias now, not an absence: it resolves to the project under its new id.
        assert_eq!(
            store.get("old").expect("get old").expect("present").id,
            "new"
        );
        let moved = store.get("new").expect("get new").expect("present");
        assert_eq!(moved.manifest.name, "Old");
        assert_eq!(moved.manifest.description.as_deref(), Some("keep me"));
        // The directory travelled whole — a rename must never cost a project its files.
        let notes = store.project_dir("new").expect("dir").join("NOTES.md");
        assert_eq!(std::fs::read_to_string(notes).expect("read"), "inside");

        let kid = store.get("kid").expect("get kid").expect("present");
        assert_eq!(kid.manifest.parent.as_deref(), Some("new"));
        assert_eq!(store.children("new").expect("children").len(), 1);
    }

    #[test]
    fn renaming_to_the_same_id_changes_nothing() {
        let store = scratch("rename-noop");
        store.create_with_id("same", None, None, None).expect("p");
        let renamed = store.rename("same", "same").expect("rename");
        assert_eq!(renamed.project.id, "same");
        assert_eq!(renamed.subprojects, 0);
        assert!(store.get("same").expect("get").is_some());
    }

    #[test]
    fn rename_refuses_an_occupied_id_or_an_unknown_project() {
        let store = scratch("rename-guard");
        store.create_with_id("a", None, None, None).expect("a");
        store.create_with_id("b", None, None, None).expect("b");
        assert!(matches!(store.rename("a", "b"), Err(Error::Exists(_))));
        assert!(matches!(
            store.rename("ghost", "c"),
            Err(Error::NotFound(_))
        ));
        assert!(matches!(
            store.rename("a", "../x"),
            Err(Error::InvalidId(_))
        ));
        // A bare directory with no manifest still occupies the id.
        std::fs::create_dir_all(store.dir().join("squatter")).expect("mkdir");
        assert!(matches!(
            store.rename("a", "squatter"),
            Err(Error::Exists(_))
        ));
        // Every refusal left both projects exactly where they were.
        assert!(store.get("a").expect("get a").is_some());
        assert!(store.get("b").expect("get b").is_some());
    }

    #[test]
    fn duplicate_create_is_an_error() {
        let store = scratch("dup");
        store.create_with_id("x", None, None, None).expect("first");
        assert!(matches!(
            store.create_with_id("x", None, None, None),
            Err(Error::Exists(_))
        ));
    }

    #[test]
    fn archive_and_unarchive_toggle_state() {
        let store = scratch("archive");
        store.create_with_id("p", None, None, None).expect("create");

        let archived = store.archive("p").expect("archive");
        assert!(archived.is_archived());
        let stamp = archived.manifest.archived_at.expect("stamp");

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
        store
            .create_with_id("gone", None, None, None)
            .expect("create");
        assert!(store.remove("gone").expect("remove"));
        assert!(store.get("gone").expect("get").is_none());
        assert!(!store.remove("gone").expect("remove missing"));
    }

    #[test]
    fn invalid_ids_never_touch_disk() {
        let store = scratch("invalid");
        assert!(matches!(store.get("../escape"), Err(Error::InvalidId(_))));
        assert!(matches!(
            store.create_with_id("a/b", None, None, None),
            Err(Error::InvalidId(_))
        ));
        assert!(matches!(store.remove(".."), Err(Error::InvalidId(_))));
        assert!(matches!(store.hive_path("../x"), Err(Error::InvalidId(_))));
        assert!(matches!(
            store.project_dir("../x"),
            Err(Error::InvalidId(_))
        ));
    }

    #[test]
    fn project_dir_is_the_id_directory_under_projects() {
        let store = scratch("projdir");
        let dir = store.project_dir("demo").expect("project dir");
        assert!(dir.ends_with("projects/demo"), "got {}", dir.display());
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
        store
            .create_with_id("real", None, None, None)
            .expect("create");
        std::fs::create_dir_all(store.dir().join("bare")).expect("mkdir");
        let all = store.list().expect("list");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "real");
    }
}
