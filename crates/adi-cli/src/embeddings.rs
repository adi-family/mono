//! The `embeddings` command group: the embedding backend registry, and which consumer
//! (`indexer`/`knowledge`/`facts`) resolves through each.
//!
//! A **backend** is one complete way to turn text into a vector — a runtime, a model, the
//! dimensions it produces, and the fallbacks it may fail over to (only ever between backends
//! declaring the same model and width; the registry refuses anything else). `indexer`, `knowledge`
//! and `facts` do not build an embedder of their own any more — each names exactly one backend in
//! `embeddings/settings.toml`, and this group is where that assignment is read and changed.
//!
//! Deliberately smaller than [`crate::llm`]: no holds, no prober, and no migrate. There is nothing
//! here rate-limited the way a chat subscription is — a `candle`/`hash` backend is local and free,
//! and `ollama`/`openai` fail over per call through the registry's own chain rather than a
//! background sweep. See `docs/embedding-backends.md`'s "why this is narrower than the LLM design."

use adi_core::Adi;
use adi_core::embeddings::{
    CONSUMER_FACTS, CONSUMER_INDEXER, CONSUMER_KNOWLEDGE, EMBEDDINGS_MODULE, EmbeddingBackend,
    EmbeddingBackendManifest, EmbeddingBackends, EmbeddingSettings, Runtime, ensure_seeded,
};
use clap::Subcommand;

use crate::format::{clean, print_json};

/// Every consumer this build knows to ask, in the order the CLI lists them — the same fixed order
/// the panel shows them in (`adi-webapp-api`'s own copy of this list).
const CONSUMERS: [&str; 3] = [CONSUMER_INDEXER, CONSUMER_KNOWLEDGE, CONSUMER_FACTS];

