//! Guessing a new conversation's name from its opening message.
//!
//! [`spawn`] is the whole of it: fired once, right after a conversation opens, from a thread of its
//! own. Naming a chat is a nicety and never load-bearing, so nothing here is allowed to cost a
//! launch anything — not latency (the model is asked off the calling thread, never on it) and not
//! correctness (a model that cannot be reached, isn't pulled, or answers nonsense just leaves the
//! conversation titled the way [`SessionRecord::message`](crate::store::SessionRecord::message)
//! would have titled it anyway, exactly as if this module did not exist).
//!
//! # Why a local model, and why this one
//!
//! Free and instant is the only budget a background nicety gets — a hosted call would turn every
//! chat's opening line into a token spend nobody asked for, and this crate already leans on a local
//! [`Ollama`](https://ollama.com) the same way `adi-facts` does for fact extraction. It is a
//! separate client rather than a shared one, though: `adi-facts`'s classifier reasons over pairs of
//! facts and is tuned for that job's accuracy, while this asks one question — a few words summarising
//! one message — where speed matters far more than reasoning depth. `llama3.2:1b` answers a title in
//! well under a second on a CPU and, unlike the smaller Qwen2.5 0.5B tried against it, reliably stops
//! at the words asked for instead of padding the line with an explanation of what it did.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use adi_config::Module;

use crate::store::SessionStore;

/// The file the toggle lives in, within the [sessions module](crate::Agents::sessions).
const SETTINGS_FILE: &str = "auto_title.toml";

/// Whether [`spawn`] does anything at all. Lives beside [`RunLimits`](crate::RunLimits) — a
/// `sessions/auto_title.toml` next to `sessions/settings.toml` — because it is the same kind of
/// thing: a person's standing preference about how runs behave, not a fact about any one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoTitleSettings {
    /// On by default: the call is local and free, and a bad guess costs nothing more than
    /// [`crate::Agents::set_run_title`] to overwrite by hand — the same thing typing over any
    /// other title does.
    pub enabled: bool,
}

impl Default for AutoTitleSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl AutoTitleSettings {
    /// Read the toggle, materializing the file from the default on first use so it's there to edit.
    /// A corrupt or unreadable file reads as the default — a settings file must never be the reason
    /// every new chat stops getting named.
    #[must_use]
    pub fn load(module: &Module) -> Self {
        module
            .file::<Self>(SETTINGS_FILE)
            .load_or_create()
            .unwrap_or_default()
    }

    /// Write the toggle back.
    ///
    /// # Errors
    /// [`crate::Error::Config`] if the file can't be written.
    pub fn save(&self, module: &Module) -> crate::error::Result<()> {
        module.file(SETTINGS_FILE).save(self)?;
        Ok(())
    }
}

/// Where ollama listens for the title guesser, when nothing says otherwise. Deliberately its own
/// variable rather than shared with the harness's own Ollama provider
/// (`backends/harness/adi_loop.rs`): that one is a per-*agent* setting an operator points at
/// wherever they run their models, while this fires for every conversation on the machine and has
/// no business moving just because somebody repointed one agent.
const DEFAULT_HOST: &str = "http://127.0.0.1:11434";

/// The environment variable that moves the host.
const HOST_VAR: &str = "ADI_AUTO_TITLE_OLLAMA";

/// The model asked to name a chat. See the [module docs](self) for why this one.
const DEFAULT_MODEL: &str = "llama3.2:1b";

/// The environment variable that moves the model.
const MODEL_VAR: &str = "ADI_AUTO_TITLE_MODEL";

/// How long to wait for an answer before giving up. Generous for a 1B model's first token on a cold
/// CPU load, but this runs on a thread nobody is watching — there is no turn in flight for it to
/// hold up either way.
const TIMEOUT: Duration = Duration::from_secs(20);

