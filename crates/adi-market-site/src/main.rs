//! `adi-market-site` — build the public marketplace, or look at it before publishing it.
//!
//! ```text
//! adi-market-site build --source adi=apps/marketplace.json \
//!                       --source-url adi=https://raw.githubusercontent.com/…/marketplace.json \
//!                       --base-url https://marketplace.withadi.dev --out dist
//! adi-market-site serve --source adi=apps/marketplace.json
//! ```
//!
//! The input is the manifest **file** a publisher already has in the repository the manifest lives
//! in — not a machine's synced cache. A public listing is published by whoever publishes the
//! manifest, and building it from somebody's local cache would publish whatever that machine last
//! managed to fetch.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use adi_market_site::{Site, Source};
use anyhow::{Context, bail};
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "adi-market-site",
    about = "The public marketplace: plain HTML generated from a marketplace manifest"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Write the site into a directory.
    Build {
        #[command(flatten)]
        input: Input,
        /// Where the site will be published, e.g. `https://marketplace.withadi.dev`. Every
        /// canonical URL, the sitemap and the structured data are built from it.
        #[arg(long)]
        base_url: String,
        /// The directory to write into. Existing files are overwritten; nothing is deleted.
        #[arg(long, default_value = "dist")]
        out: PathBuf,
    },
    /// Build the site in memory and serve it on loopback, to look at.
    Serve {
        #[command(flatten)]
        input: Input,
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        #[arg(long, default_value_t = 9085)]
        port: u16,
        /// The base URL to render canonical addresses with. Defaults to the address being served,
        /// so a preview's `<head>` is checkable without pretending to be the published site.
        #[arg(long)]
        base_url: Option<String>,
    },
}

/// What both commands read: the manifests, and what to call the site they make.
#[derive(clap::Args, Debug)]
struct Input {
    /// A marketplace to publish, as `<name>=<path to its manifest.json>`. Repeatable — one site
    /// can carry several. The name is the first half of every install address.
    #[arg(long = "source", required = true, value_name = "NAME=PATH")]
    sources: Vec<String>,
    /// Where a source's manifest is published, as `<name>=<url>`. It is what lets a page print
    /// the `marketplace add` line, so a reader who already runs adi can follow the listing.
    #[arg(long = "source-url", value_name = "NAME=URL")]
    source_urls: Vec<String>,
    /// What this marketplace is called on its own pages.
    #[arg(long, default_value = "ADI marketplace")]
    name: String,
}

impl Input {
    /// Read every manifest named on the command line.
    fn read(&self) -> anyhow::Result<Vec<Source>> {
        let mut sources = Vec::new();
        for spec in &self.sources {
            let (name, path) = pair(spec, "--source")?;
            let url = self
                .source_urls
                .iter()
                .map(|spec| pair(spec, "--source-url"))
                .collect::<anyhow::Result<Vec<_>>>()?
                .into_iter()
                .find(|(other, _)| *other == name)
                .map(|(_, url)| url.to_string());
            sources.push(
                Source::read(name, &PathBuf::from(path), url)
                    .with_context(|| format!("the {name} marketplace"))?,
            );
        }
        Ok(sources)
    }
}

/// Split a `name=value` argument, saying which flag was wrong when it is not one.
fn pair<'a>(spec: &'a str, flag: &str) -> anyhow::Result<(&'a str, &'a str)> {
    match spec.split_once('=') {
        Some((name, value)) if !name.is_empty() && !value.is_empty() => Ok((name, value)),
        _ => bail!("{flag} wants <name>=<value>, not {spec:?}"),
    }
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Build {
            input,
            base_url,
            out,
        } => {
            let sources = input.read()?;
            let site = Site {
                name: input.name.clone(),
                base_url: base_url.trim_end_matches('/').to_string(),
            };
            let files = adi_market_site::render(&site, &sources);
            adi_market_site::write(&out, &files)
                .with_context(|| format!("writing into {}", out.display()))?;
            let pages = sources
                .iter()
                .map(|source| source.manifest.bundles.len())
                .sum::<usize>();
            println!(
                "{} files into {} \u{2014} the shelf and {pages} item page(s)",
                files.len(),
                out.display()
            );
            Ok(())
        }
        Command::Serve {
            input,
            host,
            port,
            base_url,
        } => {
            let sources = input.read()?;
            let addr = SocketAddr::new(host, port);
            let site = Site {
                name: input.name.clone(),
                base_url: base_url.unwrap_or_else(|| format!("http://{addr}")),
            };
            let files = adi_market_site::render(&site, &sources);
            adi_market_site::serve::run(addr, files)
                .with_context(|| format!("serving on {addr}"))
        }
    }
}
