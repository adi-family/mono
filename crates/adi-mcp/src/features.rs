//! The runtime feature system: which tool groups — and, optionally, which *individual tools*
//! within a group — an `adi-mcp` process exposes.
//!
//! Agents launch us as `adi-mcp --features "tasks,projects"`; each name turns on one
//! [`Feature`]'s tools. A feature name may be followed by a square-bracket selector to expose
//! only a subset of that feature's tools (by their short name, i.e. without the group prefix):
//!
//! ```text
//! adi-mcp --features "tasks"                      # all tasks_* tools
//! adi-mcp --features "tasks[create,list]"         # only tasks_create and tasks_list
//! adi-mcp --features "tasks[create],files[read],status"
//! ```
//!
//! This keeps a single binary while letting the init flow scope an agent to exactly the tools
//! it should see. [`FeatureSet::parse`] turns the command-line spec into the selection that
//! [`AdiMcp::new`](crate::AdiMcp::new) reads (via [`FeatureSet::contains`] and
//! [`FeatureSet::includes_tool`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

/// One selectable group of MCP tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Feature {
    /// A persistent task tracker agents read and write (`tasks_*`).
    Tasks,
    /// The adi projects registry (`projects_*`).
    Projects,
    /// Jailed file access inside a project directory (`files_*`).
    Files,
    /// Read-only platform service status (`status_*`).
    Status,
}

impl Feature {
    /// Every feature, in display order.
    pub const ALL: [Feature; 4] = [
        Feature::Tasks,
        Feature::Projects,
        Feature::Files,
        Feature::Status,
    ];

    /// The token used on the `--features` command line.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Feature::Tasks => "tasks",
            Feature::Projects => "projects",
            Feature::Files => "files",
            Feature::Status => "status",
        }
    }

    /// A one-line summary for `--list-features`.
    #[must_use]
    pub const fn summary(self) -> &'static str {
        match self {
            Feature::Tasks => "persistent task tracker",
            Feature::Projects => "adi projects registry",
            Feature::Files => "jailed project file access",
            Feature::Status => "platform service status (read-only)",
        }
    }

    /// The *short* names of this feature's tools — what goes inside the `[...]` selector. The
    /// full MCP tool name is `<feature>_<short>` (see [`Feature::full_tool_name`]); this is the
    /// single source of truth for what a bracket selector may name.
    #[must_use]
    pub const fn tools(self) -> &'static [&'static str] {
        match self {
            Feature::Tasks => &["create", "list", "get", "update", "delete"],
            Feature::Projects => &["list", "get", "create", "archive", "unarchive"],
            Feature::Files => &["list", "read", "write"],
            Feature::Status => &["report"],
        }
    }

    /// The full MCP tool name for one of this feature's short tool names, e.g.
    /// `(Tasks, "create") -> "tasks_create"`. Matches the `#[tool]` fn names in the modules.
    #[must_use]
    pub fn full_tool_name(self, short: &str) -> String {
        format!("{}_{short}", self.name())
    }

    /// Whether `short` names one of this feature's tools.
    #[must_use]
    fn has_tool(self, short: &str) -> bool {
        self.tools().contains(&short)
    }
}

impl fmt::Display for Feature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Feature {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "tasks" => Ok(Feature::Tasks),
            "projects" => Ok(Feature::Projects),
            "files" => Ok(Feature::Files),
            "status" => Ok(Feature::Status),
            other => Err(format!("unknown feature {other:?}; valid features: {}", all_names())),
        }
    }
}

/// Which of a feature's tools are enabled.
#[derive(Debug, Clone)]
enum Selection {
    /// Every tool the feature offers.
    All,
    /// Only these tools (short names). Never empty — [`FeatureSet::parse`] rejects `feature[]`.
    Only(BTreeSet<String>),
}

impl Selection {
    /// Fold another selection of the *same* feature into this one (repeated features union
    /// their tools; any `All` wins).
    fn absorb(&mut self, other: Selection) {
        match (&mut *self, other) {
            (Selection::All, _) => {}
            (slot, Selection::All) => *slot = Selection::All,
            (Selection::Only(here), Selection::Only(more)) => here.extend(more),
        }
    }

    /// Whether `short` is enabled under this selection.
    fn includes(&self, short: &str) -> bool {
        match self {
            Selection::All => true,
            Selection::Only(set) => set.contains(short),
        }
    }
}

/// The set of features (and per-feature tool selections) enabled for a process — parsed once
/// from `--features`.
#[derive(Debug, Clone, Default)]
pub struct FeatureSet {
    features: BTreeMap<Feature, Selection>,
}

