//! The handful of global switches, at `llm/settings.toml`.
//!
//! ```toml
//! ask_on_switch = false     # ask before every switch, even a quota one
//! probe_every   = 300       # how often the prober looks for holds whose time is up
//! ```

use serde::{Deserialize, Serialize};

use adi_config::Module;

use crate::error::Result;
use crate::llm::LLM_MODULE;

/// The file the settings live in, within the [`llm`](crate::llm) module.
const SETTINGS_FILE: &str = "settings.toml";

/// How often the prober wakes to look for holds whose time is up. Five minutes: a probe is a real
/// model call, and a backend that came back four minutes ago is not worth a request a second to
/// notice.
pub const DEFAULT_PROBE_EVERY: u64 = 300;

/// The `llm/settings.toml` shape. Unknown fields are ignored, so an older store keeps loading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmSettings {
    /// Ask before *every* switch, including a quota one.
    ///
    /// Off by default, because the whole point of the feature is that a usage limit stops being the
    /// human's problem. It is here for the operator who wants nothing changing under them — and it
    /// only ever adds questions: `auth`, an unclassified error, and a chain with every row held
    /// stop and ask whatever this says, because those are not cases where a machine should pick.
    pub ask_on_switch: bool,
    /// How often the prober looks for due holds, in seconds. `0` stops it looking at all, which
    /// leaves holds to expire on their own.
    pub probe_every: u64,
}

impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            ask_on_switch: false,
            probe_every: DEFAULT_PROBE_EVERY,
        }
    }
}

impl LlmSettings {
    /// Read the settings, materializing the file from defaults on first use so it is there to edit.
    /// A corrupt file reads as the defaults: a settings file must never be the reason nothing can
    /// run.
    #[must_use]
    pub fn load(module: &Module) -> Self {
        module
            .file::<Self>(SETTINGS_FILE)
            .load_or_create()
            .unwrap_or_default()
    }

    /// Read them from a config root.
    #[must_use]
    pub fn open(config: &adi_config::Config) -> Self {
        Self::load(&config.module(LLM_MODULE))
    }

    /// Write them back.
    ///
    /// # Errors
    /// [`Error::Config`](crate::Error::Config) if the file cannot be written.
    pub fn save(&self, module: &Module) -> Result<()> {
        module.file(SETTINGS_FILE).save(self)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Module {
        let root = std::env::temp_dir().join(format!(
            "adi-agents-llm-settings-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_config::Config::with_root(root).module(LLM_MODULE)
    }

    /// The default is the quiet one: a usage limit reroutes without asking.
    #[test]
    fn a_fresh_store_does_not_ask_before_switching() {
        let module = scratch("default");
        let settings = LlmSettings::load(&module);
        assert!(!settings.ask_on_switch);
        assert_eq!(settings.probe_every, DEFAULT_PROBE_EVERY);
        assert!(
            module.dir().join(SETTINGS_FILE).exists(),
            "the file is materialized so it can be edited by hand"
        );
    }

    #[test]
    fn settings_round_trip() {
        let module = scratch("round-trip");
        LlmSettings {
            ask_on_switch: true,
            probe_every: 60,
        }
        .save(&module)
        .expect("save");

        let read = LlmSettings::load(&module);
        assert!(read.ask_on_switch);
        assert_eq!(read.probe_every, 60);
    }

    #[test]
    fn a_corrupt_file_reads_as_the_default() {
        let module = scratch("corrupt");
        module.ensure_dir().expect("mkdir");
        std::fs::write(module.dir().join(SETTINGS_FILE), "not = [toml").expect("write");
        assert_eq!(LlmSettings::load(&module), LlmSettings::default());
    }
}
