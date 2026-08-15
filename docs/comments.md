# Comments — the nine rules, and what they mean here

The standard this tree is held to. `crates/adi-agents` is the worked example: every rule below is
illustrated from it, and it was audited against all nine before this document was written.

The short version: **a comment earns its place by saying something the code cannot.** Code says
*what* it does, exhaustively and without ever going out of date. A comment is for the part that
isn't in the code at all — the reason, the alternative that was tried, the constraint imposed by
somebody else's API, the bug this line exists to prevent.

---

## 0. The inline test: only where a wrong guess costs you

The nine rules below govern *what a comment must say*. This one governs *whether an inline `//`
comment should be there at all*, and it is the strictest rule in this document.

**Module headers (`//!`) and item docs (`///`) carry the explanation. An inline `//` comment is for
the line a reader would otherwise get wrong.** Not "would find quicker with help" — *get wrong*: a
change they would make, a reading they would take away, a "simplification" they would apply.

Keep an inline comment when it holds one of these, and delete it otherwise:

| Keep | Example in this tree |
|---|---|
| Another system's behaviour or requirement | "Anthropic rejects a request whose history contains `tool_use` blocks unless that same request also declares tools" |
| A past bug and its symptom | "A pid is a slot the kernel reissues, not a name" |
| An ordering or concurrency hazard | "Dropped from the queue *before* the turn starts … would retry it on every poll for ever" |
| A deliberate non-idiom, incl. every lint suppression | "Saturating rather than casting: a model that asks for line 2^40 gets the end of the file, not a number that wrapped" |
| A magic value's derivation | "2^63 rather than `u64::MAX`: the comparison is in floats, and `u64::MAX` has no exact float form" |
| A citation, or an incompleteness marker | rules 6, 7, 9 below |
| In a test: why a fixture is built the strange way it is | "The same pid against a log that went quiet days before this process existed — which is what both of the stuck runs looked like" |

Delete when it narrates. These were all real comments here, and all of them went:

```rust
// Newest first, so this is the run somebody watching the agent means.
// Commit the answer before showing it, so what a reader sees settle is the same text
// that is on disk a moment later.
// tool_use id → index into `steps`, so its later `tool_result` attaches to the right tool.
```

Each is true, well written, and recoverable by reading the two lines under it. In a test, a comment
restating the `assert!` below it goes the same way — the test's own name is the sentence.

Applying this to `crates/adi-agents` removed 346 lines: inline comments fell from 819 to 473, which
is 0.028 per line of code — the leanest in this workspace. The crate's *total* comment ratio barely
moved (0.30 → 0.28), because seven eighths of it is `///` API documentation, which this rule does
not touch.

---

## 1. Comments should not duplicate the code

A comment that restates its line is worse than no comment: it doubles the reading and halves the
trust, because now two things have to be kept in step and only one of them is compiled.

The test is not "does this comment mention the code" — it is **"would deleting this line lose
anything a reader could not recover by reading the next line?"**

```rust
// No.
// Drop the message from the queue before starting the turn.
let Ok(Some(message)) = store.dequeue(&agent.name, conv_id) else { … };

// Yes — the same site, as it is actually written (crates/adi-agents/src/lib.rs).
// Dropped from the queue *before* the turn starts: a message that fails to launch has still
// had its turn, and leaving it at the head would retry it on every poll for ever.
```

The second one is not a description of `dequeue`. It is the answer to "why here and not after the
spawn?", which no amount of reading the function will supply.

Field and constant docs are the common exception people get wrong in both directions. A doc that
adds a unit, a range, or a consequence is not duplication (`/// Cost in micro-dollars (1e-6 USD).`);
one that expands the identifier into a sentence is (`/// The agent's name.` on `agent: String`).

## 2. Good comments do not excuse unclear code

Prose is not a fix for a bad name, a nested match, or a function doing three things. Fix the code;
then write the comment the fixed code still needs.

Where this bites hardest here is naming that carries a distinction the reader must not miss. The
crate has two different "which runner?" questions — the agent's current backend, and the runner that
started *this* session — and they disagree for a re-pointed agent. That is settled with two named
entry points, `runner_for(&backend)` and `runner_of(&record)`, so most call sites need no comment at
all. The comments that remain say only the thing the names cannot: *which* of the two is right here,
and what breaks if you swap them.

## 3. If you can't write a clear comment, there may be a problem with the code

Failing to explain a function is evidence about the function, not about your writing. When the
comment will only come out as a list of exceptions, the design is the thing to change.

`crates/adi-agents/src/backends/mcp.rs` has a worked example of the fix: two CLI flags, `--tools`
and `--allowed-tools`, that everyone assumed did the same job. The explanation kept coming out as a
paragraph of "except". So the code grew a type — `ToolScope { builtins, allowed }` — and the
paragraph became two field docs, one line each.

## 4. Comments should dispel confusion, not cause it

A comment that is wrong, stale, or half-deleted is a live trap: readers extend trust to prose that
no compiler checks.

Three failures found in the audit of `crates/adi-agents`, all of the same family — an edit moved the
code and left the prose behind:

- `progress.rs` — a doc comment for a `parse(backend, log)` function that had been deleted, spliced
  onto `text_of` and ending mid-sentence.
- `lib.rs` — `emit`'s doc comment stranded above `emit_answered`, so the wrong function was
  documented as "publish an event onto the shared bus" and the right one had no doc at all.