/// The instruction. Deliberately narrow — a title, not a summary: the rail cuts one to 72
/// characters and a listing to 300, so anything longer is spent before a reader ever sees it.
const SYSTEM: &str = "You name conversations. You are given the opening message of a chat. Reply \
with a short title for it: 3 to 6 words, sentence case, no punctuation, no quotes, no prefix like \
\"Title:\". Reply with the title and nothing else.";

/// How much of the opening message reaches the model. A launch's task is routinely a full brief;
/// naming it needs only enough to see what it's about, and a shorter prompt answers faster.
const MAX_INPUT_CHARS: usize = 600;

/// How long a guess may be before it reads as the model ignoring the instruction rather than
/// naming the chat — at which point it is worth nothing over the title `message` derives anyway.
const MAX_TITLE_CHARS: usize = 80;

/// Ask the local model for a short title. `None` covers every way this can fail to help — the host
/// unreachable, the model not pulled, an answer with nothing usable in it — and every one of them
/// is the same outcome to a caller: nothing to write down.
fn ask(message: &str) -> Option<String> {
    let input: String = message.trim().chars().take(MAX_INPUT_CHARS).collect();
    if input.is_empty() {
        return None;
    }
    // reqwest is built with `rustls-no-provider` workspace-wide, so nobody installs a crypto
    // provider for us and `Client::build` fails until somebody does. Idempotent, so a second
    // install (another thread's guess, running at the same time) is a harmless `Err`.
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    let client = reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .ok()?;
    let host = env_or(HOST_VAR, DEFAULT_HOST);
    let model = env_or(MODEL_VAR, DEFAULT_MODEL);
    let body = json!({
        "model": model,
        "system": SYSTEM,
        "prompt": input,
        "stream": false,
        "think": false,
        "options": { "temperature": 0.2, "num_predict": 24 },
    });
    let response = client
        .post(format!("{}/api/generate", host.trim_end_matches('/')))
        .json(&body)
        .send()
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let answer: Value = response.json().ok()?;
    clean(answer["response"].as_str()?)
}