#[derive(Debug, Subcommand)]
pub(crate) enum EmbeddingsCommand {
    /// List the backends on this machine, the model and width each runs, and whether this binary
    /// can actually build it.
    Backends {
        #[arg(long)]
        json: bool,
    },
    /// Show one backend in full.
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Create or replace a backend. A save states the whole object: a backend is a handful of
    /// fields, and an omit-to-keep save of a partly-remembered one is how a fallback goes missing.
    Save {
        /// The id consumers name in `embeddings/settings.toml`. Also the filename, so it is a
        /// single path segment.
        id: String,
        /// `candle` · `ollama` · `openai` · `hash`.
        #[arg(long)]
        runtime: String,
        /// What a human calls it, shown in the panel.
        #[arg(long)]
        label: Option<String>,
        /// The model this backend embeds with. Every stored vector is recorded against this name.
        /// Ignored for `candle`/`hash`, which always produce one fixed model and take this flag
        /// from nowhere but that fixed answer; required for `ollama`/`openai`.
        #[arg(long)]
        model: Option<String>,
        /// The width of the vectors this backend produces. Ignored for `candle`/`hash` for the
        /// same reason `--model` is; required for `ollama`/`openai` — the same-model failover
        /// check compares both.
        #[arg(long)]
        dimensions: Option<u32>,
        /// The `ollama` host or `openai` endpoint base. Unused by `candle` and `hash`.
        #[arg(long = "base-url")]
        base_url: Option<String>,
        /// The environment variable an `openai` backend's key is read from — a name, never the
        /// key itself; this command never accepts a raw key value.
        #[arg(long = "api-key-env")]
        api_key_env: Option<String>,
        /// Repeatable: another backend id to fail over to if this one fails to build or answer.
        /// Refused unless it declares the same model and width — see
        /// `docs/embedding-backends.md`'s "the same-model failover rule."
        #[arg(long = "fallback")]
        fallbacks: Vec<String>,
    },
    /// Delete a backend. Refused while a consumer is still assigned to it — unlike an LLM agent's
    /// row, a consumer whose assignment names nothing cannot resolve at all.
    Delete { id: String },
    /// Read or change which backend each consumer resolves through, and show whether that
    /// assignment would actually resolve in this binary right now.
    Settings {
        /// Repeatable `consumer=backend`, e.g. `facts=ollama`. An empty backend
        /// (`facts=`) clears that consumer's assignment. Without any, only prints the current
        /// assignments.
        #[arg(long = "assign", value_name = "CONSUMER=BACKEND")]
        assign: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Materialize the backends and assignments that reproduce today's hardcoded behaviour, if
    /// this store has never been seeded. Safe to run again: a store that has already been seeded
    /// (or one an operator has since edited by hand) is left exactly as it is.
    Seed {
        #[arg(long)]
        json: bool,
    },
}

/// Dispatch an `embeddings` subcommand over the backend registry.
#[allow(clippy::too_many_lines)] // one arm per verb; splitting it would only move the match
pub(crate) fn run_embeddings(adi: Adi, command: EmbeddingsCommand) -> Result<(), String> {
    let config = adi.agents().config().clone();
    let registry = EmbeddingBackends::with_config(config.clone());
    match command {
        EmbeddingsCommand::Backends { json } => {
            let backends = registry.list().map_err(|e| e.to_string())?;
            if json {
                let items: Vec<_> = backends
                    .iter()
                    .map(|b| {
                        serde_json::json!({
                            "id": b.id,
                            "label": b.manifest.label,
                            "runtime": b.manifest.runtime.to_string(),
                            "model": b.manifest.model,
                            "dimensions": b.manifest.dimensions,
                            "available": b.manifest.runtime.available(),
                        })
                    })
                    .collect();
                print_json(&items);
            } else if backends.is_empty() {
                println!(
                    "No embedding backends yet. `adi-mono embeddings seed` materializes the \
                     defaults, or save one by hand with `adi-mono embeddings save <id> --runtime …`."
                );
            } else {
                for b in &backends {
                    let unavailable = if b.manifest.runtime.available() {
                        ""
                    } else {
                        "  [unavailable in this binary]"
                    };
                    println!(
                        "{} — {} on {} \u{b7} {} dims{unavailable}",
                        b.id,
                        blank(&b.manifest.model, "(unset)"),
                        b.manifest.runtime,
                        b.manifest.dimensions,
                    );
                }
            }
        }
        EmbeddingsCommand::Show { id, json } => {
            let backend = require(&registry, &id)?;
            if json {
                print_json(&backend);
            } else {
                print_backend(&backend);
            }
        }
        EmbeddingsCommand::Save {
            id,
            runtime,
            label,
            model,
            dimensions,
            base_url,
            api_key_env,
            fallbacks,
        } => {
            let runtime = parse_runtime(&runtime)?;
            // `candle`/`hash` never take a model or width from configuration — see
            // `Runtime::fixed_model`'s own doc — so `--model`/`--dimensions` are simply ignored
            // for those two rather than trusted to agree with the one honest answer.
            let (model, dims) = match runtime.fixed_model() {
                Some((model, dims)) => (model.to_string(), dims),
                None => (clean(model).unwrap_or_default(), dimensions.unwrap_or(0)),
            };
            let manifest = EmbeddingBackendManifest {
                label: clean(label).unwrap_or_default(),
                runtime,
                model,
                dimensions: dims,
                base_url: clean(base_url),
                api_key_env: clean(api_key_env),
                fallbacks: fallbacks
                    .into_iter()
                    .map(|f| f.trim().to_string())
                    .filter(|f| !f.is_empty())
                    .collect(),
                // The store owns the timestamps.
                created_at: 0,
                updated_at: 0,
            };
            let saved = registry
                .save(id.trim(), manifest)
                .map_err(|e| e.to_string())?;
            println!(
                "Saved backend {} — {} on {}.",
                saved.id,
                blank(&saved.manifest.model, "(unset)"),
                saved.manifest.runtime,
            );
            if !saved.manifest.runtime.available() {
                println!(
                    "This binary cannot build the {} runtime yet — rebuild with the `candle` \
                     feature, or point consumers elsewhere until then.",
                    saved.manifest.runtime,
                );
            }
        }
        EmbeddingsCommand::Delete { id } => {
            let id = id.trim();
            let settings = EmbeddingSettings::open(&config).map_err(|e| e.to_string())?;
            let users = assigned_to(&settings, id);
            if !users.is_empty() {
                return Err(format!(
                    "{id} is still assigned to {} — point {} at another backend first",
                    users.join(", "),
                    if users.len() == 1 { "it" } else { "them" }
                ));
            }
            if registry.delete(id).map_err(|e| e.to_string())? {
                println!("Deleted backend {id}.");
            } else {
                return Err(format!("no embedding backend named {id}"));
            }
        }
        EmbeddingsCommand::Settings { assign, json } => {
            let mut settings = EmbeddingSettings::open(&config).map_err(|e| e.to_string())?;
            if !assign.is_empty() {
                for raw in assign {
                    let (consumer, backend) = raw
                        .split_once('=')
                        .ok_or_else(|| format!("--assign {raw:?} must be consumer=backend"))?;
                    let (consumer, backend) = (consumer.trim(), backend.trim());
                    if backend.is_empty() {
                        settings.assignments.remove(consumer);
                        continue;
                    }
                    if registry.get(backend).map_err(|e| e.to_string())?.is_none() {
                        return Err(format!(
                            "{consumer} cannot be assigned to {backend} — no such backend"
                        ));
                    }
                    settings
                        .assignments
                        .insert(consumer.to_string(), backend.to_string());
                }
                settings
                    .save(&config.module(EMBEDDINGS_MODULE))
                    .map_err(|e| e.to_string())?;
            }
            if json {
                print_json(&settings);
            } else {
                for consumer in CONSUMERS {
                    print_assignment(&registry, &settings, consumer);
                }
            }
        }
        EmbeddingsCommand::Seed { json } => {
            let settings = ensure_seeded(&config).map_err(|e| e.to_string())?;
            if json {
                print_json(&settings);
            } else {
                let backends = registry.list().map_err(|e| e.to_string())?;
                println!(
                    "{} backend(s), {} assignment(s) — a no-op if this store was already seeded.",
                    backends.len(),
                    settings.assignments.len(),
                );
            }
        }
    }
    Ok(())
}

/// Which consumers are assigned to `id` right now.
fn assigned_to<'a>(settings: &'a EmbeddingSettings, id: &str) -> Vec<&'a str> {
    CONSUMERS
        .into_iter()
        .filter(|consumer| settings.assignments.get(*consumer).map(String::as_str) == Some(id))
        .collect()
}

