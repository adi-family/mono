//! The `marketplace` command group: manage manifest sources, sync them, install and start apps.
//!
//! No `adi` facade, like `mesh` and `indexer`: the state is the marketplace module's own under
//! the store, and the handle opens it itself.

use adi_marketplace::{Kind, Marketplace};
use clap::Subcommand;

/// The verbs, as `adi-mono marketplace <verb>` spells them.
#[derive(Debug, Subcommand)]
pub(crate) enum MarketplaceCommand {
    /// Add a marketplace: a name to know it by, and the https:// URL of its manifest.
    Add {
        /// The local name — the first half of <marketplace>/<slug> installs are addressed by.
        #[arg(value_name = "NAME")]
        name: String,
        /// Where the manifest is fetched from. https only.
        #[arg(value_name = "URL")]
        url: String,
    },
    /// Remove a marketplace and its cached manifest.
    Remove {
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// List the configured marketplaces and when each was last synced.
    List,
    /// Fetch every marketplace's manifest and cache it. A failing URL keeps the stale cache and
    /// warns; only a first fetch that fails with nothing to fall back on is an error.
    Sync,
    /// List the cached entries, grouped by marketplace, with where each stands on this machine —
    /// a plain app's copies, or a bundle's installed elements as a fraction of what it declares.
    Apps,
    /// Install a bundle, or one `<kind>/<name>` element of it: clone its repository at the commit
    /// its manifest pins, and land the selection. A legacy single-dashboard bundle installs
    /// exactly as v1 always has — a dashboard of your own naming, inert until `start`.
    Install {
        /// Which bundle, or element: <marketplace>/<slug>, <marketplace>/<slug>/<kind>/<name>, or
        /// <marketplace>/<slug>/project for the scaffold. e.g. adi/crm or
        /// adi/crm-suite/agents/sales-bot.
        #[arg(value_name = "SPEC")]
        spec: String,
        /// What to call your copy, for a legacy single-dashboard bundle. It becomes the
        /// dashboard's name, its id and its hostname, and you can rename it later. Defaults to
        /// the name the marketplace published. Ignored for a general bundle: every element lands
        /// under its own published name.
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
        /// Start it as part of installing it, rather than leaving it inert. Only meaningful for a
        /// legacy single-dashboard bundle.
        #[arg(long)]
        start: bool,
    },
    /// Uninstall one element of a bundle: <marketplace>/<slug>/<kind>/<name>, by its own kind's
    /// ordinary path. There is no whole-bundle uninstall verb — a legacy single-dashboard bundle
    /// still goes through the Dashboards page (Archive → Delete), same as v1.
    Uninstall {
        #[arg(value_name = "MARKETPLACE/SLUG/KIND/NAME")]
        spec: String,
    },
    /// Start an installed app, or a parked hive service element: its servers (or its copied-in
    /// `ServiceSpec` block) come up within a few seconds of the supervisor's next read.
    Start {
        /// A dashboard id (as `apps` lists it) to start an app, or
        /// <marketplace>/<slug>/services/<name> to start a parked service element.
        #[arg(value_name = "ID_OR_MARKETPLACE/SLUG/services/NAME")]
        target: String,
    },
    /// Update an installed copy onto the commit its marketplace now pins — a fast-forward, so
    /// your own commits on top of it are never walked over.
    Update {
        /// A dashboard id (as `apps` lists it) for a legacy single-dashboard bundle, or
        /// <marketplace>/<slug> for a bundle installed from this design — the whole ledger is
        /// re-applied, per element.
        #[arg(value_name = "ID_OR_MARKETPLACE/SLUG")]
        target: String,
        /// Reset a legacy dashboard onto the pin, throwing away uncommitted changes and local
        /// commits. Ignored for a bundle spec — forcing there is per element (`--force-element`).
        #[arg(long)]
        force: bool,
        /// Force one element of a bundle update past its own local edit: <kind>/<name>,
        /// repeatable. Every element not named here fast-forwards, or is left alone and reported.
        #[arg(long = "force-element", value_name = "KIND/NAME")]
        force_element: Vec<String>,
    },
}

/// Run one marketplace verb.
///
/// # Errors
/// The sentence to print before a non-zero exit: every refusal the store can offer, in its own
/// words.
pub(crate) fn run_marketplace(command: MarketplaceCommand) -> Result<(), String> {
    let market = Marketplace::open();
    match command {
        MarketplaceCommand::Add { name, url } => {
            let source = adi_marketplace::sources::add(market.config(), &name, &url)
                .map_err(|e| e.to_string())?;
            println!("added {} → {}", source.name, source.url);
            println!("sync it now: adi-mono marketplace sync");
            Ok(())
        }
        MarketplaceCommand::Remove { name } => {
            if adi_marketplace::sources::remove(market.config(), &name)
                .map_err(|e| e.to_string())?
            {
                println!("removed {name} (and its cached manifest)");
                Ok(())
            } else {
                Err(format!("no marketplace named {name}"))
            }
        }
        MarketplaceCommand::List => {
            list_sources(&market);
            Ok(())
        }
        MarketplaceCommand::Sync => sync(&market),
        MarketplaceCommand::Apps => {
            list_apps(&market);
            Ok(())
        }
        MarketplaceCommand::Install { spec, name, start } => {
            install(&market, &spec, name.as_deref().unwrap_or(""), start)
        }
        MarketplaceCommand::Uninstall { spec } => uninstall(&market, &spec),
        MarketplaceCommand::Start { target } => start_target(&market, &target),
        MarketplaceCommand::Update { target, force, force_element } => {
            update_target(&market, &target, force, &force_element)
        }
    }
}

/// `list`: one line per source — where it points, and whether what is cached is fresh.
fn list_sources(market: &Marketplace) {
    let states = adi_marketplace::source_states(market.config());
    if states.is_empty() {
        println!("no marketplaces configured — add one:");
        println!("  adi-mono marketplace add <name> <https://manifest-url>");
        return;
    }
    for state in states {
        let freshness = match (state.synced_at, &state.error) {
            (None, _) => "never synced".to_string(),
            (Some(at), Some(error)) => {
                format!(
                    "stale — synced {}, and the fetch since failed: {error}",
                    ago(at)
                )
            }
            (Some(at), None) => format!("synced {}", ago(at)),
        };
        println!("{}  {}  ({})", state.name, state.url, freshness);
    }
}

/// `sync`: fetch every source, one line each; a source with nothing to fall back on fails the run.
fn sync(market: &Marketplace) -> Result<(), String> {
    let results = adi_marketplace::sync::sync(market).map_err(|e| e.to_string())?;
    let mut failed = false;
    for result in &results {
        println!("{}", result.summary());
        failed |= !result.has_listing();
    }
    if failed {
        return Err(
            "some marketplaces could not be fetched and had no cache to fall back on".to_string(),
        );
    }
    Ok(())
}

/// `apps`: the cached entries grouped by marketplace, each with where it stands here.
fn list_apps(market: &Marketplace) {
    let apps = adi_marketplace::install::cached_apps(market.config());
    if apps.is_empty() {
        // A cached marketplace with zero apps is not "nothing cached" — an empty list is a
        // valid marketplace (the starter repo), and telling somebody to add and sync when
        // they just did is a wrong answer. Name the sources that are standing instead.
        let cached = adi_marketplace::source_states(market.config())
            .iter()
            .filter(|state| state.synced_at.is_some())
            .count();
        if cached == 0 {
            println!("nothing cached yet — add a marketplace and run `adi-mono marketplace sync`");
        } else {
            println!("{cached} marketplace(s) cached, no apps between them yet");
        }
        return;
    }
    let mut current = String::new();
    for app in apps {
        if app.marketplace != current {
            current.clone_from(&app.marketplace);
            println!("{current}:");
        }
        let version = app
            .version
            .as_deref()
            .map(|v| format!(" {v}"))
            .unwrap_or_default();
        println!(
            "  {}/{}  {}{version} — {} @ {}",
            app.marketplace,
            app.slug,
            app.name,
            app.repo,
            adi_marketplace::git::short(&app.commit)
        );
        if let Some(description) = &app.description {
            println!("    {description}");
        }
        // What the publisher filed it under, on its own line — the panel draws these as tags, and
        // a terminal that did not print them would be the door that knows less.
        if !app.keywords.is_empty() {
            println!("    {}", app.keywords.join(" · "));
        }
        // The long form and the gallery are a page's business, not a listing's — so this says
        // they exist and where to read them, rather than printing markdown into a terminal.
        let more = [
            app.readme.is_some().then_some("a description".to_string()),
            (!app.gallery.is_empty()).then(|| match app.gallery.len() {
                1 => "1 picture or clip".to_string(),
                n => format!("{n} pictures and clips"),
            }),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        if !more.is_empty() {
            println!(
                "    {} — http://app.adi/marketplace/{}/{}",
                more.join(", "),
                app.marketplace,
                app.slug
            );
        }
        // The manifest's own preview and this bundle's live status — a general bundle, or a
        // legacy single-dashboard one that simply has nothing to show here.
        let entry = adi_marketplace::entry_of(market.config(), &app.marketplace, &app.slug).ok();
        let status = entry
            .as_ref()
            .and_then(|e| adi_marketplace::bundle::status(market, &app.marketplace, &app.slug, e));
        if let Some(entry) = &entry
            && !entry.elements().is_empty()
        {
            print_bundle_status(&app.marketplace, &app.slug, entry, status.as_ref());
        }

        if app.installs.is_empty() {
            if status.is_none() {
                println!("    not installed");
            }
            continue;
        }
        // One line per copy: the same app installed twice under two names is ordinary, and the
        // listing has to show which of them is behind.
        for install in &app.installs {
            let standing = if install.started {
                match install.host.as_deref() {
                    Some(host) => format!("running at {host}"),
                    None => "running".to_string(),
                }
            } else {
                "installed, not started".to_string()
            };
            let behind = if install.outdated {
                format!(
                    " — behind; `adi-mono marketplace update {}` moves it onto {}",
                    install.id,
                    adi_marketplace::git::short(&app.commit)
                )
            } else {
                String::new()
            };
            println!(
                "    {} “{}” @ {} — {standing}{behind}",
                install.id,
                install.name,
                adi_marketplace::git::short(&install.commit)
            );
        }
    }
}

/// A general bundle's own line in the listing: installed as a fraction of what the manifest's
/// preview declares, which elements under what ids, whether it is behind the pin, and any secret
/// its installed elements still lack — the panel's own "installed as a fraction," in text.
fn print_bundle_status(
    marketplace: &str,
    slug: &str,
    entry: &adi_marketplace::BundleEntry,
    status: Option<&adi_marketplace::BundleStatus>,
) {
    let declared = entry.elements().len();
    let Some(status) = status else {
        println!("    0 of {declared} elements installed");
        return;
    };
    println!("    {} of {declared} elements installed", status.installed.len());
    for element in &status.installed {
        println!("      {}/{} → {}", element.kind, element.name, element.id);
    }
    if status.outdated {
        println!(
            "    behind — `adi-mono marketplace update {marketplace}/{slug}` moves it onto the current pin"
        );
    }
    if !status.missing_secrets.is_empty() {
        println!(
            "    missing secret(s): {} — set them, or the elements that name them will not work",
            status.missing_secrets.join(", ")
        );
    }
}

/// `install`: install a bundle, or one element of it. A legacy single-dashboard bundle installs
/// exactly as v1 always has; a general bundle lands every element the spec selects and reports
/// each one's own outcome.
fn install(market: &Marketplace, spec: &str, name: &str, start: bool) -> Result<(), String> {
    match adi_marketplace::bundle::install(market, spec, name, start).map_err(|e| e.to_string())? {
        adi_marketplace::BundleOutcome::Legacy(done) => {
            let dir = market.dashboards_dir().join(&done.id);
            println!(
                "installed “{}” as {} at {} → {}",
                done.name,
                done.id,
                adi_marketplace::git::short(&done.commit),
                dir.display()
            );
            for note in &done.notes {
                println!("  note: {note}");
            }
            if done.started {
                println!(
                    "started — http://{} (a few seconds for the servers to come up)",
                    done.host
                );
            } else {
                println!(
                    "not started — run it when you want it: adi-mono marketplace start {}",
                    done.id
                );
            }
            println!(
                "it is a clone: edit it in place, commit, and `git -C {} pull` when you want the \
                 publisher's newer work",
                dir.display()
            );
        }
        adi_marketplace::BundleOutcome::Bundle(done) => {
            println!("{}/{}:", done.marketplace, done.slug);
            for element in &done.elements {
                match (&element.id, &element.note) {
                    (Some(id), _) if !element.renamed => {
                        println!("  {}/{} → {id}", element.kind, element.name);
                    }
                    (Some(id), _) => println!("  {}/{} → {id} (renamed)", element.kind, element.name),
                    (None, Some(note)) => println!("  {}/{}: {note}", element.kind, element.name),
                    (None, None) => println!("  {}/{}: did not land", element.kind, element.name),
                }
            }
            for note in &done.notes {
                println!("  note: {note}");
            }
        }
    }
    Ok(())
}

/// `uninstall`: remove one element of a bundle by its own kind's ordinary path.
fn uninstall(market: &Marketplace, spec: &str) -> Result<(), String> {
    let done = adi_marketplace::bundle::uninstall_element(market, spec).map_err(|e| e.to_string())?;
    println!("uninstalled {}/{} ({})", done.kind, done.name, done.id);
    if done.bundle_removed {
        println!("  the last element from this bundle — its own record is gone too");
    }
    if let Some(note) = &done.note {
        println!("  note: {note}");
    }
    Ok(())
}

/// `start`: a dashboard id starts an app; a `<marketplace>/<slug>/services/<name>` address starts
/// a parked hive service element instead.
fn start_target(market: &Marketplace, target: &str) -> Result<(), String> {
    if target.contains('/') {
        let started = adi_marketplace::bundle::start_service(market, target).map_err(|e| e.to_string())?;
        let where_ = match &started.project {
            Some(project) => format!("into {project}'s own hive.yaml"),
            None => "into the global hive.yaml".to_string(),
        };
        println!(
            "started {} ({}) — copied {where_}; the supervisor picks it up on its next read",
            started.name, started.id
        );
        return Ok(());
    }
    let started = adi_marketplace::install::start(market, target).map_err(|e| e.to_string())?;
    println!(
        "started {} — http://{} (a few seconds for the servers to come up)",
        started.id, started.host
    );
    Ok(())
}

/// `update`: a dashboard id fast-forwards a legacy copy onto the commit its marketplace pins now,
/// exactly as v1 always has; a `<marketplace>/<slug>` address re-applies a general bundle's whole
/// ledger instead, per element.
fn update_target(market: &Marketplace, target: &str, force: bool, force_element: &[String]) -> Result<(), String> {
    let short = adi_marketplace::git::short;
    if let Some((marketplace, slug)) = target.split_once('/') {
        let pairs: Vec<(Kind, &str)> = force_element.iter().filter_map(|f| force_pair(f)).collect();
        let done = adi_marketplace::bundle::update(market, marketplace, slug, &pairs)
            .map_err(|e| e.to_string())?;
        if done.changed {
            println!("{}/{} updated from {} to {}:", done.marketplace, done.slug, short(&done.from), short(&done.to));
            for element in &done.elements {
                match (&element.note, element.changed) {
                    (Some(note), _) => println!("  {}/{}: {note}", element.kind, element.name),
                    (None, true) => println!("  {}/{} → {}", element.kind, element.name, element.id),
                    (None, false) => println!("  {}/{} unchanged", element.kind, element.name),
                }
            }
        } else {
            println!(
                "{}/{} is already at {}, the commit its marketplace pins",
                done.marketplace,
                done.slug,
                short(&done.to)
            );
        }
        return Ok(());
    }
    let done = adi_marketplace::install::update(market, target, force).map_err(|e| e.to_string())?;
    if done.changed {
        println!("updated {} from {} to {}", done.id, short(&done.from), short(&done.to));
        if done.started {
            println!("it is running, so bun has already reloaded it");
        }
    } else {
        println!(
            "{} is already at {}, the commit its marketplace pins",
            done.id,
            short(&done.to)
        );
    }
    Ok(())
}

/// `<kind>/<name>` onto the pair [`adi_marketplace::bundle::update`] takes — silently dropped
/// when it does not parse, so a stray `--force-element` value forces nothing rather than refusing
/// the whole update.
fn force_pair(raw: &str) -> Option<(Kind, &str)> {
    let (kind, name) = raw.split_once('/')?;
    Some((Kind::from_dir(kind)?, name))
}

/// Unix seconds as a person reads the gap: `just now`, `5m ago`, `3h ago`, `4d ago`.
fn ago(at: u64) -> String {
    let elapsed = adi_config::now_unix().saturating_sub(at);
    match elapsed {
        0..=59 => "just now".to_string(),
        secs if secs < 3600 => format!("{}m ago", secs / 60),
        secs if secs < 86_400 => format!("{}h ago", secs / 3600),
        secs => format!("{}d ago", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// The argv surface, as the wiring test in `main.rs` pins the group's reachability.
    #[derive(Debug, Parser)]
    struct Cli {
        #[command(subcommand)]
        command: MarketplaceCommand,
    }

    #[test]
    fn the_verbs_parse_as_documented() {
        let cli = Cli::try_parse_from(["marketplace", "add", "adi", "https://example/m.json"])
            .expect("add parses");
        assert!(matches!(
            cli.command,
            MarketplaceCommand::Add { ref name, ref url } if name == "adi" && url.contains("https://")
        ));

        let cli = Cli::try_parse_from(["marketplace", "install", "adi/crm", "--name", "Sales CRM"])
            .expect("install");
        assert!(matches!(
            cli.command,
            MarketplaceCommand::Install { ref spec, ref name, start: false }
                if spec == "adi/crm" && name.as_deref() == Some("Sales CRM")
        ));
        // The name is optional — the entry's own is the default — and starting is opt-in.
        let cli = Cli::try_parse_from(["marketplace", "install", "adi/crm", "--start"])
            .expect("install");
        assert!(matches!(
            cli.command,
            MarketplaceCommand::Install { ref name, start: true, .. } if name.is_none()
        ));

        let cli = Cli::try_parse_from(["marketplace", "start", "crm"]).expect("start");
        assert!(matches!(cli.command, MarketplaceCommand::Start { ref target } if target == "crm"));
        let cli = Cli::try_parse_from(["marketplace", "start", "adi/crm-suite/services/redis"])
            .expect("start a service element");
        assert!(matches!(
            cli.command,
            MarketplaceCommand::Start { ref target } if target == "adi/crm-suite/services/redis"
        ));

        let cli =
            Cli::try_parse_from(["marketplace", "update", "crm", "--force"]).expect("update");
        assert!(matches!(
            cli.command,
            MarketplaceCommand::Update { ref target, force: true, ref force_element }
                if target == "crm" && force_element.is_empty()
        ));
        let cli = Cli::try_parse_from([
            "marketplace",
            "update",
            "adi/crm-suite",
            "--force-element",
            "agents/sales-bot",
            "--force-element",
            "tools/csv-import",
        ])
        .expect("bundle update with per-element force");
        assert!(matches!(
            cli.command,
            MarketplaceCommand::Update { ref target, force: false, ref force_element }
                if target == "adi/crm-suite"
                    && force_element == &["agents/sales-bot".to_string(), "tools/csv-import".to_string()]
        ));

        let cli = Cli::try_parse_from(["marketplace", "uninstall", "adi/crm-suite/agents/sales-bot"])
            .expect("uninstall");
        assert!(matches!(
            cli.command,
            MarketplaceCommand::Uninstall { ref spec } if spec == "adi/crm-suite/agents/sales-bot"
        ));

        for verb in ["list", "sync", "apps"] {
            assert!(Cli::try_parse_from(["marketplace", verb]).is_ok(), "{verb}");
        }
        assert!(Cli::try_parse_from(["marketplace", "remove", "adi"]).is_ok());
        // Only the flags above exist; nothing else is.
        assert!(Cli::try_parse_from(["marketplace", "install", "adi/crm", "--yes"]).is_err());
    }

    #[test]
    fn ago_reads_as_a_person_reads_a_gap() {
        let now = adi_config::now_unix();
        assert_eq!(ago(now), "just now");
        assert_eq!(ago(now - 5 * 60), "5m ago");
        assert_eq!(ago(now - 3 * 3600), "3h ago");
        assert_eq!(ago(now - 4 * 86_400), "4d ago");
        assert_eq!(ago(now + 600), "just now", "a clock behind ours saturates");
    }
}
