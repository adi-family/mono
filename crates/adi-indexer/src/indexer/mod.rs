// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

use crate::cache::{CachedFileData, GlobalCache};
use crate::config::Config;
use crate::error::Result;
use crate::parser::Parser;
use crate::search::VectorIndex;
use crate::storage::Storage;
use crate::types::{SymbolId, ParsedReference, IndexProgress, Status, Reference, Language, File, FileId, ParsedSymbol, Symbol, SymbolKind};
use ignore::WalkBuilder;
use crate::embed::Embedder;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Result from processing a single file
struct FileProcessResult {
    symbols_count: usize,
    /// Map from symbol name to symbol ID (for reference resolution)
    symbol_map: HashMap<String, SymbolId>,
    /// Unresolved references found in the file
    references: Vec<ParsedReference>,
}

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

    let files = collect_files(project_path, config)?;
    let total = files.len() as u64;

    info!("Found {} files to index", total);

    let mut progress = IndexProgress {
        files_processed: 0,
        files_total: total,
        symbols_indexed: 0,
        errors: Vec::new(),
    };

    // Phase 1: Index all symbols
    storage.begin_transaction()?;

    // Collect all unresolved references and build global symbol map
    let mut all_references: Vec<ParsedReference> = Vec::new();
    let mut global_symbol_map: HashMap<String, Vec<SymbolId>> = HashMap::new();

    for file_path in &files {
        match process_file(
            project_path,
            file_path,
            &storage,
            &embedder,
            &parser,
            &index,
            &cache,
        )
        .await
        {
            Ok(result) => {
                progress.symbols_indexed += result.symbols_count as u64;

                // Add to global symbol map
                for (name, id) in result.symbol_map {
                    global_symbol_map.entry(name).or_default().push(id);
                }

                // Collect references for phase 2
                all_references.extend(result.references);
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

    // Phase 2: Resolve and store references
    info!("Resolving {} references...", all_references.len());

    let resolved_refs = resolve_references(&all_references, &global_symbol_map, &storage)?;

    if !resolved_refs.is_empty() {
        info!("Storing {} resolved references...", resolved_refs.len());
        storage.begin_transaction()?;
        storage.insert_references_batch(&resolved_refs)?;
        storage.commit_transaction()?;
    }

    index.save()?;

    // Update status
    let status = Status {
        indexed_files: progress.files_processed,
        indexed_symbols: progress.symbols_indexed,
        embedding_dimensions: embedder.dimensions(),
        embedding_model: embedder.model_name().to_string(),
        last_indexed: Some(chrono_now()),
        storage_size_bytes: 0,
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

/// Resolve unresolved references to symbol IDs
fn resolve_references(
    references: &[ParsedReference],
    symbol_map: &HashMap<String, Vec<SymbolId>>,
    storage: &Arc<dyn Storage>,
) -> Result<Vec<Reference>> {
    let mut resolved = Vec::new();

    for parsed_ref in references {
        // We need a source symbol (the symbol that contains this reference)
        let source_id = match parsed_ref.containing_symbol_index {
            Some(id) => SymbolId(id as i64),
            None => {
                // Reference not within any symbol, skip
                continue;
            }
        };

        // Try to find the target symbol
        let target_ids = find_target_symbol(&parsed_ref.name, symbol_map, storage)?;

        if target_ids.is_empty() {
            // Could not resolve this reference
            debug!("Could not resolve reference: {}", parsed_ref.name);
            continue;
        }

        // Create references for each potential target
        // In most cases there's only one, but for overloaded names there could be multiple
        for target_id in target_ids {
            // Don't create self-references
            if source_id == target_id {
                continue;
            }

            resolved.push(Reference {
                from_symbol_id: source_id,
                to_symbol_id: target_id,
                kind: parsed_ref.kind,
                location: parsed_ref.location.clone(),
            });
        }
    }

    Ok(resolved)
}

/// Find target symbol(s) by name
fn find_target_symbol(
    name: &str,
    symbol_map: &HashMap<String, Vec<SymbolId>>,
    storage: &Arc<dyn Storage>,
) -> Result<Vec<SymbolId>> {
    // First, try exact match in our collected symbols
    if let Some(ids) = symbol_map.get(name) {
        return Ok(ids.clone());
    }

    // Try matching just the last component (for qualified names like foo::bar)
    let short_name = name.rsplit("::").next().unwrap_or(name);
    if short_name != name
        && let Some(ids) = symbol_map.get(short_name) {
            return Ok(ids.clone());
        }

    // Try database lookup for previously indexed symbols
    if let Ok(symbols) = storage.find_symbols_by_name(name)
        && !symbols.is_empty() {
            return Ok(symbols.into_iter().map(|s| s.id).collect());
        }

    // Try short name in database
    if short_name != name
        && let Ok(symbols) = storage.find_symbols_by_name(short_name)
            && !symbols.is_empty() {
                return Ok(symbols.into_iter().map(|s| s.id).collect());
            }

    Ok(vec![])
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
            )
            .await;
        }
    }

    storage.commit_transaction()?;
    index.save()?;

    Ok(())
}

fn collect_files(project_path: &Path, config: &Config) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();

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
                warn!("Error walking directory: {}", e);
            }
        }
    }

    Ok(files)
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
) -> Result<FileProcessResult> {
    let relative_path = file_path.strip_prefix(project_path).unwrap_or(file_path);
    debug!("Processing: {}", relative_path.display());

    // Read file content
    let content = std::fs::read_to_string(file_path)?;
    let hash = compute_hash(&content);

    // Check if file has changed in per-project storage
    if let Ok(Some(existing_hash)) = storage.get_file_hash(relative_path)
        && existing_hash == hash {
            debug!("File unchanged, skipping: {}", relative_path.display());
            return Ok(FileProcessResult {
                symbols_count: 0,
                symbol_map: HashMap::new(),
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
                symbols_count: 0,
                symbol_map: HashMap::new(),
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

    // Process symbols and collect name -> id mapping
    let mut symbols_count = 0;
    let mut texts_to_embed: Vec<(SymbolId, String)> = Vec::new();
    let mut symbol_map: HashMap<String, SymbolId> = HashMap::new();
    let mut symbol_ranges: Vec<(SymbolId, u32, u32)> = Vec::new();

    #[allow(clippy::too_many_arguments)]
    fn insert_symbols(
        symbols: &[ParsedSymbol],
        file_id: FileId,
        file_path: PathBuf,
        parent_id: Option<SymbolId>,
        storage: &Arc<dyn Storage>,
        texts_to_embed: &mut Vec<(SymbolId, String)>,
        symbol_map: &mut HashMap<String, SymbolId>,
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
            };

            let symbol_id = storage.insert_symbol(&symbol)?;
            *count += 1;

            symbol_map.insert(parsed.name.clone(), symbol_id);
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
            );
            texts_to_embed.push((symbol_id, embed_text));

            insert_symbols(
                &parsed.children,
                file_id,
                file_path.clone(),
                Some(symbol_id),
                storage,
                texts_to_embed,
                symbol_map,
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
        None,
        storage,
        &mut texts_to_embed,
        &mut symbol_map,
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
                },
            );
        }

        // Add embeddings to per-project vector index
        for ((symbol_id, _), embedding) in texts_to_embed.iter().zip(&embeddings) {
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
        symbols_count,
        symbol_map,
        references: references_with_context,
    })
}

fn compute_embeddings(
    embedder: &Arc<dyn Embedder>,
    texts_to_embed: &[(SymbolId, String)],
) -> Result<Vec<Vec<f32>>> {
    let texts: Vec<&str> = texts_to_embed.iter().map(|(_, t)| t.as_str()).collect();
    match embedder.embed(&texts) {
        Ok(embeddings) => Ok(embeddings),
        Err(e) => {
            warn!("Failed to generate embeddings: {}", e);
            Ok(vec![vec![]; texts_to_embed.len()])
        }
    }
}

fn build_embed_text(
    name: &str,
    kind: SymbolKind,
    signature: &Option<String>,
    doc_comment: &Option<String>,
) -> String {
    let mut parts = Vec::new();

    parts.push(format!("{} {}", kind.as_str(), name));

    if let Some(sig) = signature {
        parts.push(sig.clone());
    }

    if let Some(doc) = doc_comment {
        parts.push(doc.clone());
    }

    parts.join(" | ")
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