/// One consumer's line: what it is assigned to, and whether that would actually resolve — the
/// question an operator asks this command for, not just "what does the file say."
fn print_assignment(registry: &EmbeddingBackends, settings: &EmbeddingSettings, consumer: &str) {
    let Some(backend_id) = settings.assignments.get(consumer).filter(|b| !b.is_empty()) else {
        println!("{consumer:<10}  (unassigned)");
        return;
    };
    let resolvable = registry
        .get(backend_id)
        .ok()
        .flatten()
        .is_some_and(|b| b.manifest.runtime.available());
    let flag = if resolvable { "" } else { "  [will not resolve]" };
    println!("{consumer:<10}  {backend_id}{flag}");
}

fn require(registry: &EmbeddingBackends, id: &str) -> Result<EmbeddingBackend, String> {
    registry
        .get(id.trim())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no embedding backend named {}", id.trim()))
}

/// The four runtimes, as the closed set they are — an unrecognised spelling is refused by name
/// rather than silently read as `hash`, this crate's default.
fn parse_runtime(value: &str) -> Result<Runtime, String> {
    match value.trim() {
        "candle" => Ok(Runtime::Candle),
        "ollama" => Ok(Runtime::Ollama),
        "openai" => Ok(Runtime::OpenAi),
        "hash" => Ok(Runtime::Hash),
        other => Err(format!(
            "unknown runtime {other:?}; expected candle, ollama, openai, or hash"
        )),
    }
}

fn print_backend(backend: &EmbeddingBackend) {
    let m = &backend.manifest;
    println!("{} — {}", backend.id, blank(&m.label, "(no label)"));
    println!("runtime         {}", m.runtime);
    println!("model           {}", blank(&m.model, "(unset)"));
    println!("dimensions      {}", m.dimensions);
    if let Some(base_url) = m.base_url.as_deref().filter(|v| !v.is_empty()) {
        println!("base url        {base_url}");
    }
    if let Some(env) = m.api_key_env.as_deref().filter(|v| !v.is_empty()) {
        println!("api key env     {env}");
    }
    if m.fallbacks.is_empty() {
        println!("fallbacks       none");
    } else {
        println!("fallbacks       {}", m.fallbacks.join(", "));
    }
    println!(
        "available       {}",
        if m.runtime.available() {
            "yes"
        } else {
            "no — rebuild this binary with the `candle` feature"
        }
    );
}

fn blank<'a>(value: &'a str, instead: &'a str) -> &'a str {
    if value.trim().is_empty() { instead } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_runtime_spelling_is_refused() {
        let err = parse_runtime("ollamaa").expect_err("refused");
        assert!(err.contains("candle, ollama, openai, or hash"), "{err}");
        assert_eq!(parse_runtime("hash").expect("hash"), Runtime::Hash);
    }

    #[test]
    fn blank_falls_back_and_a_real_value_is_kept() {
        assert_eq!(blank("", "(unset)"), "(unset)");
        assert_eq!(blank("  ", "(unset)"), "(unset)");
        assert_eq!(blank("nomic-embed-text", "(unset)"), "nomic-embed-text");
    }
}
