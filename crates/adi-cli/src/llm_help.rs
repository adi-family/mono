//! `llm` under every command group: help written for a language model rather than for a person.
//!
//! `adi-tools` asks every tool `llm help` first (then `help`, then `--help`) and folds whatever
//! answers into the system prompt of each agent that has the tool, on every turn, capped at
//! 3,000 characters. Each system tool is a shim over one group here, so a group without `llm`
//! handed its agents clap's own listing — verb names with no arguments — and sent them off to run
//! `--help` on each verb before they could do anything. `adi-mono dns llm` was an error.
//!
//! The text is two halves. The **intro** is written by hand, in [`GROUPS`]: what the group is
//! for and the rules that keep a caller out of trouble — instructions, not rationale, because an
//! agent that has read a justification has spent context and learned nothing it can act on. The
//! **command reference** is generated from the clap tree, one usage line and one sentence per
//! verb, so it cannot drift from the verbs that really exist.
//!
//! `facts` and `workflows` had a hand-written `llm help` before this module existed and keep it
//! whole: theirs teach a session by example, which a generated listing cannot.

use std::fmt::Write as _;

use clap::{Arg, ArgAction, Args, Command, Subcommand};

/// The most of one tool's help that reaches a prompt — `adi_tools`' own cap. A group whose text
/// runs past it loses its tail silently, and the tail is the command reference.
#[cfg(test)]
pub(crate) const MAX_HELP_CHARS: usize = 3_000;

/// The `llm` verb itself, the same under every group.
///
/// `disable_help_subcommand`: clap generates a `help` subcommand for anything that has
/// subcommands, which collides with the `help` this verb exists to provide. `adi-tools` captures a
/// tool by running `llm help`, so that name is not ours to move. Bare `llm` answers the same text:
/// `help` is the only thing there is to ask for, and a caller should not have to spell it out.
// `Clone, Copy`: `analytics` and `update` derive both on their own enums, which this sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Args)]
#[command(disable_help_subcommand = true)]
pub(crate) struct LlmHelp {
    #[command(subcommand)]
    pub(crate) command: Option<LlmHelpCommand>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub(crate) enum LlmHelpCommand {
    /// Everything an agent needs to use this command group correctly, and nothing else.
    Help,
}

/// What one group's `llm help` is made of.
pub(crate) enum Text {
    /// A hand-written intro, followed by the command reference generated from the clap tree.
    Intro(&'static str),
    /// The whole text, written by hand and printed as it is.
    Whole(&'static str),
}

/// One command group: its `adi-mono` subcommand, the name an agent runs it by, and its text.
pub(crate) struct Group {
    /// The `adi-mono` subcommand, e.g. `dns`.
    pub(crate) name: &'static str,
    /// What the text calls the command: the system tool's name where there is one (`adi-dns`),
    /// since that is the name an agent's prompt gives it; `adi-mono <name>` otherwise.
    pub(crate) prog: &'static str,
    pub(crate) text: Text,
}

/// Look a group up by its `adi-mono` subcommand.
pub(crate) fn group(name: &str) -> Option<&'static Group> {
    GROUPS.iter().find(|group| group.name == name)
}

/// Every command group that has verbs. A group added to the CLI without an entry here fails the
/// tests in `main`, which walk the whole tree.
pub(crate) const GROUPS: &[Group] = &[
    Group {
        name: "dns",
        prog: "adi-dns",
        text: Text::Intro(INTRO_DNS),
    },
    Group {
        name: "projects",
        prog: "adi-projects",
        text: Text::Intro(INTRO_PROJECTS),
    },
    Group {
        name: "tasks",
        prog: "adi-tasks",
        text: Text::Intro(INTRO_TASKS),
    },
    Group {
        name: "tools",
        prog: "adi-tools",
        text: Text::Intro(INTRO_TOOLS),
    },
    Group {
        name: "secrets",
        prog: "adi-secrets",
        text: Text::Intro(INTRO_SECRETS),
    },
    Group {
        name: "db",
        prog: "adi-db",
        text: Text::Intro(INTRO_DB),
    },
    Group {
        name: "agents",
        prog: "adi-agents",
        text: Text::Intro(INTRO_AGENTS),
    },
    Group {
        name: "llm",
        prog: "adi-mono llm",
        text: Text::Intro(INTRO_LLM),
    },
    Group {
        name: "embeddings",
        prog: "adi-mono embeddings",
        text: Text::Intro(INTRO_EMBEDDINGS),
    },
    Group {
        name: "goals",
        prog: "adi-mono goals",
        text: Text::Intro(INTRO_GOALS),
    },
    Group {
        name: "triggers",
        prog: "adi-triggers",
        text: Text::Intro(INTRO_TRIGGERS),
    },
    Group {
        name: "workflows",
        prog: "adi-workflows",
        text: Text::Whole(crate::workflows::LLM_HELP),
    },
    Group {
        name: "knowledge",
        prog: "adi-knowledge",
        text: Text::Intro(INTRO_KNOWLEDGE),
    },
    Group {
        name: "facts",
        prog: "adi-facts",
        text: Text::Whole(crate::facts::LLM_HELP),
    },
    Group {
        name: "indexer",
        prog: "adi-mono indexer",
        text: Text::Intro(INTRO_INDEXER),
    },
    Group {
        name: "mesh",
        prog: "adi-mono mesh",
        text: Text::Intro(INTRO_MESH),
    },
    Group {
        name: "marketplace",
        prog: "adi-mono marketplace",
        text: Text::Intro(INTRO_MARKETPLACE),
    },
    Group {
        name: "channels",
        prog: "adi-mono channels",
        text: Text::Intro(INTRO_CHANNELS),
    },
    Group {
        name: "events",
        prog: "adi-mono events",
        text: Text::Intro(INTRO_EVENTS),
    },
    Group {
        name: "logs",
        prog: "adi-mono logs",
        text: Text::Intro(INTRO_LOGS),
    },
    Group {
        name: "update",
        prog: "adi-mono update",
        text: Text::Intro(INTRO_UPDATE),
    },
    Group {
        name: "analytics",
        prog: "adi-mono analytics",
        text: Text::Intro(INTRO_ANALYTICS),
    },
    Group {
        name: "mcp",
        prog: "adi-mono mcp",
        text: Text::Intro(INTRO_MCP),
    },
];

const INTRO_DNS: &str = r"adi-dns (adi-mono dns) — the resolver that makes `<name>.adi` names answer on this
machine, and the front door (port 80) that serves them.

