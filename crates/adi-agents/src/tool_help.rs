//! Telling an agent about the tools it has.
//!
//! An agent's enabled tools are materialized as shims in its own `.bin`, prepended to the run's
//! `PATH` — so the commands are *there*, but nothing said they were. An agent had to already know
//! the names, or discover them by listing a directory it was never told about. So at launch each
//! tool is asked to describe itself (`llm help`, else `help`; see [`adi_tools::ToolHelp`]) and the
//! answers are folded into the agent's system prompt.
//!
//! Two properties this leans on. It is **appended, never substituted**: whatever the agent's own
//! prompt says survives, with the tool section behind it. And it is **derived at launch, not
//! stored**: the manifest keeps only the user's prompt, so enabling a tool, editing its help, or
//! upgrading the CLI underneath it shows up on the next run without anyone rewriting a prompt.

use std::fmt::Write as _;

use adi_tools::ToolHelp;

use crate::agent::RawAgentArguments;
use crate::backend::Backend;

/// The key every backend's arguments spell its system prompt under.
const SYSTEM_PROMPT: &str = "system_prompt";

/// The most the whole tool section may add to a prompt. A tool's own help is already capped; this
/// caps their sum, so a fleet of well-documented tools can't bury the agent's instructions.
const MAX_BLOCK_CHARS: usize = 20_000;

const HEADING: &str = "# Your tools\n\nThese commands are on your PATH — run them from your shell. \
Each one's own help follows, so you can use it without guessing at its arguments.";

/// Whether this backend's `system_prompt` argument is really a *system* prompt — the only place
/// tool help belongs.
///
/// The Claude backends fold it into `--append-system-prompt`, and the adi loop sends it as the
/// conversation's system message: in both, help arrives as context. Codex has no such flag — its
/// `system_prompt` is pushed as the opening *user* prompt, so help would arrive as a question to
/// answer rather than as background, and an agent with no prompt of its own would open its session
/// by reading a wall of usage text. Those are left alone.
pub(crate) fn applies_to(backend: &Backend) -> bool {
    matches!(
        backend,
        Backend::PtyClaude | Backend::ProcessClaude | Backend::HarnessClaudeSdk | Backend::HarnessAdi
    )
}

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

/// Append `block` to the arguments' system prompt, in place. The agent's own prompt leads and the
/// tool section follows, so instructions are read before inventory. A backend that had no prompt
/// gets the section alone.
pub(crate) fn fold_into(arguments: &mut RawAgentArguments, block: &str) {
    let existing = arguments
        .get(SYSTEM_PROMPT)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim();
    let merged = if existing.is_empty() {
        block.to_string()
    } else {
        format!("{existing}\n\n{block}")
    };
    arguments.insert(SYSTEM_PROMPT.to_string(), serde_json::Value::String(merged));
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
    fn only_backends_whose_prompt_is_a_system_prompt_take_it() {
        for backend in [
            Backend::PtyClaude,
            Backend::ProcessClaude,
            Backend::HarnessClaudeSdk,
            Backend::HarnessAdi,
        ] {
            assert!(applies_to(&backend), "{backend} should take tool help");
        }
        // Codex puts `system_prompt` in the opening user turn; help there would be a question.
        for backend in [
            Backend::PtyCodex,
            Backend::ProcessCodex,
            Backend::Wasm,
            Backend::from("cloud:worker"),
        ] {
            assert!(!applies_to(&backend), "{backend} should not take tool help");
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
        // Nothing to quote, so no empty fence is left behind.
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

    #[test]
    fn the_agents_own_prompt_leads_and_survives() {
        let mut arguments = RawAgentArguments::new();
        arguments.insert(
            SYSTEM_PROMPT.to_string(),
            serde_json::Value::String("  You are a careful operator.  ".into()),
        );
        fold_into(&mut arguments, "# Your tools\n\n## adi-db");

        let merged = arguments[SYSTEM_PROMPT].as_str().expect("a string");
        assert!(merged.starts_with("You are a careful operator."));
        assert!(merged.ends_with("## adi-db"));
        // Other arguments are untouched, and the prompt is one value, not two.
        assert_eq!(arguments.len(), 1);
    }

    #[test]
    fn an_agent_with_no_prompt_gets_the_section_alone() {
        let mut arguments = RawAgentArguments::new();
        arguments.insert("model".to_string(), serde_json::Value::String("opus".into()));
        fold_into(&mut arguments, "# Your tools");
        assert_eq!(arguments[SYSTEM_PROMPT].as_str(), Some("# Your tools"));
        assert_eq!(arguments["model"].as_str(), Some("opus"));
    }
}
