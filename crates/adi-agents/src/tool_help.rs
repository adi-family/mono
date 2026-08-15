//! Telling an agent about the tools it has.
//!
//! An agent's enabled tools are materialized as shims in its own `.bin`, prepended to the run's
//! `PATH` — so the commands are *there*, but nothing said they were. An agent had to already know
//! the names, or discover them by listing a directory it was never told about. So at launch each
//! tool is asked to describe itself (`llm help`, else `help`; see [`adi_tools::ToolHelp`]) and the
//! answers are rendered into the prompt section this module builds.
//!
//! **Rendering only.** Where the section goes — and whether it goes anywhere at all — is a fact
//! about the engine, so the runner decides it. This module used to also answer "which backends take
//! it" and edit the stored arguments in place; both moved down, because the answer was never about
//! tools. Codex is the case that makes it concrete: it has no append-system-prompt flag, so its
//! `system_prompt` is pushed as the opening *user* turn, where help arrives as a wall of usage text
//! to answer rather than as background. Its runners hand it neither this nor the location block.
//!
//! Two properties the section leans on. It is **appended, never substituted**: whatever the agent's
//! own prompt says survives, with the tool section behind it. And it is **derived at launch, not
//! stored**: the manifest keeps only the user's prompt, so enabling a tool, editing its help, or
//! upgrading the CLI underneath it shows up on the next run without anyone rewriting a prompt.

use std::fmt::Write as _;

use adi_tools::ToolHelp;

/// The most the whole tool section may add to a prompt. A tool's own help is already capped; this
/// caps their sum, so a fleet of well-documented tools can't bury the agent's instructions.
const MAX_BLOCK_CHARS: usize = 20_000;

const HEADING: &str = "# Your tools\n\nThese commands are on your PATH — run them from your shell. \
Each one's own help follows, so you can use it without guessing at its arguments.";

/// The prompt section describing `tools`, or `None` when there is nothing to say. A tool with no
/// help is still listed — that the command exists, under that name, is the half worth having.
pub(crate) fn block(tools: &[ToolHelp]) -> Option<String> {
    if tools.is_empty() {
        return None;
    }
    let mut out = String::from(HEADING);
    let mut left_out = 0;
    for tool in tools {
        let section = section(tool);
        // Once the budget is spent, keep counting rather than packing in whatever still fits —
        // a section is a unit, and a prompt that trails off mid-tool reads as a bug.
        if left_out > 0 || out.len() + section.len() > MAX_BLOCK_CHARS {
            left_out += 1;
            continue;
        }
        out.push_str(&section);
    }
    if left_out > 0 {
        let plural = if left_out == 1 { "" } else { "s" };
        let _ = write!(
            out,
            "\n\n({left_out} more tool{plural} on your PATH — run one with `help` to see its usage.)"
        );
    }
    Some(out)
}

/// One tool's section: the name it is run by, its one-line description, and its own help.
fn section(tool: &ToolHelp) -> String {
    let mut out = format!("\n\n## {}", tool.name);
    if let Some(description) = tool
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        let _ = write!(out, " — {description}");
    }
    if let Some(help) = tool.help.as_deref().map(str::trim).filter(|h| !h.is_empty()) {
        let _ = write!(out, "\n\n```\n{help}\n```");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn help(name: &str, description: Option<&str>, help: Option<&str>) -> ToolHelp {
        ToolHelp {
            name: name.to_string(),
            description: description.map(ToString::to_string),
            help: help.map(ToString::to_string),
        }
    }

    #[test]
    fn a_block_names_each_tool_and_carries_its_help() {
        let block = block(&[
            help("adi-tasks", Some("Work the task tree."), Some("Usage: adi-tasks <CMD>")),
            help("adi-db", None, Some("Usage: adi-db query <SQL>")),
        ])
        .expect("a block");
        assert!(block.starts_with("# Your tools"));
        assert!(block.contains("## adi-tasks — Work the task tree."));
        assert!(block.contains("Usage: adi-tasks <CMD>"));
        assert!(block.contains("## adi-db"));
        assert!(block.contains("Usage: adi-db query <SQL>"));
    }

    #[test]
    fn a_tool_with_nothing_to_say_is_still_named() {
        let block = block(&[help("mystery", None, None)]).expect("a block");
        assert!(block.contains("## mystery"), "{block}");
        assert!(!block.contains("```"), "{block}");
    }

    #[test]
    fn no_tools_means_no_block() {
        assert_eq!(block(&[]), None);
    }

    #[test]
    fn an_oversized_fleet_is_capped_and_says_how_many_it_left_out() {
        let long = "x".repeat(2_000);
        let tools: Vec<ToolHelp> = (0..40)
            .map(|i| help(&format!("tool-{i}"), None, Some(&long)))
            .collect();
        let block = block(&tools).expect("a block");
        assert!(block.len() < MAX_BLOCK_CHARS + 2_500, "{} chars", block.len());
        assert!(block.contains("more tools on your PATH"), "{block}");
    }

}
