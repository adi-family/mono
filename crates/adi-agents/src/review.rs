//! What a conversation is evidence of, written for another agent to read.
//!
//! [`analytics`](crate::analytics) answers what a conversation *cost*. This module asks the other
//! question — what it *did*, and what should be different next time — and writes the answer as a
//! document some other agent is pointed at. Nothing here draws a conclusion: a dossier is the
//! evidence, and the reviewer is the one that reads it.
//!
//! # Why a file, and not the message
//!
//! The reviewer is started like any other run, so the obvious thing is to paste the whole dossier
//! into its opening message. Two reasons not to. The message is also the conversation's *title* and
//! its first bubble, so a twenty-thousand-character one makes the chat unreadable before it starts.
//! And a message is fixed at launch, while a file can be re-read: the reviewer that wants turn 30's
//! arguments again asks for those lines instead of scrolling its own context.
//!
//! So the launch [`brief`] carries the headline — the counts that decide whether there is anything
//! to say at all — and the path to everything else.
//!
//! # The size of it
//!
//! [`Options::budget`] defaults just under the 32 KB an `harness:adi` `Read` returns before it
//! truncates. That ceiling is the point: a dossier that fits is read in one call, and one that
//! doesn't turns a review into a treasure hunt through `offset`/`limit`. Sections are written
//! shortest-first and the trace — the only one that grows without limit — gives up its middle,
//! saying how many turns it dropped. A review that silently described half a conversation would be
//! worse than one that admits it.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::agent::StoredAgent;
use crate::analytics::{self, Shape, Source, TokenReport};
use crate::progress::{Step, ToolStatus};
use crate::store::{SessionRecord, Turn};

/// How much of a conversation to describe, and how much of the agent's past to fold in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// The agent's most recent sessions to count into the cross-session tallies, newest first. Each
    /// costs one indexed read of its turns, so this is the knob between "what this agent is like"
    /// and how long the button takes.
    pub history_sessions: usize,
    /// The longest the document may get, in bytes. See the module note on why it is what it is.
    pub budget: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            history_sessions: 20,
            budget: 28_000,
        }
    }
}

/// A written dossier, and the message that points a reviewer at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    /// Where the document was written — absolute, because the reviewing agent runs in its own
    /// directory and generally not this one's.
    pub path: PathBuf,
    /// What to launch the reviewer with: the headline counts, the path, and the question.
    pub brief: String,
}

/// Everything a dossier is written from, gathered by the caller.
///
/// This module does no I/O and holds no store: it is given the evidence and formats it, which is
/// what makes the shape of the document testable without a session on disk.
#[derive(Debug)]
pub struct Evidence<'a> {
    pub agent: &'a StoredAgent,
    pub run_id: &'a str,
    /// The session's own record — where it ran, what it was opened with, when.
    pub record: &'a SessionRecord,
    /// The conversation, oldest first, exactly as a reader sees it.
    pub turns: &'a [Turn],
    pub report: &'a TokenReport,
    /// What this agent looks like across its recent sessions.
    pub history: &'a History,
    /// The adi tools on this agent's PATH: name and one-line description.
    pub tools_on: &'a [(String, String)],
    /// Tools in the store this agent has *not* been given — the shortest path from "it did that by
    /// hand" to "it could have run one command".
    pub tools_off: &'a [(String, String)],
}

/// What an agent looks like beyond the one conversation: which of its tools actually work, what it
/// is refused, and how often it is asked for anything at all.
///
/// Folded session by session so no caller has to hold every transcript at once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    /// Sessions actually counted, and how many the agent has in total.
    pub sessions_seen: usize,
    pub sessions_total: usize,
    /// The span the counted sessions cover, in whole days.
    pub window_days: u64,
    pub turns: usize,
    pub errored_turns: usize,
    /// Per tool: calls, then how many of them failed. Most-called first.
    pub by_tool: Vec<(String, usize, usize)>,
    /// Tools refused by permission, and how often. Worst first.
    pub blocked: Vec<(String, usize)>,
    /// The oldest and newest session start folded in, as unix milliseconds.
    oldest: u64,
    newest: u64,
}

impl History {
    /// Count one of the agent's sessions in. Call newest-first; ordering only affects
    /// [`window_days`](Self::window_days), which takes the span of everything folded.
    pub fn fold(&mut self, record: &SessionRecord, turns: &[Turn]) {
        self.sessions_seen += 1;
        if record.started_at > 0 {
            self.oldest = if self.oldest == 0 {
                record.started_at
            } else {
                self.oldest.min(record.started_at)
            };
            self.newest = self.newest.max(record.started_at);
        }
        self.window_days = (self.newest.saturating_sub(self.oldest)) / 86_400_000;

        for turn in turns {
            self.turns += 1;
            if let Some(m) = &turn.metrics {
                if m.is_error {
                    self.errored_turns += 1;
                }
                for name in &m.permission_denials {
                    bump(&mut self.blocked, name);
                }
            }
            for step in &turn.steps {
                if let Step::Tool { name, status, .. } = step {
                    match self.by_tool.iter_mut().find(|(n, _, _)| n == name) {
                        Some((_, calls, bad)) => {
                            *calls += 1;
                            *bad += usize::from(*status == ToolStatus::Error);
                        }
                        None => self.by_tool.push((
                            name.clone(),
                            1,
                            usize::from(*status == ToolStatus::Error),
                        )),
                    }
                }
            }
        }
    }

