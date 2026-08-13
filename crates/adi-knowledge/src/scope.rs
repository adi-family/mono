//! Who a knowledge base belongs to, and who may touch it.
//!
//! Three isolation levels, and the whole access model falls out of them:
//!
//! * **global** — the machine's shared knowledge. Everyone reads it, everyone writes it.
//! * **project** — knowledge about one codebase. Visible to whoever is working *in* that
//!   project, invisible to anyone who isn't.
//! * **agent** — one agent's own notes. Its owner writes; **every other agent may read**.
//!
//! That last rule is deliberate, and it is the reason agent-level exists as a level rather than
//! as a private file: an agent's memory is worth more to the fleet than to the agent. What
//! isolation buys is that nobody else can *edit* your recollection — not that nobody can consult
//! it.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The base name used when an id names only a scope — `global` means `global/default`.
pub const DEFAULT_BASE: &str = "default";

/// The base name an agent's own memory lives under: `agent:<name>/memory`.
pub const MEMORY_BASE: &str = "memory";

/// The isolation level of a knowledge base, and its owner.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "level", rename_all = "lowercase")]
pub enum Scope {
    /// Machine-wide, shared by everything.
    Global,
    /// Owned by one project id (the `adi-projects` convention).
    Project {
        /// The project id.
        project: String,
    },
    /// Owned by one agent, by name.
    Agent {
        /// The agent name.
        agent: String,
    },
}

impl Scope {
    /// A project scope, validating the id as a path segment.
    ///
    /// # Errors
    /// [`Error::InvalidName`] when `project` is not a safe segment.
    pub fn project(project: impl Into<String>) -> Result<Self> {
        let project = project.into();
        validate_segment(&project)?;
        Ok(Self::Project { project })
    }

    /// An agent scope, validating the name as a path segment.
    ///
    /// # Errors
    /// [`Error::InvalidName`] when `agent` is not a safe segment.
    pub fn agent(agent: impl Into<String>) -> Result<Self> {
        let agent = agent.into();
        validate_segment(&agent)?;
        Ok(Self::Agent { agent })
    }

    /// The one-word level name — `global`, `project`, or `agent`.
    #[must_use]
    pub fn level(&self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project { .. } => "project",
            Self::Agent { .. } => "agent",
        }
    }

    /// The owner id, or `None` for [`Scope::Global`].
    #[must_use]
    pub fn owner(&self) -> Option<&str> {
        match self {
            Self::Global => None,
            Self::Project { project } => Some(project),
            Self::Agent { agent } => Some(agent),
        }
    }

    /// This scope's directory path, relative to the store's `knowledge` module: `global`,
    /// `projects/<id>`, `agents/<name>`.
    #[must_use]
    pub fn rel_dir(&self) -> String {
        match self {
            Self::Global => "global".to_string(),
            Self::Project { project } => format!("projects/{project}"),
            Self::Agent { agent } => format!("agents/{agent}"),
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Global => f.write_str("global"),
            Self::Project { project } => write!(f, "project:{project}"),
            Self::Agent { agent } => write!(f, "agent:{agent}"),
        }
    }
}

impl FromStr for Scope {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        match s.split_once(':') {
            None if s.eq_ignore_ascii_case("global") => Ok(Self::Global),
            Some(("project", id)) => Self::project(id),
            Some(("agent", name)) => Self::agent(name),
            _ => Err(Error::BadBaseId(s.to_string())),
        }
    }
}

/// A knowledge base's address: its [`Scope`] and its name within that scope.
///
/// Written `<scope>/<name>` — `global/notes`, `project:acme/runbook`, `agent:solver/memory` —
/// and a bare scope means that scope's [`DEFAULT_BASE`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BaseId {
    /// Which isolation level this base sits at, and whose it is.
    pub scope: Scope,
    /// The base's name within its scope.
    pub name: String,
}

impl BaseId {
    /// Build an id, validating the name as a path segment.
    ///
    /// # Errors
    /// [`Error::InvalidName`] when `name` is not a safe segment.
    pub fn new(scope: Scope, name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        validate_segment(&name)?;
        Ok(Self { scope, name })
    }

    /// `global/default` — the base a machine has before anybody makes one.
    #[must_use]
    pub fn global_default() -> Self {
        Self {
            scope: Scope::Global,
            name: DEFAULT_BASE.to_string(),
        }
    }

    /// The base an agent's own memory lives in: `agent:<agent>/memory`.
    ///
    /// # Errors
    /// [`Error::InvalidName`] when `agent` is not a safe segment.
    pub fn memory(agent: &str) -> Result<Self> {
        Self::new(Scope::agent(agent)?, MEMORY_BASE)
    }

