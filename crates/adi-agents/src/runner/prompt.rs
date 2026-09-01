//! The one place a run's system prompt is composed.
//!
//! A prompt is the agent's own instructions with three things appended, in a fixed order: where the
//! run is, what it knows, and what it can call. Every runner in this tree follows that order, and
//! [`compose`] is the whole of it — one function, so a screen that shows a person "the prompt the
//! model receives" is showing the same string the model receives, byte for byte.
//!
//! That is not a tidiness argument. The moment composition exists twice, the second copy renders a
//! prompt nobody is ever actually run under, and every conclusion drawn from reading it is about a
//! run that did not happen.
//!
//! # Why the pieces are still separate
//!
//! [`compose`] is the whole chain, and it is what an engine with a single prompt slot wants. Some
//! engines have two — the Claude CLI takes `--system-prompt` and `--append-system-prompt`
//! separately — and for those the agent's own words and the three appended blocks go to different
//! flags. So the steps stay callable on their own, and `compose` is their composition rather than a
//! fourth thing that reimplements them.

use crate::tool_help;

use super::RunSpec;

/// This run's system prompt, whole: the agent's own words, then where it is, then what it knows,
/// then what it can call.
///
/// `stored` is whatever the engine's saved arguments already carried, used only when the spec has
/// no prompt of its own.
///
/// `None` means there is genuinely nothing to say — no prompt, no workspace note, no knowledge, no
/// tools — which is different from an empty string and is why the type says so.
#[must_use]
pub fn compose(spec: &RunSpec, stored: Option<String>) -> Option<String> {
    with_tool_help(
        spec,
        with_knowledge(spec, with_workspace(spec, own_prompt(spec, stored))),
    )
}

/// One stretch of a composed prompt, and what it is.
///
/// Borrowed from the prompt rather than copied, and the slices are contiguous and complete: joined
/// back together they are the prompt again, exactly. That is what lets a caller tokenize each piece
/// and end up with token *ranges* for the whole rather than an approximation of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section<'a> {
    /// What this stretch is, for a reader: `instructions`, `where you are`, `what you know`,
    /// `your tools`.
    pub label: &'static str,
    /// The text, verbatim.
    pub text: &'a str,
}

/// The headings [`compose`] appends, in the order it appends them.
///
/// Here rather than in the three modules that write them because this is the file that knows the
/// order — and a reader splitting the prompt back up needs the order, not the individual strings.
/// They are matched at the start of a line, so a prompt that merely *mentions* `# Your tools` in
/// prose does not split there.
const HEADINGS: [(&str, &str); 3] = [
    ("where you are", "# Where you are"),
    ("what you know", "# What you know"),
    ("your tools", "# Your tools"),
];

/// A composed prompt, cut back into the sections it was assembled from.
///
/// The point is a reader, not a parser: a person shown six thousand tokens of prompt needs to see
/// where the agent's own words stop and the machinery starts, and no amount of colour does that if
/// the boundaries are guessed at. So the boundaries come from here, where they were put in.
///
/// A prompt with nothing appended is one section. A section that is not there is not reported —
/// there is no empty `what you know` for an agent with no memory.
#[must_use]
pub fn sections(prompt: &str) -> Vec<Section<'_>> {
    // Where each heading starts, in the order they appear. Searched in composition order and each
    // from the end of the last, so a heading quoted *inside* a later section cannot pull a cut
    // backwards.
    let mut cuts: Vec<(&'static str, usize)> = Vec::new();
    let mut from = 0usize;
    for (label, heading) in HEADINGS {
        let Some(at) = find_heading(prompt, from, heading) else {
            continue;
        };
        cuts.push((label, at));
        from = at + heading.len();
    }

    let mut out = Vec::with_capacity(cuts.len() + 1);
    let head = cuts.first().map_or(prompt.len(), |(_, at)| *at);
    if head > 0 {
        out.push(Section {
            label: "instructions",
            text: &prompt[..head],
        });
    }
    for (i, (label, at)) in cuts.iter().enumerate() {
        let end = cuts.get(i + 1).map_or(prompt.len(), |(_, next)| *next);
        out.push(Section {
            label,
            text: &prompt[*at..end],
        });
    }
    out
}

