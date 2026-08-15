// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

use crate::cache::{CachedFileData, GlobalCache};
use crate::config::Config;
use crate::error::Result;
use crate::parser::Parser;
use crate::search::VectorIndex;
use crate::storage::{PendingRef, Storage};
use crate::types::{SymbolId, ParsedReference, IndexProgress, Location, Status, Reference, Language, File, FileId, ParsedSymbol, Symbol, SymbolKind};
use ignore::WalkBuilder;
use crate::embed::Embedder;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Result from processing a single file
struct FileProcessResult {
    /// The file's row, when this call actually (re)wrote it. `None` for a file skipped as
    /// unchanged — whose references are already stored, and must not be replaced with the empty
    /// set this call returns.
    file_id: Option<FileId>,
    symbols_count: usize,
    /// Unresolved references found in the file
    references: Vec<ParsedReference>,
}

/// What the indexing pipeline stores, as a number the index carries so it can tell whether it
/// is current.
///
/// A run skips any file whose content hash it already has, which is what makes reindexing
/// cheap — and what makes a change to this crate invisible: the files did not change, so
/// nothing is reprocessed and the index keeps whatever the old pipeline put there. Bumping this
/// forces one full pass, after which incremental runs resume.
///
/// * 1 — symbols carry a structural fingerprint, and embeddings are built from a text that
///   includes the symbol's body.
/// * 2 — the declaration is repeated after the body (see [`build_embed_text`]).
/// * 3 — references are stored unresolved as well as resolved, so the graph can be rebuilt
///   without reparsing. An index built by 2 has no `pending_refs` rows, and resolving from them
///   would empty its graph rather than restore it.
pub const PIPELINE_VERSION: u32 = 3;

pub async fn index_project(
    project_path: &Path,
    config: &Config,
    storage: Arc<dyn Storage>,
    embedder: Arc<dyn Embedder>,
    parser: Arc<dyn Parser>,
    index: Arc<dyn VectorIndex>,
    cache: Arc<GlobalCache>,
) -> Result<IndexProgress> {
    info!("Starting project indexing: {}", project_path.display());

    let stored_version = storage.get_status().map_or(0, |s| s.pipeline_version);
    let rebuild = stored_version != PIPELINE_VERSION;
    if rebuild {
        info!(
            "Index was built by pipeline {stored_version}, this is {PIPELINE_VERSION} — \
             reindexing every file once"
        );
    }

    let walk = collect_files(project_path, config)?;
    let total = walk.files.len() as u64;

    info!("Found {} files to index", total);

    let mut progress = IndexProgress {
        files_processed: 0,
        files_total: total,
        symbols_indexed: 0,
        errors: Vec::new(),
    };

    // Phase 1: Index all symbols
    storage.begin_transaction()?;


    for file_path in &walk.files {
        match process_file(
            project_path,
            file_path,
            &storage,
            &embedder,
            &parser,
            &index,
            &cache,
            rebuild,
        )
        .await
        {
            Ok(result) => {
                progress.symbols_indexed += result.symbols_count as u64;

                // Only for a file this run rewrote: a skipped file's references are already
                // stored, and `result.references` is empty for it.
                if let Some(file_id) = result.file_id {
                    storage.replace_pending_refs(file_id, &pending_refs(&result.references))?;
                }
            }
            Err(e) => {
                warn!("Error processing {}: {}", file_path.display(), e);
                progress
                    .errors
                    .push(format!("{}: {}", file_path.display(), e));
            }
        }
        progress.files_processed += 1;

        // Periodic checkpoint: flush to disk to reduce memory pressure
        if progress.files_processed.is_multiple_of(200) {
            storage.commit_transaction()?;
            index.save()?;
            storage.begin_transaction()?;
            info!("Checkpoint at {} files", progress.files_processed);
        }
    }

    storage.commit_transaction()?;

    let pruned = prune_departed_files(project_path, &walk, &storage, &index)?;
    if pruned > 0 {
        info!("Pruned {pruned} file(s) that are no longer part of the tree");
    }

    // Phase 2: resolve the whole graph, not this run's share of it.
    //
    // Reprocessing a file gives its symbols new ids, which kills every edge pointing into it —
    // including edges from files this run never looked at, whose parses it therefore does not
    // have. Resolving from the stored unresolved references instead makes the graph a function of
    // the symbol table as it now stands, so it comes out the same whether one file changed or all
    // of them did.
    let pending = storage.all_pending_refs()?;
    let symbol_map = symbols_by_name(&storage)?;
    info!("Resolving {} references...", pending.len());

    let resolved_refs = resolve_references(&pending, &symbol_map);

    info!("Storing {} resolved references...", resolved_refs.len());
    storage.begin_transaction()?;
    storage.replace_symbol_refs(&resolved_refs)?;
    storage.commit_transaction()?;

    index.save()?;

    // Update status
    let status = Status {
        indexed_files: progress.files_processed,
        indexed_symbols: progress.symbols_indexed,
        embedding_dimensions: embedder.dimensions(),
        embedding_model: embedder.model_name().to_string(),
        last_indexed: Some(chrono_now()),
        storage_size_bytes: 0,
        pipeline_version: PIPELINE_VERSION,
    };
    storage.update_status(&status)?;

    info!(
        "Indexing complete: {} files, {} symbols, {} references",
        progress.files_processed,
        progress.symbols_indexed,
        resolved_refs.len()
    );

    Ok(progress)
}