  `status` changes nothing; start there when a `.adi` URL does not load.
  `install-route`, `grant-dns`, `grant-network`, `remove-route` — and `enable` the first
  time — raise a macOS admin prompt that a person must approve at this screen. From an
  unattended run, ask them before you run one.
";

const INTRO_PROJECTS: &str = r"adi-projects (adi-mono projects) — the registry of projects. Agents, tools, triggers,
secrets, databases and knowledge bases are filed under a project by its id.

  `add <NAME>` generates the id and prints it; pass that id, not the display name,
  wherever another command takes `--project`.
  `rename` changes the id itself, and everything filed under it follows.
  `archive` is reversible with `unarchive`; `rm` deletes the project's directory for good.
";

const INTRO_TASKS: &str = r#"adi-tasks (adi-mono tasks) — the task tree. A task has a title and details, and may have a
parent, a project, a tag and a working directory (`--cwd`, inherited by its subtasks).

  Stored status is open, done or archived. `list --effective ready` is what can be worked
  now: open, with no open direct child. An open task with open children is `blocked`.
  `add` prints the new id. `--parent <ID>` files a subtask.
  `complete` finishes a task. If the task declares an output shape, the call is
  refused unless `--result '<json>'` (or `--result-file`) fits it; the error names the path.
  `give-up <ID> --reason "…"` records why you stopped, and leaves the task open.
  `edit` with an empty string clears an optional field. `rm` is permanent; prefer `archive`.
"#;

const INTRO_TOOLS: &str = r"adi-tools (adi-mono tools) — the registry of tools: small sh or ts CLIs that agents run by
name from their own PATH.