    /// This base's directory relative to the `knowledge` module — `<scope dir>/<name>`.
    #[must_use]
    pub fn rel_dir(&self) -> String {
        format!("{}/{}", self.scope.rel_dir(), self.name)
    }

    /// Whether this base is an agent's own memory.
    #[must_use]
    pub fn is_memory(&self) -> bool {
        matches!(self.scope, Scope::Agent { .. }) && self.name == MEMORY_BASE
    }
}

impl fmt::Display for BaseId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.scope, self.name)
    }
}

impl FromStr for BaseId {
    type Err = Error;

    /// Parse `<scope>[/<name>]`. The scope half never contains a `/`, so the *first* slash is
    /// the separator; a missing name means [`DEFAULT_BASE`].
    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        match s.split_once('/') {
            Some((scope, name)) => Self::new(scope.parse()?, name.trim()),
            None => Self::new(s.parse()?, DEFAULT_BASE),
        }
    }
}

/// What a reader may do to a base. [`Access::Write`] implies [`Access::Read`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    /// May search and read, but not add, edit, or delete.
    Read,
    /// May do anything, reading included.
    Write,
}

impl Access {
    /// Whether this access permits reading — always true, stated so call sites read plainly.
    #[must_use]
    pub fn can_read(self) -> bool {
        true
    }

    /// Whether this access permits mutation.
    #[must_use]
    pub fn can_write(self) -> bool {
        self == Self::Write
    }
}

/// Who is asking. Every access decision is a function of this and a [`BaseId`].
///
/// A person at the CLI is [`Reader::admin`] — the store is their data, and a permission model
/// that told them otherwise would only be theatre. An agent is [`Reader::agent`], and the levels
/// mean what [the module docs](self) say they mean.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reader {
    /// The agent asking, if one is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The project the asker is working in, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Whether every base is readable and writable regardless of the rest — the human at the
    /// terminal, and the control panel acting for them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub admin: bool,
}

impl Reader {
    /// The person who owns the store: everything, everywhere.
    #[must_use]
    pub fn admin() -> Self {
        Self {
            admin: true,
            ..Self::default()
        }
    }

    /// An agent, optionally working inside a project.
    #[must_use]
    pub fn agent(name: impl Into<String>, project: Option<&str>) -> Self {
        Self {
            agent: Some(name.into()),
            project: project.map(ToString::to_string),
            admin: false,
        }
    }

    /// Somebody working in a project but not on any agent's behalf.
    #[must_use]
    pub fn project(id: impl Into<String>) -> Self {
        Self {
            agent: None,
            project: Some(id.into()),
            admin: false,
        }
    }

    /// What this reader may do to `base`, or `None` when the base is not theirs to see.
    ///
    /// The three levels, in one match:
    ///
    /// | base scope | reader | access |
    /// | --- | --- | --- |
    /// | any | admin | write |
    /// | global | anyone | write |
    /// | project `p` | in project `p` | write |
    /// | project `p` | anyone else | none |
    /// | agent `a` | agent `a` | write |
    /// | agent `a` | any other agent | **read** |
    /// | agent `a` | nobody in particular | none |
    #[must_use]
    pub fn access(&self, base: &BaseId) -> Option<Access> {
        if self.admin {
            return Some(Access::Write);
        }
        match &base.scope {
            Scope::Global => Some(Access::Write),
            Scope::Project { project } => (self.project.as_deref() == Some(project.as_str()))
                .then_some(Access::Write),
            Scope::Agent { agent } => match self.agent.as_deref() {
                Some(me) if me == agent => Some(Access::Write),
                // Every other agent may consult it. This is the point of the level.
                Some(_) => Some(Access::Read),
                None => None,
            },
        }
    }

    /// Check that this reader may read `base`.
    ///
    /// # Errors
    /// [`Error::Denied`] when the base is invisible to them.
    pub fn require_read(&self, base: &BaseId) -> Result<Access> {
        self.access(base).ok_or_else(|| self.denied(base, "read"))
    }

    /// Check that this reader may write to `base`.
    ///
    /// # Errors
    /// [`Error::Denied`] when the base is invisible or read-only to them.
    pub fn require_write(&self, base: &BaseId) -> Result<()> {
        match self.access(base) {
            Some(Access::Write) => Ok(()),
            _ => Err(self.denied(base, "write")),
        }
    }

    fn denied(&self, base: &BaseId, verb: &'static str) -> Error {
        Error::Denied {
            reader: self.label(),
            verb,
            base: base.to_string(),
        }
    }

