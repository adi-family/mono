//! The `facts` command group: a base of plain sentences an agent can write to in bulk.
//!
//! The name is the instruction. A caller who sees `facts add` knows what shape of input is
//! wanted: sentences, not prose, and more than one at a time.
//!
//! A base is addressed the way a knowledge base is — `global/notes`, `project:acme/default`,
//! `agent:solver/default` — and `--as-agent` / `--as-project` / `--root` mean exactly what they
//! mean there, because they are the same code (`crate::reader`). `--base` is stated once, before
//! the verb, like the identity flags; with none of them the base is `global/default`, or
//! whatever `ADI_FACTS_BASE` says.
//!
//! **Output is plain text everywhere, and there is no `--json`.** The reader is a language
//! model, and JSON only costs it tokens to unwrap something a laid-out line already said. The
//! prototype dropped every `--json` flag it had; this does not put them back.

use std::io::Read as _;

use adi_facts::{BaseId, FactStore, Incoming, KIND_ARTIFACT, KIND_FACT, Verdict, render};
use clap::Subcommand;

use crate::reader::reader_for;

/// What an agent is told about this tool, folded into its system prompt on every turn.
///
/// `adi-tools` captures this by running `facts llm help` and caps it at 3,000 characters, so
/// every sentence here displaces something else the agent was going to be told. The order is
/// deliberate and it is the order a caller needs things in: what to write, how the session ends,
/// which verdict fits, and the one rule that prevents damage.
///
/// It is instructions, not rationale. **Why** the design is like this belongs in `docs/facts.md`
/// and in the code; an agent that has read a paragraph of justification has spent context and
/// learned nothing it can act on. The examples do the teaching — a bad example teaches faster
/// than a rule, which is why two of them are here and the corresponding rules are not.
const LLM_HELP: &str = r"adi-facts — record what somebody said as plain sentences, and keep what you
build on them honest.

A FACT IS ONE SENTENCE THAT STANDS ALONE
  Write it as you would tell someone who was not there. Put negation inside the
  sentence. One fact per line on stdin; send fifty at once if you have them.
    good:  We do not support the CIS.
    bad:   that direction is fine  (a pointer nobody can resolve later)
           run the marketer again  (an instruction, not a fact)

SEARCH FIRST, THEN WRITE ONLY WHAT IS NEW
  $ adi-facts search 'pricing in China' --top 5
  0.812  f_9c1  China pricing is per seat, not per workspace.
  Everything you add is compared against the whole base and every near pair comes
  back for you to rule on by hand. Asking first is how you avoid a queue you then
  work through. No id needed, no cut-off: weak matches show with their score.

A SESSION
  $ printf '%s\n' 'We support all countries.' 'We do not support the CIS.' \
      | adi-facts add --author igor --creator agent:chat@1
  tx_1a02d2f  2 staged, 1 to decide

  [p0] 0.583  controversy
    new   #0   We support all countries.
    base  #1   We do not support the CIS.

  $ adi-facts tx resolve tx_1a02d2f 0 --verdict coexist --confirmer igor
  $ adi-facts tx commit tx_1a02d2f

  NOTHING IS IN THE BASE UNTIL commit; commit refuses while a pair is open.
  'base' is the pair's other side: '#N' if new here too, else a committed id.

VERDICTS  (one per pair, then commit)
  coexist             both are true — you are CONFIRMING that, not dismissing it
  drop                the new fact is wrong or already known; the base was right
  merge --fact '...'  they say the same thing; your sentence replaces both
  supersede --keep ID one wins; ID is '#N' (a new fact) or the base id shown
  merge and supersede rewrite the loser; what was built on it goes stale.

NEVER GUESS A VERDICT. If a pair is genuinely unclear, run `adi-facts tx abort TX`
and tell the person which pair you could not decide. A wrong verdict silently
deletes or keeps the wrong sentence; an abort costs nothing.

--author IS NOT --creator
  --author is whose meaning it is; --creator is who wrote it down. Recording what
  a person said is --author <person> --creator <you>; backwards, every record is
  filed as your own opinion.

A CONCLUSION YOU DREW FROM FACTS
  $ adi-facts derive --from f_9c1 --from f_9c2 \
      --fact 'Market entry plan: skip China.' --creator agent:planner@1
  Reviewed like any other write. --from is what makes `adi-facts stale` report the
  conclusion when a fact under it later changes. Works on `add` too.

