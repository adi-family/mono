//! The on-demand test — a human presses "Test" and finds out, right now, whether a backend embeds.
//!
//! There is no prober here to be distinct from: an embedding backend never holds, never fails over
//! silently and is never asked on a timer (see "the same-model failover rule" in
//! `docs/embedding-backends.md` for why the LLM design's background sweep has no counterpart here).
//! This is the first thing that asks one at all, other than a real caller storing real vectors.
//!
//! Embedding one short string and reporting success is not the whole test. **The width is the
//! test.** Every vector a base holds is recorded against the backend's declared
//! [`dimensions`](EmbeddingBackendManifest::dimensions), and a runtime that quietly returns a
//! different width poisons every future search against that base rather than failing loudly — so a
//! mismatch here is reported as a failure, not as a warning beside a green checkmark.

use std::time::Instant;

use adi_config::Config;

use crate::backend::{EmbeddingBackendManifest, EmbeddingBackends, build_embedder};
use crate::error::{Error, Result};

/// Short, and specific enough that a mangled response (truncated, wrong encoding) would not embed
/// identically to an empty string — a couple of words is plenty for a test that never reads the
/// vector's content, only its shape and the fact that it arrived.
const TEST_TEXT: &str = "the quick brown fox jumps over the lazy dog";

/// What a test found, and how long it took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestResult {
    pub verdict: TestVerdict,
    /// Wall-clock time the embed call itself took — not the store lookup or the width check that
    /// follows it, both of which are local and free.
    pub elapsed_ms: u64,
}

impl TestResult {
    /// One line for a human — what happened, and the width it happened at.
    #[must_use]
    pub fn message(&self) -> String {
        match &self.verdict {
            TestVerdict::Answered { dimensions } => {
                format!("embedded into {dimensions} dimensions in {}ms", self.elapsed_ms)
            }
            TestVerdict::Failed { error } => error.clone(),
        }
    }
}

/// The outcome. There is no rate-limited state to distinguish here the way an LLM backend has one:
/// none of the four runtimes' failure modes are usefully "try again later" rather than "broken",
/// so every failure is reported the same way, in the runtime's own words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestVerdict {
    /// It embedded, at this many dimensions — the number that matters, since a width mismatch
    /// against the manifest's declared one turns an apparent success into a failure instead.
    Answered { dimensions: u32 },
    /// Failed to embed at all, or embedded at a width that disagrees with the manifest's own.
    Failed { error: String },
}

impl TestVerdict {
    /// The wire tag this verdict is reported under — the vocabulary the API and the CLI's `--json`
    /// share.
    #[must_use]
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Answered { .. } => "ok",
            Self::Failed { .. } => "failed",
        }
    }
}

/// Test a saved backend by id.
///
/// # Errors
/// [`Error::NotFound`] when there is no such backend; otherwise a store error reading it.
pub fn test_backend(config: &Config, id: &str) -> Result<TestResult> {
    let backend = EmbeddingBackends::with_config(config.clone())
        .get(id)?
        .ok_or_else(|| Error::NotFound(id.to_string()))?;
    Ok(test_manifest(&backend.manifest))
}

/// Test a backend that may not be saved at all — a draft an operator is still typing into the
/// panel. Takes a manifest rather than an id for the same reason [`crate::llm`]'s equivalent does
/// (see `adi_agents::llm::ondemand`): the point of a "Test" button beside a form is to test the form
/// as it stands.
#[must_use]
pub fn test_manifest(manifest: &EmbeddingBackendManifest) -> TestResult {
    let start = Instant::now();
    let outcome = ask(manifest);
    let elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    let verdict = match outcome {
        Ok(dimensions) => TestVerdict::Answered { dimensions },
        Err(error) => TestVerdict::Failed { error },
    };
    TestResult { verdict, elapsed_ms }
}

/// Build the manifest's own runtime, embed [`TEST_TEXT`] once, and check the width it came back at
/// against what the manifest declares.
fn ask(manifest: &EmbeddingBackendManifest) -> std::result::Result<u32, String> {
    let embedder = build_embedder(manifest).map_err(|e| e.to_string())?;
    let vectors = embedder.embed(&[TEST_TEXT]).map_err(|e| e.to_string())?;
    let vector = vectors
        .into_iter()
        .next()
        .ok_or_else(|| "the runtime returned no vector at all".to_string())?;
    let got = u32::try_from(vector.len()).unwrap_or(u32::MAX);
    if manifest.dimensions != 0 && got != manifest.dimensions {
        return Err(format!(
            "returned a {got}-dimension vector, but this backend declares {} — a wrong width \
             poisons every base that trusts it",
            manifest.dimensions
        ));
    }
    Ok(got)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Runtime;

    fn hash_manifest() -> EmbeddingBackendManifest {
        EmbeddingBackendManifest {
            runtime: Runtime::Hash,
            model: crate::runtimes::hash::MODEL_NAME.to_string(),
            dimensions: crate::runtimes::hash::HASH_DIMENSIONS,
            ..Default::default()
        }
    }

    /// The one runtime every build can always resolve is also the one a test can always exercise
    /// without a network, so it is what proves the whole path end to end.
    #[test]
    fn a_hash_backend_answers_at_its_declared_width() {
        let result = test_manifest(&hash_manifest());
        assert_eq!(
            result.verdict,
            TestVerdict::Answered { dimensions: crate::runtimes::hash::HASH_DIMENSIONS },
        );
    }

    /// The whole reason this test exists rather than a bare "it answered": a backend that declares
    /// the wrong width for its own runtime must fail loudly, not report success at the wrong number.
    #[test]
    fn a_declared_width_that_disagrees_with_the_runtime_is_a_failure() {
        let manifest = EmbeddingBackendManifest {
            dimensions: crate::runtimes::hash::HASH_DIMENSIONS + 1,
            ..hash_manifest()
        };
        let result = test_manifest(&manifest);
        assert!(
            matches!(&result.verdict, TestVerdict::Failed { error } if error.contains("wrong width")),
            "{result:?}"
        );
    }

    /// A backend that has never declared a width yet (mid-edit in the panel, say) is not failed for
    /// disagreeing with zero — there is nothing to disagree with.
    #[test]
    fn an_undeclared_width_is_not_checked_against() {
        let manifest = EmbeddingBackendManifest { dimensions: 0, ..hash_manifest() };
        let result = test_manifest(&manifest);
        assert!(matches!(result.verdict, TestVerdict::Answered { .. }), "{result:?}");
    }

    #[test]
    fn an_unknown_backend_id_is_not_found() {
        let root = std::env::temp_dir().join(format!(
            "adi-embeddings-ondemand-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        let config = Config::with_root(root);
        assert!(test_backend(&config, "nowhere").is_err());
    }
}