    /// Order the tallies for reading — most-used and worst-blocked first. Called once, after the
    /// last [`fold`](Self::fold).
    pub fn settle(&mut self) {
        self.by_tool
            .sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        self.blocked
            .sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    }
}

/// What one conversation adds up to. The counts the brief leads with, so the reviewer knows what it
/// is looking at before it has read anything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Totals {
    you: usize,
    agent: usize,
    queued: usize,
    tools: usize,
    failed: usize,
    thinking: usize,
    errored: usize,
    blocked: usize,
    tokens: u64,
    cost_micro: u64,
    work_ms: u64,
    first_at: u64,
    last_at: u64,
}

fn totals(turns: &[Turn]) -> Totals {
    let mut t = Totals::default();
    for turn in turns {
        if turn.queued {
            t.queued += 1;
            continue;
        }
        if turn.role == "user" {
            t.you += 1;
        } else {
            t.agent += 1;
        }
        if turn.at > 0 {
            if t.first_at == 0 {
                t.first_at = turn.at;
            }
            t.last_at = turn.at;
        }
        if let Some(m) = &turn.metrics {
            t.tokens += m.input_tokens.unwrap_or(0) + m.output_tokens.unwrap_or(0);
            t.cost_micro += m.cost_micro_usd.unwrap_or(0);
            t.work_ms += m.duration_ms.unwrap_or(0);
            t.errored += usize::from(m.is_error);
            t.blocked += m.permission_denials.len();
        }
        for step in &turn.steps {
            match step {
                Step::Thinking { .. } => t.thinking += 1,
                Step::Tool { status, .. } => {
                    t.tools += 1;
                    t.failed += usize::from(*status == ToolStatus::Error);
                }
                Step::Message { .. } => {}
            }
        }
    }
    t
}

/// The message the reviewing agent is launched with.
///
/// Short on purpose — it is the new conversation's title and its first bubble. It leads with the
/// counts rather than the path so that a reviewer which somehow never opens the file still answers
/// about *this* conversation, and it ends with the question, which is the one thing the dossier
/// deliberately does not contain.
#[must_use]
pub fn brief(evidence: &Evidence<'_>, path: &Path) -> String {
    let t = totals(evidence.turns);
    let r = evidence.report;
    let mut out = String::new();

    let _ = writeln!(
        out,
        "Review one of my agent conversations and tell me how to run this workflow better.\n"
    );
    let _ = writeln!(
        out,
        "Under review: agent `{}`, session `{}`, on `{}`.",
        evidence.agent.name, evidence.run_id, evidence.agent.manifest.backend
    );
    let _ = writeln!(out, "Opened with: {}", quote_line(&evidence.record.message, 160));

    let mut counts = vec![format!(
        "{} ({} you, {} it)",
        plural(t.you + t.agent, "turn", "turns"),
        t.you,
        t.agent
    )];
    counts.push(plural(t.tools, "tool call", "tool calls"));
    if t.failed > 0 {
        counts.push(format!("{} of them failed", t.failed));
    }
    if t.blocked > 0 {
        counts.push(format!("{} blocked by permission", t.blocked));
    }
    if t.errored > 0 {
        counts.push(format!("{} turns the engine gave up on", t.errored));
    }
    let _ = writeln!(out, "It ran {}.", counts.join(", "));

    if r.total > 0 {
        let share = r.wasted * 100 / r.total;
        let _ = writeln!(
            out,
            "Its context is ~{} tokens, of which ~{} ({share}%) is text that was sent more than once.",
            r.total, r.wasted
        );
    }

    let _ = writeln!(
        out,
        "\nThe evidence — its configuration and system prompt, the tool-by-tool trace, what failed, \
         what repeated, and what this agent looks like across its other sessions — is in:\n\n    {}\n",
        path.display()
    );

    let _ = writeln!(
        out,
        "Read that file first, then answer in four parts:\n\
         \n\
         1. **Workflow** — what this conversation did the long way, and what would make it one step. \
         Cite the turns you mean.\n\
         2. **Hardening** — every failure, block, and dead end in the trace, and the durable fix for \
         each: a line in the system prompt, a permission, a tool argument, a guard.\n\
         3. **Tools** — hand-work that repeated and deserves a tool of its own. Name it, say what it \
         would take as arguments, and give the `adi-tools add` that would create it. Say if an \
         existing tool it wasn't given already covers it.\n\
         4. **Context** — what got sent twice, and what would stop it.\n\
         \n\
         Rank by what it would actually save. Skip a part that has nothing real in it rather than \
         filling it — a review that finds three things worth doing beats one that finds twelve.\n\
         \n\
         **Propose only. Change nothing yet** — no edits to the agent, its prompt, or the tool store \
         until I say so. End with the shortest list of commands that would apply what you recommend, \
         and I'll pick from it."
    );

    out
}