    /// How this reader is named in an error.
    #[must_use]
    pub fn label(&self) -> String {
        match (&self.agent, &self.project) {
            (Some(a), _) => format!("agent {a}"),
            (None, Some(p)) => format!("project {p}"),
            (None, None) => "an anonymous reader".to_string(),
        }
    }
}

/// Reject anything that is not a safe single path segment.
///
/// Bases, projects, and agents all become directory names, so the rule is the same for all
/// three: ASCII letters, digits, `-`, `_`, `.`, no leading `.`, no `/`, and non-empty. The
/// leading-dot ban is what keeps a base out of `..` and away from the dot-directories the store
/// uses for its own bookkeeping.
///
/// # Errors
/// [`Error::InvalidName`] when `value` breaks any of that.
pub fn validate_segment(value: &str) -> Result<()> {
    let ok = !value.is_empty()
        && !value.starts_with('.')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidName(value.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_scope_is_that_scopes_default_base() {
        assert_eq!("global".parse::<BaseId>().unwrap(), BaseId::global_default());
        assert_eq!(
            "project:acme".parse::<BaseId>().unwrap().name,
            DEFAULT_BASE
        );
    }

    #[test]
    fn base_ids_round_trip_through_their_written_form() {
        for text in [
            "global/default",
            "global/runbooks",
            "project:acme/notes",
            "agent:solver/memory",
        ] {
            let id: BaseId = text.parse().expect("parse");
            assert_eq!(id.to_string(), text);
        }
    }

    #[test]
    fn a_scope_nobody_defined_is_not_quietly_accepted() {
        for text in ["team:acme", "", "global:extra", "/notes", "project:"] {
            assert!(text.parse::<BaseId>().is_err(), "{text:?} should not parse");
        }
    }

    #[test]
    fn a_name_that_would_escape_its_directory_is_refused() {
        for bad in ["..", ".hidden", "a/b", "a b", ""] {
            assert!(validate_segment(bad).is_err(), "{bad:?} should be refused");
        }
        for good in ["notes", "run-books", "v1.2", "_scratch"] {
            assert!(validate_segment(good).is_ok(), "{good:?} should be allowed");
        }
    }

    #[test]
    fn global_is_everybodys() {
        let base = BaseId::global_default();
        for reader in [
            Reader::admin(),
            Reader::agent("solver", None),
            Reader::project("acme"),
            Reader::default(),
        ] {
            assert_eq!(reader.access(&base), Some(Access::Write));
        }
    }

    #[test]
    fn a_project_base_is_invisible_outside_its_project() {
        let base: BaseId = "project:acme/notes".parse().unwrap();
        assert_eq!(
            Reader::agent("solver", Some("acme")).access(&base),
            Some(Access::Write)
        );
        assert_eq!(Reader::agent("solver", Some("other")).access(&base), None);
        assert_eq!(Reader::agent("solver", None).access(&base), None);
        assert_eq!(Reader::admin().access(&base), Some(Access::Write));
    }

    /// The rule the agent level exists for: another agent's memory is readable, never writable.
    #[test]
    fn agents_may_read_each_others_knowledge_but_only_write_their_own() {
        let base = BaseId::memory("solver").unwrap();
        assert_eq!(
            Reader::agent("solver", None).access(&base),
            Some(Access::Write)
        );
        assert_eq!(
            Reader::agent("reviewer", None).access(&base),
            Some(Access::Read)
        );
        assert!(Reader::agent("reviewer", None).require_read(&base).is_ok());
        assert!(Reader::agent("reviewer", None).require_write(&base).is_err());
        // Nobody in particular is not "some agent" — an unattributed caller gets nothing.
        assert_eq!(Reader::project("acme").access(&base), None);
    }

    #[test]
    fn a_denial_says_who_wanted_what() {
        let base = BaseId::memory("solver").unwrap();
        let err = Reader::agent("reviewer", None)
            .require_write(&base)
            .unwrap_err()
            .to_string();
        assert!(err.contains("reviewer"), "{err}");
        assert!(err.contains("write"), "{err}");
        assert!(err.contains("agent:solver/memory"), "{err}");
    }

    #[test]
    fn a_scopes_directory_keeps_the_three_levels_apart() {
        assert_eq!(Scope::Global.rel_dir(), "global");
        assert_eq!(Scope::project("acme").unwrap().rel_dir(), "projects/acme");
        assert_eq!(Scope::agent("solver").unwrap().rel_dir(), "agents/solver");
        assert_eq!(
            BaseId::memory("solver").unwrap().rel_dir(),
            "agents/solver/memory"
        );
    }
}