READING
  adi-facts list        every fact, most recently changed first
  adi-facts stale       what is out of date, and which fact changed under it
  adi-facts near ID     the facts closest to one you already have
  adi-facts get ID@2    what a fact says now, and what changed since version 2
  --base ID             default global/default; also project:<id>/… agent:<n>/…
";

/// The base a command works on when nothing names one.
const DEFAULT_BASE: &str = "global/default";

#[derive(Debug, Subcommand)]
pub(crate) enum FactsCommand {
    /// Stage facts — one per line on stdin — and print the pairs that need a decision.
    ///
    /// Send fifty at once if you have fifty. Nothing is visible to the base until `tx commit`,
    /// and commit refuses while a pair is open. Nothing is ever merged automatically, at any
    /// similarity.
    Add {
        /// Whose meaning this is — the person who said it.
        #[arg(long, default_value = "human")]
        author: String,
        /// Who is writing the record — your own agent id and version.
        #[arg(long, default_value = "agent:unknown")]
        creator: String,
        /// stdin is one raw note, not a fact list; extract facts from it first.
        ///
        /// The fallback, not the default: a caller in a live conversation knows what "that
        /// direction" meant and a background extractor never will.
        #[arg(long)]
        text: bool,
        /// Id for the stored source note. Default: generated.
        #[arg(long)]
        note_id: Option<String>,
        /// A fact this batch was derived from — a fact id, or `#N` for a fact in this same
        /// batch. Repeat for more.
        ///
        /// Applies to the whole batch. Every fact that lands gets an edge from every source, so
        /// it goes stale the moment any of them moves. Two conclusions from two different source
        /// sets are two calls.
        #[arg(long = "from")]
        from: Vec<String>,
        /// What these become: `fact` for something stated, `artifact` for something derived and
        /// regenerable.
        #[arg(long, default_value = KIND_FACT, value_parser = [KIND_FACT, KIND_ARTIFACT])]
        kind: String,
    },
    /// Work a staged transaction: show, resolve, commit, abort.
    Tx {
        #[command(subcommand)]
        command: TxCommand,
    },
    /// What is out of date, and which fact changed under it.
    Stale,
    /// A derived node was regenerated: bring its incoming edges up to its sources' versions.
    Refresh {
        /// The node that was regenerated.
        id: String,
    },
    /// Find facts by meaning. **Run this before `add`.**
    ///
    /// No id needed: every fact is ranked and the best come back with their scores, so a weak
    /// match is shown rather than hidden. Asking what the base already knows before writing is
    /// how you avoid staging something it already holds — and a pair you never created is a pair
    /// nobody has to rule on.
    Search {
        /// What you are looking for, in words.
        query: String,
        #[arg(long, default_value_t = 10)]
        top: usize,
    },
    /// Every fact in the base, most recently changed first.
    List {
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Facts close to this one, for a verifier to work through.
    ///
    /// Unlike `search`, this starts from an id you already have — it walks the queue around one
    /// fact rather than answering a question in words.
    Near {
        /// The fact to look around.
        id: String,
        #[arg(long, default_value_t = 10)]
        top: usize,
    },
    /// What a referenced fact id says now, and what changed under it.
    Get {
        /// A fact id, optionally with the version you referenced: `f_abc@3`.
        id: String,
        /// Show the log even if nothing has changed.
        #[arg(long)]
        full: bool,
    },
    /// Record something built *on* facts — a plan, a summary, a conclusion — so it goes stale
    /// when they move.
    ///
    /// Exactly `add --from … --kind artifact` with the sentence given as a flag instead of on
    /// stdin, for a caller that has one conclusion and no stdin to spare. It stages, ranks, and
    /// waits for a verdict like any other write; there is no second way into the base.
    Derive {
        /// A fact this was built from. Repeat for more.
        #[arg(long = "from", required = true)]
        from: Vec<String>,
        /// The derived node's own sentence.
        #[arg(long)]
        fact: String,
        #[arg(long, default_value = "human")]
        author: String,
        #[arg(long, default_value = "agent:unknown")]
        creator: String,
        /// `artifact` for something regenerated, `fact` for something stated.
        #[arg(long, default_value = KIND_ARTIFACT, value_parser = [KIND_FACT, KIND_ARTIFACT])]
        kind: String,
    },
    /// List fact bases. Only the ones the caller may read are shown.
    Bases,
    /// Help written for a language model rather than for a person.
    ///
    // `disable_help_subcommand`: clap generates a `help` subcommand for anything that has
    // subcommands, which collides with the `help` this group exists to provide. `adi-tools`
    // captures a tool by running `llm help`, so that name is not ours to move.
    #[command(disable_help_subcommand = true)]
    Llm {
        #[command(subcommand)]
        command: LlmCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum LlmCommand {
    /// Everything an agent needs to use this tool correctly, and nothing else.
    Help,
}

#[derive(Debug, Subcommand)]
pub(crate) enum TxCommand {
    /// Pairs awaiting a decision, strongest first.
    Show {
        /// The transaction id.
        tx: String,
    },
    /// Rule on one pair.
    ///
    /// `coexist` — both stand, and you are confirming that, not skipping it.
    /// `merge` — one sentence replaces both; give it with `--fact`.
    /// `supersede` — the winner replaces the loser; name it with `--keep`.
    /// `drop` — throw the new fact away, the base was already right.
    ///
    /// `merge` and `supersede` both rewrite the OLD fact in place and bump its version, so
    /// anything derived from it goes stale. No duplicate row is ever created.
    Resolve {
        /// The transaction id.
        tx: String,
        /// The pair number, as printed.
        pair: i64,
        #[arg(long, value_parser = ["coexist", "merge", "supersede", "drop"])]
        verdict: String,
        /// supersede: the winner — a base id, or `#N` for a staged fact.
        #[arg(long)]
        keep: Option<String>,
        /// merge: the single sentence that replaces both.
        #[arg(long)]
        fact: Option<String>,
        /// Who decided — a person, or an agent id and version. Recorded on the verdict.
        #[arg(long, default_value = "human")]
        confirmer: String,
    },
    /// Land the transaction; refuses while any pair is open.
    Commit {
        /// The transaction id.
        tx: String,
    },
    /// Discard the whole transaction.
    Abort {
        /// The transaction id.
        tx: String,
    },
}

/// Dispatch a `facts` subcommand, surfacing any store error as a `String` like the other command
/// groups so error families print uniformly.
pub(crate) fn run_facts(
    base: Option<String>,
    as_agent: Option<String>,
    as_project: Option<String>,
    root: bool,
    command: FactsCommand,
) -> Result<(), String> {
    let store = match reader_for(as_agent, as_project, root) {
        Some(reader) => FactStore::open().as_reader(reader),
        None => FactStore::open(),
    };
    let id = parse_base(base)?;

    // Answered before anything opens the store: `adi-tools` captures this on every agent launch
    // with a three-second budget, and a base that cannot be opened must not cost it that.
    if let FactsCommand::Llm {
        command: LlmCommand::Help,
    } = command
    {
        print!("{LLM_HELP}");
        return Ok(());
    }

    match command {
        FactsCommand::Llm { .. } => unreachable!("answered above, before the store is opened"),
        FactsCommand::Bases => {
            let bases = store.list_bases(None);
            if bases.is_empty() {
                println!("No fact bases yet. `facts add` creates one.");
            }
            for base in &bases {
                let count = store.count(base).map_err(err)?;
                println!("{:<34} {:<8} {count} fact(s)", base.to_string(), base.scope.level());
            }
        }
        FactsCommand::Add {
            author,
            creator,
            text,
            note_id,
            from,
            kind,
        } => {
            let raw = read_stdin()?;
            let incoming = Incoming {
                author,
                creator,
                sources: from,
                kind,
            };
            // `add` is the only command that creates a base: anything else opening one on demand
            // would turn a mistyped base id into an empty base and an answer of "nothing here".
            store.ensure_base(&id).map_err(err)?;
            let staging = if text {
                store
                    .add_note(&id, &incoming, &raw, note_id.as_deref())
                    .map_err(err)?
            } else {
                let facts: Vec<String> = raw
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(ToString::to_string)
                    .collect();
                if facts.is_empty() {
                    return Err("nothing on stdin: one fact per line, or --text for a note".into());
                }
                store.add(&id, &incoming, facts).map_err(err)?
            };
            println!("{}", render::staging(&staging));
        }
        FactsCommand::Tx { command } => run_tx(&store, &id, command)?,
        FactsCommand::Stale => println!("{}", render::stale(&store.stale(&id).map_err(err)?)),
        FactsCommand::Search { query, top } => {
            println!(
                "{}",
                render::search(&store.search(&id, &query, top).map_err(err)?)
            );
        }
        FactsCommand::List { limit } => {
            println!("{}", render::list(&store.list(&id, limit).map_err(err)?));
        }
        FactsCommand::Refresh { id: node } => {
            store.refresh(&id, &node).map_err(err)?;
            println!("{node} refreshed");
        }
        FactsCommand::Near { id: node, top } => {
            println!("{}", render::near(&store.near(&id, &node, top).map_err(err)?));
        }
        FactsCommand::Get { id: node, full } => {
            let reference = store.get(&id, &node).map_err(err)?;
            println!("{}", render::reference(&reference, full));
        }
        FactsCommand::Derive {
            from,
            fact,
            author,
            creator,
            kind,
        } => {
            store.ensure_base(&id).map_err(err)?;
            let incoming = Incoming {
                author,
                creator,
                sources: from,
                kind,
            };
            let staging = store.add(&id, &incoming, vec![fact]).map_err(err)?;
            println!("{}", render::staging(&staging));
        }
    }
    Ok(())
}

fn run_tx(store: &FactStore, id: &BaseId, command: TxCommand) -> Result<(), String> {
    match command {
        TxCommand::Show { tx } => {
            println!("{}", render::staging(&store.show(id, &tx).map_err(err)?));
        }
        TxCommand::Resolve {
            tx,
            pair,
            verdict,
            keep,
            fact,
            confirmer,
        } => {
            let verdict: Verdict = verdict.parse().map_err(err)?;
            let staging = store
                .resolve(
                    id,
                    &tx,
                    pair,
                    verdict,
                    keep.as_deref(),
                    fact.as_deref(),
                    &confirmer,
                )
                .map_err(err)?;
            println!("p{pair} -> {verdict} by {confirmer}");
            if staging.open().is_empty() {
                println!("all decided.\n  facts tx commit {tx}");
            } else {
                println!("{}", render::staging(&staging));
            }
        }
        TxCommand::Commit { tx } => {
            println!("{}", render::committed(&store.commit(id, &tx).map_err(err)?));
        }
        TxCommand::Abort { tx } => {
            store.abort(id, &tx).map_err(err)?;
            println!("{tx} aborted");
        }
    }
    Ok(())
}

/// The base to work on: the flag, else `ADI_FACTS_BASE`, else [`DEFAULT_BASE`].
fn parse_base(base: Option<String>) -> Result<BaseId, String> {
    base.or_else(|| std::env::var("ADI_FACTS_BASE").ok())
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE.to_string())
        .parse::<BaseId>()
        .map_err(err)
}

fn read_stdin() -> Result<String, String> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| format!("reading facts from stdin: {e}"))?;
    Ok(buf)
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// A parser for this group alone, so the tests state argv the way a user types it without
    /// carrying the whole top-level CLI's required arguments.
    #[derive(Debug, Parser)]
    struct Harness {
        #[command(subcommand)]
        command: FactsCommand,
    }