/// Write the dossier itself.
///
/// Ordered by how much a reader needs it: what the agent *is* before what it did, because a trace
/// read without the system prompt invites recommendations the prompt already makes.
#[must_use]
pub fn document(evidence: &Evidence<'_>, opts: Options) -> String {
    let t = totals(evidence.turns);
    let mut out = String::new();

    let _ = writeln!(
        out,
        "# Workflow review — `{}` / session `{}`\n",
        evidence.agent.name, evidence.run_id
    );
    let _ = writeln!(
        out,
        "The evidence for one conversation of one agent. Nothing here is a conclusion; the question \
         is in the message that pointed you at this file.\n"
    );

    configuration(&mut out, evidence);
    conversation(&mut out, &t);
    failures(&mut out, evidence);
    context_waste(&mut out, evidence);
    across_sessions(&mut out, evidence);
    unused_tools(&mut out, evidence);

    // The trace last and sized to what is left: every other section has a natural end, and this one
    // does not. Written into its own buffer so the elision is decided against the real remainder.
    let room = opts.budget.saturating_sub(out.len());
    out.push_str(&trace(evidence.turns, room));
    out
}

/// Section 1 — the agent as configured. What a recommendation has to be compatible with.
fn configuration(out: &mut String, e: &Evidence<'_>) {
    let m = &e.agent.manifest;
    let _ = writeln!(out, "## 1. The agent as configured\n");
    let _ = writeln!(out, "- runtime: `{}`", m.backend);
    let _ = writeln!(out, "- working directory: `{}`", e.record.cwd.display());
    if let Some(project) = &m.project {
        let _ = writeln!(out, "- project: `{project}`");
    }
    if !m.tags.is_empty() {
        let _ = writeln!(out, "- tags: {}", m.tags.join(", "));
    }

    // Arguments minus the system prompt, which gets its own block below: inlined here it would be a
    // wall of text in a bullet list, and it is the one setting most worth reading verbatim.
    let args: Vec<String> = m
        .arguments
        .iter()
        .filter(|(k, _)| k.as_str() != "system_prompt")
        .map(|(k, v)| format!("`{k}` = {}", clip(&v.to_string(), 120)))
        .collect();
    if !args.is_empty() {
        let _ = writeln!(out, "- engine arguments: {}", args.join(", "));
    }

    // Names only, never values: a dossier is read by another agent and lands in its context, and a
    // secret that travelled there would have been leaked by the act of asking for a review.
    if !m.secrets.is_empty() {
        let names: Vec<&str> = m.secrets.iter().map(|s| s.name.as_str()).collect();
        let _ = writeln!(out, "- secrets injected (names only): {}", names.join(", "));
    }
    if !m.env.is_empty() {
        let keys: Vec<&str> = m.env.keys().map(String::as_str).collect();
        let _ = writeln!(out, "- extra env (names only): {}", keys.join(", "));
    }
    if !m.path.is_empty() {
        let _ = writeln!(out, "- extra PATH: {}", m.path.join(", "));
    }

    if e.tools_on.is_empty() {
        let _ = writeln!(out, "- adi tools on its PATH: none");
    } else {
        let _ = writeln!(out, "\n**adi tools on its PATH** ({}):\n", e.tools_on.len());
        for (name, desc) in e.tools_on {
            let _ = writeln!(out, "- `{name}` — {}", clip(desc, 120));
        }
    }

    match e.agent.manifest.system_prompt() {
        Some(prompt) => {
            let prompt = clip(&prompt, 4_000);
            let _ = writeln!(out, "\n**Its system prompt**, verbatim:\n");
            let fence = fence_for(&prompt);
            let _ = writeln!(out, "{fence}\n{prompt}\n{fence}\n");
        }
        None => {
            let _ = writeln!(
                out,
                "\n**It has no system prompt.** Everything it knows about this environment, it was \
                 told in the conversation below or worked out for itself.\n"
            );
        }
    }
}