  `add <NAME> --runtime sh|ts` writes a new script into the store; `link <PATH>` registers
  an existing file in place and never copies it. Either way a `.bin/<name>` shim is made.
  Edit an owned tool's script at the path `adi-tools path <ID>` prints.
  A tool describes itself by answering `<name> llm help`; that text is what each agent
  with the tool is told about it, so write one.
  An agent gets a tool through `adi-agents save <AGENT> --tool <ID>` — that flag replaces
  the agent's whole tool list, so restate the ones it already has.
  `run <ID> -- <ARGS>` runs a tool that is not on your PATH.
  System tools (`sys-*`) are re-seeded by the platform; edits to them are overwritten.
";

const INTRO_SECRETS: &str = r#"adi-secrets (adi-mono secrets) — encrypted key-values, global or per project
(`--project <ID>`). A run gets the secrets attached to its agent, as env vars of the same name.

  `read <NAME>` prints the raw value and nothing else: `TOKEN=$(adi-secrets read NAME)`.
  Never print a value, or write it to a file, a note, a commit or a reply.
  `set <NAME>` reads the value from stdin when it is not given; use that, so it never
  sits in argv or shell history: `printf %s "$V" | adi-secrets set NAME`.
  `list` and `get` show names and metadata only (`get --reveal` decrypts).
  `--user --node`, `policy` and `leases` are for a user's own scope on a paired server.
"#;

const INTRO_DB: &str = r"adi-db (adi-mono db) — SQL against the shared SQLite store: one global database, and one
per project (`--project <ID>`).

  Read `schema [TABLE]` before you query a table you did not create.
  `query` returns rows (`select`, or anything with `returning`); `exec` is for effects and
  DDL, and without `--param` runs its whole input as one batch.
  Bind every value with `--param`, one per `?`, in order; never paste values into the SQL.
  Long or quoted SQL goes on stdin: `adi-db query --json <<'SQL'` … `SQL`.
  Take a `backup <PATH>` before anything destructive.
";

const INTRO_AGENTS: &str = r#"adi-agents (adi-mono agents) — agent definitions, and their runs.

  `run <NAME> "<task>"` starts a run and returns at once. Launched from inside a run, you
  are woken when it ends — do not poll it. `--wait` blocks and prints its answer instead.
  `--pre-run "<cmd>"` hands it that command's real output (`adi-tasks show <id>`) rather
  than an instruction it spends a turn obeying.
  `save` creates or edits a definition. A flag left out keeps its stored value; a list flag
  given (`--tool`, `--secret`, `--tag`, `--llm`, …) replaces the whole list, so restate what
  stays. Read `show <NAME>` before changing an agent.
  `questions` lists runs waiting on a person; `answer` replies to one.
"#;

const INTRO_LLM: &str = r"adi-mono llm — LLM backends. A backend is one way to answer a turn: a runtime, a login,
a model and its dials. An agent lists backend ids in order (`agents save <NAME> --llm <ID>`):
the first is where a new conversation starts, the rest are where it goes when one runs out.

  `backends` shows each with its model and whether it is held (spent) right now; `holds`
  says until when, and `release <ID>` lifts a hold by hand.
  `save` states the whole backend: read `show <ID>` first and restate every field, limit
  rules included, or what you leave out is gone.
  `test <ID>` asks a backend its test prompt now. `probe` is the background check.
";

const INTRO_EMBEDDINGS: &str = r"adi-mono embeddings — embedding backends. A backend is one way to turn text into a vector:
a runtime, a model and its width. `indexer`, `knowledge` and `facts` each resolve through
exactly one, assigned with `settings --assign <CONSUMER=BACKEND>`.

  A fallback must be a backend with the same model and width; `save` refuses any other.
  Vectors made by one model are not comparable with another's: after changing what a
  consumer uses, `adi-mono knowledge reembed <BASE> --force` (or re-index).
  `test <ID>` embeds a test string now and reports the width it got.
";

const INTRO_GOALS: &str = r#"adi-mono goals — what a conversation is for, and the two ways it ends.