    fn parse(args: &[&str]) -> FactsCommand {
        Harness::try_parse_from(std::iter::once("facts").chain(args.iter().copied()))
            .expect("parses")
            .command
    }

    /// `adi-tools` caps a captured help at 3,000 characters and folds it into the agent's system
    /// prompt on **every turn**, so this is both a hard limit and a running cost. Pinned because
    /// the failure is invisible: nothing errors, the agent is just silently told less than it
    /// needs — and the tail is what gets cut, which is where `--from` and the reading commands
    /// are.
    #[test]
    fn the_llm_help_fits_what_an_agent_is_actually_given() {
        const MAX_HELP_CHARS: usize = 3_000;
        assert!(
            LLM_HELP.len() <= MAX_HELP_CHARS,
            "{} chars, cap is {MAX_HELP_CHARS}",
            LLM_HELP.len()
        );

        // The six things an agent cannot work the tool without. A rewrite that drops one of these
        // is the regression this test exists to catch.
        for needle in [
            "One fact per line",          // what to send
            "NOTHING IS IN THE BASE UNTIL commit",
            "coexist",
            "supersede --keep",
            "NEVER GUESS A VERDICT",
            "--from",                     // provenance, and what makes staleness work
            "--author",
            "--creator",
        ] {
            assert!(
                LLM_HELP.contains(needle),
                "the agent is never told about {needle:?}"
            );
        }
    }