/// The references a file's parse produced, as rows to store against it.
///
/// A reference outside every symbol has no source to hang off and is dropped here rather than
/// stored unresolvable.
fn pending_refs(references: &[ParsedReference]) -> Vec<PendingRef> {
    references
        .iter()
        .filter_map(|parsed| {
            Some(PendingRef {
                from_symbol_id: SymbolId(parsed.containing_symbol_index? as i64),
                target_name: parsed.name.clone(),
                kind: parsed.kind,
                location: parsed.location.clone(),
            })
        })
        .collect()
}

/// Every indexed symbol, by name — the table resolution answers against.
///
/// Built from storage rather than from what this run parsed, because a name is resolved against
/// the whole index and an incremental run parses almost none of it.
///
/// One candidate per name *per file*, later declaration winning, which is not an obvious rule and
/// is kept because it is the one the index already used: resolution built its map by inserting
/// each parsed symbol under its name, so a file declaring the name twice kept only the last. Every
/// same-named symbol being a candidate instead is a defensible graph and a different one — it
/// linked 58% more edges here — and changing what an edge means is not this pass's business.
fn symbols_by_name(storage: &Arc<dyn Storage>) -> Result<HashMap<String, Vec<SymbolId>>> {
    let mut per_file: HashMap<(String, FileId), SymbolId> = HashMap::new();
    for symbol in storage.get_all_symbols()? {
        per_file.insert((symbol.name, symbol.file_id), symbol.id);
    }

    let mut map: HashMap<String, Vec<SymbolId>> = HashMap::new();
    for ((name, _), id) in per_file {
        map.entry(name).or_default().push(id);
    }
    Ok(map)
}

/// Turn stored references into edges, against the symbol table as it now stands.
fn resolve_references(
    references: &[PendingRef],
    symbol_map: &HashMap<String, Vec<SymbolId>>,
) -> Vec<Reference> {
    let mut resolved = Vec::new();

    for pending in references {
        let target_ids = find_target_symbol(&pending.target_name, symbol_map);

        if target_ids.is_empty() {
            debug!("Could not resolve reference: {}", pending.target_name);
            continue;
        }

        // Overloads and same-named symbols in different modules both land here; an edge to each is
        // the existing behaviour, and a caller reading the graph filters by what it knows.
        for target_id in target_ids {
            if pending.from_symbol_id == *target_id {
                continue;
            }

            resolved.push(Reference {
                from_symbol_id: pending.from_symbol_id,
                to_symbol_id: *target_id,
                kind: pending.kind,
                location: pending.location.clone(),
            });
        }
    }

    resolved
}

/// The symbols a written name could mean.
///
/// A qualified name (`foo::bar`) falls back to its last component, which is how a reference
/// written through a module path still finds the item it names.
fn find_target_symbol<'a>(
    name: &str,
    symbol_map: &'a HashMap<String, Vec<SymbolId>>,
) -> &'a [SymbolId] {
    if let Some(ids) = symbol_map.get(name) {
        return ids;
    }

    let short_name = name.rsplit("::").next().unwrap_or(name);
    if short_name != name
        && let Some(ids) = symbol_map.get(short_name) {
            return ids;
        }

    &[]
}