/// Section 2 — the shape of the conversation, before the turn-by-turn of it.
fn conversation(out: &mut String, t: &Totals) {
    let _ = writeln!(out, "## 2. The conversation in numbers\n");
    let _ = writeln!(
        out,
        "- {}: {} from the human, {} from the agent{}",
        plural(t.you + t.agent, "turn", "turns"),
        t.you,
        t.agent,
        if t.queued > 0 {
            format!(", and {} still queued (typed, not yet asked)", t.queued)
        } else {
            String::new()
        }
    );
    let _ = writeln!(
        out,
        "- {}, {} failed, {} thinking blocks",
        plural(t.tools, "tool call", "tool calls"),
        t.failed,
        t.thinking
    );
    if t.tokens > 0 || t.cost_micro > 0 {
        let _ = writeln!(
            out,
            "- engine telemetry: {} tokens, ${:.2}, {} spent answering",
            t.tokens,
            t.cost_micro as f64 / 1_000_000.0,
            duration(t.work_ms)
        );
    }
    if t.last_at > t.first_at {
        let _ = writeln!(out, "- wall clock: {}", duration(t.last_at - t.first_at));
    }
    out.push('\n');
}

/// Section 3 — everything that went wrong, gathered off the trace so a reader gets it in one place
/// rather than by scanning for it. The trace below shows each in its context.
fn failures(out: &mut String, e: &Evidence<'_>) {
    let mut failed: Vec<(usize, &str, &str, &str)> = Vec::new();
    let mut errored: Vec<usize> = Vec::new();
    let mut blocked: Vec<(String, usize)> = Vec::new();

    for (n, turn) in e.turns.iter().enumerate() {
        if let Some(m) = &turn.metrics {
            if m.is_error {
                errored.push(n + 1);
            }
            for name in &m.permission_denials {
                bump(&mut blocked, name);
            }
        }
        for step in &turn.steps {
            if let Step::Tool {
                name,
                input,
                status: ToolStatus::Error,
                output,
            } = step
            {
                failed.push((n + 1, name, input, output));
            }
        }
    }

    if failed.is_empty() && errored.is_empty() && blocked.is_empty() {
        let _ = writeln!(
            out,
            "## 3. What went wrong\n\nNothing failed, nothing was blocked, and no turn errored.\n"
        );
        return;
    }

    let _ = writeln!(out, "## 3. What went wrong\n");
    if !failed.is_empty() {
        let _ = writeln!(out, "**Tool calls that failed** ({}):\n", failed.len());
        for (turn, name, input, output) in failed.iter().take(20) {
            let _ = writeln!(
                out,
                "- turn {turn} · `{name}` {}\n  - it said: {}",
                clip(input, 140),
                clip(&one_line(output), 200)
            );
        }
        if failed.len() > 20 {
            let _ = writeln!(out, "- … and {} more.", failed.len() - 20);
        }
        out.push('\n');
    }
    if !blocked.is_empty() {
        blocked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let named: Vec<String> = blocked
            .iter()
            .map(|(name, n)| format!("`{name}` ×{n}"))
            .collect();
        let _ = writeln!(
            out,
            "**Blocked by permission**: {}. Each one is work the agent wanted to do and was refused \
             mid-task.\n",
            named.join(", ")
        );
    }
    if !errored.is_empty() {
        let turns: Vec<String> = errored.iter().map(usize::to_string).collect();
        let _ = writeln!(
            out,
            "**Turns the engine gave up on**: {}. Distinct from a failed tool call — the agent never \
             finished these at all.\n",
            turns.join(", ")
        );
    }
}

/// Section 4 — the itemization, folded down to the findings worth a sentence each.
fn context_waste(out: &mut String, e: &Evidence<'_>) {
    let r = e.report;
    if r.total == 0 {
        return;
    }
    let _ = writeln!(out, "## 4. What the context went on\n");
    let split: Vec<String> = r
        .by_source
        .iter()
        .filter(|(_, n)| *n * 100 / r.total.max(1) >= 1)
        .map(|(s, n)| format!("{} {}%", source(*s), n * 100 / r.total.max(1)))
        .collect();
    let _ = writeln!(
        out,
        "~{} tokens (estimated with `{}`, not the provider's own count){}.",
        r.total,
        r.encoding,
        if split.is_empty() {
            String::new()
        } else {
            format!(" — {}", split.join(", "))
        }
    );
    if r.truncated {
        let _ = writeln!(
            out,
            "\nThe conversation was too long to itemize whole; only its recent end was read."
        );
    }

    if r.repeats.is_empty() && r.near_duplicates.is_empty() {
        let _ = writeln!(out, "\nNothing was sent twice.\n");
        return;
    }

    if !r.repeats.is_empty() {
        let share = r.wasted * 100 / r.total.max(1);
        let _ = writeln!(
            out,
            "\n**Sent more than once** — ~{} tokens ({share}% of the whole):\n",
            r.wasted
        );
        for rep in r.repeats.iter().take(8) {
            let hint = rep.shape.hint().map(|h| format!(" — {h}")).unwrap_or_default();
            let _ = writeln!(
                out,
                "- ×{} · ~{} tokens wasted · {}{hint}\n  - `{}`\n  - sent from: {}",
                rep.count,
                rep.wasted,
                shape(rep.shape),
                clip(&one_line(&rep.preview), 160),
                sites(&rep.sites)
            );
        }
        if r.repeats.len() > 8 {
            let _ = writeln!(out, "- … and {} smaller ones.", r.repeats.len() - 8);
        }
    }

    if !r.near_duplicates.is_empty() {
        let _ = writeln!(
            out,
            "\n**Nearly the same thing, several times** — usually one file read, edited, and read \
             again:\n"
        );
        for g in r.near_duplicates.iter().take(4) {
            let _ = writeln!(
                out,
                "- {} versions · ~{} tokens each · ~{} wasted\n  - `{}`\n  - at: {}",
                g.count,
                g.tokens,
                g.wasted,
                clip(&one_line(&g.preview), 140),
                sites(&g.sites)
            );
        }
    }
    out.push('\n');
}

