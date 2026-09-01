//! The `indexer` command group: parse a project into an index of its symbols, then ask that
//! index questions — by meaning, by name, by path, or as a tree.
//!
//! Every subcommand works on one project directory: `--path` if given, the current directory
//! otherwise. The index lives in that directory's `.adi/tree`.
//!
//! Unlike the rest of this CLI, the group does not go through the [`Adi`](adi_core::Adi) facade
//! — an index is per-project state under the project itself, not platform state under
//! `~/.adi/mono`, and nothing else in the platform reads it.

use std::path::{Path, PathBuf};

use adi_indexer::{Indexer, Symbol, SymbolNode};
use clap::Subcommand;

use crate::format::print_json;

#[derive(Debug, Subcommand)]
pub(crate) enum IndexerCommand {
    /// Index a project: parse every supported file, store its symbols and references, and embed
    /// them for semantic search. Re-running only re-does what changed.
    Index {
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Semantic search: rank indexed symbols by what the query means, not how it is spelled.
    Search {
        query: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Search symbol names and doc comments (full-text).
    Symbols {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Search indexed file paths.
    Files {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Find symbols like a given one — by what the code means, or with `--structural`, by what
    /// its syntax looks like once the names are stripped out.
    Similar {
        /// Name of the symbol to match against. If several symbols share it, the largest is
        /// used and the choice is reported.
        symbol: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Match on syntax shape rather than meaning.
        #[arg(long)]
        structural: bool,
        /// With `--structural`: how many bits of the 64-bit shape fingerprint may differ.
        /// Unrelated code sits near 32.
        #[arg(long, default_value_t = 6)]
        distance: u32,
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Report duplicated code: groups of symbols with the same syntax shape, names and literal
    /// values ignored.
    Clones {
        /// Smallest symbol to consider, in parse-tree nodes. Below roughly 40 every codebase
        /// has thousands of accessors that share a shape and mean nothing by it.
        #[arg(long, default_value_t = 40)]
        min_nodes: u32,
        /// Also group symbols that are merely *close* in shape, within this many bits — the
        /// copies that have since drifted. 0 keeps the report to exact matches only.
        #[arg(long, default_value_t = 0)]
        distance: u32,
        /// Include containers — modules, classes, traits. Off by default: a container's shape
        /// is its children's concatenated, so it repeats what they already said and, being the
        /// largest symbol around, says it first.
        #[arg(long)]
        all_kinds: bool,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Show what is in the index: file and symbol counts, and the embedding model behind them.
    Status {
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Print the indexed symbol tree, file by file.
    Tree {
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// List the languages this build can parse. Grammars are compiled in (see adi-indexer's
    /// `lang-*` features), so this is a property of the binary, not of the machine.
    Languages {
        #[arg(long)]
        json: bool,
    },
}

/// Dispatch an `indexer` subcommand.
///
/// Opening an index loads the embedding model, which is slow enough to be worth doing once and
/// blocking on — so each arm builds the runtime it needs rather than `main` wrapping everything.
pub(crate) fn run_indexer(command: IndexerCommand) -> Result<(), String> {
    match command {
        IndexerCommand::Languages { json } => {
            languages(json);
            Ok(())
        }
        IndexerCommand::Index { path, json } => index(path.as_deref(), json),
        IndexerCommand::Search {
            query,
            limit,
            path,
            json,
        } => search(&query, limit, path.as_deref(), json),
        IndexerCommand::Symbols {
            query,
            limit,
            path,
            json,
        } => symbols(&query, limit, path.as_deref(), json),
        IndexerCommand::Files {
            query,
            limit,
            path,
            json,
        } => files(&query, limit, path.as_deref(), json),
        IndexerCommand::Similar {
            symbol,
            limit,
            structural,
            distance,
            path,
            json,
        } => similar(&symbol, limit, structural, distance, path.as_deref(), json),
        IndexerCommand::Clones {
            min_nodes,
            distance,
            all_kinds,
            limit,
            path,
            json,
        } => clones(min_nodes, distance, all_kinds, limit, path.as_deref(), json),
        IndexerCommand::Status { path, json } => status(path.as_deref(), json),
        IndexerCommand::Tree { path, json } => tree(path.as_deref(), json),
    }
}

fn languages(json: bool) {
    let languages: Vec<&str> = adi_indexer::lang::supported()
        .iter()
        .map(adi_indexer::Language::as_str)
        .collect();

    if json {
        print_json(&languages);
    } else {
        for language in languages {
            println!("{language}");
        }
    }
}

fn index(path: Option<&Path>, json: bool) -> Result<(), String> {
    let indexer = open(path)?;
    let progress = block_on(indexer.index()).map_err(|e| format!("indexing failed: {e}"))?;

    if json {
        print_json(&progress);
        return Ok(());
    }

    println!(
        "Indexed {} / {} files, {} symbols",
        progress.files_processed, progress.files_total, progress.symbols_indexed
    );
    if !progress.errors.is_empty() {
        println!("{} error(s):", progress.errors.len());
        for e in progress.errors.iter().take(10) {
            println!("  - {e}");
        }
        if progress.errors.len() > 10 {
            println!("  ... and {} more", progress.errors.len() - 10);
        }
    }
    Ok(())
}

fn search(query: &str, limit: usize, path: Option<&Path>, json: bool) -> Result<(), String> {
    let indexer = open(path)?;
    let results =
        block_on(indexer.search(query, limit)).map_err(|e| format!("search failed: {e}"))?;

    if json {
        print_json(&results);
        return Ok(());
    }
    if results.is_empty() {
        println!("No results found.");
        return Ok(());
    }

    println!("{} result(s):", results.len());
    for r in &results {
        let s = &r.symbol;
        println!(
            "  {:.3}  {} {} ({}:{})",
            r.score,
            s.kind.as_str(),
            s.name,
            s.file_path.display(),
            s.location.start_line
        );
        if let Some(sig) = &s.signature {
            println!("        {sig}");
        }
    }
    Ok(())
}

fn symbols(query: &str, limit: usize, path: Option<&Path>, json: bool) -> Result<(), String> {
    let indexer = open(path)?;
    let symbols = block_on(indexer.search_symbols(query, limit))
        .map_err(|e| format!("symbol search failed: {e}"))?;

    if json {
        print_json(&symbols);
        return Ok(());
    }
    if symbols.is_empty() {
        println!("No symbols found.");
        return Ok(());
    }

    println!("{} symbol(s):", symbols.len());
    for s in &symbols {
        println!(
            "  {} {} ({}:{})",
            s.kind.as_str(),
            s.name,
            s.file_path.display(),
            s.location.start_line
        );
    }
    Ok(())
}

fn files(query: &str, limit: usize, path: Option<&Path>, json: bool) -> Result<(), String> {
    let indexer = open(path)?;
    let files = block_on(indexer.search_files(query, limit))
        .map_err(|e| format!("file search failed: {e}"))?;

    if json {
        print_json(&files);
        return Ok(());
    }
    if files.is_empty() {
        println!("No files found.");
        return Ok(());
    }

    println!("{} file(s):", files.len());
    for f in &files {
        println!("  {} [{}]", f.path.display(), f.language.as_str());
    }
    Ok(())
}

fn similar(
    symbol: &str,
    limit: usize,
    structural: bool,
    distance: u32,
    path: Option<&Path>,
    json: bool,
) -> Result<(), String> {
    let indexer = open(path)?;
    let target = resolve_symbol(&indexer, symbol)?;

    if structural {
        let matches = indexer
            .similar_structure(target.id, distance, limit)
            .map_err(|e| format!("structural search failed: {e}"))?;

        if json {
            print_json(&matches);
            return Ok(());
        }
        if matches.is_empty() {
            println!("Nothing within {distance} bits of {symbol}.");
            return Ok(());
        }

        println!("{} match(es) by shape:", matches.len());
        for (row, bits) in &matches {
            println!(
                "  {bits:>2} bits  {} {} ({}:{})",
                row.kind.as_str(),
                row.name,
                row.file_path.display(),
                row.start_line
            );
        }
        return Ok(());
    }

    let results = indexer
        .similar(target.id, limit)
        .map_err(|e| format!("similarity search failed: {e}"))?;

    if json {
        print_json(&results);
        return Ok(());
    }
    if results.is_empty() {
        println!(
            "No neighbours for {symbol} — it has no embedding (a build without the candle \
             feature stores none)."
        );
        return Ok(());
    }

    println!("{} match(es) by meaning:", results.len());
    for r in &results {
        let s = &r.symbol;
        println!(
            "  {:.3}  {} {} ({}:{})",
            r.score,
            s.kind.as_str(),
            s.name,
            s.file_path.display(),
            s.location.start_line
        );
    }
    Ok(())
}

fn clones(
    min_nodes: u32,
    distance: u32,
    all_kinds: bool,
    limit: usize,
    path: Option<&Path>,
    json: bool,
) -> Result<(), String> {
    let indexer = open(path)?;

    let groups = if distance == 0 {
        indexer.clones(min_nodes, all_kinds)
    } else {
        indexer.near_clones(min_nodes, distance, all_kinds)
    }
    .map_err(|e| format!("clone detection failed: {e}"))?;

    let total = groups.len();
    let shown: Vec<_> = groups.into_iter().take(limit).collect();

    if json {
        print_json(&shown);
        return Ok(());
    }
    if shown.is_empty() {
        println!("No duplicated symbols of at least {min_nodes} nodes.");
        return Ok(());
    }

    let duplicated: usize = shown.iter().map(|g| g.members.len()).sum();
    println!(
        "{total} group(s), {duplicated} symbols{}:",
        if total > shown.len() {
            format!(" (showing {})", shown.len())
        } else {
            String::new()
        }
    );
    for group in &shown {
        let head = &group.members[0];
        let drift = if group.distance == 0 {
            String::new()
        } else {
            format!(", up to {} bits apart", group.distance)
        };
        println!(
            "\n  {} copies, ~{} lines, {} nodes{drift}:",
            group.members.len(),
            head.line_count(),
            head.structure.node_count
        );
        for member in &group.members {
            println!(
                "    {} {} ({}:{})",
                member.kind.as_str(),
                member.name,
                member.file_path.display(),
                member.start_line
            );
        }
    }
    Ok(())
}

/// The symbol a name refers to.
///
/// A name can be held by several symbols — an overload, a trait method implemented twice, a
/// helper repeated per module. The largest wins, because that is the one a question about
/// duplication or similarity is nearly always about, and the choice is printed so it can be
/// seen and overridden.
fn resolve_symbol(indexer: &Indexer, name: &str) -> Result<Symbol, String> {
    let mut matches = indexer
        .find_symbols_by_name(name)
        .map_err(|e| format!("symbol lookup failed: {e}"))?;

    // Analyzers qualify a method with its type — `row_to_symbol` is stored as
    // `SqliteStorage::row_to_symbol` — but nobody types it that way. Fall back to matching the
    // last component, which is what the name on the screen looks like.
    if matches.is_empty() {
        let suffix = format!("::{name}");
        matches = block_on(indexer.search_symbols(name, 200))
            .map_err(|e| format!("symbol lookup failed: {e}"))?
            .into_iter()
            .filter(|s| s.name.ends_with(&suffix))
            .collect();
    }

    if matches.is_empty() {
        return Err(format!("no indexed symbol named {name}"));
    }

    matches.sort_by_key(|s| {
        std::cmp::Reverse(s.location.end_line.saturating_sub(s.location.start_line))
    });
    if matches.len() > 1 {
        let chosen = &matches[0];
        println!(
            "{} symbols named {name}; using {}:{}",
            matches.len(),
            chosen.file_path.display(),
            chosen.location.start_line
        );
    }
    Ok(matches.remove(0))
}

fn status(path: Option<&Path>, json: bool) -> Result<(), String> {
    let indexer = open(path)?;
    let status = indexer
        .status()
        .map_err(|e| format!("status failed: {e}"))?;

    if json {
        print_json(&status);
        return Ok(());
    }

    println!("Files:      {}", status.indexed_files);
    println!("Symbols:    {}", status.indexed_symbols);
    println!("Model:      {}", status.embedding_model);
    println!("Dimensions: {}", status.embedding_dimensions);
    println!("Storage:    {} bytes", status.storage_size_bytes);
    Ok(())
}

fn tree(path: Option<&Path>, json: bool) -> Result<(), String> {
    let indexer = open(path)?;
    let tree = indexer
        .get_tree()
        .map_err(|e| format!("tree failed: {e}"))?;

    if json {
        print_json(&tree);
        return Ok(());
    }
    if tree.files.is_empty() {
        println!("No indexed files.");
        return Ok(());
    }

    println!("{} file(s):", tree.files.len());
    for f in &tree.files {
        println!("{} [{}]", f.path.display(), f.language.as_str());
        for s in &f.symbols {
            print_node(s, 1);
        }
    }
    Ok(())
}

/// Open the index for `path`, or for the current directory when none was given.
fn open(path: Option<&Path>) -> Result<Indexer, String> {
    let project = match path {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir().map_err(|e| format!("no current directory: {e}"))?,
    };

    block_on(Indexer::open(&project)).map_err(|e| format!("could not open the index: {e}"))
}

/// Every entry point here does one bounded piece of work and then exits, so each builds its own
/// runtime instead of `main` holding one open for subcommands that never touch it.
fn block_on<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(future)
}

fn print_node(node: &SymbolNode, depth: usize) {
    println!("{}{} {}", "  ".repeat(depth), node.kind.as_str(), node.name);
    for child in &node.children {
        print_node(child, depth + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    /// Parse an argv fragment the way `adi-mono indexer …` would reach this group.
    #[derive(Debug, clap::Parser)]
    struct Harness {
        #[command(subcommand)]
        command: IndexerCommand,
    }

    fn parse(args: &[&str]) -> IndexerCommand {
        Harness::try_parse_from(std::iter::once("indexer").chain(args.iter().copied()))
            .expect("parses")
            .command
    }

    #[test]
    fn search_takes_a_query_and_defaults_its_limit() {
        match parse(&["search", "retry failed uploads"]) {
            IndexerCommand::Search { query, limit, .. } => {
                assert_eq!(query, "retry failed uploads");
                assert_eq!(limit, 10);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn a_query_is_required() {
        assert!(Harness::try_parse_from(["indexer", "search"]).is_err());
    }

    #[test]
    fn the_project_defaults_to_the_current_directory() {
        match parse(&["status"]) {
            IndexerCommand::Status { path, .. } => assert!(path.is_none()),
            other => panic!("got {other:?}"),
        }

        match parse(&["status", "--path", "/tmp/project"]) {
            IndexerCommand::Status { path, .. } => {
                assert_eq!(path, Some(PathBuf::from("/tmp/project")));
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn languages_reports_what_this_build_carries() {
        // The list is fixed at compile time, so it can be asserted without an index.
        let languages = adi_indexer::lang::supported();
        assert!(!languages.is_empty(), "a default build parses something");
        assert!(languages.iter().any(|l| l.as_str() == "rust"));
    }
}