pub async fn reindex_paths(
    project_path: &Path,
    paths: &[PathBuf],
    _config: &Config,
    storage: Arc<dyn Storage>,
    embedder: Arc<dyn Embedder>,
    parser: Arc<dyn Parser>,
    index: Arc<dyn VectorIndex>,
    cache: Arc<GlobalCache>,
) -> Result<()> {
    info!("Re-indexing {} paths", paths.len());

    storage.begin_transaction()?;

    for path in paths {
        // Remove old data for this file
        if storage.file_exists(path)? {
            if let Ok(file_info) = storage.get_file(path) {
                // Remove symbols from vector index
                for symbol in &file_info.symbols {
                    let _ = index.remove(symbol.id.0);
                }
                storage.delete_symbols_for_file(file_info.file.id)?;
            }
            storage.delete_file(path)?;
        }

        // Re-process the file
        let full_path = project_path.join(path);
        if full_path.exists() {
            let _ = process_file(
                project_path,
                &full_path,
                &storage,
                &embedder,
                &parser,
                &index,
                &cache,
                // The caller named these paths explicitly (the watcher saw them change), and
                // the rows were just deleted above — there is nothing left to skip against.
                true,
            )
            .await;
        }
    }

    storage.commit_transaction()?;
    index.save()?;

    Ok(())
}

/// Drop what the walk did not reach.
///
/// A run only ever visits files that are there, so a file deleted since the last one is never
/// looked at and no comparison of content hashes can notice it left: its symbols keep answering
/// searches and keep pairing with live code in a duplication report, forever. The walk is the
/// authority on what belongs in the index, which makes anything beyond it stale — gone from
/// disk, newly ignored, or grown past the size limit.
///
/// Scope is the walk's root, and the root of a walk is the project whose index this is
/// ([`crate::paths::index_dir`]): every row is therefore in scope, and a run of
/// [`index_project`] can prune on the whole `files` table. The one path-limited entry point,
/// [`reindex_paths`], deletes exactly the paths it was handed and prunes nothing.
///
/// Only what the walk could actually see counts as absent. A directory the walker failed to
/// read yields no files, which reads exactly like a directory whose files were all deleted —
/// so [`Walk::unreachable`] is subtracted first, and a walk that could not attribute its error
/// to any path at all ([`Walk::blind`]) prunes nothing.
fn prune_departed_files(
    project_path: &Path,
    walk: &Walk,
    storage: &Arc<dyn Storage>,
    index: &Arc<dyn VectorIndex>,
) -> Result<usize> {
    if walk.blind {
        warn!("Not pruning: the walk failed somewhere it could not name, so what it did not reach is no evidence that anything left the tree");
        return Ok(0);
    }

    fn relative<'a>(path: &'a Path, root: &Path) -> &'a Path {
        path.strip_prefix(root).unwrap_or(path)
    }

    let in_scope: HashSet<&Path> = walk
        .files
        .iter()
        .map(|path| relative(path, project_path))
        .collect();
    let unreachable: Vec<&Path> = walk
        .unreachable
        .iter()
        .map(|path| relative(path, project_path))
        .collect();

    let indexed = storage.indexed_files()?;
    let held = indexed.len();

    let departed: Vec<(FileId, PathBuf)> = indexed
        .into_iter()
        .filter(|(_, path)| !in_scope.contains(path.as_path()))
        .filter(|(_, path)| !unreachable.iter().any(|dir| path.starts_with(dir)))
        .collect();

    if departed.is_empty() {
        return Ok(0);
    }

    // Loud rather than fatal: a branch switch or a big delete is allowed to empty the index,
    // and refusing would leave the stale rows this function exists to remove. But the same
    // shape is what a misconfigured ignore rule produces, and that is worth reading in a log
    // rather than inferring from a reindex that suddenly takes four minutes.
    if departed.len() * 2 > held {
        warn!(
            "Pruning {} of {held} indexed file(s) — more than half the index",
            departed.len()
        );
    }

    storage.begin_transaction()?;
    let mut orphaned: Vec<SymbolId> = Vec::new();
    for (id, path) in &departed {
        debug!("Pruning {}: no longer in the tree", path.display());
        orphaned.extend(storage.delete_file_cascade(*id)?);
    }
    storage.commit_transaction()?;

    // Only once the rows are committed: a vector removed for a symbol still in the index is a
    // symbol that silently stops answering semantic search.
    for symbol in orphaned {
        if let Err(e) = index.remove(symbol.0) {
            debug!("Could not remove embedding for symbol {}: {e}", symbol.0);
        }
    }

    Ok(departed.len())
}