  Inside a run, every verb finds its conversation from `$ADI_AGENT` and `$ADI_RUN_ID`, so
  you pass only the text. From a terminal, pass `--agent` and `--session`.
  `create "<what done means>"` sets a goal. While it is open, the run is asked about it
  each time it falls quiet.
  Close it with `met <ID> --evidence "…"` — the test that passed, the file that exists —
  or with `knowingly-give-up <ID> --why "…"`. Nothing else closes a goal.
"#;

const INTRO_TRIGGERS: &str = r"adi-triggers (adi-mono triggers) — code blocks the platform runs for you: on an inbound
webhook (`/api/hooks/<name>`), on a platform event, or kept alive in the background.

  Start from a preset: `presets` lists them, and `save <NAME> --preset <ID>` fills the
  kind, runtime and code; anything else you pass wins over it.
  `save` replaces the whole definition. Settings reach the code as `ADI_<KEY>` env vars
  (`--extra key=value`).
  Gate a webhook with `--hook-secret <SECRET_NAME>`, after setting that secret with
  adi-secrets; never put a secret's value in `--extra`.
  `fire <NAME>` runs it once by hand; `log <NAME>` prints the last fire's output.
";

const INTRO_KNOWLEDGE: &str = r#"adi-knowledge (adi-mono knowledge) — bases of text notes, searched by meaning.

  A base is `global/<name>`, `project:<id>/<name>` or `agent:<name>/<base>`; a bare scope
  means its `default` base. An agent with a memory finds its name in `$ADI_MEMORY`.
  Search before you write, and ask the question you have, not keywords:
    adi-knowledge search "why does the front door 502" --full
  `--full` prints each hit's body (no `get` needed); `--text` matches words, not meaning.
  `add <BASE> -t "<title>"` takes the body from `-b`, or from stdin. One note per thing
  learned, worded so it is still true next time. `--id` replaces the note with that id.
  Another agent's `agent:` bases are read-only to you.
"#;

const INTRO_INDEXER: &str = r#"adi-mono indexer — an index of one project's source code: its symbols, searchable by
meaning, by name or by path. Every verb works on `--path <DIR>`, or the current directory;
the index lives in that directory's `.adi/tree`.

  `index` builds or refreshes it; `status` says whether it is current.
  `search "<what the code does>"` finds code by meaning, `symbols` by name, `files` by path.
"#;

const INTRO_MESH: &str = r"adi-mono mesh — the fleet: other adi machines paired with this one.

  Pairing runs on two machines. `invite` on the viewer (the machine you sit at) prints a
  one-time token; `join <TOKEN>` on the node completes it. The node never listens.
  A node reaches only what it is granted (`grant <PETNAME> http:<name>`); `revoke` takes a
  grant back, `unpair` forgets the node both ways.
";

const INTRO_MARKETPLACE: &str = r"adi-mono marketplace — apps from manifest sources you host anywhere.

  `add <NAME> <URL>` registers a source, `sync` fetches every source, `apps` lists what
  they offer and what is installed here.
  `install` lands a bundle and starts it; `--no-start` leaves it inert until `start`.
  `--project <ID>` files the install under a project, the only way to hold one bundle twice.
";

const INTRO_CHANNELS: &str = r"adi-mono channels — Telegram and Slack connections to an agent. Every verb calls this
machine's adi-app (`/api/channels/*`), so adi-app must be running here.

  `connect <PROVIDER> --agent <AGENT>` prints a link URL and waits until a person completes
  the link there. `route` points a connection at another agent, or pauses it; `allow` says
  who may talk to it (only whoever linked it, by default); `disconnect` removes it.
";

const INTRO_EVENTS: &str = r"adi-mono events — the platform event bus.