- `memo.rs`, `backends/detached.rs`, `backends/pty/codex.rs`, `backends/process/codex.rs` —
  four references to functions that no longer exist (`parsed_log`, `tail_log`, `engine_argv`,
  `engine_run`), each sending a reader looking for something that was renamed or absorbed years ago.

Two cheap checks catch most of this class, and both are worth running after any refactor:

```bash
cargo doc -p <crate> --no-deps        # rustdoc reports unresolved intra-doc links
```

and a grep for backticked identifiers in comments that match no definition in the tree — a dead
`[`name`]` is a comment that has already stopped being true.

## 5. Explain unidiomatic code in comments

When a line is deliberately not the obvious one, say so — otherwise the next reader "fixes" it.
A lint suppression is a written statement that you know better than the lint, and it is only half a
statement without the reason:

```rust
/// Taking `&bool` is what serde's `skip_serializing_if` requires — it hands the predicate a
/// reference to the field, so the by-value form clippy asks for cannot be named there.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool { !*b }
```

Prefer `#[allow(lint, reason = "…")]` where the justification is short enough to sit inside the
attribute, as `arguments.rs` and `backends/harness/tools.rs` do. The same applies to any cast,
`unwrap`, or manual loop that a reader would otherwise assume was an oversight.

## 6. Provide links to the original source of copied code

Code taken or adapted from somewhere else carries an obligation that outlives the commit: the reader
needs to check it against the original, and needs to know which lines are *not* ours to redesign.

`crates/adi-agents/src/analytics/suffix.rs` is the case in this tree. Its suffix-array construction
is transcribed from cp-algorithms, variable names and all; its LCP array is Kasai's; the stack walk
that reads maximal repeats off LCP intervals is Abouelhoda–Kurtz–Ohlebusch. All three are now cited
in the module docs, with the boundary drawn explicitly:

> What is *not* from the textbook is everything after the walk — non-overlapping occurrences,
> dropping a repeat that lies inside one already reported, the site cap.

That boundary is the useful half of the citation. It tells you which code to fix by reading the
paper, and which to fix by thinking.

Magic constants count as copied code. FNV's offset basis and prime are cited where they are used, so
nobody tunes them.

## 7. Include links to external references where they will be most helpful

When code is written against somebody else's specification, link it — at the place where the next
question will be asked, which is usually the module header rather than the line.

Applied in this crate:

- `backends/harness/adi_loop.rs` — one link per provider (Anthropic, OpenAI, Kimi, Gemini, Ollama),
  because every field name in that file is theirs and the page to open when one moves is not
  guessable from the code.
- `backends/mcp.rs` — the MCP revision the server answers, and the JSON-RPC spec section that
  defines the error code it returns.
- `backends/claude_stream.rs` — the CLI's `stream-json` format, which grows event kinds we ignore.

Link the specification, not a blog post about it, and prefer a URL with a revision in it where the
protocol is versioned. A link to a page that has since been reorganized is a Rule 4 problem, so
check them when the surrounding code changes.

## 8. Add comments when fixing bugs

The bug is the thing a reader cannot see. Once fixed, the code looks like it could have been written
that way for no reason at all — which is exactly how a fix gets refactored away.

Record what went wrong, and what the symptom was:

```rust
/// The pid alone is a slot, not an identity: the kernel reissues it after the child exits, so a
/// number written down here and read back tomorrow may by then belong to a stranger.
```

That comment exists because two finished runs sat "running" in the control panel for three days,
their pids having been handed on to a Chrome renderer and an audio helper. Anyone tempted to
simplify `pid_alive_as` back to `pid_alive` reads the reason before they do it.

The commit message is not a substitute. `git blame` is a tool for someone who already suspects a
line; a comment is for someone who doesn't.

## 9. Use comments to mark incomplete implementations

Partial work is legitimate — silently partial work is not. Mark it where it is, say what is missing,
what a reader will observe, and what finishing it takes.

This tree has no `TODO` convention and does not want one; markers are written in prose, prefixed
with a single greppable word, and kept next to the gap rather than in a tracker:

```rust
// UNIMPLEMENTED: `process:codex`. Codex emits a structured stream under `--json` and nothing
// here reads it, so a Codex run's log arrives as plain text: the answer is whole, the turn has
// no tool steps and no metrics. …
//
// Note what this does *not* line up with: `emits` below already claims `tool_call` and
// `metrics` for `ProcessCodex`, so `crate::progress::capabilities` advertises steps a reader
// will never be shown.
```

The second paragraph is the point. A marker that only says "not done yet" is a note to its author; a
marker that names the inconsistency it leaves behind is a note to whoever hits it.

---

## Reviewing against this

In order of how often it catches something:

0. Would a reader get this line *wrong* without the comment? If not, delete it — the module header
   and the item doc are where explanation lives. (0)
1. Does any comment restate its line? (1)
2. Does any comment name a function, flag, or file that no longer exists? (4) — `cargo doc` finds
   the doc-link half of this for free.
3. Does every lint suppression, cast, and deliberate non-idiom say why? (5)
4. Is every algorithm or constant that came from outside cited? (6, 7)
5. Does every fix carry its symptom? (8)
6. Is every gap marked where it is, with what a reader will observe? (9)
7. And the two that are about the code rather than the comments: if the comment is hard to write,
   change the code (3); if the comment is holding up a bad name, change the name (2).