/// What a walk of the project found, and where it could not look.
///
/// The second half is only interesting to [`prune_departed_files`], which treats absence from
/// `files` as proof a file left the tree. That inference holds only over the part of the tree
/// the walker actually read.
struct Walk {
    files: Vec<PathBuf>,
    /// Paths the walker reported an error for, and so saw nothing under.
    unreachable: Vec<PathBuf>,
    /// Whether an error arrived with no path attached. There is no prefix to exclude for one
    /// of those, so the walk stops being usable as evidence of absence at all.
    blind: bool,
}

/// The path a walk error is about, if it names one.
///
/// `ignore` wraps its errors — a `WithDepth` around a `WithPath` around an `Io` — and offers no
/// accessor for the path inside, so the chain is walked by hand. An error that names nothing
/// leaves the walk with no prefix to distrust, which is what [`Walk::blind`] records.
fn errored_path(error: &ignore::Error) -> Option<&Path> {
    match error {
        ignore::Error::WithPath { path, .. } => Some(path),
        ignore::Error::WithDepth { err, .. } | ignore::Error::WithLineNumber { err, .. } => {
            errored_path(err)
        }
        ignore::Error::Loop { child, .. } => Some(child),
        ignore::Error::Partial(errors) => errors.iter().find_map(errored_path),
        _ => None,
    }
}

fn collect_files(project_path: &Path, config: &Config) -> Result<Walk> {
    let mut files = Vec::new();
    let mut unreachable = Vec::new();
    let mut blind = false;

    let mut builder = WalkBuilder::new(project_path);
    builder
        .hidden(true)
        .git_ignore(config.ignore.use_gitignore)
        .ignore(config.ignore.use_ignore_file);

    for entry in builder.build() {
        match entry {
            Ok(entry) => {
                let path = entry.path();

                if !path.is_file() {
                    continue;
                }

                // Check if file should be ignored
                if should_ignore(path, project_path, config) {
                    continue;
                }

                // Check file size
                if let Ok(metadata) = path.metadata()
                    && metadata.len() > config.parser.max_file_size {
                        debug!("Skipping large file: {}", path.display());
                        continue;
                    }

                // Check language support
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    let lang = Language::from_extension(ext);
                    if lang != Language::Unknown {
                        files.push(path.to_path_buf());
                    }
                }
            }
            Err(e) => {
                if let Some(path) = errored_path(&e) {
                    warn!("Error walking {}: {}", path.display(), e);
                    unreachable.push(path.to_path_buf());
                } else {
                    warn!("Error walking directory: {}", e);
                    blind = true;
                }
            }
        }
    }

    Ok(Walk {
        files,
        unreachable,
        blind,
    })
}

fn should_ignore(path: &Path, project_path: &Path, config: &Config) -> bool {
    let relative = path.strip_prefix(project_path).unwrap_or(path);
    let path_str = relative.to_string_lossy();

    for pattern in &config.ignore.patterns {
        if path_str.contains(pattern) {
            return true;
        }
        // Simple glob matching for patterns ending with *
        if pattern.ends_with('*') {
            let prefix = &pattern[..pattern.len() - 1];
            if path_str.starts_with(prefix) {
                return true;
            }
        }
    }

    false
}

