//! adi-triggers — trigger definitions ([`TriggerManifest`]) for the adi platform: a library
//! over the shared [`adi_config`] store. A trigger is a named code block plus one fact about
//! *how it launches*, and there are only two ways:
//!
//! * [`KIND_WEBHOOK`] — launched by an inbound call to the app's `/api/hooks/<name>`, with the
//!   request body as its payload.
//! * [`KIND_BACKGROUND`] — a long-lived independent process, kept alive by [`Supervisor`] for
//!   as long as the trigger is enabled.
//!
//! What a trigger *does* is its code block — shell or TypeScript, per its
//! [runtime](RUNTIME_TS) — prefilled from a [preset](presets) rather than baked into a kind.
//! Each trigger is one `<name>.toml` file under `~/.adi/mono/triggers/`.
//!
//! Both launch paths run through the same layer: [`Triggers::fire`] spawns a code block
//! detached (own process group, output to `triggers/logs/<name>.log`, settings exported as
//! `ADI_<KEY>`, payload via `ADI_PAYLOAD_FILE`), and [`Supervisor`] does the same under tokio
//! so it can wait on the process and relaunch it. Status is published to
//! `triggers/run/<name>.toml` so [`Triggers::status`] answers "is it up?" from any process.
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
pub mod presets;
mod run;
#[cfg(feature = "supervisor")]
mod dispatch;
#[cfg(feature = "supervisor")]
mod supervisor;
mod trigger;

use std::path::PathBuf;

use adi_config::{Config, ConfigFile, now_unix};

pub use error::{Error, Result};
pub use fire::Firing;
pub use presets::{Preset, PresetField};
pub use run::{RunState, Status};
#[cfg(feature = "supervisor")]
pub use dispatch::EventDispatcher;
#[cfg(feature = "supervisor")]
pub use supervisor::Supervisor;
pub use trigger::{
    KIND_BACKGROUND, KIND_EVENT, KIND_WEBHOOK, RUNTIME_SH, RUNTIME_TS, Trigger, TriggerManifest,
    normalize_kind, normalize_runtime, payload_project,
};

use trigger::validate_name;

/// The store module triggers live under, and each trigger file's extension.
const TRIGGERS_MODULE: &str = "triggers";
const MANIFEST_EXT: &str = "toml";