/// Section 5 — the same agent's other sessions, which is where a tool that fails half the time
/// becomes visible. One conversation cannot tell a bad run from a bad tool.
fn across_sessions(out: &mut String, e: &Evidence<'_>) {
    let h = e.history;
    if h.sessions_seen == 0 {
        return;
    }
    let _ = writeln!(out, "## 5. This agent beyond this conversation\n");
    let _ = writeln!(
        out,
        "Counted over its most recent {}{} — {} turns in all{}.",
        plural(h.sessions_seen, "session", "sessions"),
        if h.sessions_total > h.sessions_seen {
            format!(" (of {})", h.sessions_total)
        } else {
            String::new()
        },
        h.turns,
        if h.window_days > 0 {
            format!(", spanning {} days", h.window_days)
        } else {
            String::new()
        }
    );
    if h.errored_turns > 0 {
        let _ = writeln!(
            out,
            "{} of those turns errored ({}%).",
            h.errored_turns,
            h.errored_turns * 100 / h.turns.max(1)
        );
    }

    if !h.by_tool.is_empty() {
        let _ = writeln!(out, "\n**Which tools it actually uses, and which ones work**:\n");
        for (name, calls, bad) in h.by_tool.iter().take(12) {
            let rate = if *bad > 0 {
                format!(" · {} failed ({}%)", bad, bad * 100 / calls.max(&1))
            } else {
                String::new()
            };
            let _ = writeln!(out, "- `{name}` — {}{rate}", plural(*calls, "call", "calls"));
        }
    }
    if !h.blocked.is_empty() {
        let named: Vec<String> = h
            .blocked
            .iter()
            .take(8)
            .map(|(name, n)| format!("`{name}` ×{n}"))
            .collect();
        let _ = writeln!(
            out,
            "\n**Refused by permission across those sessions**: {}.",
            named.join(", ")
        );
    }
    out.push('\n');
}

/// Section 6 — what the store has that this agent was never given. The cheapest recommendation
/// there is, and one a reviewer cannot make without being told the catalog.
fn unused_tools(out: &mut String, e: &Evidence<'_>) {
    if e.tools_off.is_empty() {
        return;
    }
    let _ = writeln!(out, "## 6. In the tool store, but not enabled here\n");
    for (name, desc) in e.tools_off.iter().take(30) {
        let _ = writeln!(out, "- `{name}` — {}", clip(desc, 110));
    }
    if e.tools_off.len() > 30 {
        let _ = writeln!(out, "- … and {} more.", e.tools_off.len() - 30);
    }
    out.push('\n');
}

/// Section 7 — the trace: every turn, one line per tool call.
///
/// Tool *output* is left out except where a call failed. It is what dominates a transcript by
/// weight, and it is the least of what a workflow review needs: the question is which calls were
/// made, in what order, and which of them came back wrong.
fn trace(turns: &[Turn], budget: usize) -> String {
    let head = "## 7. What it actually did, turn by turn\n\nOne line per tool call. Outputs are \
                omitted except where a call failed — the order and the arguments are the workflow.\n\n";
    let mut blocks: Vec<String> = turns
        .iter()
        .enumerate()
        .filter(|(_, t)| !t.queued)
        .map(|(n, t)| turn_block(n + 1, t))
        .collect();

    // Give up the middle, not the tail. The opening is the ask and the end is what it settled on;
    // the grind between them is the part a reader can lose most of and still follow the shape.
    let mut room = budget.saturating_sub(head.len() + 120);
    let mut dropped = 0usize;
    let mut total: usize = blocks.iter().map(String::len).sum();
    while total > room && blocks.len() > 4 {
        let middle = blocks.len() / 2;
        total -= blocks[middle].len();
        blocks.remove(middle);
        dropped += 1;
        if dropped == 1 {
            // The marker is written once and costs room of its own.
            room = room.saturating_sub(80);
        }
    }

    let mut out = String::from(head);
    for (i, block) in blocks.iter().enumerate() {
        if dropped > 0 && i == blocks.len() / 2 {
            let _ = writeln!(
                out,
                "*[{dropped} turns in the middle omitted to fit — ask for them by turn number if the \
                 shape here doesn't explain itself.]*\n"
            );
        }
        out.push_str(block);
    }
    out
}