async fn process_file(
    project_path: &Path,
    file_path: &Path,
    storage: &Arc<dyn Storage>,
    embedder: &Arc<dyn Embedder>,
    parser: &Arc<dyn Parser>,
    index: &Arc<dyn VectorIndex>,
    cache: &Arc<GlobalCache>,
    rebuild: bool,
) -> Result<FileProcessResult> {
    let relative_path = file_path.strip_prefix(project_path).unwrap_or(file_path);
    debug!("Processing: {}", relative_path.display());

    // Read file content
    let content = std::fs::read_to_string(file_path)?;
    let hash = compute_hash(&content);

    // Check if file has changed in per-project storage. An unchanged file still has to be
    // reprocessed when the pipeline itself moved on — see `PIPELINE_VERSION`.
    if !rebuild
        && let Ok(Some(existing_hash)) = storage.get_file_hash(relative_path)
        && existing_hash == hash {
            debug!("File unchanged, skipping: {}", relative_path.display());
            return Ok(FileProcessResult {
                file_id: None,
                symbols_count: 0,
                references: Vec::new(),
            });
        }

    // Detect language
    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let language = Language::from_extension(ext);

    // Try global cache first
    let cached = cache.get(&hash, embedder.model_name());

    let (parsed, cached_embeddings) = if let Some(cached_data) = cached {
        debug!("Global cache hit for: {}", relative_path.display());
        let has_embeddings = !cached_data.embeddings.is_empty();
        (
            cached_data.parsed,
            if has_embeddings {
                Some(cached_data.embeddings)
            } else {
                None
            },
        )
    } else {
        // Cache miss — parse from scratch
        if !parser.supports(language) {
            return Ok(FileProcessResult {
                file_id: None,
                symbols_count: 0,
                references: Vec::new(),
            });
        }
        let parsed = parser.parse(&content, language)?;
        (parsed, None)
    };

    // Create/update file record
    let file = File {
        id: FileId(0),
        path: relative_path.to_path_buf(),
        language,
        hash: hash.clone(),
        size: content.len() as u64,
        description: None,
    };

    // Remove old data if exists
    if storage.file_exists(relative_path)? {
        if let Ok(old_file) = storage.get_file(relative_path) {
            for symbol in &old_file.symbols {
                if let Err(e) = index.remove(symbol.id.0) {
                    debug!(
                        "Could not remove embedding for symbol {} ({}): {}. This is expected if the index was rebuilt.",
                        symbol.id.0, symbol.name, e
                    );
                }
            }
            storage.delete_references_for_file(old_file.file.id)?;
            storage.delete_symbols_for_file(old_file.file.id)?;
        }
        storage.delete_file(relative_path)?;
    }

    let file_id = storage.insert_file(&file)?;

    let mut symbols_count = 0;
    let mut texts_to_embed: Vec<(SymbolId, String)> = Vec::new();
    let mut symbol_ranges: Vec<(SymbolId, u32, u32)> = Vec::new();

    #[allow(clippy::too_many_arguments)]
    fn insert_symbols(
        symbols: &[ParsedSymbol],
        file_id: FileId,
        file_path: PathBuf,
        source: &str,
        parent_id: Option<SymbolId>,
        storage: &Arc<dyn Storage>,
        texts_to_embed: &mut Vec<(SymbolId, String)>,
        symbol_ranges: &mut Vec<(SymbolId, u32, u32)>,
        count: &mut usize,
    ) -> Result<()> {
        for parsed in symbols {
            let symbol = Symbol {
                id: SymbolId(0),
                name: parsed.name.clone(),
                kind: parsed.kind,
                file_id,
                file_path: file_path.clone(),
                location: parsed.location.clone(),
                parent_id,
                signature: parsed.signature.clone(),
                description: None,
                doc_comment: parsed.doc_comment.clone(),
                visibility: parsed.visibility,
                is_entry_point: false,
                structure: parsed.structure.clone(),
            };

            let symbol_id = storage.insert_symbol(&symbol)?;
            *count += 1;

            symbol_ranges.push((
                symbol_id,
                parsed.location.start_byte,
                parsed.location.end_byte,
            ));

            let embed_text = build_embed_text(
                &parsed.name,
                parsed.kind,
                &parsed.signature,
                &parsed.doc_comment,
                symbol_body(source, &parsed.location),
            );
            texts_to_embed.push((symbol_id, embed_text));

            insert_symbols(
                &parsed.children,
                file_id,
                file_path.clone(),
                source,
                Some(symbol_id),
                storage,
                texts_to_embed,
                symbol_ranges,
                count,
            )?;
        }
        Ok(())
    }

    insert_symbols(
        &parsed.symbols,
        file_id,
        relative_path.to_path_buf(),
        &content,
        None,
        storage,
        &mut texts_to_embed,
        &mut symbol_ranges,
        &mut symbols_count,
    )?;

    // Get embeddings: from cache or compute fresh
    if !texts_to_embed.is_empty() {
        let embeddings = if let Some(ref cached_emb) = cached_embeddings {
            if cached_emb.len() == texts_to_embed.len() {
                // Cache hit with matching embedding count — use directly
                cached_emb.clone()
            } else {
                debug!("Cached embedding count mismatch, recomputing");
                compute_embeddings(embedder, &texts_to_embed)?
            }
        } else {
            compute_embeddings(embedder, &texts_to_embed)?
        };

        // Store to global cache if we computed fresh embeddings or had a cache miss
        if cached_embeddings.is_none() {
            let _ = cache.put(
                &hash,
                &CachedFileData {
                    parsed: parsed.clone(),
                    embeddings: embeddings.clone(),
                    embed_model: embedder.model_name().to_string(),
                    schema_version: crate::cache::SCHEMA_VERSION,
                },
            );
        }

        // Add embeddings to per-project vector index
        for ((symbol_id, _), embedding) in texts_to_embed.iter().zip(&embeddings) {
            // A batch that failed to embed left empty vectors behind; adding one is a
            // guaranteed dimension error, and warning about it says nothing the batch's own
            // warning did not already say.
            if embedding.is_empty() {
                continue;
            }
            if let Err(e) = index.add(symbol_id.0, embedding) {
                let error_msg = format!("{e}");
                if error_msg.to_lowercase().contains("duplicate") {
                    debug!("Duplicate key for symbol {}, re-adding", symbol_id.0);
                    let _ = index.remove(symbol_id.0);
                    if let Err(e2) = index.add(symbol_id.0, embedding) {
                        warn!(
                            "Failed to re-add embedding for symbol {}: {}",
                            symbol_id.0, e2
                        );
                    }
                } else {
                    warn!(
                        "Failed to add embedding for symbol {}: {}",
                        symbol_id.0, error_msg
                    );
                }
            }
        }
    }

    // Process references — find containing symbol for each reference
    let mut references_with_context: Vec<ParsedReference> = Vec::new();
    for mut parsed_ref in parsed.references {
        let ref_byte = parsed_ref.location.start_byte;
        let containing_symbol = symbol_ranges
            .iter()
            .filter(|(_, start, end)| ref_byte >= *start && ref_byte <= *end)
            .min_by_key(|(_, start, end)| end - start);

        if let Some((symbol_id, _, _)) = containing_symbol {
            parsed_ref.containing_symbol_index = Some(symbol_id.0 as usize);
        }

        references_with_context.push(parsed_ref);
    }

    Ok(FileProcessResult {
        file_id: Some(file_id),
        symbols_count,
        references: references_with_context,
    })
}

