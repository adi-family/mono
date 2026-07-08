//! `adi-mcp` — the adi platform MCP server. Agents spawn it over stdio and speak the Model
//! Context Protocol over its stdin/stdout; the `--features` flag scopes which tool groups it
//! exposes (see [`adi_mcp::FeatureSet`]).
//!
//! ```text
//! adi-mcp                       # every tool group (default: --features all)
//! adi-mcp --features "tasks"    # only the tasks_* tools
//! adi-mcp --list-features       # print the available groups and exit
//! ```
//!
//! Registering with Claude Code, for example:
//! `claude mcp add adi -- /path/to/adi-mcp --features "tasks,projects,files"`.

use adi_mcp::{AdiMcp, Feature, FeatureSet};
use anyhow::Result;
use clap::Parser;
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::EnvFilter;

/// The adi platform MCP server (stdio transport).
#[derive(Debug, Parser)]
#[command(name = "adi-mcp", version, about)]
struct Cli {
    /// Comma-separated tool groups to expose, e.g. `tasks,projects`. `all` (the default)
    /// enables every group. Run with `--list-features` to see them.
    #[arg(long, value_name = "LIST", default_value = "all")]
    features: String,

    /// Print the available tool features and exit.
    #[arg(long)]
    list_features: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.list_features {
        print_features();
        return Ok(());
    }

    // Logs MUST go to stderr — stdout carries the JSON-RPC stream and cannot be polluted.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let features = FeatureSet::parse(&cli.features).map_err(|e| anyhow::anyhow!(e))?;
    if features.is_empty() {
        anyhow::bail!(
            "no tool features enabled; pass --features with at least one of: {}",
            feature_names()
        );
    }

    let enabled = features
        .iter()
        .map(Feature::name)
        .collect::<Vec<_>>()
        .join(", ");
    tracing::info!("starting adi-mcp over stdio; features: {enabled}");

    let service = AdiMcp::new(features)
        .serve(stdio())
        .await
        .inspect_err(|e| tracing::error!("failed to start MCP server: {e:?}"))?;
    service.waiting().await?;
    Ok(())
}

/// Print the available tool features and their tools (for `--list-features`).
fn print_features() {
    println!("Available adi-mcp tool features (and their tools):\n");
    for f in Feature::ALL {
        println!("  {:9} {}", f.name(), f.summary());
        println!("  {:9}   tools: {}", "", f.tools().join(", "));
    }
    println!(
        "\nSelect whole groups, or specific tools with a [tool,...] selector:\n  \
         adi-mcp --features \"{all}\"\n  \
         adi-mcp --features \"tasks[create,list],files[read],status\"",
        all = feature_names_csv()
    );
}

/// Comma+space separated feature names (for error messages).
fn feature_names() -> String {
    Feature::ALL
        .into_iter()
        .map(Feature::name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Comma separated feature names (for the example command line).
fn feature_names_csv() -> String {
    Feature::ALL
        .into_iter()
        .map(Feature::name)
        .collect::<Vec<_>>()
        .join(",")
}