    /// It is reachable as `facts llm help` — the first form `adi-tools` tries. Clap generates its
    /// own `help` subcommand for anything with subcommands, which collides with this one, so the
    /// wiring is load-bearing rather than incidental.
    #[test]
    fn llm_help_is_reachable_under_the_name_the_capture_uses() {
        assert!(matches!(
            parse(&["llm", "help"]),
            FactsCommand::Llm {
                command: LlmCommand::Help
            }
        ));
    }

    #[test]
    fn the_group_is_well_formed() {
        use clap::CommandFactory as _;
        Harness::command().debug_assert();
    }

    #[test]
    fn adding_takes_the_two_identities_and_defaults_to_a_fact_list() {
        let FactsCommand::Add {
            author,
            creator,
            text,
            note_id,
            ..
        } = parse(&["add", "--author", "igor", "--creator", "agent:chat@1"])
        else {
            panic!("expected add");
        };
        assert_eq!((author.as_str(), creator.as_str()), ("igor", "agent:chat@1"));
        assert!(!text, "one fact per line unless --text says otherwise");
        assert_eq!(note_id, None);
    }

    #[test]
    fn a_verdict_outside_the_four_is_refused_before_anything_opens_the_base() {
        assert!(
            Harness::try_parse_from([
                "facts", "tx", "resolve", "tx_1", "0", "--verdict", "review",
            ])
            .is_err(),
            "`review` is not a verdict this tool records"
        );
        let FactsCommand::Tx { command } = parse(&[
            "tx", "resolve", "tx_1", "3", "--verdict", "supersede", "--keep", "#0", "--confirmer",
            "agent:verifier@3",
        ]) else {
            panic!("expected tx");
        };
        let TxCommand::Resolve {
            pair,
            verdict,
            keep,
            confirmer,
            ..
        } = command
        else {
            panic!("expected resolve");
        };
        assert_eq!(pair, 3);
        assert_eq!(verdict.parse::<Verdict>().expect("verdict"), Verdict::Supersede);
        assert_eq!(keep.as_deref(), Some("#0"));
        assert_eq!(confirmer, "agent:verifier@3");
    }