/// Where `heading` starts a line at or after `from`, if it does.
fn find_heading(prompt: &str, from: usize, heading: &str) -> Option<usize> {
    let mut at = from;
    while let Some(hit) = prompt[at..].find(heading) {
        let start = at + hit;
        if start == 0 || prompt.as_bytes()[start - 1] == b'\n' {
            return Some(start);
        }
        at = start + heading.len();
    }
    None
}

/// The agent's own prompt for this run: the spec's when it carries one, else whatever the engine's
/// stored arguments already said. The spec's copy is the user's prompt *unmodified* — nothing above
/// this layer writes into it any more — so it wins where both exist.
pub(super) fn own_prompt(spec: &RunSpec, stored: Option<String>) -> Option<String> {
    spec.system_prompt
        .as_deref()
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
        .map(ToString::to_string)
        .or(stored)
}

/// `existing` with the run's location behind it, or `existing` unchanged when there is none.
///
/// Applied before [`with_tool_help`] so the order the old decorator established survives: location
/// first — it is the shorter block, and it governs every path named in the sections around it.
pub(super) fn with_workspace(spec: &RunSpec, existing: Option<String>) -> Option<String> {
    behind(existing, spec.workspace_note.as_deref())
}

/// `existing` with what the run knows behind it, or `existing` unchanged when it knows nothing.
///
/// Between the location and the tool inventory, deliberately: knowing *where* you are governs the
/// paths in every section after it, and knowing *what you know* is worth reading before the list
/// of commands you could run to find out.
pub(super) fn with_knowledge(spec: &RunSpec, existing: Option<String>) -> Option<String> {
    behind(existing, spec.knowledge_note.as_deref())
}

/// `existing` with the run's tool help behind it, or `existing` unchanged when there are no tools.
///
/// Appended, never substituted: whatever the agent was told to be survives, with the inventory
/// after it — instructions are read before equipment.
///
/// A conversation past its first turn carries the section it opened with
/// ([`RunSpec::tool_help`]) and that is used verbatim; only a fresh run renders one.
pub(super) fn with_tool_help(spec: &RunSpec, existing: Option<String>) -> Option<String> {
    let block = spec
        .tool_help
        .clone()
        .or_else(|| tool_help::block(&spec.tools));
    behind(existing, block.as_deref())
}