impl FeatureSet {
    /// Every feature enabled, with all of its tools.
    #[must_use]
    pub fn all() -> Self {
        Self {
            features: Feature::ALL
                .into_iter()
                .map(|f| (f, Selection::All))
                .collect(),
        }
    }

    /// Parse a comma-separated `--features` spec. `"all"` (or an empty spec) enables every
    /// feature and all of its tools. Otherwise each comma-separated segment is a feature name,
    /// optionally followed by a `[tool,tool,…]` selector naming a subset of that feature's
    /// tools (short names, case-insensitive). Commas inside `[...]` separate tools, not
    /// features. Surrounding whitespace and empty segments are ignored, and a repeated feature
    /// unions its tool selections. Examples: `"tasks, projects"`, `"tasks[create,list],status"`.
    ///
    /// # Errors
    /// Returns a human-readable message for an unknown feature, an unknown tool, an empty
    /// `feature[]` selector, or unbalanced brackets.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let spec = spec.trim();
        if spec.is_empty() || spec.eq_ignore_ascii_case("all") {
            return Ok(Self::all());
        }
        let mut features: BTreeMap<Feature, Selection> = BTreeMap::new();
        for segment in split_segments(spec)? {
            let segment = segment.trim();
            if segment.is_empty() {
                continue;
            }
            if segment.eq_ignore_ascii_case("all") {
                return Ok(Self::all());
            }
            let (name, tools) = parse_segment(segment)?;
            let feature = name.parse::<Feature>()?;
            let selection = build_selection(feature, tools)?;
            features
                .entry(feature)
                .and_modify(|existing| existing.absorb(selection.clone()))
                .or_insert(selection);
        }
        Ok(Self { features })
    }

    /// Whether `feature`'s tools should be registered at all.
    #[must_use]
    pub fn contains(&self, feature: Feature) -> bool {
        self.features.contains_key(&feature)
    }

    /// Whether a specific tool (its short name) of `feature` is enabled. `false` if the feature
    /// itself is not enabled.
    #[must_use]
    pub fn includes_tool(&self, feature: Feature, short: &str) -> bool {
        self.features
            .get(&feature)
            .is_some_and(|sel| sel.includes(short))
    }

    /// Whether no features are enabled.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }

    /// The enabled features, in display order.
    pub fn iter(&self) -> impl Iterator<Item = Feature> + '_ {
        self.features.keys().copied()
    }
}

/// Build a [`Selection`] for `feature` from an optional list of short tool names, validating
/// each name and rejecting an empty selector.
fn build_selection(feature: Feature, tools: Option<Vec<&str>>) -> Result<Selection, String> {
    let Some(list) = tools else {
        return Ok(Selection::All);
    };
    let mut set = BTreeSet::new();
    for tool in list {
        let tool = tool.trim().to_ascii_lowercase();
        if tool.is_empty() {
            continue;
        }
        if !feature.has_tool(&tool) {
            return Err(format!(
                "feature {:?} has no tool {tool:?}; its tools: {}",
                feature.name(),
                feature.tools().join(", ")
            ));
        }
        set.insert(tool);
    }
    if set.is_empty() {
        return Err(format!(
            "feature {:?} has an empty tool selector; name at least one of: {}",
            feature.name(),
            feature.tools().join(", ")
        ));
    }
    Ok(Selection::Only(set))
}