    /// `--from` is repeatable and applies to the whole batch — per-line sources were considered
    /// and rejected, because one plain sentence per line is a deliberate property of the format.
    #[test]
    fn adding_takes_repeated_sources_and_a_kind() {
        let FactsCommand::Add { from, kind, .. } = parse(&[
            "add", "--from", "f_abc", "--from", "#0", "--kind", "artifact",
        ]) else {
            panic!("expected add");
        };
        assert_eq!(from, vec!["f_abc", "#0"]);
        assert_eq!(kind, KIND_ARTIFACT);

        let FactsCommand::Add { from, kind, .. } = parse(&["add"]) else {
            panic!("expected add");
        };
        assert!(from.is_empty(), "a batch derived from nothing is the common case");
        assert_eq!(kind, KIND_FACT);

        // A kind nobody defined is refused before the base is opened.
        assert!(Harness::try_parse_from(["facts", "add", "--kind", "composed"]).is_err());
    }

    #[test]
    fn deriving_needs_at_least_one_source() {
        assert!(
            Harness::try_parse_from(["facts", "derive", "--fact", "A plan."]).is_err(),
            "a derived node with no sources can never go stale, which is the whole point"
        );
    }

    #[test]
    fn a_base_id_is_parsed_before_anything_touches_the_store() {
        assert_eq!(
            parse_base(Some("agent:solver/default".into()))
                .expect("parses")
                .to_string(),
            "agent:solver/default"
        );
        // A bare scope is that scope's default base, exactly as in `knowledge`.
        assert_eq!(parse_base(Some("global".into())).expect("parses").to_string(), "global/default");
        assert!(parse_base(Some("team:acme/notes".into())).is_err());
        // A blank flag is not a base id — it falls through to the default rather than failing.
        assert_eq!(parse_base(Some("  ".into())).expect("parses").to_string(), DEFAULT_BASE);
    }
}