/// The padded size of one embedding call, in characters.
///
/// [`Embedder::embed`] pads every text in a batch out to the longest of them, and attention then
/// costs the *square* of that padded length per sequence. So what has to be bounded is the
/// batch's padded area — length × width — not its length alone.
///
/// Bounding length alone is not a smaller version of this, it is a different thing entirely:
/// 32 symbols at the token limit is a padded area 40× a batch of 32 short ones, and puts a
/// multi-gigabyte command buffer on the GPU that then sits there. Short symbols still batch
/// wide under this rule — a hundred 150-character texts fit in one call — and only long ones
/// pack thin.
const MAX_BATCH_PADDED_CHARS: usize = 16_000;

/// A ceiling on batch length for when every text is tiny and the area rule never binds.
const MAX_BATCH: usize = 64;

fn compute_embeddings(
    embedder: &Arc<dyn Embedder>,
    texts_to_embed: &[(SymbolId, String)],
) -> Result<Vec<Vec<f32>>> {
    let mut embeddings = Vec::with_capacity(texts_to_embed.len());
    let mut batch: Vec<&str> = Vec::new();
    let mut widest = 0usize;

    for (_, text) in texts_to_embed {
        // What this text would cost if it joined: every member padded to the new widest.
        let padded_area = widest.max(text.len()) * (batch.len() + 1);
        if !batch.is_empty() && (batch.len() >= MAX_BATCH || padded_area > MAX_BATCH_PADDED_CHARS)
        {
            embeddings.extend(embed_batch(embedder, &batch));
            batch.clear();
            widest = 0;
        }
        widest = widest.max(text.len());
        batch.push(text.as_str());
    }
    if !batch.is_empty() {
        embeddings.extend(embed_batch(embedder, &batch));
    }

    Ok(embeddings)
}

