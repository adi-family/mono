//! adi-secrets — encrypted, scoped secrets for the adi platform: a pure library (no CLI, no
//! daemon) over the shared [`adi_config`] store. Each secret is one AEAD-encrypted
//! `<name>.toml` under `~/.adi/mono/secrets/`, in one of two scopes:
//!
//! * **global** — `secrets/global/<name>.toml`, available everywhere.
//! * **per-project** — `secrets/projects/<project-id>/<name>.toml`, scoped to an
//!   [`adi-projects`](adi_config) id, and overriding a global secret of the same name when a
//!   run [resolves](Secrets::resolve) its environment.
//!
//! Scope is the platform-wide `project: Option<&str>` convention — `None` is global. Values
//! are encrypted at rest (see [`crypto`]); metadata (name, description, timestamps) is
//! plaintext. Only [`reveal`](Secrets::reveal) and [`resolve`](Secrets::resolve) ever produce
//! a plaintext value; [`list`](Secrets::list)/[`get`](Secrets::get) return metadata alone.
//!
//! ```
//! # let tmp = std::env::temp_dir().join(format!("adi-secrets-doctest-{}", std::process::id()));
//! # let _ = std::fs::remove_dir_all(&tmp);
//! use adi_secrets::Secrets;
//!
//! # let store = Secrets::with_config(adi_config::Config::with_root(&tmp));
//! // In real code: let store = Secrets::open();
//! store.set(None, "API_KEY", "s3cr3t", Some("demo key"))?;
//! assert_eq!(store.reveal(None, "API_KEY")?.as_deref(), Some("s3cr3t"));
//!
//! // A project-scoped secret overrides the global one of the same name on resolve.
//! store.set(Some("proj"), "API_KEY", "proj-value", None)?;
//! let env = store.resolve(Some("proj"))?;
//! assert_eq!(env.get("API_KEY").map(String::as_str), Some("proj-value"));
//! # std::fs::remove_dir_all(&tmp).ok();
//! # Ok::<(), adi_secrets::Error>(())
//! ```

mod crypto;
mod error;
mod secret;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use adi_config::{Config, ConfigFile, now_unix};

pub use error::{Error, Result};
pub use secret::{OAuthInfo, OAuthToken, Secret};

use secret::{Manifest, OAuthMeta, validate_name, validate_project};

/// The store module secrets live under, and the two scope subdirectories within it.
const SECRETS_MODULE: &str = "secrets";
const GLOBAL_DIR: &str = "global";
const PROJECTS_DIR: &str = "projects";

/// The secrets store: sets, reads, and resolves per-scope encrypted secrets under the
/// `secrets` module dir. Cheap to clone; all state is on disk.
#[derive(Debug, Clone)]
pub struct Secrets {
    config: Config,
}

impl Default for Secrets {
    fn default() -> Self {
        Self::open()
    }
}

impl Secrets {
    /// Open the store backed by the standard store (`~/.adi/mono`, honoring `$ADI_DIR`).
    #[must_use]
    pub fn open() -> Self {
        Self {
            config: Config::open(),
        }
    }