/// One turn of the trace.
fn turn_block(n: usize, turn: &Turn) -> String {
    let mut out = String::new();
    if turn.role == "user" {
        let _ = writeln!(out, "### Turn {n} — human\n");
        let _ = writeln!(out, "{}\n", quote_block(&turn.text, 600));
        return out;
    }

    let meta = turn
        .metrics
        .as_ref()
        .map(|m| {
            let mut bits = Vec::new();
            if let Some(d) = m.duration_ms {
                bits.push(duration(d));
            }
            let tok = m.input_tokens.unwrap_or(0) + m.output_tokens.unwrap_or(0);
            if tok > 0 {
                bits.push(format!("{tok} tok"));
            }
            if m.is_error {
                bits.push("ERRORED".to_string());
            }
            if bits.is_empty() {
                String::new()
            } else {
                format!(" ({})", bits.join(" · "))
            }
        })
        .unwrap_or_default();
    let _ = writeln!(out, "### Turn {n} — agent{meta}\n");

    let mut thinking = 0usize;
    for step in &turn.steps {
        match step {
            Step::Thinking { .. } => thinking += 1,
            Step::Message { .. } => {}
            Step::Tool {
                name,
                input,
                status,
                output,
            } => {
                let tail = match status {
                    ToolStatus::Ok => String::new(),
                    ToolStatus::Running => " → still running".to_string(),
                    ToolStatus::Unanswered => " → *never returned* (the run ended first)".to_string(),
                    ToolStatus::Error => format!(" → **failed**: {}", clip(&one_line(output), 160)),
                };
                let _ = writeln!(out, "- `{name}` {}{tail}", clip(&one_line(input), 140));
            }
        }
    }
    if thinking > 0 {
        let _ = writeln!(out, "- *({thinking} thinking blocks)*");
    }
    if !turn.text.trim().is_empty() {
        let _ = writeln!(out, "\n{}", quote_block(&turn.text, 500));
    }
    out.push('\n');
    out
}

/// Where a repeat was sent, said in transcript coordinates a reader can go to.
///
/// Sites that name the same place are folded into one entry with a count. A long turn calling one
/// tool forty times produces forty identical sites, and spelling them out would fill the line with
/// the one word it already said — while hiding the turn numbers that are the actual information.
fn sites(list: &[analytics::Site]) -> String {
    if list.is_empty() {
        return "—".to_string();
    }
    let mut places: Vec<(String, usize)> = Vec::new();
    for site in list {
        let what = if site.tool.is_empty() {
            source(site.source).to_string()
        } else {
            format!("`{}`", site.tool)
        };
        bump(&mut places, &format!("turn {} {what}", site.turn + 1));
    }
    let shown = places.len().min(6);
    let named: Vec<String> = places
        .iter()
        .take(shown)
        .map(|(place, n)| {
            if *n > 1 {
                format!("{place} \u{d7}{n}")
            } else {
                place.clone()
            }
        })
        .collect();
    if places.len() > shown {
        format!("{}, +{} more", named.join(", "), places.len() - shown)
    } else {
        named.join(", ")
    }
}

/// A markdown fence long enough that `body` cannot end it early. A system prompt that contains a
/// code block is ordinary, and a three-backtick fence around one closes on the first line of it —
/// spilling the rest of the prompt into the document as if it were the document's own.
fn fence_for(body: &str) -> String {
    let longest = body
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    "`".repeat(longest.max(2) + 1)
}

/// `1 call` / `2 calls`. A tally that says "1 calls" reads as a bug in the thing being reviewed.
fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {many}")
    }
}

fn source(s: Source) -> &'static str {
    match s {
        Source::User => "the human",
        Source::Agent => "the agent",
        Source::Thinking => "thinking",
        Source::ToolInput => "tool input",
        Source::ToolOutput => "tool output",
    }
}

fn shape(s: Shape) -> &'static str {
    match s {
        Shape::Path => "a filesystem path",
        Shape::Url => "a URL",
        Shape::Literal => "an opaque literal",
        Shape::Block => "a block of text",
        Shape::Phrase => "a line of text",
    }
}

/// Add one to a `(name, count)` tally, appending the name the first time it is seen.
fn bump(list: &mut Vec<(String, usize)>, name: &str) {
    match list.iter_mut().find(|(n, _)| n == name) {
        Some((_, n)) => *n += 1,
        None => list.push((name.to_string(), 1)),
    }
}

