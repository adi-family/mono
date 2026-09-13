//! The eight kinds a bundle element may be — agents, tools, dashboards, LLM and embedding
//! backends, hive services, triggers, a project scaffold (`docs/marketplace-bundles.md`).
//!
//! Each kind carries two spellings, and they are deliberately not the same word:
//!
//! * [`Kind::wire`] is what `elements[].kind` publishes in the manifest — singular, matching the
//!   table in the design document exactly.
//! * [`Kind::dir`] is the directory the kind lives under in a bundle repository, and the segment
//!   an address names it by (`<marketplace>/<slug>/<kind>/<name>`) — taken verbatim from the
//!   layout, because a manifest's advisory `kind` and the tree's real layout are two different
//!   things that happen to agree most of the time, not one fact written twice.

use std::fmt;

/// One of the eight platform surfaces a bundle may carry an element for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Agent,
    Tool,
    Dashboard,
    Llm,
    Embedding,
    Service,
    Trigger,
    Project,
}

impl Kind {
    /// Every kind, in the order the repository layout table lists them.
    pub const ALL: [Kind; 8] = [
        Kind::Agent,
        Kind::Tool,
        Kind::Dashboard,
        Kind::Llm,
        Kind::Embedding,
        Kind::Service,
        Kind::Trigger,
        Kind::Project,
    ];

    /// The word `elements[].kind` publishes.
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Kind::Agent => "agent",
            Kind::Tool => "tool",
            Kind::Dashboard => "dashboard",
            Kind::Llm => "llm",
            Kind::Embedding => "embedding",
            Kind::Service => "service",
            Kind::Trigger => "trigger",
            Kind::Project => "project",
        }
    }

    /// The directory this kind lives under in a bundle repository, and the segment an address
    /// names it by.
    #[must_use]
    pub fn dir(self) -> &'static str {
        match self {
            Kind::Agent => "agents",
            Kind::Tool => "tools",
            Kind::Dashboard => "dashboards",
            Kind::Llm => "llm",
            Kind::Embedding => "embeddings",
            Kind::Service => "services",
            Kind::Trigger => "triggers",
            Kind::Project => "project",
        }
    }

    /// Whether this kind is a singleton — at most one per bundle, addressed with no name
    /// (`<marketplace>/<slug>/project`), because the bundle carries at most one project scaffold.
    #[must_use]
    pub fn is_singleton(self) -> bool {
        matches!(self, Kind::Project)
    }

    /// The kind `elements[].kind` names, or `None` for a word this build does not understand.
    ///
    /// Not a validation failure to come back `None`: [`crate::BundleEntry::elements`] simply
    /// leaves such an element out of the preview, the same tolerance every other unrecognised
    /// field in this manifest gets (`docs/marketplace-bundles.md` decision #2).
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Kind> {
        Self::ALL.into_iter().find(|k| k.wire() == word)
    }

    /// The kind a repository's top-level directory name is, or `None` for anything else there —
    /// which is not an error, just not one of the eight kind directories.
    #[must_use]
    pub fn from_dir(word: &str) -> Option<Kind> {
        Self::ALL.into_iter().find(|k| k.dir() == word)
    }
}

impl fmt::Display for Kind {
    /// The address spelling — the directory name — since that is where a `Kind` is rendered back
    /// out to a person, in `<marketplace>/<slug>/<kind>/<name>`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.dir())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_round_trips_through_both_spellings() {
        for kind in Kind::ALL {
            assert_eq!(Kind::from_wire(kind.wire()), Some(kind));
            assert_eq!(Kind::from_dir(kind.dir()), Some(kind));
        }
    }

    #[test]
    fn llm_and_project_spell_the_same_either_way_everything_else_does_not() {
        assert_eq!(Kind::Llm.wire(), Kind::Llm.dir());
        assert_eq!(Kind::Project.wire(), Kind::Project.dir());
        assert_ne!(Kind::Agent.wire(), Kind::Agent.dir(), "singular vs. the directory's plural");
    }

    #[test]
    fn only_project_is_a_singleton() {
        assert!(Kind::Project.is_singleton());
        for kind in Kind::ALL {
            if kind != Kind::Project {
                assert!(!kind.is_singleton(), "{kind}");
            }
        }
    }

    #[test]
    fn an_unrecognised_word_is_not_a_kind() {
        assert_eq!(Kind::from_wire("apps"), None);
        assert_eq!(Kind::from_wire("agents"), None, "the manifest spells it singular");
        assert_eq!(Kind::from_dir("agent"), None, "the address spells it plural");
    }
}