    /// Open the store backed by a caller-supplied [`Config`] — for tests or alternate installs.
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self { config }
    }

    /// The store this reads from.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The `secrets` directory: `~/.adi/mono/secrets`.
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.config.module(SECRETS_MODULE).dir().to_path_buf()
    }

    /// Set (create or overwrite) a secret in a scope: `None` for global, `Some(project-id)`
    /// for a project. The value is encrypted at rest; `created_at` is preserved across an
    /// overwrite. Returns the metadata [`Secret`] (never the value).
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe name or project id, [`Error::Crypto`] on a key or
    /// cipher failure, or [`Error::Config`]/[`Error::Io`] on a write failure.
    pub fn set(
        &self,
        project: Option<&str>,
        name: &str,
        value: &str,
        description: Option<&str>,
    ) -> Result<Secret> {
        let file = self.manifest_file(project, name)?;
        self.ensure_scope_dir(project)?;

        let key = crypto::load_or_create_key(&self.dir())?;
        let (nonce, ciphertext) = crypto::encrypt(&key, &aad(project, name), value.as_bytes())?;

        let now = now_unix();
        // Preserve the original creation time when overwriting an existing secret.
        let created_at = file
            .load()
            .ok()
            .map_or(now, |m: Manifest| if m.created_at == 0 { now } else { m.created_at });

        let manifest = Manifest {
            description: clean(description),
            nonce,
            ciphertext,
            created_at,
            updated_at: now,
            // A plain `set` makes (or keeps) this a text secret — any prior OAuth provenance is
            // dropped, since the value no longer came from a provider.
            oauth: None,
        };
        file.save(&manifest)?;
        harden_file(file.path())?;
        Ok(view(project, name, &manifest))
    }

    /// Set (create or overwrite) a secret from an OAuth token. The access token becomes the
    /// secret's encrypted value (so it injects into runs exactly like a text secret); the
    /// refresh token is encrypted separately; the provider, lifetime, and scope are stored as
    /// metadata. `created_at` is preserved across a refresh. Returns the metadata [`Secret`].
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe name / project id / provider, [`Error::Crypto`] on a
    /// key or cipher failure, or [`Error::Config`]/[`Error::Io`] on a write failure.
    pub fn set_oauth(
        &self,
        project: Option<&str>,
        name: &str,
        token: &OAuthToken,
        description: Option<&str>,
    ) -> Result<Secret> {
        let file = self.manifest_file(project, name)?;
        // The provider is later joined into a URL path (`/refresh/<provider>`), so it must be a
        // safe segment too.
        validate_project(&token.provider)?;
        self.ensure_scope_dir(project)?;

        let key = crypto::load_or_create_key(&self.dir())?;
        let (nonce, ciphertext) =
            crypto::encrypt(&key, &aad(project, name), token.access_token.as_bytes())?;
        let (refresh_nonce, refresh_ciphertext) = match token.refresh_token.as_deref() {
            Some(rt) if !rt.is_empty() => {
                let (n, c) = crypto::encrypt(&key, &aad_refresh(project, name), rt.as_bytes())?;
                (Some(n), Some(c))
            }
            _ => (None, None),
        };

        let now = now_unix();
        let created_at = file
            .load()
            .ok()
            .map_or(now, |m: Manifest| if m.created_at == 0 { now } else { m.created_at });

        let manifest = Manifest {
            description: clean(description),
            nonce,
            ciphertext,
            created_at,
            updated_at: now,
            oauth: Some(OAuthMeta {
                provider: token.provider.clone(),
                obtained_at: now,
                expires_at: token.expires_at,
                scope: token.scope.clone(),
                refresh_nonce,
                refresh_ciphertext,
            }),
        };
        file.save(&manifest)?;
        harden_file(file.path())?;
        Ok(view(project, name, &manifest))
    }

    /// Decrypt and return an OAuth secret's stored refresh token, or `None` if it isn't set or
    /// holds no refresh token. Server-side only — the refresh token is used to mint a new access
    /// token and is never surfaced to a client.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe name/id, [`Error::Decrypt`] if it can't be decrypted,
    /// or [`Error::Config`]/[`Error::Crypto`] on a load/key failure.
    pub fn reveal_refresh(&self, project: Option<&str>, name: &str) -> Result<Option<String>> {
        let file = self.manifest_file(project, name)?;
        if !file.exists() {
            return Ok(None);
        }
        let manifest: Manifest = file.load()?;
        let Some(oauth) = manifest.oauth else {
            return Ok(None);
        };
        let (Some(nonce), Some(ciphertext)) = (oauth.refresh_nonce, oauth.refresh_ciphertext)
        else {
            return Ok(None);
        };
        let key = crypto::load_or_create_key(&self.dir())?;
        let plaintext = crypto::decrypt(&key, &aad_refresh(project, name), &nonce, &ciphertext)?;
        Ok(Some(decode_value(plaintext)?))
    }

    /// Every secret in one scope, metadata only, sorted by name. `None` lists global secrets;
    /// `Some(id)` lists that project's. A missing scope dir yields an empty list. This is
    /// scope-specific — it does **not** merge global into a project (that is [`resolve`]).
    ///
    /// [`resolve`]: Self::resolve
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe project id, [`Error::Io`] on a read failure, or
    /// [`Error::Config`] if a manifest is invalid TOML.
    pub fn list(&self, project: Option<&str>) -> Result<Vec<Secret>> {
        let dir = self.scope_dir(project)?;
        let Some(entries) = optional(std::fs::read_dir(dir))? else {
            return Ok(Vec::new());
        };

        let mut secrets = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let Ok(file_name) = entry.file_name().into_string() else {
                continue;
            };
            // The key name is the `<name>.toml` stem; skip the key file and anything else.
            let Some(name) = file_name.strip_suffix(".toml") else {
                continue;
            };
            if validate_name(name).is_err() {
                continue;
            }
            let manifest = self.manifest_file(project, name)?.load()?;
            secrets.push(view(project, name, &manifest));
        }
        secrets.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(secrets)
    }

    /// Every secret across every scope — the global set plus each project's — metadata only,
    /// sorted by scope then name (global first). This is what a UI lists once and filters by
    /// `project` client-side; a run uses [`resolve`](Self::resolve) instead.
    ///
    /// # Errors
    /// [`Error::Io`] on a read failure, or [`Error::Config`] if a manifest is invalid TOML.
    pub fn list_all(&self) -> Result<Vec<Secret>> {
        let mut secrets = self.list(None)?;

        let projects_dir = self.dir().join(PROJECTS_DIR);
        if let Some(entries) = optional(std::fs::read_dir(projects_dir))? {
            for entry in entries {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let Ok(id) = entry.file_name().into_string() else {
                    continue;
                };
                if validate_project(&id).is_err() {
                    continue;
                }
                secrets.extend(self.list(Some(&id))?);
            }
        }
        // Global (project == None) sorts first, then by project id, then by name.
        secrets.sort_by(|a, b| (&a.project, &a.name).cmp(&(&b.project, &b.name)));
        Ok(secrets)
    }

    /// The metadata for one secret, or `None` if it isn't set in this scope. Never the value.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe name/id, or [`Error::Config`] on invalid TOML.
    pub fn get(&self, project: Option<&str>, name: &str) -> Result<Option<Secret>> {
        let file = self.manifest_file(project, name)?;
        if !file.exists() {
            return Ok(None);
        }
        Ok(Some(view(project, name, &file.load()?)))
    }

    /// Decrypt and return one secret's plaintext value, or `None` if it isn't set in this
    /// scope. This is the **only** single-secret path that produces a plaintext value — kept
    /// explicit so callers opt into revealing.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe name/id, [`Error::Decrypt`] if the value can't be
    /// decrypted, or [`Error::Config`]/[`Error::Crypto`] on a load/key failure.
    pub fn reveal(&self, project: Option<&str>, name: &str) -> Result<Option<String>> {
        let file = self.manifest_file(project, name)?;
        if !file.exists() {
            return Ok(None);
        }
        let manifest: Manifest = file.load()?;
        let key = crypto::load_or_create_key(&self.dir())?;
        let plaintext = crypto::decrypt(&key, &aad(project, name), &manifest.nonce, &manifest.ciphertext)?;
        Ok(Some(decode_value(plaintext)?))
    }

    /// Delete a secret from a scope. Returns `false` if it wasn't there.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe name/id, or [`Error::Io`] on a removal failure.
    pub fn remove(&self, project: Option<&str>, name: &str) -> Result<bool> {
        let file = self.manifest_file(project, name)?;
        Ok(optional(std::fs::remove_file(file.path()))?.is_some())
    }

    /// The resolved environment for a run: every global secret, then every secret of `project`
    /// overlaid on top (so a project secret **overrides** a global one of the same name). This
    /// is the single function the run-injection paths call. `None` resolves the global set only.
    ///
    /// # Errors
    /// Anything [`list`](Self::list)/`reveal` can return, including [`Error::Decrypt`] if any
    /// value in scope can't be decrypted.
    pub fn resolve(&self, project: Option<&str>) -> Result<BTreeMap<String, String>> {
        let global = self.list(None)?;
        let scoped = match project {
            Some(id) => self.list(Some(id))?,
            None => Vec::new(),
        };
        // No secrets in scope ⇒ no key touched. Keeps every run on a secrets-free system from
        // materializing a master key it never needs.
        if global.is_empty() && scoped.is_empty() {
            return Ok(BTreeMap::new());
        }

        let key = crypto::load_or_create_key(&self.dir())?;
        let mut env = BTreeMap::new();
        for secret in global {
            let value = self.decrypt_named(&key, None, &secret.name)?;
            env.insert(secret.name, value);
        }
        for secret in scoped {
            // `scoped` is only non-empty when `project` is `Some`.
            let value = self.decrypt_named(&key, project, &secret.name)?;
            env.insert(secret.name, value); // project overrides global by key name
        }
        Ok(env)
    }

    /// Load and decrypt one secret's value with an already-loaded key.
    fn decrypt_named(&self, key: &[u8; 32], project: Option<&str>, name: &str) -> Result<String> {
        let manifest = self.manifest_file(project, name)?.load()?;
        let plaintext = crypto::decrypt(key, &aad(project, name), &manifest.nonce, &manifest.ciphertext)?;
        decode_value(plaintext)
    }

    /// The manifest-file handle for a secret in a scope (touches no disk). Validates the name
    /// (and the project id, if any) before joining onto the store path.
    fn manifest_file(&self, project: Option<&str>, name: &str) -> Result<ConfigFile<Manifest>> {
        validate_name(name)?;
        let rel = match project {
            None => format!("{GLOBAL_DIR}/{name}.toml"),
            Some(id) => {
                validate_project(id)?;
                format!("{PROJECTS_DIR}/{id}/{name}.toml")
            }
        };
        Ok(self.config.module(SECRETS_MODULE).file(&rel))
    }

    /// The directory a scope's secrets live in (touches no disk).
    fn scope_dir(&self, project: Option<&str>) -> Result<PathBuf> {
        Ok(match project {
            None => self.dir().join(GLOBAL_DIR),
            Some(id) => {
                validate_project(id)?;
                self.dir().join(PROJECTS_DIR).join(id)
            }
        })
    }

    /// Create a scope's directory tree with `0700` throughout, before a secret is written into
    /// it — so the file's parents are never left world-readable by the default umask.
    fn ensure_scope_dir(&self, project: Option<&str>) -> Result<()> {
        let base = self.dir();
        std::fs::create_dir_all(&base)?;
        harden_dir(&base)?;
        if project.is_some() {
            let projects = base.join(PROJECTS_DIR);
            std::fs::create_dir_all(&projects)?;
            harden_dir(&projects)?;
        }
        let scope = self.scope_dir(project)?;
        std::fs::create_dir_all(&scope)?;
        harden_dir(&scope)?;
        Ok(())
    }
}