/// Cut to `max` bytes on a character boundary, saying so — a silent cut reads as the whole thing,
/// which is how a reviewer ends up recommending a fix for text that isn't there.
fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.len() <= max {
        return text.to_string();
    }
    let mut cut = max;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}… [+{} bytes]", &text[..cut], text.len() - cut)
}

/// Flatten to one line, so a multi-line tool argument stays one bullet.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A short single-line quotation, for a title.
fn quote_line(text: &str, max: usize) -> String {
    format!("“{}”", clip(&one_line(text), max))
}

/// A markdown block quotation, so a message containing its own headings can't restructure the
/// document it is quoted in.
fn quote_block(text: &str, max: usize) -> String {
    clip(text, max)
        .lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn duration(ms: u64) -> String {
    match ms {
        0 => "0s".to_string(),
        ms if ms < 1_000 => format!("{ms}ms"),
        ms if ms < 60_000 => format!("{:.1}s", ms as f64 / 1_000.0),
        ms if ms < 3_600_000 => format!("{}m{}s", ms / 60_000, (ms % 60_000) / 1_000),
        ms => format!("{}h{}m", ms / 3_600_000, (ms % 3_600_000) / 60_000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Backend;
    use crate::progress::TurnMetrics;

    fn agent() -> StoredAgent {
        let mut manifest = crate::agent::StoredAgentManifest {
            backend: Backend::from("harness:adi"),
            ..Default::default()
        };
        manifest.arguments.insert(
            "system_prompt".to_string(),
            serde_json::Value::String("You are careful.".to_string()),
        );
        StoredAgent {
            name: "solver".to_string(),
            manifest,
        }
    }

    fn record() -> SessionRecord {
        SessionRecord {
            id: "0000000000000-0001".to_string(),
            agent: "solver".to_string(),
            backend: Backend::from("harness:adi"),
            cwd: PathBuf::from("/tmp/work"),
            message: "fix the failing test".to_string(),
            started_at: 1_000,
            last_activity: 2_000,
            hidden: false,
            runner_state: None,
            outcome: None,
        }
    }

    fn tool(name: &str, input: &str, status: ToolStatus, output: &str) -> Step {
        Step::Tool {
            name: name.to_string(),
            input: input.to_string(),
            status,
            output: output.to_string(),
        }
    }

    fn conversation() -> Vec<Turn> {
        vec![
            Turn {
                role: "user".to_string(),
                text: "fix the failing test".to_string(),
                at: 1_000,
                pending: false,
                queued: false,
                steps: Vec::new(),
                metrics: None,
            },
            Turn {
                role: "assistant".to_string(),
                text: "Fixed it.".to_string(),
                at: 2_000,
                pending: false,
                queued: false,
                steps: vec![
                    tool("Bash", "cargo test", ToolStatus::Error, "linker not found"),
                    tool("Read", "src/lib.rs", ToolStatus::Ok, "fn main() {}"),
                ],
                metrics: Some(TurnMetrics {
                    input_tokens: Some(100),
                    output_tokens: Some(20),
                    permission_denials: vec!["WebFetch".to_string()],
                    ..Default::default()
                }),
            },
        ]
    }

    fn evidence<'a>(
        agent: &'a StoredAgent,
        record: &'a SessionRecord,
        turns: &'a [Turn],
        report: &'a TokenReport,
        history: &'a History,
        off: &'a [(String, String)],
    ) -> Evidence<'a> {
        Evidence {
            agent,
            run_id: "0000000000000-0001",
            record,
            turns,
            report,
            history,
            tools_on: &[],
            tools_off: off,
        }
    }

    #[test]
    fn the_dossier_names_what_failed_and_what_was_blocked() {
        let (agent, record, turns) = (agent(), record(), conversation());
        let doc = document(
            &evidence(
                &agent,
                &record,
                &turns,
                &TokenReport::default(),
                &History::default(),
                &[],
            ),
            Options::default(),
        );
        assert!(doc.contains("linker not found"), "{doc}");
        assert!(doc.contains("`WebFetch` ×1"), "{doc}");
        assert!(doc.contains("You are careful."), "{doc}");
    }

    /// A tool that came back fine contributes its call to the trace and nothing else: its output is
    /// the bulk of a transcript and the least of what a workflow review needs.
    #[test]
    fn a_successful_tools_output_is_not_in_the_dossier() {
        let (agent, record, turns) = (agent(), record(), conversation());
        let doc = document(
            &evidence(
                &agent,
                &record,
                &turns,
                &TokenReport::default(),
                &History::default(),
                &[],
            ),
            Options::default(),
        );
        assert!(doc.contains("`Read` src/lib.rs"), "{doc}");
        assert!(!doc.contains("fn main() {}"), "{doc}");
    }

    /// Secrets are attached by name and injected as env vars. The dossier is read by another agent,
    /// so a value that reached it would have been leaked by asking for a review.
    #[test]
    fn a_secrets_value_never_reaches_the_dossier() {
        let (mut agent, record, turns) = (agent(), record(), conversation());
        agent.manifest.secrets.push(crate::agent::SecretAttachment {
            project: None,
            name: "OPENAI_API_KEY".to_string(),
        });
        agent
            .manifest
            .env
            .insert("TOKEN".to_string(), "sk-live-42".to_string());
        let doc = document(
            &evidence(
                &agent,
                &record,
                &turns,
                &TokenReport::default(),
                &History::default(),
                &[],
            ),
            Options::default(),
        );
        assert!(doc.contains("OPENAI_API_KEY"), "{doc}");
        assert!(doc.contains("TOKEN"), "{doc}");
        assert!(!doc.contains("sk-live-42"), "{doc}");
    }

    /// A system prompt containing a code block is ordinary. Fenced with three backticks it would
    /// close on the prompt's own fence and spill the rest into the document as the document's own
    /// text — headings and all.
    #[test]
    fn a_system_prompt_with_its_own_fence_cannot_break_out() {
        let (mut agent, record, turns) = (agent(), record(), conversation());
        agent.manifest.arguments.insert(
            "system_prompt".to_string(),
            serde_json::Value::String(
                "Run this:\n```sh\nls\n```\n## Not a section of the dossier".to_string(),
            ),
        );
        let doc = document(
            &evidence(
                &agent,
                &record,
                &turns,
                &TokenReport::default(),
                &History::default(),
                &[],
            ),
            Options::default(),
        );
        let opening = doc
            .lines()
            .find(|l| l.starts_with("````"))
            .expect("a fence longer than the prompt's own");
        // Everything between the opening fence and its match is the prompt, so the heading it
        // contains cannot be read as one of this document's.
        let body = doc.split(opening).nth(1).expect("a closed fence");
        assert!(body.contains("## Not a section of the dossier"), "{doc}");
        assert!(body.contains("```sh"), "{doc}");
    }

    /// The budget is what makes the dossier one `Read` rather than a hunt through offsets, so a
    /// conversation long enough to blow it gives up its middle and says how much.
    #[test]
    fn a_long_conversation_gives_up_its_middle_and_says_so() {
        let (agent, record) = (agent(), record());
        let mut turns = Vec::new();
        for i in 0..600 {
            turns.push(Turn {
                role: "assistant".to_string(),
                text: format!("step {i}"),
                at: 1_000 + i,
                pending: false,
                queued: false,
                steps: vec![tool(
                    "Bash",
                    &format!("cargo test --test really_quite_a_long_test_name_{i}"),
                    ToolStatus::Ok,
                    "",
                )],
                metrics: None,
            });
        }
        let doc = document(
            &evidence(
                &agent,
                &record,
                &turns,
                &TokenReport::default(),
                &History::default(),
                &[],
            ),
            Options::default(),
        );
        assert!(doc.len() <= Options::default().budget, "{} bytes", doc.len());
        assert!(doc.contains("turns in the middle omitted"), "{doc}");
        // The two ends survive: the ask and what it settled on are what the middle explains.
        assert!(doc.contains("really_quite_a_long_test_name_0\n"), "{doc}");
        assert!(doc.contains("really_quite_a_long_test_name_599\n"), "{doc}");
    }

    /// The brief is the reviewer's whole instruction, and the one thing it must always carry is
    /// where the evidence is.
    #[test]
    fn the_brief_carries_the_headline_and_the_path() {
        let (agent, record, turns) = (agent(), record(), conversation());
        let brief = brief(
            &evidence(
                &agent,
                &record,
                &turns,
                &TokenReport::default(),
                &History::default(),
                &[],
            ),
            Path::new("/tmp/sessions/solver/0000000000000-0001.review.md"),
        );
        assert!(brief.contains("/tmp/sessions/solver/0000000000000-0001.review.md"), "{brief}");
        assert!(brief.contains("2 tool calls"), "{brief}");
        assert!(brief.contains("1 of them failed"), "{brief}");
        assert!(brief.contains("Propose only"), "{brief}");
    }

    /// One conversation cannot tell a bad run from a bad tool; the cross-session tally is where a
    /// tool that fails half the time becomes visible.
    #[test]
    fn history_tallies_failure_rates_across_sessions() {
        let mut history = History::default();
        for _ in 0..3 {
            history.fold(&record(), &conversation());
        }
        history.settle();
        assert_eq!(history.sessions_seen, 3);
        assert_eq!(history.by_tool[0], ("Bash".to_string(), 3, 3));
        assert_eq!(history.blocked[0], ("WebFetch".to_string(), 3));
    }
}
