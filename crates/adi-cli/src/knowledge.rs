//! The `knowledge` command group: scoped bases of text notes, embedded so they can be searched
//! by meaning.
//!
//! A base is addressed the way it is written — `global/runbooks`, `project:acme/notes`,
//! `agent:solver/memory` — and a bare scope means that scope's `default` base. `--as-agent` /
//! `--as-project` run a command as somebody in particular, which is how the isolation levels are
//! inspected without an agent to run them from; with neither, they fall back to the `ADI_AGENT` /
//! `ADI_PROJECT` an agent run already carries, and with none of those the CLI is the owner of the
//! store and may reach everything.
//!
//! The levels organize knowledge and decide what a run reaches by default. They are **not a
//! sandbox**: anything that can run this binary can also pass a different `--as-agent`, or read
//! the files directly. Isolation here is about keeping one agent's memory from being *rewritten*
//! by another, not about keeping a secret — secrets belong in `adi-mono secrets`.

use std::collections::BTreeMap;
use std::io::Read as _;

use adi_core::Adi;
use adi_knowledge::{
    BaseId, Filter, Hit, Knowledge, KnowledgePatch, KnowledgeStore, NewKnowledge, Reader, Scope,
};
use clap::Subcommand;

use crate::format::{clean, print_json};

#[derive(Debug, Subcommand)]
pub(crate) enum KnowledgeCommand {
    /// List knowledge bases, newest scope first. Only the ones the caller may read are shown.
    Bases {
        /// Only global bases.
        #[arg(long)]
        global: bool,
        /// Only this project's bases.
        #[arg(long)]
        project: Option<String>,
        /// Only this agent's bases.
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Create, delete, and inspect the bases themselves.
    Base {
        #[command(subcommand)]
        command: BaseCommand,
    },
    /// List the storage providers this build can hold a base in.
    Providers {
        #[arg(long)]
        json: bool,
    },
    /// Add a note. The body comes from `--body`, or from stdin when that's omitted — so a long
    /// note needn't fit in one shell argument (`cat notes.md | adi-mono knowledge add … -t "…"`).
    Add {
        /// The base to add to.
        base: String,
        /// The note's one-line title.
        #[arg(short, long)]
        title: String,
        /// The note itself. If omitted, it's read from stdin.
        #[arg(short, long)]
        body: Option<String>,
        /// A tag. Repeat for more, or pass a comma-separated list.
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Where this came from — a URL, a file, a run id.
        #[arg(long)]
        source: Option<String>,
        /// Force a specific note id. Adding again under the same id replaces the note.
        #[arg(long)]
        id: Option<String>,
        /// Create the base first if it isn't there.
        #[arg(long)]
        create: bool,
        #[arg(long)]
        json: bool,
    },
    /// Print one note in full.
    Get {
        base: String,
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Edit a note. Everything omitted is left as it was.
    Edit {
        base: String,
        id: String,
        #[arg(short, long)]
        title: Option<String>,
        /// The new body. Pass `-` to read it from stdin.
        #[arg(short, long)]
        body: Option<String>,
        /// Replace the tag set. Repeat for more.
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Take every tag off.
        #[arg(long)]
        no_tag: bool,
        #[arg(long)]
        source: Option<String>,
        /// Take the source off.
        #[arg(long)]
        no_source: bool,
        #[arg(long)]
        json: bool,
    },
    /// Delete a note.
    Rm {
        base: String,
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// List the notes in a base, newest first.
    List {
        base: String,
        /// Keep only notes carrying every one of these tags.
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Only the notes whose vectors are out of date.
        #[arg(long)]
        stale: bool,
        #[arg(long)]
        json: bool,
    },
    /// Search by meaning. Without `--base`, every base the caller may read.
    Search {
        /// What you're looking for, in words.
        query: String,
        /// A base to search. Repeat for more.
        #[arg(long = "base")]
        bases: Vec<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Search by word instead of by meaning — no model, no network.
        #[arg(long)]
        text: bool,
        #[arg(long)]
        json: bool,
    },
    /// Embed everything in a base that needs it — after an import, or a model change.
    Reembed {
        base: String,
        /// Re-embed every note, current or not.
        #[arg(long)]
        force: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum BaseCommand {
    /// Create a base. `--provider` picks what holds it (see `knowledge providers`).
    New {
        /// The base id, e.g. `global/runbooks` or `agent:solver/memory`.
        base: String,
        /// Which backend provider holds it. Default: sqlite.
        #[arg(long)]
        provider: Option<String>,
        /// A line about what belongs in here.
        #[arg(long)]
        description: Option<String>,
        /// A provider setting, `key=value`. Repeat for more. Passed through untouched.
        #[arg(long = "set", value_name = "KEY=VALUE")]
        settings: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Delete a base and every note in it.
    Rm {
        base: String,
        #[arg(long)]
        json: bool,
    },
    /// What a base holds, and how much of it is searchable by meaning right now.
    Status {
        base: String,
        #[arg(long)]
        json: bool,
    },
}

/// Dispatch a `knowledge` subcommand, surfacing any store error as a `String` like the other
/// command groups so error families print uniformly.
pub(crate) fn run_knowledge(
    adi: Adi,
    as_agent: Option<String>,
    as_project: Option<String>,
    command: KnowledgeCommand,
) -> Result<(), String> {
    let store = reader_store(adi.knowledge(), as_agent, as_project);
    match command {
        KnowledgeCommand::Bases {
            global,
            project,
            agent,
            json,
        } => {
            let scope = scope_filter(global, project, agent)?;
            let bases = store.list_bases(scope.as_ref()).map_err(err)?;
            if json {
                print_json(&bases);
            } else if bases.is_empty() {
                println!("No knowledge bases{}.", scope_suffix(scope.as_ref()));
            } else {
                for base in &bases {
                    let description = base.manifest.description.as_deref().unwrap_or("");
                    println!(
                        "{:<34} {:<8} {:<8} {description}",
                        base.id.to_string(),
                        base.level(),
                        base.manifest.provider
                    );
                }
            }
        }
        KnowledgeCommand::Base { command } => return run_base(&store, command),
        KnowledgeCommand::Providers { json } => {
            let providers = store.providers();
            if json {
                let rows: Vec<_> = providers
                    .all()
                    .iter()
                    .map(|p| serde_json::json!({ "name": p.name(), "description": p.description() }))
                    .collect();
                print_json(&rows);
            } else {
                for provider in providers.all() {
                    println!("{:<10} {}", provider.name(), provider.description());
                }
            }
        }
        KnowledgeCommand::Add {
            base,
            title,
            body,
            tags,
            source,
            id,
            create,
            json,
        } => {
            let base_id = parse_base(&base)?;
            if create {
                store.ensure_base(&base_id).map_err(err)?;
            }
            let body = match body {
                Some(body) => body,
                None => read_stdin()?,
            };
            let saved = store
                .add(
                    &base_id,
                    NewKnowledge {
                        title,
                        body,
                        tags: split_tags(tags),
                        source: clean(source),
                        id: clean(id),
                    },
                )
                .map_err(err)?;
            if json {
                print_json(&saved);
            } else {
                println!("Added {} to {base_id}.", saved.knowledge.id);
                // Never silent about an unembedded note: it is findable by word and not by
                // meaning until something re-embeds it, and the operator has to know that.
                if let Some(reason) = &saved.embed_error {
                    println!("  not embedded: {reason}");
                    println!("  run `adi-mono knowledge reembed {base_id}` once that is fixed.");
                }
            }
        }
        KnowledgeCommand::Get { base, id, json } => {
            let base_id = parse_base(&base)?;
            let note = store
                .get(&base_id, &id)
                .map_err(err)?
                .ok_or_else(|| format!("no such knowledge: {id}"))?;
            if json {
                print_json(&note);
            } else {
                print_note(&note);
            }
        }
        KnowledgeCommand::Edit {
            base,
            id,
            title,
            body,
            tags,
            no_tag,
            source,
            no_source,
            json,
        } => {
            let base_id = parse_base(&base)?;
            // `-` is the convention for "the body is on stdin"; anything else is the body, and
            // omitting `--body` entirely leaves the note's own text alone.
            let body = match body {
                Some(text) if text == "-" => Some(read_stdin()?),
                other => other,
            };
            let patch = KnowledgePatch {
                title,
                body,
                tags: if no_tag {
                    Some(Vec::new())
                } else if tags.is_empty() {
                    None
                } else {
                    Some(split_tags(tags))
                },
                source: if no_source {
                    Some(None)
                } else {
                    source.map(Some)
                },
            };
            let saved = store.update(&base_id, &id, patch).map_err(err)?;
            if json {
                print_json(&saved);
            } else {
                println!("Updated {} in {base_id}.", saved.knowledge.id);
                if let Some(reason) = &saved.embed_error {
                    println!("  not re-embedded: {reason}");
                }
            }
        }
        KnowledgeCommand::Rm { base, id, json } => {
            let base_id = parse_base(&base)?;
            let removed = store.remove(&base_id, &id).map_err(err)?;
            if json {
                print_json(&serde_json::json!({ "id": id, "removed": removed }));
            } else if removed {
                println!("Deleted {id} from {base_id}.");
            } else {
                println!("No such knowledge: {id}");
            }
        }
        KnowledgeCommand::List {
            base,
            tags,
            limit,
            stale,
            json,
        } => {
            let base_id = parse_base(&base)?;
            let notes = store
                .list(
                    &base_id,
                    &Filter {
                        tags: split_tags(tags),
                        limit: Some(limit),
                        stale_only: stale,
                    },
                )
                .map_err(err)?;
            if json {
                print_json(&notes);
            } else if notes.is_empty() {
                println!("Nothing in {base_id}.");
            } else {
                for note in &notes {
                    print_row(note);
                }
            }
        }
        KnowledgeCommand::Search {
            query,
            bases,
            limit,
            text,
            json,
        } => {
            let ids = if bases.is_empty() {
                store.visible_bases().map_err(err)?
            } else {
                bases.iter().map(|b| parse_base(b)).collect::<Result<_, _>>()?
            };
            let hits = if text {
                store.search_text(&ids, &query, limit)
            } else {
                store.search(&ids, &query, limit)
            }
            .map_err(err)?;
            if json {
                print_json(&hits);
            } else if hits.is_empty() {
                println!("Nothing matched.");
            } else {
                for hit in &hits {
                    print_hit(hit);
                }
            }
        }
        KnowledgeCommand::Reembed { base, force, json } => {
            let base_id = parse_base(&base)?;
            let report = store.reembed(&base_id, force).map_err(err)?;
            if json {
                print_json(&report);
            } else {
                println!(
                    "{}: embedded {} of {} note(s) into {} chunk(s); {} already current.",
                    base_id, report.embedded, report.scanned, report.chunks, report.unchanged
                );
                for failure in &report.failed {
                    println!("  {} — {}", failure.id, failure.error);
                }
            }
        }
    }
    Ok(())
}

/// Dispatch a `knowledge base` subcommand.
fn run_base(store: &KnowledgeStore, command: BaseCommand) -> Result<(), String> {
    match command {
        BaseCommand::New {
            base,
            provider,
            description,
            settings,
            json,
        } => {
            let id = parse_base(&base)?;
            let base = store
                .create_base(
                    &id,
                    provider.as_deref(),
                    description.as_deref(),
                    parse_settings(settings)?,
                )
                .map_err(err)?;
            if json {
                print_json(&base);
            } else {
                println!(
                    "Created {} ({} isolation, {} provider).",
                    base.id,
                    base.level(),
                    base.manifest.provider
                );
            }
        }
        BaseCommand::Rm { base, json } => {
            let id = parse_base(&base)?;
            let removed = store.delete_base(&id).map_err(err)?;
            if json {
                print_json(&serde_json::json!({ "base": id.to_string(), "removed": removed }));
            } else if removed {
                println!("Deleted {id} and everything in it.");
            } else {
                println!("No such knowledge base: {id}");
            }
        }
        BaseCommand::Status { base, json } => {
            let status = store.base_status(&parse_base(&base)?).map_err(err)?;
            if json {
                print_json(&status);
            } else {
                println!("{}", status.base.id);
                println!("  level:     {}", status.base.level());
                println!("  provider:  {}", status.base.manifest.provider);
                println!("  notes:     {}", status.notes);
                println!("  embedded:  {} ({} stale)", status.embedded, status.stale);
                // "(not loaded)" rather than a model name: `status` is what somebody runs to find
                // out whether the model is even available, so it must not load one to answer.
                println!(
                    "  model:     {}",
                    status.model.as_deref().unwrap_or("(not loaded)")
                );
            }
        }
    }
    Ok(())
}

/// Narrow the store to whoever is asking.
///
/// `adi-agents` exports `ADI_AGENT` and `ADI_PROJECT` into every run, so an agent that invokes
/// `adi-knowledge` through its own `.bin` gets its isolation applied without having to name
/// itself — while a person's shell, which has neither, stays the owner of the store.
fn reader_store(
    store: KnowledgeStore,
    as_agent: Option<String>,
    as_project: Option<String>,
) -> KnowledgeStore {
    match identity(
        as_agent,
        as_project,
        std::env::var("ADI_AGENT").ok(),
        std::env::var("ADI_PROJECT").ok(),
    ) {
        Some(reader) => store.as_reader(reader),
        None => store,
    }
}

/// Who a command runs as: the flags first, then the run environment, then nobody in particular
/// (which means the owner). Split out from [`reader_store`] so it can be tested without a test
/// reaching into the process environment every other test shares.
fn identity(
    as_agent: Option<String>,
    as_project: Option<String>,
    env_agent: Option<String>,
    env_project: Option<String>,
) -> Option<Reader> {
    let agent = clean(as_agent).or_else(|| clean(env_agent));
    let project = clean(as_project).or_else(|| clean(env_project));
    if agent.is_none() && project.is_none() {
        return None;
    }
    Some(Reader {
        agent,
        project,
        admin: false,
    })
}

fn parse_base(value: &str) -> Result<BaseId, String> {
    value.parse::<BaseId>().map_err(|e| e.to_string())
}

fn scope_filter(
    global: bool,
    project: Option<String>,
    agent: Option<String>,
) -> Result<Option<Scope>, String> {
    match (global, clean(project), clean(agent)) {
        (false, None, None) => Ok(None),
        (true, None, None) => Ok(Some(Scope::Global)),
        (false, Some(id), None) => Scope::project(id).map(Some).map_err(err),
        (false, None, Some(name)) => Scope::agent(name).map(Some).map_err(err),
        _ => Err("pass at most one of --global, --project, --agent".to_string()),
    }
}

fn scope_suffix(scope: Option<&Scope>) -> String {
    scope.map_or_else(String::new, |s| format!(" in {s}"))
}

/// `key=value` pairs into a settings map.
fn parse_settings(pairs: Vec<String>) -> Result<BTreeMap<String, String>, String> {
    pairs
        .into_iter()
        .map(|pair| {
            pair.split_once('=')
                .map(|(k, v)| (k.trim().to_string(), v.to_string()))
                .filter(|(k, _)| !k.is_empty())
                .ok_or_else(|| format!("--set expects key=value, got {pair:?}"))
        })
        .collect()
}

/// Accept both `--tag a --tag b` and `--tag a,b`.
fn split_tags(tags: Vec<String>) -> Vec<String> {
    tags.iter()
        .flat_map(|t| t.split(','))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn read_stdin() -> Result<String, String> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| format!("reading the note from stdin: {e}"))?;
    Ok(buf)
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn print_note(note: &Knowledge) {
    println!("{}", note.title);
    println!("  id:       {}", note.id);
    if let Some(base) = &note.base {
        println!("  base:     {base}");
    }
    if !note.tags.is_empty() {
        println!("  tags:     {}", note.tags.join(", "));
    }
    if let Some(source) = &note.source {
        println!("  source:   {source}");
    }
    println!("  embedded: {}", embedding_label(note));
    println!();
    println!("{}", note.body);
}

fn print_row(note: &Knowledge) {
    let mark = if note.is_embedded() { ' ' } else { '*' };
    println!(
        "{mark} {:<32} {:<40} {}",
        note.id,
        note.title,
        note.preview(48)
    );
}

fn print_hit(hit: &Hit) {
    let base = hit
        .knowledge
        .base
        .as_ref()
        .map_or_else(String::new, ToString::to_string);
    println!(
        "{:.3}  {:<28} {:<34} {}",
        hit.score,
        base,
        hit.knowledge.id,
        hit.knowledge.title
    );
}

fn embedding_label(note: &Knowledge) -> String {
    match (&note.embedding.model, note.embedding.chunks) {
        (Some(model), chunks) if note.is_embedded() => {
            format!("{chunks} chunk(s) by {model}")
        }
        _ => "no (run `knowledge reembed`)".to_string(),
    }
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
        command: KnowledgeCommand,
    }

    fn parse(args: &[&str]) -> KnowledgeCommand {
        Harness::try_parse_from(std::iter::once("knowledge").chain(args.iter().copied()))
            .expect("parses")
            .command
    }

    #[test]
    fn the_group_is_well_formed() {
        use clap::CommandFactory as _;
        Harness::command().debug_assert();
    }

    #[test]
    fn adding_a_note_takes_its_tags_either_way_round() {
        let KnowledgeCommand::Add { base, title, tags, create, .. } = parse(&[
            "add", "global/runbooks", "-t", "Restarting", "--tag", "ops,adi", "--tag", "deploy",
            "--create",
        ]) else {
            panic!("expected add");
        };
        assert_eq!(base, "global/runbooks");
        assert_eq!(title, "Restarting");
        assert!(create);
        // Repeated and comma-separated both arrive; splitting is the adapter's job.
        assert_eq!(split_tags(tags), vec!["ops", "adi", "deploy"]);
    }

    #[test]
    fn a_base_is_created_through_the_base_group() {
        let KnowledgeCommand::Base { command } = parse(&[
            "base", "new", "project:acme/notes", "--provider", "memory", "--set", "room=12",
        ]) else {
            panic!("expected base");
        };
        let BaseCommand::New { base, provider, settings, .. } = command else {
            panic!("expected base new");
        };
        assert_eq!(base, "project:acme/notes");
        assert_eq!(provider.as_deref(), Some("memory"));
        assert_eq!(
            parse_settings(settings).expect("settings").get("room").map(String::as_str),
            Some("12")
        );
    }

    #[test]
    fn search_defaults_to_meaning_over_every_readable_base() {
        let KnowledgeCommand::Search { query, bases, limit, text, .. } =
            parse(&["search", "how do I deploy"])
        else {
            panic!("expected search");
        };
        assert_eq!(query, "how do I deploy");
        assert!(bases.is_empty(), "no --base means every readable base");
        assert_eq!(limit, 10);
        assert!(!text, "meaning, not words, unless --text is asked for");
    }

    #[test]
    fn clearing_a_notes_tags_is_distinct_from_leaving_them_alone() {
        let KnowledgeCommand::Edit { tags, no_tag, .. } = parse(&["edit", "global/n", "note"])
        else {
            panic!("expected edit");
        };
        assert!(tags.is_empty() && !no_tag, "omitted means unchanged");

        let KnowledgeCommand::Edit { no_tag, no_source, .. } =
            parse(&["edit", "global/n", "note", "--no-tag", "--no-source"])
        else {
            panic!("expected edit");
        };
        assert!(no_tag && no_source);
    }

    #[test]
    fn a_base_id_is_parsed_before_anything_touches_the_store() {
        assert!(parse_base("agent:solver/memory").is_ok());
        assert!(parse_base("global").is_ok(), "a bare scope is its default base");
        assert!(parse_base("team:acme/notes").is_err());
    }

    #[test]
    fn a_scope_filter_takes_at_most_one_level() {
        assert_eq!(scope_filter(false, None, None).expect("none"), None);
        assert_eq!(
            scope_filter(true, None, None).expect("global"),
            Some(Scope::Global)
        );
        assert!(scope_filter(false, Some("acme".into()), Some("solver".into())).is_err());
        assert!(scope_filter(true, Some("acme".into()), None).is_err());
    }

    #[test]
    fn a_setting_without_an_equals_sign_is_refused() {
        assert!(parse_settings(vec!["room".into()]).is_err());
        assert!(parse_settings(vec!["=12".into()]).is_err());
        assert_eq!(
            parse_settings(vec!["url=https://x/y?a=b".into()])
                .expect("settings")
                .get("url")
                .map(String::as_str),
            Some("https://x/y?a=b"),
            "only the first = separates; the value may contain more"
        );
    }

    /// The fallback that makes the system tool work: a run already carries who it is.
    #[test]
    fn the_run_environment_supplies_an_identity_no_flag_stated() {
        let from_env = identity(None, None, Some("solver".into()), Some("acme".into()))
            .expect("an agent run is somebody");
        assert_eq!(from_env.agent.as_deref(), Some("solver"));
        assert_eq!(from_env.project.as_deref(), Some("acme"));
        assert!(!from_env.admin);

        // A stated flag beats the environment — that is what makes `--as-agent` useful for
        // inspecting another agent's view from inside a run.
        let stated = identity(Some("reviewer".into()), None, Some("solver".into()), None)
            .expect("stated");
        assert_eq!(stated.agent.as_deref(), Some("reviewer"));

        // A person's shell has neither, and stays the owner of the store.
        assert_eq!(identity(None, None, None, None), None);
        // …and an empty variable is not an identity, which is how an exported-but-blank
        // `ADI_PROJECT` avoids hiding every project base from a run that has no project.
        assert_eq!(identity(None, None, Some(String::new()), Some("  ".into())), None);
    }

    /// Identity is a flag on the group, not on each subcommand — so it is stated once, before
    /// the verb, and every verb gets it.
    #[test]
    fn the_reader_is_the_owner_unless_the_flags_say_otherwise() {
        // `open` resolves paths and touches no disk; the assertions are all about the reader.
        let store = KnowledgeStore::open();
        let scoped = store
            .clone()
            .as_reader(identity(Some("solver".into()), Some("acme".into()), None, None).expect("reader"));
        assert!(!scoped.reader().admin);
        assert_eq!(scoped.reader().agent.as_deref(), Some("solver"));
        assert_eq!(scoped.reader().project.as_deref(), Some("acme"));

        // A blank flag is not an identity — it would otherwise silently drop every base.
        assert_eq!(identity(Some("  ".into()), None, None, None), None);
        assert!(store.reader().admin);
    }
}