/// The additional-authenticated-data string binding a ciphertext to its exact location, so a
/// value copied to a different scope/name won't decrypt.
fn aad(project: Option<&str>, name: &str) -> String {
    match project {
        None => format!("{GLOBAL_DIR}/{name}"),
        Some(id) => format!("{PROJECTS_DIR}/{id}/{name}"),
    }
}

/// The AAD binding an OAuth secret's **refresh token** to its location — distinct from the
/// value's AAD so the two ciphertexts in one file can't be swapped for each other.
fn aad_refresh(project: Option<&str>, name: &str) -> String {
    format!("{}#refresh", aad(project, name))
}

/// Build the metadata view of a stored secret — including OAuth provenance, but never a token.
fn view(project: Option<&str>, name: &str, manifest: &Manifest) -> Secret {
    Secret {
        project: project.map(str::to_string),
        name: name.to_string(),
        description: manifest.description.clone(),
        created_at: manifest.created_at,
        updated_at: manifest.updated_at,
        oauth: manifest.oauth.as_ref().map(|m| OAuthInfo {
            provider: m.provider.clone(),
            obtained_at: m.obtained_at,
            expires_at: m.expires_at,
            scope: m.scope.clone(),
            has_refresh: m.refresh_ciphertext.is_some(),
        }),
    }
}