/// One `embed` call, yielding an empty vector per text if it fails.
///
/// A failed batch costs those symbols their searchability, not their place in the index — they
/// are already stored, and the caller skips empty vectors when populating the vector index.
fn embed_batch(embedder: &Arc<dyn Embedder>, batch: &[&str]) -> Vec<Vec<f32>> {
    match embedder.embed(batch) {
        Ok(embeddings) => embeddings,
        Err(e) => {
            warn!("Failed to generate embeddings: {}", e);
            vec![vec![]; batch.len()]
        }
    }
}

/// How much of a symbol's source goes into its embedding, in bytes.
///
/// The model reads 8192 tokens, but spending them is the wrong trade: attention costs the
/// square of the sequence length, so doubling what a long symbol contributes quadruples what
/// its batch costs. At roughly 3–4 bytes per code token this covers a function of about
/// twenty-five lines whole, and leaves the declaration — repeated by `build_embed_text` — a
/// share of the pooled vector big enough to still steer a query about what a symbol is called.
const MAX_BODY_BYTES: usize = 1200;

/// The source a symbol spans, capped at [`MAX_BODY_BYTES`].
///
/// Returns `None` rather than a partial read when the range does not land on character
/// boundaries — `content` is UTF-8 and the offsets come from tree-sitter's byte view of it,
/// which agree in practice, but a disagreement should drop the body, not panic the index.
fn symbol_body<'src>(source: &'src str, location: &Location) -> Option<&'src str> {
    let body = source.get(location.start_byte as usize..location.end_byte as usize)?;

    if body.len() <= MAX_BODY_BYTES {
        return Some(body);
    }
    // Cut on a character boundary at or below the cap.
    let mut cut = MAX_BODY_BYTES;
    while cut > 0 && !body.is_char_boundary(cut) {
        cut -= 1;
    }
    Some(&body[..cut])
}

/// The text that stands in for a symbol when it is embedded.
///
/// The declaration comes first and the body after it: truncation bites at the end, so what a
/// symbol *is* survives it and only the tail of what it does is lost.
///
/// The declaration is then repeated once at the end, which is not decoration. The embedding is
/// a mean over token vectors, so a part's influence is its share of the tokens — and a 60
/// character signature inside 2000 characters of body is three percent of the answer. Embedding
/// bodies without this traded one failure for another: "restart a launchd service" had ranked
/// `restart_onto` first, and with the body alone diluting it, the function dropped out of the
/// top fifteen entirely while generic service-handling code took its place. Repeating the
/// declaration buys back roughly double its share for a few dozen tokens, and the copy at the
/// front means truncation can only ever cost the second one.
fn build_embed_text(
    name: &str,
    kind: SymbolKind,
    signature: &Option<String>,
    doc_comment: &Option<String>,
    body: Option<&str>,
) -> String {
    let mut declaration = vec![format!("{} {}", kind.as_str(), name)];
    if let Some(sig) = signature {
        declaration.push(sig.clone());
    }
    if let Some(doc) = doc_comment {
        declaration.push(doc.clone());
    }
    let declaration = declaration.join(" | ");

    match body {
        Some(body) => format!("{declaration} | {body} | {declaration}"),
        None => declaration,
    }
}

fn compute_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

