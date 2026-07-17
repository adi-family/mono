//! adi-triggers — trigger definitions ([`TriggerManifest`]) for the adi platform: a pure
//! library (no CLI, no daemon) over the shared [`adi_config`] store. A trigger is a named
//! background code block fired by an external event source — a webhook call, a Telegram bot,
//! a cron schedule, or a manual fire. Each trigger is one `<name>.toml` file under
//! `~/.adi/mono/triggers/`, holding its kind, shell code block, enabled flag, and
//! kind-specific extras.
//!
//! This is the **definition/store layer** plus the first slice of the run layer:
//! [`Triggers::fire`] spawns the code block detached (own process group, output to
//! `triggers/logs/<name>.log`, payload via `ADI_PAYLOAD_FILE`). Live listeners — a Telegram
//! poller, a cron scheduler — are future work; today's event sources are the app's
//! `/api/hooks/<name>` webhook endpoint and explicit manual fires.
//!
//! ```
//! # let tmp = std::env::temp_dir().join(format!("adi-triggers-doctest-{}", std::process::id()));
//! # let _ = std::fs::remove_dir_all(&tmp);
//! use adi_triggers::{Triggers, TriggerManifest};
//!
//! # let store = Triggers::with_config(adi_config::Config::with_root(&tmp));
//! // In real code: let store = Triggers::open();
//! let mut spec = TriggerManifest::default();
//! spec.kind = "webhook".into();
//! spec.code = "echo deployed".into();
//! let saved = store.save("deploy-hook", spec)?;
//! assert_eq!(saved.name, "deploy-hook");
//! assert!(saved.manifest.enabled);
//! assert!(saved.manifest.created_at > 0);
//!
//! assert_eq!(store.list()?.len(), 1);
//! assert!(store.delete("deploy-hook")?);
//! # std::fs::remove_dir_all(&tmp).ok();
//! # Ok::<(), adi_triggers::Error>(())
//! ```

mod error;
mod fire;
mod trigger;

use std::path::PathBuf;

use adi_config::{Config, ConfigFile};

pub use error::{Error, Result};
pub use fire::Firing;
pub use trigger::{
    KIND_CRON, KIND_MANUAL, KIND_TELEGRAM, KIND_WEBHOOK, Trigger, TriggerManifest,
};

use trigger::{now_unix, validate_name};

/// The store module triggers live under, and each trigger file's extension.
const TRIGGERS_MODULE: &str = "triggers";
const MANIFEST_EXT: &str = "toml";

/// The triggers registry: lists, reads, mutates, and fires the per-trigger manifests under the
/// `triggers` module dir. Cheap to clone; all state is on disk.
#[derive(Debug, Clone)]
pub struct Triggers {
    config: Config,
}

impl Default for Triggers {
    fn default() -> Self {
        Self::open()
    }
}

impl Triggers {
    /// Open the registry backed by the standard store (`~/.adi/mono`, honoring `$ADI_DIR`).
    #[must_use]
    pub fn open() -> Self {
        Self {
            config: Config::open(),
        }
    }