/// A decrypted value must be valid UTF-8 (secrets are text); non-UTF-8 reads as tampering.
fn decode_value(plaintext: Vec<u8>) -> Result<String> {
    String::from_utf8(plaintext).map_err(|_| Error::Decrypt)
}

/// Set a file to `0600` (owner read/write only).
pub(crate) fn harden_file(path: &Path) -> Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

/// Set a directory to `0700` (owner-only).
pub(crate) fn harden_dir(path: &Path) -> Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// Fold a "not found" I/O error into `Ok(None)`, propagating any other failure as [`Error::Io`].
fn optional<T>(result: std::io::Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Io(e)),
    }
}

/// Trim a string, dropping it entirely when blank.
fn clean(value: Option<&str>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Secrets {
        let root = std::env::temp_dir().join(format!(
            "adi-secrets-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Secrets::with_config(Config::with_root(root))
    }

    #[test]
    fn set_then_get_reveal_and_list_round_trip() {
        let store = scratch("crud");
        assert!(store.list(None).expect("empty").is_empty());

        let meta = store
            .set(None, "API_KEY", "s3cr3t", Some("a demo key"))
            .expect("set");
        assert_eq!(meta.name, "API_KEY");
        assert_eq!(meta.project, None);
        assert_eq!(meta.description.as_deref(), Some("a demo key"));
        assert!(meta.created_at > 0);

        // get returns metadata, never the value.
        let got = store.get(None, "API_KEY").expect("get").expect("present");
        assert_eq!(got, meta);

        // reveal is the only path to the plaintext.
        assert_eq!(
            store.reveal(None, "API_KEY").expect("reveal").as_deref(),
            Some("s3cr3t")
        );

        let all = store.list(None).expect("list");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "API_KEY");
    }

    #[test]
    fn no_plaintext_value_is_written_to_disk() {
        let store = scratch("nodisk");
        store.set(None, "TOKEN", "super-secret-value", None).expect("set");
        let path = store.dir().join("global/TOKEN.toml");
        let raw = std::fs::read_to_string(&path).expect("read");
        assert!(!raw.contains("super-secret-value"), "value leaked into {path:?}");
        assert!(raw.contains("ciphertext"));
    }

    #[test]
    fn set_overwrites_value_but_keeps_created_at() {
        let store = scratch("overwrite");
        let first = store.set(None, "K", "v1", None).expect("set");
        let second = store.set(None, "K", "v2", None).expect("overwrite");
        assert_eq!(first.created_at, second.created_at);
        assert!(second.updated_at >= first.updated_at);
        assert_eq!(store.reveal(None, "K").expect("reveal").as_deref(), Some("v2"));
    }

    #[test]
    fn project_scope_is_separate_from_global_and_overrides_on_resolve() {
        let store = scratch("scope");
        store.set(None, "SHARED", "global-value", None).expect("global");
        store.set(None, "ONLY_GLOBAL", "g", None).expect("global2");
        store.set(Some("proj"), "SHARED", "project-value", None).expect("project");
        store.set(Some("proj"), "ONLY_PROJECT", "p", None).expect("project2");

        // list is scope-specific.
        assert_eq!(
            store.list(None).expect("g").iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
            vec!["ONLY_GLOBAL", "SHARED"]
        );
        assert_eq!(
            store.list(Some("proj")).expect("p").iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
            vec!["ONLY_PROJECT", "SHARED"]
        );

        // resolve(None) = global only.
        let global = store.resolve(None).expect("resolve global");
        assert_eq!(global.get("SHARED").map(String::as_str), Some("global-value"));
        assert!(!global.contains_key("ONLY_PROJECT"));

        // resolve(project) = global overlaid by project; project wins on SHARED.
        let merged = store.resolve(Some("proj")).expect("resolve merged");
        assert_eq!(merged.get("SHARED").map(String::as_str), Some("project-value"));
        assert_eq!(merged.get("ONLY_GLOBAL").map(String::as_str), Some("g"));
        assert_eq!(merged.get("ONLY_PROJECT").map(String::as_str), Some("p"));
    }

    #[test]
    fn resolving_an_empty_store_creates_no_master_key() {
        let store = scratch("emptyresolve");
        assert!(store.resolve(Some("proj")).expect("resolve").is_empty());
        // A run on a secrets-free system must not materialize a key just by resolving.
        assert!(!store.dir().join(crypto::KEY_FILE).exists());
    }

    #[test]
    fn list_all_spans_every_scope_global_first() {
        let store = scratch("listall");
        assert!(store.list_all().expect("empty").is_empty());
        store.set(None, "G", "g", None).expect("global");
        store.set(Some("beta"), "B", "b", None).expect("beta");
        store.set(Some("alpha"), "A", "a", None).expect("alpha");

        let all = store.list_all().expect("list_all");
        let scoped: Vec<(Option<&str>, &str)> = all
            .iter()
            .map(|s| (s.project.as_deref(), s.name.as_str()))
            .collect();
        assert_eq!(
            scoped,
            vec![(None, "G"), (Some("alpha"), "A"), (Some("beta"), "B")]
        );
    }

    #[test]
    fn files_are_0600_and_dirs_are_0700() {
        let store = scratch("perms");
        store.set(Some("proj"), "K", "v", None).expect("set");

        let mode = |p: &Path| std::fs::metadata(p).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode(&store.dir().join("projects/proj/K.toml")), 0o600);
        assert_eq!(mode(&store.dir().join(crypto::KEY_FILE)), 0o600);
        assert_eq!(mode(&store.dir()), 0o700);
        assert_eq!(mode(&store.dir().join("projects/proj")), 0o700);
    }

    #[test]
    fn remove_deletes_only_the_scoped_secret() {
        let store = scratch("remove");
        store.set(None, "K", "g", None).expect("global");
        store.set(Some("proj"), "K", "p", None).expect("project");

        assert!(store.remove(Some("proj"), "K").expect("remove project"));
        assert!(store.get(Some("proj"), "K").expect("get").is_none());
        // The global one is untouched.
        assert_eq!(store.reveal(None, "K").expect("reveal").as_deref(), Some("g"));
        assert!(!store.remove(Some("proj"), "K").expect("remove missing"));
    }

    #[test]
    fn set_oauth_stores_tokens_encrypted_with_metadata() {
        let store = scratch("oauth");
        let token = OAuthToken {
            provider: "google".to_string(),
            access_token: "ya29.access".to_string(),
            refresh_token: Some("1//refresh".to_string()),
            expires_at: Some(1_800_000_000),
            scope: Some("email profile".to_string()),
        };
        let meta = store.set_oauth(None, "GOOGLE_TOKEN", &token, Some("login")).expect("set_oauth");

        // The view exposes provenance but never a token.
        let info = meta.oauth.expect("oauth info");
        assert_eq!(info.provider, "google");
        assert_eq!(info.expires_at, Some(1_800_000_000));
        assert!(info.has_refresh);

        // The access token is the secret's value — reveal + resolve return it, injecting like text.
        assert_eq!(store.reveal(None, "GOOGLE_TOKEN").expect("reveal").as_deref(), Some("ya29.access"));
        assert_eq!(
            store.resolve(None).expect("resolve").get("GOOGLE_TOKEN").map(String::as_str),
            Some("ya29.access")
        );
        // The refresh token round-trips through its server-side path.
        assert_eq!(store.reveal_refresh(None, "GOOGLE_TOKEN").expect("refresh").as_deref(), Some("1//refresh"));

        // Neither token appears in plaintext on disk.
        let raw = std::fs::read_to_string(store.dir().join("global/GOOGLE_TOKEN.toml")).expect("read");
        assert!(!raw.contains("ya29.access") && !raw.contains("1//refresh"));
        assert!(raw.contains("provider = \"google\""));
    }

    #[test]
    fn set_oauth_without_refresh_reports_no_refresh() {
        let store = scratch("norefresh");
        let token = OAuthToken {
            provider: "github".to_string(),
            access_token: "gho_x".to_string(),
            refresh_token: None,
            expires_at: None,
            scope: None,
        };
        let meta = store.set_oauth(None, "GH", &token, None).expect("set_oauth");
        assert!(!meta.oauth.expect("info").has_refresh);
        assert_eq!(store.reveal_refresh(None, "GH").expect("refresh"), None);
    }

    #[test]
    fn a_plain_set_clears_prior_oauth_provenance() {
        let store = scratch("cleared");
        let token = OAuthToken {
            provider: "google".to_string(),
            access_token: "at".to_string(),
            refresh_token: Some("rt".to_string()),
            expires_at: Some(1),
            scope: None,
        };
        store.set_oauth(None, "K", &token, None).expect("oauth");
        assert!(store.get(None, "K").expect("get").expect("present").oauth.is_some());
        // Overwriting with a plain text value drops the OAuth metadata + refresh token.
        store.set(None, "K", "plain", None).expect("set");
        let after = store.get(None, "K").expect("get").expect("present");
        assert!(after.oauth.is_none());
        assert_eq!(store.reveal_refresh(None, "K").expect("refresh"), None);
    }

    #[test]
    fn invalid_names_never_touch_disk() {
        let store = scratch("invalid");
        assert!(matches!(store.get(None, "../escape"), Err(Error::InvalidName(_))));
        assert!(matches!(
            store.set(Some("a/b"), "K", "v", None),
            Err(Error::InvalidName(_))
        ));
        assert!(matches!(store.reveal(None, ".."), Err(Error::InvalidName(_))));
        assert!(matches!(store.list(Some("../x")), Err(Error::InvalidName(_))));
    }

    #[test]
    fn a_moved_ciphertext_fails_to_decrypt() {
        // A value encrypted under global/K is bound there by AAD; copying its file into the
        // project scope must not decrypt as projects/proj/K.
        let store = scratch("aadmove");
        store.set(None, "K", "v", None).expect("set");
        let from = store.dir().join("global/K.toml");
        store.ensure_scope_dir(Some("proj")).expect("scope dir");
        let to = store.dir().join("projects/proj/K.toml");
        std::fs::copy(&from, &to).expect("copy");
        assert!(matches!(store.reveal(Some("proj"), "K"), Err(Error::Decrypt)));
    }
}