fn chrono_now() -> String {
    // Simple ISO 8601 timestamp without chrono dependency
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::EmbedError;
    use std::sync::Mutex;

    /// Records the shape of every batch it is handed.
    #[derive(Debug, Default)]
    struct RecordingEmbedder {
        batches: Mutex<Vec<Vec<usize>>>,
    }

    impl Embedder for RecordingEmbedder {
        fn embed(&self, texts: &[&str]) -> std::result::Result<Vec<Vec<f32>>, EmbedError> {
            self.batches
                .lock()
                .unwrap()
                .push(texts.iter().map(|t| t.len()).collect());
            Ok(texts.iter().map(|_| vec![0.0; 8]).collect())
        }
        fn dimensions(&self) -> u32 {
            8
        }
        fn model_name(&self) -> &'static str {
            "recording"
        }
    }

    fn batches_for(lengths: &[usize]) -> Vec<Vec<usize>> {
        let embedder = Arc::new(RecordingEmbedder::default());
        let texts: Vec<(SymbolId, String)> = lengths
            .iter()
            .enumerate()
            .map(|(i, len)| (SymbolId(i as i64), "x".repeat(*len)))
            .collect();

        let dynamic: Arc<dyn Embedder> = embedder.clone();
        let embeddings = compute_embeddings(&dynamic, &texts).unwrap();
        assert_eq!(
            embeddings.len(),
            texts.len(),
            "every symbol must come back with a vector, batching or not"
        );

        embedder.batches.lock().unwrap().clone()
    }

    /// The padded area of a batch is what the GPU pays for — length alone bounded, a run of
    /// long symbols allocated multiple gigabytes and stalled.
    #[test]
    fn no_batch_exceeds_the_padded_area_budget() {
        for lengths in [
            vec![2100; 40],
            vec![150; 400],
            vec![2100, 100, 2100, 100, 2100, 100],
        ] {
            for batch in batches_for(&lengths) {
                let area = batch.iter().copied().max().unwrap_or(0) * batch.len();
                assert!(
                    area <= MAX_BATCH_PADDED_CHARS || batch.len() == 1,
                    "a batch of {} texts padded to {} is area {area}",
                    batch.len(),
                    batch.iter().copied().max().unwrap_or(0)
                );
            }
        }
    }

    #[test]
    fn short_symbols_still_batch_wide() {
        // The area rule must not collapse into one-at-a-time for ordinary short symbols.
        let batches = batches_for(&vec![120; 200]);
        assert!(
            batches.iter().any(|b| b.len() >= 32),
            "shortest batches were {:?}",
            batches.iter().map(Vec::len).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_single_oversized_symbol_is_still_embedded() {
        // On its own rather than never: dropping it would silently cost that symbol its
        // searchability.
        let batches = batches_for(&[MAX_BATCH_PADDED_CHARS * 3]);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 1);
    }

    #[test]
    fn a_symbol_body_is_capped_on_a_character_boundary() {
        // A multi-byte character straddling the cap must not panic or split.
        let source = "fn f() { let s = \"".to_string() + &"é".repeat(MAX_BODY_BYTES) + "\"; }";
        let location = Location {
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 0,
            start_byte: 0,
            end_byte: source.len() as u32,
        };

        let body = symbol_body(&source, &location).expect("a body came back");
        assert!(body.len() <= MAX_BODY_BYTES);
        assert!(source.starts_with(body));
    }

    #[test]
    fn a_range_outside_the_source_yields_no_body() {
        let location = Location {
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 0,
            start_byte: 0,
            end_byte: 9_999,
        };
        assert!(symbol_body("fn f() {}", &location).is_none());
    }

    #[test]
    fn the_embed_text_puts_the_declaration_before_the_body() {
        // Truncation bites at the end, so what a symbol *is* has to survive it.
        let text = build_embed_text(
            "restart_onto",
            SymbolKind::Function,
            &Some("fn restart_onto(app: &Path)".to_string()),
            &Some("Restart the service".to_string()),
            Some("{ do_the_thing(); }"),
        );

        let signature = text.find("fn restart_onto").expect("signature present");
        let body = text.find("do_the_thing").expect("body present");
        assert!(signature < body, "body came before the signature in {text:?}");
        assert!(text.starts_with("function restart_onto"));
    }

    #[test]
    fn the_declaration_is_repeated_after_the_body() {
        // Its share of the tokens is its share of the pooled vector; see `build_embed_text`.
        let text = build_embed_text(
            "restart_onto",
            SymbolKind::Function,
            &Some("fn restart_onto(app: &Path)".to_string()),
            &None,
            Some("{ do_the_thing(); }"),
        );

        assert_eq!(
            text.matches("fn restart_onto(app: &Path)").count(),
            2,
            "declaration not repeated in {text:?}"
        );
        let body = text.find("do_the_thing").expect("body present");
        let last = text.rfind("fn restart_onto").expect("trailing declaration");
        assert!(body < last, "the repeat has to come after the body");
    }

    #[test]
    fn a_symbol_with_no_body_is_not_repeated() {
        // Nothing to dilute, so nothing to compensate for.
        let text = build_embed_text(
            "Config",
            SymbolKind::Type,
            &Some("struct Config".to_string()),
            &None,
            None,
        );
        assert_eq!(text.matches("struct Config").count(), 1);
    }

    #[test]
    fn a_symbol_with_no_body_still_embeds_its_declaration() {
        let text = build_embed_text("Config", SymbolKind::Type, &None, &None, None);
        assert_eq!(text, "type Config");
    }
}