/// Split a spec into feature segments on top-level commas — commas inside a `[...]` tool
/// selector are left intact.
///
/// # Errors
/// A message if the brackets are unbalanced.
fn split_segments(spec: &str) -> Result<Vec<&str>, String> {
    let mut segments = Vec::new();
    let mut depth: i32 = 0;
    let mut start = 0;
    // Feature names, commas, and brackets are all ASCII, so byte indices line up with chars.
    for (i, c) in spec.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth < 0 {
                    return Err("unbalanced ']' in --features".to_string());
                }
            }
            ',' if depth == 0 => {
                segments.push(&spec[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err("unbalanced '[' in --features".to_string());
    }
    segments.push(&spec[start..]);
    Ok(segments)
}

/// Parse one segment into a feature name and an optional list of short tool names.
///
/// # Errors
/// A message if a `[` opens without a closing `]` at the end of the segment.
fn parse_segment(segment: &str) -> Result<(&str, Option<Vec<&str>>), String> {
    match segment.find('[') {
        None => Ok((segment.trim(), None)),
        Some(open) => {
            if !segment.ends_with(']') {
                return Err(format!(
                    "malformed tool selector in {segment:?}: expected 'feature[tool,...]'"
                ));
            }
            let name = segment[..open].trim();
            let inner = &segment[open + 1..segment.len() - 1];
            Ok((name, Some(inner.split(',').collect())))
        }
    }
}

/// The comma-separated list of every feature name (for help/error text).
fn all_names() -> String {
    Feature::ALL
        .into_iter()
        .map(Feature::name)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_enables_every_feature_and_tool() {
        let set = FeatureSet::parse("all").expect("parse all");
        for f in Feature::ALL {
            assert!(set.contains(f), "{f} should be enabled");
            for tool in f.tools() {
                assert!(set.includes_tool(f, tool), "{f}_{tool} should be enabled");
            }
        }
    }

    #[test]
    fn empty_spec_defaults_to_all() {
        let set = FeatureSet::parse("   ").expect("parse blank");
        assert_eq!(set.iter().count(), Feature::ALL.len());
    }

    #[test]
    fn a_subset_of_features_enables_only_those() {
        let set = FeatureSet::parse("tasks, projects,").expect("parse subset");
        assert!(set.contains(Feature::Tasks));
        assert!(set.contains(Feature::Projects));
        assert!(!set.contains(Feature::Files));
        assert!(!set.contains(Feature::Status));
    }

    #[test]
    fn bracket_selector_enables_only_named_tools() {
        let set = FeatureSet::parse("tasks[create,list]").expect("parse selector");
        assert!(set.contains(Feature::Tasks));
        assert!(set.includes_tool(Feature::Tasks, "create"));
        assert!(set.includes_tool(Feature::Tasks, "list"));
        assert!(!set.includes_tool(Feature::Tasks, "get"));
        assert!(!set.includes_tool(Feature::Tasks, "delete"));
    }

    #[test]
    fn a_bare_feature_enables_all_its_tools() {
        let set = FeatureSet::parse("files").expect("parse bare");
        for tool in Feature::Files.tools() {
            assert!(set.includes_tool(Feature::Files, tool));
        }
    }

    #[test]
    fn commas_inside_brackets_do_not_split_features() {
        let set = FeatureSet::parse("tasks[create,list],files[read],status").expect("parse mixed");
        assert!(set.includes_tool(Feature::Tasks, "create"));
        assert!(set.includes_tool(Feature::Tasks, "list"));
        assert!(!set.includes_tool(Feature::Tasks, "delete"));
        assert!(set.includes_tool(Feature::Files, "read"));
        assert!(!set.includes_tool(Feature::Files, "write"));
        // status has no selector, so all of its tools are on.
        assert!(set.includes_tool(Feature::Status, "report"));
    }

    #[test]
    fn feature_and_tool_names_are_case_insensitive() {
        let set = FeatureSet::parse("TASKS[Create],Files").expect("parse mixed case");
        assert!(set.includes_tool(Feature::Tasks, "create"));
        assert!(set.includes_tool(Feature::Files, "read"));
    }

    #[test]
    fn a_repeated_feature_unions_its_tool_selections() {
        let set = FeatureSet::parse("tasks[create],tasks[list]").expect("parse repeated");
        assert!(set.includes_tool(Feature::Tasks, "create"));
        assert!(set.includes_tool(Feature::Tasks, "list"));
        assert!(!set.includes_tool(Feature::Tasks, "get"));

        // A bare mention promotes the selection to all tools.
        let promoted = FeatureSet::parse("tasks[create],tasks").expect("parse promote");
        assert!(promoted.includes_tool(Feature::Tasks, "delete"));
    }

    #[test]
    fn an_unknown_feature_is_an_error() {
        let err = FeatureSet::parse("tasks,bogus").expect_err("bogus rejected");
        assert!(err.contains("bogus"), "error should name the bad token: {err}");
    }

    #[test]
    fn an_unknown_tool_is_an_error() {
        let err = FeatureSet::parse("tasks[create,fly]").expect_err("fly rejected");
        assert!(err.contains("fly"), "error should name the bad tool: {err}");
    }

    #[test]
    fn an_empty_selector_is_an_error() {
        assert!(FeatureSet::parse("tasks[]").is_err());
        assert!(FeatureSet::parse("tasks[ , ]").is_err());
    }

    #[test]
    fn unbalanced_brackets_are_an_error() {
        assert!(FeatureSet::parse("tasks[create").is_err());
        assert!(FeatureSet::parse("tasks create]").is_err());
    }
}
