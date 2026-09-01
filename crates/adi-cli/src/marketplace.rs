//! The `marketplace` command group: manage manifest sources, sync them, install and start apps.
//!
//! No `adi` facade, like `mesh` and `indexer`: the state is the marketplace module's own under
//! the store, and the handle opens it itself.

use adi_marketplace::Marketplace;
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
    /// List the cached entries, grouped by marketplace, with where each stands on this machine.
    Apps,
    /// Install an app: land its files under dashboards/<slug>/, started nothing. Installed is
    /// not started — `start` is the act that runs it.
    Install {
        /// Which app: <marketplace>/<slug>, e.g. adi/crm.
        #[arg(value_name = "MARKETPLACE/SLUG")]
        spec: String,
        /// Replace an already-installed app's files in place, keeping its run state.
        #[arg(long)]
        force: bool,
    },
    /// Start an installed app: its servers come up on leased ports within a few seconds.
    Start {
        /// The app to start — <marketplace>/<slug> or the bare slug.
        #[arg(value_name = "SLUG")]
        spec: String,
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
        MarketplaceCommand::Install { spec, force } => install(&market, &spec, force),
        MarketplaceCommand::Start { spec } => start(&market, &spec),
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
        println!("nothing cached yet — add a marketplace and run `adi-mono marketplace sync`");
        return;
    }
    let mut current = String::new();
    for app in apps {
        if app.marketplace != current {
            current.clone_from(&app.marketplace);
            println!("{current}:");
        }
        let standing = if app.started {
            match app.host.as_deref() {
                Some(host) => format!("running at {host}"),
                None => "running".to_string(),
            }
        } else if app.installed {
            "installed, not started".to_string()
        } else {
            "not installed".to_string()
        };
        let version = app
            .version
            .as_deref()
            .map(|v| format!(" {v}"))
            .unwrap_or_default();
        println!(
            "  {}/{}  {}{version} — {standing}",
            app.marketplace, app.slug, app.name
        );
        if let Some(description) = &app.description {
            println!("    {description}");
        }
    }
}

/// `install`: land the app, then say where, and what the next deliberate act is.
fn install(market: &Marketplace, spec: &str, force: bool) -> Result<(), String> {
    let done = adi_marketplace::install::install(market, spec, force).map_err(|e| e.to_string())?;
    let dir = market.dashboards_dir().join(&done.slug);
    if done.started {
        println!(
            "reinstalled {} → {} (kept running at {})",
            done.slug,
            dir.display(),
            done.host
        );
    } else {
        println!("installed {} → {}", done.slug, dir.display());
        println!(
            "not started — run it when you want it: adi-mono marketplace start {}",
            done.slug
        );
    }
    Ok(())
}

/// `start`: move the app into the supervisor's glob and say where it will answer.
fn start(market: &Marketplace, spec: &str) -> Result<(), String> {
    let started = adi_marketplace::install::start(market, spec).map_err(|e| e.to_string())?;
    println!(
        "started {} — http://{} (a few seconds for the servers to come up)",
        started.slug, started.host
    );
    Ok(())
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

        let cli =
            Cli::try_parse_from(["marketplace", "install", "adi/crm", "--force"]).expect("install");
        assert!(matches!(
            cli.command,
            MarketplaceCommand::Install { ref spec, force: true } if spec == "adi/crm"
        ));

        let cli = Cli::try_parse_from(["marketplace", "start", "crm"]).expect("start");
        assert!(matches!(cli.command, MarketplaceCommand::Start { ref spec } if spec == "crm"));

        for verb in ["list", "sync", "apps"] {
            assert!(Cli::try_parse_from(["marketplace", verb]).is_ok(), "{verb}");
        }
        assert!(Cli::try_parse_from(["marketplace", "remove", "adi"]).is_ok());
        // The one flag that exists is opt-in; nothing else is.
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
