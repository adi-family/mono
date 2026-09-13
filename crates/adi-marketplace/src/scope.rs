//! Where an install files everything it lands: this machine's global scope, or one project.
//!
//! v2 shipped with one install per bundle and no choice about where it went — a bundle's elements
//! landed globally unless the bundle itself carried a `project/config.toml`, which is what
//! `docs/marketplace-bundles.md` decision #4 means by "a bundle's own project is the only way to
//! get project-scoped elements". That is still true of what a *publisher* can ask for. What this
//! module adds is what an **operator** can ask for: the destination is theirs to choose, and the
//! same bundle may be installed into as many projects as they like.
//!
//! The scope is the key everything per-install is filed under — the ledger, the bundle's own
//! clone, its parked services — so two installs of one bundle never share a pin, a ledger, or an
//! update. That is what makes "the changelog tools in project A, and again in project B, each at
//! its own version" a thing this store can hold.
//!
//! **Why a project is the recommended destination, and global is not.** Nothing here enforces it;
//! it is a recommendation because of what the store already does with the `project` field every
//! kind carries:
//!
//! * A project-scoped tool runs in its project's directory and against its project's database
//!   (`adi_tools`), and a project-scoped agent, trigger or dashboard is filed the same way.
//! * Uninstalling is "the things filed under this project", not "which of the forty tools on this
//!   machine came from where".
//! * It is the only way to have the bundle twice. Every kind's id lives in one global namespace,
//!   so a second global copy of a tool called `changelog` can only land as `changelog-2` — and in
//!   the global scope that is refused outright rather than risked (decision #3), because an
//!   agent's `bin_tools = ["changelog"]` would otherwise name a stranger's script.
//!
//! Global is not *dangerous* in the sense of destroying anything. It is the scope every agent on
//! the machine can reach, it spends the shared id namespace, and it is the one where a second copy
//! of the same bundle has nowhere to go. Those are the three things the panel says out loud.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The path segment the global scope's own state is filed under. Leading underscore because a
/// project id is a slug (`adi_config::slug`), which never starts with one — so this can never be
/// the name of a project and a project can never be mistaken for it.
pub const GLOBAL_KEY: &str = "_global";

/// Where one install of a bundle lives.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Scope {
    /// The machine's own scope: nothing is filed under a project, and every agent can reach it.
    Global,
    /// One project, by id. Everything the install lands carries this in its own `project` field.
    Project(String),
}

impl Scope {
    /// The scope a `project` id names, or [`Scope::Global`] for `None` — the shape every caller
    /// (CLI flag, API field) actually has in hand.
    ///
    /// # Errors
    /// [`Error::BadScope`] for an id that is not one safe path segment, or that spells
    /// [`GLOBAL_KEY`] — either would put a project's state where the global scope's belongs.
    pub fn from_project(project: Option<&str>) -> Result<Self> {
        let Some(project) = project.map(str::trim).filter(|p| !p.is_empty()) else {
            return Ok(Scope::Global);
        };
        if project == GLOBAL_KEY || !adi_config::valid_name(project) {
            return Err(Error::BadScope(project.to_string()));
        }
        Ok(Scope::Project(project.to_string()))
    }

    /// The project this scope files under, or `None` for the global one — what every `land_*`
    /// function takes and every landed manifest's own `project` field is written from.
    #[must_use]
    pub fn project(&self) -> Option<&str> {
        match self {
            Scope::Global => None,
            Scope::Project(id) => Some(id),
        }
    }

    /// The path segment this scope's own per-install state is filed under.
    #[must_use]
    pub fn key(&self) -> &str {
        match self {
            Scope::Global => GLOBAL_KEY,
            Scope::Project(id) => id,
        }
    }

    /// Whether this install is filed under a project — the question the two collision regimes turn
    /// on (`crate::bundle`: a scoped install may mint past a collision and rewrite its own
    /// references to match; a global one may not).
    #[must_use]
    pub fn is_project(&self) -> bool {
        matches!(self, Scope::Project(_))
    }

    /// Register a project under `name` and answer the scope that files an install under it — the
    /// recommended path's own first step, in the library rather than in each door, so the CLI's
    /// `--new-project` and the panel's "a new project" do exactly the same thing.
    ///
    /// Registered *before* the install rather than after: an install that then fails leaves an
    /// empty project the operator can remove on the Projects page, where the other order would
    /// leave landed elements with nowhere to be filed.
    ///
    /// # Errors
    /// Whatever `adi_projects` refuses a creation for — a blank name, or a store it cannot write.
    pub fn new_project(config: &adi_config::Config, name: &str) -> Result<Self> {
        let project = adi_projects::Projects::with_config(config.clone())
            .create(name, None, None)
            .map_err(|e| Error::Store(e.to_string()))?;
        Self::from_project(Some(&project.id))
    }

    /// The scope a per-install directory name means, the inverse of [`Scope::key`] — how a listing
    /// reads back what is installed without being told what to look for.
    #[must_use]
    pub fn from_key(key: &str) -> Self {
        if key == GLOBAL_KEY {
            Scope::Global
        } else {
            Scope::Project(key.to_string())
        }
    }
}

impl fmt::Display for Scope {
    /// How a scope is said to a person: the project's id, or the word for having no project.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Scope::Global => f.write_str("globally"),
            Scope::Project(id) => write!(f, "in project {id}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_project_is_the_global_scope() {
        assert_eq!(Scope::from_project(None).expect("global"), Scope::Global);
        assert_eq!(Scope::from_project(Some("  ")).expect("blank"), Scope::Global);
        assert_eq!(Scope::Global.project(), None);
        assert_eq!(Scope::Global.key(), GLOBAL_KEY);
        assert!(!Scope::Global.is_project());
    }

    #[test]
    fn a_project_scope_carries_its_id_everywhere_it_is_asked_for() {
        let scope = Scope::from_project(Some("ops")).expect("project");
        assert_eq!(scope, Scope::Project("ops".to_string()));
        assert_eq!(scope.project(), Some("ops"));
        assert_eq!(scope.key(), "ops");
        assert!(scope.is_project());
    }

    #[test]
    fn a_key_round_trips_and_the_global_one_is_never_a_project() {
        assert_eq!(Scope::from_key(GLOBAL_KEY), Scope::Global);
        assert_eq!(Scope::from_key("ops"), Scope::Project("ops".to_string()));
        // The reserved key cannot be claimed by a project, in either direction.
        assert!(Scope::from_project(Some(GLOBAL_KEY)).is_err());
    }

    #[test]
    fn an_id_that_is_not_one_path_segment_is_refused_rather_than_joined() {
        for bad in ["../escape", "a/b", "", "."] {
            assert!(
                Scope::from_project(Some(bad)).is_err() || bad.is_empty(),
                "{bad:?} should not become a directory name"
            );
        }
    }
}