/// `note` behind `existing`, with a blank line between them — and whichever one exists when only
/// one does.
///
/// The three appenders differ only in which field they read, so the joining rule lives once: a
/// section that started drifting between them (one trimming, another not) would show up as a
/// prompt that changes shape depending on which parts of it are set.
fn behind(existing: Option<String>, note: Option<&str>) -> Option<String> {
    let Some(note) = note.map(str::trim).filter(|n| !n.is_empty()) else {
        return existing;
    };
    match existing.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(prompt) => Some(format!("{prompt}\n\n{note}")),
        None => Some(note.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> RunSpec {
        RunSpec {
            cwd: std::path::PathBuf::from("/tmp"),
            path: String::new(),
            env: Vec::new(),
            arguments: serde_json::Value::Null,
            tools: Vec::new(),
            tool_help: None,
            system_prompt: None,
            workspace_note: None,
            knowledge_note: None,
        }
    }

    /// The order is the contract: instructions, then where you are, then what you know, then what
    /// you can call. A run whose prompt reordered itself between two turns would throw away every
    /// cached token it had, for no reason the model could see.
    #[test]
    fn the_sections_come_in_one_order() {
        let mut spec = spec();
        spec.system_prompt = Some("You review code.".into());
        spec.workspace_note = Some("# Where you are\n\n/tmp".into());
        spec.knowledge_note = Some("# What you know\n\nnothing".into());
        spec.tool_help = Some("# Your tools\n\nBash".into());

        assert_eq!(
            compose(&spec, None).expect("a prompt"),
            "You review code.\n\n# Where you are\n\n/tmp\n\n# What you know\n\nnothing\n\n\
             # Your tools\n\nBash",
        );
    }

    /// Nothing to say is `None`, not an empty prompt — an engine handed `Some("")` sends a system
    /// message with no content in it, which is not the same as sending none.
    #[test]
    fn a_run_with_nothing_to_say_composes_nothing() {
        assert_eq!(compose(&spec(), None), None);

        let mut blank = spec();
        blank.system_prompt = Some("   ".into());
        assert_eq!(compose(&blank, None), None);
    }

    /// The stored prompt is the fallback, never an addition: an agent whose prompt was edited must
    /// not run under the old one and the new one stacked together.
    #[test]
    fn the_specs_own_prompt_replaces_the_stored_one() {
        let mut spec = spec();
        spec.system_prompt = Some("new".into());
        assert_eq!(compose(&spec, Some("old".into())).as_deref(), Some("new"));

        spec.system_prompt = None;
        assert_eq!(compose(&spec, Some("old".into())).as_deref(), Some("old"));
    }

    /// A missing section leaves no gap behind it. Blank lines are cheap but they are not free: two
    /// runs of the same agent, one with a memory and one without, must not differ by a stray break
    /// that invalidates the cache for every token after it.
    #[test]
    fn a_section_that_has_nothing_to_say_leaves_no_hole() {
        let mut spec = spec();
        spec.system_prompt = Some("You review code.".into());
        spec.knowledge_note = Some("   ".into());
        spec.tool_help = Some("# Your tools\n\nBash".into());

        assert_eq!(
            compose(&spec, None).expect("a prompt"),
            "You review code.\n\n# Your tools\n\nBash",
        );
    }

    /// The cuts are exact: joined back together the sections are the prompt again. A reader
    /// shown "instructions: 1–240" against a split that lost a newline is being shown the wrong
    /// boundary, and the whole reason to draw one is that it can be trusted.
    #[test]
    fn the_sections_are_the_prompt_again() {
        let mut spec = spec();
        spec.system_prompt = Some("You review code.".into());
        spec.workspace_note = Some("# Where you are\n\n/tmp".into());
        spec.knowledge_note = Some("# What you know\n\nnothing".into());
        spec.tool_help = Some("# Your tools\n\nBash".into());
        let prompt = compose(&spec, None).expect("a prompt");

        let cut = sections(&prompt);
        assert_eq!(
            cut.iter().map(|s| s.label).collect::<Vec<_>>(),
            [
                "instructions",
                "where you are",
                "what you know",
                "your tools"
            ],
        );
        assert_eq!(cut.iter().map(|s| s.text).collect::<String>(), prompt);
        assert!(cut[0].text.starts_with("You review code."));
    }

    /// A section that was never appended is not reported as an empty one, and a prompt with none of
    /// them is a single stretch of instructions.
    #[test]
    fn a_prompt_reports_only_the_sections_it_has() {
        let mut spec = spec();
        spec.system_prompt = Some("Just words.".into());
        let bare = compose(&spec, None).expect("a prompt");
        let cut = sections(&bare);
        assert_eq!(cut.len(), 1);
        assert_eq!(cut[0].label, "instructions");

        spec.tool_help = Some("# Your tools\n\nBash".into());
        let with_tools = compose(&spec, None).expect("a prompt");
        assert_eq!(
            sections(&with_tools)
                .iter()
                .map(|s| s.label)
                .collect::<Vec<_>>(),
            ["instructions", "your tools"],
        );
    }

    /// A heading an agent merely *writes about* is prose, not a seam. Splitting there would cut an
    /// agent's own instructions in half and label the back of them as machinery.
    #[test]
    fn a_heading_quoted_in_prose_is_not_a_cut() {
        let mut spec = spec();
        spec.system_prompt =
            Some("Never write a section called `# Your tools` in your reports.".into());
        let prompt = compose(&spec, None).expect("a prompt");
        let cut = sections(&prompt);
        assert_eq!(cut.len(), 1, "one section, not two: {cut:?}");
        assert_eq!(cut[0].label, "instructions");
    }

    /// The frozen section wins over re-rendering. `tool_help` exists precisely so a conversation's
    /// prompt does not change under it between turns; deriving it fresh when one is already
    /// recorded would undo that in the one place it is enforced.
    #[test]
    fn a_conversation_keeps_the_tool_section_it_opened_with() {
        let mut spec = spec();
        spec.tool_help = Some("# Your tools\n\nwhat this conversation opened with".into());
        spec.tools = vec![adi_tools::ToolHelp {
            name: "Something".into(),
            description: Some("else entirely".into()),
            help: None,
        }];

        let composed = compose(&spec, None).expect("a prompt");
        assert!(composed.contains("what this conversation opened with"));
        assert!(!composed.contains("else entirely"));
    }
}