    /// Open the registry backed by a caller-supplied [`Config`] — for tests or alternate installs.
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self { config }
    }

    /// The store this registry reads from.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The `triggers` directory: `~/.adi/mono/triggers`.
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.config.module(TRIGGERS_MODULE).dir().to_path_buf()
    }

    /// The manifest file handle for `name`, at `triggers/<name>.toml` (touches no disk).
    fn trigger_file(&self, name: &str) -> ConfigFile<TriggerManifest> {
        self.config
            .module(TRIGGERS_MODULE)
            .file(&format!("{name}.{MANIFEST_EXT}"))
    }

    /// Every registered trigger, sorted by name. A file without a `.toml` extension is skipped;
    /// a missing `triggers/` dir yields an empty list.
    ///
    /// # Errors
    /// [`Error::Io`] on a directory read failure, or [`Error::Config`] if a manifest is invalid TOML.
    pub fn list(&self) -> Result<Vec<Trigger>> {
        let entries = match std::fs::read_dir(self.dir()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::Io(e)),
        };

        let mut triggers = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let Ok(file_name) = entry.file_name().into_string() else {
                continue;
            };
            let Some(name) = file_name.strip_suffix(&format!(".{MANIFEST_EXT}")) else {
                continue;
            };
            if validate_name(name).is_err() {
                continue;
            }
            triggers.push(Trigger {
                name: name.to_string(),
                manifest: self.trigger_file(name).load()?,
            });
        }
        triggers.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(triggers)
    }

    /// The trigger with this name, or `None` if it isn't registered.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe name, or [`Error::Config`] if the manifest is invalid TOML.
    pub fn get(&self, name: &str) -> Result<Option<Trigger>> {
        validate_name(name)?;
        let file = self.trigger_file(name);
        if !file.exists() {
            return Ok(None);
        }
        Ok(Some(Trigger {
            name: name.to_string(),
            manifest: file.load()?,
        }))
    }

    /// Create or overwrite a trigger definition (an upsert), writing its `<name>.toml`. The
    /// store owns the timestamps: `created_at` is preserved across edits (set once on first
    /// save), `updated_at` is stamped every save — any values in `manifest` are ignored.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe name, or [`Error::Config`] on a write failure.
    pub fn save(&self, name: &str, mut manifest: TriggerManifest) -> Result<Trigger> {
        validate_name(name)?;
        let file = self.trigger_file(name);
        let now = now_unix();
        // Preserve the original creation time on edit; stamp a fresh one on first save.
        manifest.created_at = match file.load() {
            Ok(existing) if existing.created_at > 0 => existing.created_at,
            _ => now,
        };
        manifest.updated_at = now;
        file.save(&manifest)?;
        Ok(Trigger {
            name: name.to_string(),
            manifest,
        })
    }

    /// Fire a registered trigger: spawn its code block detached, with `payload` (an event body,
    /// if the source carries one) handed over via `ADI_PAYLOAD_FILE`/`ADI_PAYLOAD`. This is the
    /// mechanism every event source funnels into — the webhook endpoint passes the request body,
    /// a manual fire passes `None`. Enabled-ness is *not* checked here: gating belongs to the
    /// event source (a manual fire is an explicit user action and always allowed).
    ///
    /// # Errors
    /// [`Error::NotFound`] for an unregistered name, plus everything the spawn can return
    /// ([`Error::NoCode`], [`Error::Io`], [`Error::Launch`]).
    pub fn fire(&self, name: &str, payload: Option<&[u8]>) -> Result<Firing> {
        let trigger = self
            .get(name)?
            .ok_or_else(|| Error::NotFound(name.to_string()))?;
        fire::fire(&self.dir(), &trigger, payload)
    }

    /// When the trigger last fired (Unix epoch seconds, from its log's mtime), or `None` if it
    /// never fired or isn't safely named.
    #[must_use]
    pub fn last_fired(&self, name: &str) -> Option<u64> {
        validate_name(name).ok()?;
        fire::last_fired(&self.dir(), name)
    }

    /// The tail of the trigger's most recent fire log, or `None` if it never fired or isn't
    /// safely named.
    #[must_use]
    pub fn read_log(&self, name: &str) -> Option<String> {
        validate_name(name).ok()?;
        fire::read_log(&self.dir(), name)
    }

    /// Delete a trigger definition (its fire log is kept — history is cheap and separate).
    /// Returns `false` if it wasn't registered.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe name, or [`Error::Config`] on a removal failure.
    pub fn delete(&self, name: &str) -> Result<bool> {
        validate_name(name)?;
        Ok(self
            .config
            .module(TRIGGERS_MODULE)
            .remove_raw(&format!("{name}.{MANIFEST_EXT}"))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Triggers {
        let root = std::env::temp_dir().join(format!(
            "adi-triggers-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Triggers::with_config(Config::with_root(root))
    }

    fn spec(kind: &str, code: &str) -> TriggerManifest {
        TriggerManifest {
            kind: kind.into(),
            code: code.into(),
            ..TriggerManifest::default()
        }
    }

    #[test]
    fn save_then_get_and_list_round_trip() {
        let store = scratch("crud");
        assert!(store.list().expect("empty list").is_empty());

        let mut m = spec(KIND_WEBHOOK, "echo deployed");
        m.description = "Redeploy on push".into();
        m.project = Some("demo".into());
        m.extra.insert("secret".into(), "s3cr3t".into());
        let saved = store.save("deploy-hook", m).expect("save");
        assert_eq!(saved.name, "deploy-hook");
        assert_eq!(saved.manifest.kind, KIND_WEBHOOK);
        assert_eq!(saved.manifest.project.as_deref(), Some("demo"));
        assert_eq!(saved.manifest.extra.get("secret").map(String::as_str), Some("s3cr3t"));
        assert!(saved.manifest.enabled);
        assert!(saved.manifest.created_at > 0);

        let got = store.get("deploy-hook").expect("get").expect("present");
        assert_eq!(got, saved);
        assert_eq!(store.list().expect("list").len(), 1);
    }

    #[test]
    fn save_is_an_upsert_that_preserves_created_at() {
        let store = scratch("upsert");
        let first = store.save("a", spec(KIND_MANUAL, "true")).expect("create");
        let created = first.manifest.created_at;
        assert!(created > 0);

        let mut edited = spec(KIND_CRON, "date");
        edited.enabled = false;
        let second = store.save("a", edited).expect("update");
        assert_eq!(second.manifest.kind, KIND_CRON);
        assert!(!second.manifest.enabled);
        assert_eq!(second.manifest.created_at, created);
        assert_eq!(store.list().expect("list").len(), 1);
    }

    #[test]
    fn delete_removes_the_trigger() {
        let store = scratch("delete");
        store.save("gone", spec(KIND_MANUAL, "true")).expect("create");
        assert!(store.delete("gone").expect("delete"));
        assert!(store.get("gone").expect("get").is_none());
        assert!(!store.delete("gone").expect("delete missing"));
    }

    #[test]
    fn invalid_names_never_touch_disk() {
        let store = scratch("invalid");
        assert!(matches!(store.get("../escape"), Err(Error::InvalidName(_))));
        assert!(matches!(
            store.save("a/b", spec(KIND_MANUAL, "true")),
            Err(Error::InvalidName(_))
        ));
        assert!(matches!(store.delete(".."), Err(Error::InvalidName(_))));
        assert_eq!(store.last_fired("../x"), None);
        assert_eq!(store.read_log("../x"), None);
    }

    #[test]
    fn firing_an_unknown_trigger_is_not_found() {
        let store = scratch("fire-missing");
        assert!(matches!(store.fire("ghost", None), Err(Error::NotFound(_))));
    }

    #[test]
    fn fire_spawns_and_the_log_becomes_readable() {
        let store = scratch("fire");
        store
            .save("pinger", spec(KIND_MANUAL, "printf fired"))
            .expect("create");
        let firing = store.fire("pinger", None).expect("fire");
        assert!(firing.pid > 0);
        for _ in 0..100 {
            if store.read_log("pinger").as_deref() == Some("fired") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(store.read_log("pinger").as_deref(), Some("fired"));
        assert!(store.last_fired("pinger").is_some());
    }
}