/// The trigger registry: lists, reads, mutates, and fires the per-trigger manifests under the
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

    /// Open the registry backed by a caller-supplied [`Config`] — for tests or alternate installations.
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
        self.config.module(TRIGGERS_MODULE).manifest_file(name)
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
            let mut manifest = self.trigger_file(name).load()?;
            manifest.normalize();
            triggers.push(Trigger {
                name: name.to_string(),
                manifest,
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
        let mut manifest = file.load()?;
        manifest.normalize();
        Ok(Some(Trigger {
            name: name.to_string(),
            manifest,
        }))
    }

    /// Create or overwrite a trigger definition (an upsert), writing its `<name>.toml`. The
    /// store owns the timestamps: `created_at` is preserved across edits (set once on first
    /// save), `updated_at` is stamped every save — any values in `manifest` are ignored.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe name, or [`Error::Config`] on a writing failure.
    pub fn save(&self, name: &str, mut manifest: TriggerManifest) -> Result<Trigger> {
        validate_name(name)?;
        // Fold the incoming kind/runtime onto the live set, so a caller passing a retired kind
        // (or nothing at all) still writes a manifest the supervisor understands.
        manifest.normalize();
        let file = self.trigger_file(name);
        let now = now_unix();
        manifest.created_at = file.carried_created_at(now);
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
        let secret_env = self.secret_env(trigger.manifest.project.as_deref());
        fire::fire(&self.dir(), &trigger, payload, None, &secret_env)
    }

    /// Fire a registered trigger *because a platform event matched it*: like [`fire`](Self::fire),
    /// but the concrete event name is handed to the code block as `ADI_EVENT` in addition to the
    /// payload. This is the path the [`EventDispatcher`] takes for every [event
    /// trigger](KIND_EVENT) whose patterns match a drained event.
    ///
    /// # Errors
    /// [`Error::NotFound`] for an unregistered name, plus everything the spawn can return
    /// ([`Error::NoCode`], [`Error::Io`], [`Error::Launch`]).
    pub fn fire_event(&self, name: &str, event: &str, payload: Option<&[u8]>) -> Result<Firing> {
        let trigger = self
            .get(name)?
            .ok_or_else(|| Error::NotFound(name.to_string()))?;
        let secret_env = self.secret_env(trigger.manifest.project.as_deref());
        fire::fire(&self.dir(), &trigger, payload, Some(event), &secret_env)
    }

    /// The secret environment a trigger's code block runs with: every global secret plus the
    /// trigger's project's, resolved (project overrides global) into literal `KEY=value` pairs
    /// — a namespace distinct from the `ADI_<KEY>` settings. Resolved against **this store's**
    /// [`Config`], so a test store stays isolated. Best-effort: if the secrets store can't be
    /// read or decrypted, the launch proceeds with no injected secrets rather than failing.
    pub(crate) fn secret_env(&self, project: Option<&str>) -> Vec<(String, String)> {
        adi_secrets::Secrets::with_config(self.config.clone())
            .resolve(project)
            .unwrap_or_default()
            .into_iter()
            .collect()
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

    /// Whether a background trigger is up right now, and since when — read from the state its
    /// supervisor publishes, so this answers correctly from *any* process (the CLI included,
    /// even though supervision happens inside the app). `None` means nothing is running it: a
    /// webhook trigger, a disabled one, or one whose supervisor has gone away.
    #[must_use]
    pub fn status(&self, name: &str) -> Status {
        validate_name(name).ok()?;
        let state: RunState = self.run_file(name).load().ok()?;
        state.is_live().then_some(state)
    }

    /// Every published run state, as `(trigger name, state)` — including stale ones, which is
    /// the point: a supervisor starting up uses these to find processes a previous one left
    /// behind. Unreadable entries are skipped rather than failing the sweep.
    #[must_use]
    pub fn published_run_states(&self) -> Vec<(String, RunState)> {
        let dir = self.dir().join(run::RUN_DIR);
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| {
                let file_name = entry.file_name().into_string().ok()?;
                let name = file_name
                    .strip_suffix(&format!(".{MANIFEST_EXT}"))?
                    .to_string();
                validate_name(&name).ok()?;
                let state = self.run_file(&name).load().ok()?;
                Some((name, state))
            })
            .collect()
    }

    /// The run-state file for `name`, at `triggers/run/<name>.toml`.
    fn run_file(&self, name: &str) -> ConfigFile<RunState> {
        self.config
            .module(TRIGGERS_MODULE)
            .file(&format!("{}/{name}.{MANIFEST_EXT}", run::RUN_DIR))
    }

    /// Publish a supervised trigger's run state for other processes to read. Best-effort: a
    /// status file that can't be written costs a status readout, never the running process.
    pub(crate) fn publish_run_state(&self, name: &str, state: &RunState) {
        if validate_name(name).is_ok() {
            let _ = self.run_file(name).save(state);
        }
    }

    /// Withdraw a trigger's published run state — it is no longer running.
    pub(crate) fn clear_run_state(&self, name: &str) {
        if validate_name(name).is_ok() {
            let _ = self
                .config
                .module(TRIGGERS_MODULE)
                .remove_raw(&format!("{}/{name}.{MANIFEST_EXT}", run::RUN_DIR));
        }
    }

    /// Delete a trigger definition (its fire log is kept — history is cheap and separate).
    /// Returns `false` if it wasn't registered.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe name, or [`Error::Config`] on a removal failure.
    pub fn delete(&self, name: &str) -> Result<bool> {
        validate_name(name)?;
        Ok(self.config.module(TRIGGERS_MODULE).remove_manifest(name)?)
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
        assert_eq!(
            saved.manifest.extra.get("secret").map(String::as_str),
            Some("s3cr3t")
        );
        assert!(saved.manifest.enabled);
        assert!(saved.manifest.created_at > 0);

        let got = store.get("deploy-hook").expect("get").expect("present");
        assert_eq!(got, saved);
        assert_eq!(store.list().expect("list").len(), 1);
    }

    #[test]
    fn save_is_an_upsert_that_preserves_created_at() {
        let store = scratch("upsert");
        let first = store
            .save("a", spec(KIND_BACKGROUND, "true"))
            .expect("create");
        let created = first.manifest.created_at;
        assert!(created > 0);

        let mut edited = spec(KIND_BACKGROUND, "date");
        edited.enabled = false;
        let second = store.save("a", edited).expect("update");
        assert_eq!(second.manifest.kind, KIND_BACKGROUND);
        assert!(!second.manifest.enabled);
        assert_eq!(second.manifest.created_at, created);
        assert_eq!(store.list().expect("list").len(), 1);
    }

    #[test]
    fn delete_removes_the_trigger() {
        let store = scratch("delete");
        store
            .save("gone", spec(KIND_BACKGROUND, "true"))
            .expect("create");
        assert!(store.delete("gone").expect("delete"));
        assert!(store.get("gone").expect("get").is_none());
        assert!(!store.delete("gone").expect("delete missing"));
    }

    #[test]
    fn invalid_names_never_touch_disk() {
        let store = scratch("invalid");
        assert!(matches!(store.get("../escape"), Err(Error::InvalidName(_))));
        assert!(matches!(
            store.save("a/b", spec(KIND_BACKGROUND, "true")),
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
            .save("pinger", spec(KIND_BACKGROUND, "printf fired"))
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