/// An environment variable, treating blank as unset — the same rule `adi-facts::ollama` uses, so an
/// exported-but-empty variable falls through to the default rather than building a URL with no host
/// in it.
fn env_or(key: &str, fallback: &str) -> String {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

/// Trim a model's answer into something worth writing down: its first line, stripped of the quotes
/// and trailing punctuation a small model routinely wraps its answer in. Refused (`None`) if what's
/// left is empty or absurdly long — either means the model answered something other than a title,
/// and a caller should treat that exactly like no answer at all.
fn clean(raw: &str) -> Option<String> {
    let line = raw.lines().next().unwrap_or("").trim();
    let trimmed = line.trim_matches(|c: char| "\"'.“”‘’".contains(c) || c.is_whitespace());
    if trimmed.is_empty() || trimmed.chars().count() > MAX_TITLE_CHARS {
        return None;
    }
    Some(trimmed.to_string())
}

/// Guess a title for a fresh conversation and write it down, off the thread that opened it.
///
/// Fire-and-forget by design — see the [module docs](self). Nothing here is retried, and nothing
/// surfaces its failure anywhere: a launch has already returned by the time this even starts
/// asking. The one race worth naming: if a person renames the chat by hand before the guess comes
/// back, the guess must not clobber it, so this writes only if the session **still has no title**
/// at the moment the answer lands.
pub(crate) fn spawn(config: adi_config::Config, agent: String, run_id: String, message: String) {
    if !AutoTitleSettings::load(&config.module(crate::SESSIONS_MODULE)).enabled {
        return;
    }
    std::thread::spawn(move || {
        let Some(title) = ask(&message) else {
            return;
        };
        let store = SessionStore::new(config.module(crate::SESSIONS_MODULE).dir());
        if store
            .get(&agent, &run_id)
            .is_some_and(|r| r.title.is_none())
        {
            let _ = store.set_title(&agent, &run_id, Some(&title));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Module {
        let root = std::env::temp_dir().join(format!(
            "adi-agents-auto-title-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_config::Config::with_root(root).module("sessions")
    }

    #[test]
    fn a_fresh_store_reads_enabled_and_writes_it_down() {
        let module = scratch("default");
        assert!(AutoTitleSettings::load(&module).enabled);
        assert!(
            module.dir().join(SETTINGS_FILE).exists(),
            "materialized so it can be edited by hand"
        );
    }

    #[test]
    fn the_toggle_round_trips() {
        let module = scratch("round-trip");
        AutoTitleSettings { enabled: false }
            .save(&module)
            .expect("save");
        assert!(!AutoTitleSettings::load(&module).enabled);
    }

    #[test]
    fn a_corrupt_file_reads_as_enabled() {
        let module = scratch("corrupt");
        module.ensure_dir().expect("mkdir");
        std::fs::write(module.dir().join(SETTINGS_FILE), "not = [toml").expect("write");
        assert!(AutoTitleSettings::load(&module).enabled);
    }

    /// The real thing, against a real model — not run by default because most machines running
    /// this suite have no ollama at all, let alone [`DEFAULT_MODEL`] pulled.
    /// `ollama pull llama3.2:1b`, then `cargo test -p adi-agents -- --ignored a_live_model`.
    #[test]
    #[ignore = "needs a live ollama with llama3.2:1b pulled"]
    fn a_live_model_names_a_real_message() {
        let title = ask(
            "Can you help me fix a bug where the login form crashes when the password field \
             is empty? It happens on Safari only.",
        )
        .expect("a live ollama should answer");
        assert!(!title.is_empty());
        assert!(title.chars().count() <= MAX_TITLE_CHARS);
    }

    /// The whole path a launch actually takes: `Agents::run_in` opens the session, [`spawn`] fires
    /// off it, and the title lands on the record with nobody having polled for it — a live model
    /// and a real (if idle) launch, not the pieces exercised in isolation.
    #[test]
    #[ignore = "needs a live ollama with llama3.2:1b pulled"]
    fn a_real_launch_is_retitled_without_anybody_asking() {
        use crate::{RawAgentArguments, StoredAgentManifest};

        let root = std::env::temp_dir().join(format!(
            "adi-agents-auto-title-live-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        let store = crate::Agents::with_config(adi_config::Config::with_root(&root));
        let mut arguments = RawAgentArguments::new();
        arguments.insert("provider".into(), "ollama".into());
        arguments.insert("model".into(), DEFAULT_MODEL.into());
        store
            .save(
                "solver",
                StoredAgentManifest {
                    backend: "harness:adi".into(),
                    arguments,
                    ..Default::default()
                },
            )
            .expect("save");

        // The turn itself may still fail or take a while — this points the harness's own model
        // call at the same ollama the guesser uses, but the two are unrelated calls and nothing
        // here waits on the turn. What's under test is that the session exists and gets a title
        // regardless of how the conversation itself goes.
        store
            .run_in(
                "solver",
                "Can you help me fix a bug where the login form crashes when the password field \
                 is empty? It happens on Safari only.",
                None,
            )
            .expect("launch");
        let run_id = store
            .sessions()
            .list("solver")
            .first()
            .expect("the session exists")
            .id
            .clone();

        let title = (0..100)
            .find_map(|_| {
                std::thread::sleep(Duration::from_millis(200));
                store.sessions().get("solver", &run_id)?.title
            })
            .expect("the guess lands within 20s");
        assert!(!title.is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_models_answer_is_trimmed_of_the_wrapping_it_routinely_adds() {
        assert_eq!(
            clean("\"Fixing the login bug\"\n"),
            Some("Fixing the login bug".to_string())
        );
        assert_eq!(clean("Weekly report.  "), Some("Weekly report".to_string()));
        assert_eq!(clean(""), None);
        assert_eq!(clean("   "), None);
        assert_eq!(
            clean(&"word ".repeat(30)),
            None,
            "a runaway answer is refused rather than truncated into a fake title"
        );
    }
}
