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

use adi_facts::{BaseId, FactStore, Verdict, render};
use clap::Subcommand;

use crate::reader::reader_for;

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
    /// Facts close to this one, for a verifier to work through.
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
    /// Record something built *on* facts — a plan, a summary — so it goes stale when they move.
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
        #[arg(long, default_value = "artifact")]
        kind: String,
    },
    /// List fact bases. Only the ones the caller may read are shown.
    Bases,
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

    match command {
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
        } => {
            let raw = read_stdin()?;
            // `add` is the only command that creates a base: anything else opening one on demand
            // would turn a mistyped base id into an empty base and an answer of "nothing here".
            store.ensure_base(&id).map_err(err)?;
            let staging = if text {
                store
                    .add_note(&id, &author, &creator, &raw, note_id.as_deref())
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
                store.add(&id, &author, &creator, facts).map_err(err)?
            };
            println!("{}", render::staging(&staging));
        }
        FactsCommand::Tx { command } => run_tx(&store, &id, command)?,
        FactsCommand::Stale => println!("{}", render::stale(&store.stale(&id).map_err(err)?)),
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
            let node = store
                .derive(&id, &from, &fact, &author, &creator, &kind)
                .map_err(err)?;
            println!("{node}  derived from {}", from.join(", "));
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