  `types` lists the events you can subscribe to, with an example payload of each.
  `emit <NAME> --payload '<json>'` publishes one; every enabled event trigger whose pattern
  matches fires, exactly as for the events the platform publishes itself.
";

const INTRO_LOGS: &str = r"adi-mono logs — durable diagnostics shared by services and the dashboard's Event log.
";

const INTRO_UPDATE: &str = r"adi-mono update — auto-update. One update replaces the whole app bundle, every binary.

  `check` asks whether a newer release exists. `run` installs it, which restarts the app
  and everything running under it — ask the person first.
";

const INTRO_ANALYTICS: &str = r"adi-mono analytics — anonymous usage events, on by default. `disable` turns them off, as do
`DO_NOT_TRACK=1` and `ADI_ANALYTICS=0`; `forget` drops this install's id.
";

const INTRO_MCP: &str = r"adi-mono mcp — MCP server definitions an agent may be given (`agents save <NAME> --mcp <ID>`).

  `save` states the whole definition. A header or env value is a literal, `secret:NAME`
  to name a secret, or `bearer:NAME` for `Bearer <token>` from a secret holding the raw
  token — never write `Bearer` into the secret's own value.
  `show <ID>` says which secrets it needs and whether each is set.
";

/// Render `group`'s `llm help`. `cli` is the whole `adi-mono` command tree, unbuilt — so the
/// global `--flavor` and clap's own `--help` have not been propagated into it yet, and the
/// reference shows only what a caller of this group actually has to know.
pub(crate) fn render(cli: &Command, group: &Group) -> String {
    let intro = match group.text {
        Text::Whole(text) => return text.to_string(),
        Text::Intro(intro) => intro,
    };
    let Some(cmd) = cli.find_subcommand(group.name) else {
        return intro.to_string();
    };
    let mut negated = false;
    let mut out = String::new();
    out.push_str(intro.trim_end());
    out.push_str("\n\nCOMMANDS\n");
    // Arguments of the group itself (`knowledge --as-agent`, …) go before the verb.
    let own = usage(cmd, &mut negated);
    if !own.is_empty() {
        let _ = writeln!(out, "  {}{own} <COMMAND>", group.prog);
    }
    verbs(cmd, group.prog, &mut out, &mut negated);
    let _ = write!(
        out,
        "\n`{} <COMMAND> --help` lists every flag of one command.",
        group.prog
    );
    if negated {
        out.push_str(" `--no-<flag>` clears or turns off\nthe flag of that name.");
    }
    out.push('\n');
    out
}

/// One runnable verb: its usage and its first sentence, and the names it answers to — two
/// variants with the same arguments and the same about (`rm`/`delete`) are one verb twice.
struct Verb {
    names: Vec<String>,
    usage: String,
    about: String,
}

/// One entry per runnable verb, nested groups flattened into their path (`knowledge base new`).
fn verbs(cmd: &Command, path: &str, out: &mut String, negated: &mut bool) {
    let mut level: Vec<Verb> = Vec::new();
    let flush = |level: &mut Vec<Verb>, out: &mut String| {
        for verb in level.drain(..) {
            let _ = writeln!(out, "  {path} {}{}", verb.names[0], verb.usage);
            if !verb.about.is_empty() || verb.names.len() > 1 {
                let aliases: Vec<String> =
                    verb.names[1..].iter().map(|n| format!("`{n}`")).collect();
                let also = if aliases.is_empty() {
                    String::new()
                } else {
                    format!(" (also {})", aliases.join(", "))
                };
                let _ = writeln!(out, "      {}{also}", verb.about);
            }
        }
    };
    for sub in cmd.get_subcommands() {
        if sub.is_hide_set() || matches!(sub.get_name(), "help" | "llm") {
            continue;
        }
        if sub.has_subcommands() {
            flush(&mut level, out);
            let nested = format!("{path} {}{}", sub.get_name(), usage(sub, negated));
            verbs(sub, &nested, out, negated);
            continue;
        }
        let usage = usage(sub, negated);
        let about = sub
            .get_about()
            .map(|about| first_sentence(&about.to_string()))
            .unwrap_or_default();
        match level
            .iter_mut()
            .find(|verb| verb.usage == usage && verb.about == about)
        {
            Some(same) => same.names.push(sub.get_name().to_string()),
            None => level.push(Verb {
                names: vec![sub.get_name().to_string()],
                usage,
                about,
            }),
        }
    }
    flush(&mut level, out);
}

/// The longest usage line written out in full. Past it the optional flags fold into `[OPTIONS]`:
/// `agents save` alone has forty, and spelling them out would push the rest of the group past
/// `MAX_HELP_CHARS` — the cap cuts the tail, so the verbs after it would simply not exist.
const LONG_USAGE: usize = 160;

/// A verb's arguments as a usage line: positionals as `<NAME>`, options as `--long <VALUE>`, each
/// optional one bracketed — clap's own notation, so it reads the way `--help` does.
///
/// A `--no-<flag>` beside its own `--<flag>` is left out (and `negated` set): it only clears or
/// turns off the other, which the footer says once instead of on every line.
fn usage(cmd: &Command, negated: &mut bool) -> String {
    let longs: Vec<&str> = cmd.get_arguments().filter_map(Arg::get_long).collect();
    let (mut leading, mut optional) = (String::new(), String::new());
    for arg in cmd.get_arguments() {
        if arg.is_global_set()
            || arg.is_hide_set()
            || matches!(arg.get_id().as_str(), "help" | "version")
        {
            continue;
        }
        if arg
            .get_long()
            .and_then(|long| long.strip_prefix("no-"))
            .is_some_and(|on| longs.contains(&on))
        {
            *negated = true;
            continue;
        }
        let one = if arg.is_positional() {
            format!(
                "<{}>{}",
                value_name(arg),
                if many(arg) { "..." } else { "" }
            )
        } else {
            let flag = match (arg.get_long(), arg.get_short()) {
                (Some(long), _) => format!("--{long}"),
                (None, Some(short)) => format!("-{short}"),
                (None, None) => continue,
            };
            if takes_value(arg) {
                format!(
                    "{flag} <{}>{}",
                    value_name(arg),
                    if many(arg) { "..." } else { "" }
                )
            } else {
                flag
            }
        };
        // Positionals keep their place whether required or not, since their order is their
        // meaning; a required flag stays beside them, so folding the options never hides it.
        if arg.is_positional() || arg.is_required_set() {
            if arg.is_required_set() {
                let _ = write!(leading, " {one}");
            } else {
                let _ = write!(leading, " [{one}]");
            }
        } else {
            let _ = write!(optional, " [{one}]");
        }
    }
    if leading.len() + optional.len() > LONG_USAGE {
        format!("{leading} [OPTIONS]")
    } else {
        format!("{leading}{optional}")
    }
}

fn value_name(arg: &Arg) -> String {
    arg.get_value_names()
        .and_then(|names| names.first().map(ToString::to_string))
        .unwrap_or_else(|| arg.get_id().as_str().to_uppercase())
}

fn takes_value(arg: &Arg) -> bool {
    matches!(arg.get_action(), ArgAction::Set | ArgAction::Append)
}

fn many(arg: &Arg) -> bool {
    matches!(arg.get_action(), ArgAction::Append)
        || arg
            .get_num_args()
            .is_some_and(|range| range.max_values() > 1)
}

/// The first sentence of a verb's about: what fits on one line of a reference. A period ends it
/// unless it closes `e.g.`/`i.e.`; rustdoc's intra-doc link brackets are dropped, since the reader
/// is a model, not rustdoc.
fn first_sentence(about: &str) -> String {
    let about = about.replace("[`", "`").replace("`]", "`");
    let end = about
        .match_indices(". ")
        .map(|(i, _)| i)
        .find(|&i| !about[..i].ends_with("e.g") && !about[..i].ends_with("i.e"))
        .map_or(about.len(), |i| i + 1);
    // A one-sentence about has no period (clap strips it); a cut one keeps its own. Neither, so
    // that every line of the reference ends the same way.
    about[..end].trim().trim_end_matches('.').to_string()
}
