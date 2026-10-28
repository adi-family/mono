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
mod settings;

use adi_config::{clean, optional};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use adi_config::{Config, ConfigFile, now_unix};

pub use error::{Error, Result};
pub use secret::{OAuthInfo, OAuthToken, Secret};
pub use settings::{Missing, UserSecretsPolicy, UserSettings, effective_missing};

use secret::{Manifest, OAuthMeta, validate_name, validate_project};

/// The store module secrets live under, and the scope subdirectories within it.
const SECRETS_MODULE: &str = "secrets";
const GLOBAL_DIR: &str = "global";
const PROJECTS_DIR: &str = "projects";
const USERS_DIR: &str = "users";

/// Where a secret lives (`docs/byo-credentials.md` §4.2) — a **third kind of scope**, not a
/// reserved project id: `adi-app` keeps node passwords in a pseudo-project called `fleet-nodes`
/// (`adi-app/src/viewer.rs:27`), which works only because nothing resolves that scope into a run.
/// A user scope exists *to be* resolved into runs, so `User`'s id must never be mistaken for a
/// [`Project`](Self::Project) id even though both pass the same filesystem-name check
/// ([`validate_project`]).
///
/// [`Secrets::set`]/[`list`](Secrets::list)/[`get`](Secrets::get)/[`reveal`](Secrets::reveal)/
/// [`remove`](Secrets::remove) — the ones whose public signature is `project: Option<&str>` —
/// are thin wrappers over this: `None` is [`Global`](Self::Global), `Some(id)` is
/// [`Project`](Self::Project). There is no `Option`-shaped convenience for [`User`](Self::User),
/// because a user scope is a capability with its own rules (§4.2, §7), not a drop-in third
/// `Option` variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope<'a> {
    /// `secrets/global/<name>.toml` — available everywhere.
    Global,
    /// `secrets/projects/<id>/<name>.toml` — scoped to an [`adi-projects`](adi_config) id.
    Project(&'a str),
    /// `secrets/users/<u>/<name>.toml` — scoped to one minted user id
    /// (`docs/byo-credentials.md` §3.2). Wrapped to the user's device from phase 2 on; in phase 1
    /// nothing writes one.
    User(&'a str),
}

impl<'a> Scope<'a> {
    /// The platform-wide `project: Option<&str>` convention, lifted into a [`Scope`]: `None` is
    /// [`Global`](Self::Global), `Some(id)` is [`Project`](Self::Project). Never produces
    /// [`User`](Self::User) — there is no `Option` shape for it.
    fn from_project(project: Option<&'a str>) -> Self {
        match project {
            None => Self::Global,
            Some(id) => Self::Project(id),
        }
    }
}

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
        let scope = Scope::from_project(project);
        let file = self.manifest_file(scope, name)?;
        self.ensure_scope_dir(scope)?;

        let key = crypto::load_or_create_key(&self.dir())?;
        let (nonce, ciphertext) = crypto::encrypt(&key, &aad(scope, name), value.as_bytes())?;

        let now = now_unix();
        let created_at = file.carried_created_at(now);

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
        let scope = Scope::from_project(project);
        let file = self.manifest_file(scope, name)?;
        // The provider is later joined into a URL path (`/refresh/<provider>`), so it must be a
        // safe segment too.
        validate_project(&token.provider)?;
        self.ensure_scope_dir(scope)?;

        let key = crypto::load_or_create_key(&self.dir())?;
        let (nonce, ciphertext) =
            crypto::encrypt(&key, &aad(scope, name), token.access_token.as_bytes())?;
        let (refresh_nonce, refresh_ciphertext) = match token.refresh_token.as_deref() {
            Some(rt) if !rt.is_empty() => {
                let (n, c) = crypto::encrypt(&key, &aad_refresh(scope, name), rt.as_bytes())?;
                (Some(n), Some(c))
            }
            _ => (None, None),
        };

        let now = now_unix();
        let created_at = file.carried_created_at(now);

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
        let scope = Scope::from_project(project);
        let file = self.manifest_file(scope, name)?;
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
        let plaintext = crypto::decrypt(&key, &aad_refresh(scope, name), &nonce, &ciphertext)?;
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
        let scope = Scope::from_project(project);
        let dir = self.scope_dir(scope)?;
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
            let manifest = self.manifest_file(scope, name)?.load()?;
            secrets.push(view(project, name, &manifest));
        }
        secrets.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(secrets)
    }

    /// Every secret across every scope — the global set plus each project's — metadata only,
    /// sorted by scope then name (global first). This is what a UI lists once and filters by
    /// `project` client-side; a run uses [`resolve`](Self::resolve) instead.
    ///
    /// **Never walks `users/`.** This backs the admin-facing secrets page (`GET /api/secrets`,
    /// `docs/byo-credentials.md` §7), and a user scope is write-only: an admin and the user
    /// themselves can set, replace and remove a user secret, but no API — this one included —
    /// ever lists or reveals its value.
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
        let file = self.manifest_file(Scope::from_project(project), name)?;
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
        let scope = Scope::from_project(project);
        let file = self.manifest_file(scope, name)?;
        if !file.exists() {
            return Ok(None);
        }
        let manifest: Manifest = file.load()?;
        let key = crypto::load_or_create_key(&self.dir())?;
        let plaintext = crypto::decrypt(
            &key,
            &aad(scope, name),
            &manifest.nonce,
            &manifest.ciphertext,
        )?;
        Ok(Some(decode_value(plaintext)?))
    }

    /// Follow a project rename into this store: move every secret scoped to `from` into `to`.
    /// Returns how many were moved.
    ///
    /// This cannot be a directory move. A ciphertext is bound to its exact location by its
    /// [AAD](aad) — that binding is the thing that stops a value being copied into another
    /// project's scope and still decrypting — so each secret is decrypted under the old scope and
    /// re-encrypted under the new one, with a fresh nonce. An OAuth secret's refresh token is
    /// carried across the same way, under its own AAD. Everything else — description, both
    /// timestamps, the provider metadata — is preserved: the secret did not change, its address
    /// did.
    ///
    /// The old scope's directory is removed once it is empty. A `from` with no secrets is a no-op
    /// that touches neither the master key nor the disk.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe id on either side, [`Error::Decrypt`] if a value in
    /// scope can't be decrypted, or [`Error::Config`]/[`Error::Crypto`]/[`Error::Io`] on a key,
    /// write, or removal failure.
    pub fn rename_project(&self, from: &str, to: &str) -> Result<usize> {
        validate_project(from)?;
        validate_project(to)?;
        if from == to {
            return Ok(0);
        }
        let secrets = self.list(Some(from))?;
        if secrets.is_empty() {
            return Ok(0);
        }

        let key = crypto::load_or_create_key(&self.dir())?;
        self.ensure_scope_dir(Scope::Project(to))?;
        let mut moved = 0;
        for secret in secrets {
            let name = secret.name.as_str();
            let source = self.manifest_file(Scope::Project(from), name)?;
            let manifest: Manifest = source.load()?;

            let value = crypto::decrypt(
                &key,
                &aad(Scope::Project(from), name),
                &manifest.nonce,
                &manifest.ciphertext,
            )?;
            let (nonce, ciphertext) = crypto::encrypt(&key, &aad(Scope::Project(to), name), &value)?;
            let oauth = match manifest.oauth {
                Some(meta) => Some(recrypt_refresh(&key, from, to, name, meta)?),
                None => None,
            };

            let target = self.manifest_file(Scope::Project(to), name)?;
            target.save(&Manifest {
                description: manifest.description,
                nonce,
                ciphertext,
                created_at: manifest.created_at,
                updated_at: manifest.updated_at,
                oauth,
            })?;
            harden_file(target.path())?;
            optional(std::fs::remove_file(source.path()))?;
            moved += 1;
        }
        // Only when it is empty — anything left behind is somebody else's, and a rename is not a
        // licence to delete it.
        optional(std::fs::remove_dir(self.scope_dir(Scope::Project(from))?))?;
        Ok(moved)
    }

    /// Delete a secret from a scope. Returns `false` if it wasn't there.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe name/id, or [`Error::Io`] on a removal failure.
    pub fn remove(&self, project: Option<&str>, name: &str) -> Result<bool> {
        let file = self.manifest_file(Scope::from_project(project), name)?;
        Ok(optional(std::fs::remove_file(file.path()))?.is_some())
    }

    /// One user's own secret names, metadata only — never a value, and never which scope at all
    /// beyond "this user's own" (`docs/byo-credentials.md` §7). `project` on the returned
    /// [`Secret`] is always `None`: a user id must never be mistaken for a project id (§4.2), and
    /// nothing reads this field for a `/mine` listing anyway.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe user id, [`Error::Io`] on a read failure, or
    /// [`Error::Config`] if a manifest is invalid TOML.
    pub fn list_mine(&self, user: &str) -> Result<Vec<Secret>> {
        let scope = Scope::User(user);
        let dir = self.scope_dir(scope)?;
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
            let Some(name) = file_name.strip_suffix(".toml") else {
                continue;
            };
            // `.settings.toml` (§15 Q1) and any other dotfile can never be a secret name, so this
            // skips it the same way `list` skips anything `validate_name` would refuse.
            if validate_name(name).is_err() {
                continue;
            }
            let manifest = self.manifest_file(scope, name)?.load()?;
            secrets.push(view(None, name, &manifest));
        }
        secrets.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(secrets)
    }

    /// Delete one of `user`'s own secrets. Returns `false` if it wasn't there — the user-scope
    /// counterpart of [`remove`](Self::remove), which has no `Option`-shaped overload for
    /// [`Scope::User`] either (§4.2).
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe user id/name, or [`Error::Io`] on a removal failure.
    pub fn remove_mine(&self, user: &str, name: &str) -> Result<bool> {
        let file = self.manifest_file(Scope::User(user), name)?;
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
        let scope = Scope::from_project(project);
        let manifest = self.manifest_file(scope, name)?.load()?;
        let plaintext = crypto::decrypt(
            key,
            &aad(scope, name),
            &manifest.nonce,
            &manifest.ciphertext,
        )?;
        decode_value(plaintext)
    }

    /// The manifest-file handle for a secret in a scope (touches no disk). Validates the name
    /// (and the scope's project/user id, if any) before joining onto the store path.
    fn manifest_file(&self, scope: Scope<'_>, name: &str) -> Result<ConfigFile<Manifest>> {
        validate_name(name)?;
        let rel = match scope {
            Scope::Global => format!("{GLOBAL_DIR}/{name}.toml"),
            Scope::Project(id) => {
                validate_project(id)?;
                format!("{PROJECTS_DIR}/{id}/{name}.toml")
            }
            Scope::User(user) => {
                validate_project(user)?;
                format!("{USERS_DIR}/{user}/{name}.toml")
            }
        };
        Ok(self.config.module(SECRETS_MODULE).file(&rel))
    }

    /// The directory a scope's secrets live in (touches no disk).
    fn scope_dir(&self, scope: Scope<'_>) -> Result<PathBuf> {
        Ok(match scope {
            Scope::Global => self.dir().join(GLOBAL_DIR),
            Scope::Project(id) => {
                validate_project(id)?;
                self.dir().join(PROJECTS_DIR).join(id)
            }
            Scope::User(user) => {
                validate_project(user)?;
                self.dir().join(USERS_DIR).join(user)
            }
        })
    }

    /// Create a scope's directory tree with `0700` throughout, before a secret is written into
    /// it — so the file's parents are never left world-readable by the default umask.
    fn ensure_scope_dir(&self, scope: Scope<'_>) -> Result<()> {
        let base = self.dir();
        std::fs::create_dir_all(&base)?;
        harden_dir(&base)?;
        let parent = match scope {
            Scope::Global => None,
            Scope::Project(_) => Some(PROJECTS_DIR),
            Scope::User(_) => Some(USERS_DIR),
        };
        if let Some(parent) = parent {
            let parent_dir = base.join(parent);
            std::fs::create_dir_all(&parent_dir)?;
            harden_dir(&parent_dir)?;
        }
        let dir = self.scope_dir(scope)?;
        std::fs::create_dir_all(&dir)?;
        harden_dir(&dir)?;
        Ok(())
    }
}

/// The additional-authenticated-data string binding a ciphertext to its exact location, so a
/// value copied to a different scope/name won't decrypt. A user secret's AAD is
/// `user:<u>/<NAME>` (`docs/byo-credentials.md` §4.2) — the `user:` prefix, unlike
/// `global/`/`projects/`, isn't a path segment, since nothing ever lists `users/` the way it
/// lists the other two scopes (`list_all`), but the binding works the same way: it can't collide
/// with `global/<NAME>` or `projects/<id>/<NAME>`, so a ciphertext moved between scopes won't open.
fn aad(scope: Scope<'_>, name: &str) -> String {
    match scope {
        Scope::Global => format!("{GLOBAL_DIR}/{name}"),
        Scope::Project(id) => format!("{PROJECTS_DIR}/{id}/{name}"),
        Scope::User(user) => format!("user:{user}/{name}"),
    }
}

/// Re-encrypt an OAuth secret's refresh token from project scope `from` into scope `to`, leaving
/// the rest of its provenance untouched. A secret with no stored refresh token carries across
/// unchanged. Beside [`aad_refresh`], which is the only reason it exists.
fn recrypt_refresh(
    key: &[u8; 32],
    from: &str,
    to: &str,
    name: &str,
    meta: OAuthMeta,
) -> Result<OAuthMeta> {
    let (Some(nonce), Some(ciphertext)) = (&meta.refresh_nonce, &meta.refresh_ciphertext) else {
        return Ok(meta);
    };
    let refresh = crypto::decrypt(key, &aad_refresh(Scope::Project(from), name), nonce, ciphertext)?;
    let (refresh_nonce, refresh_ciphertext) =
        crypto::encrypt(key, &aad_refresh(Scope::Project(to), name), &refresh)?;
    Ok(OAuthMeta {
        refresh_nonce: Some(refresh_nonce),
        refresh_ciphertext: Some(refresh_ciphertext),
        ..meta
    })
}

/// The AAD binding an OAuth secret's **refresh token** to its location — distinct from the
/// value's AAD so the two ciphertexts in one file can't be swapped for each other.
fn aad_refresh(scope: Scope<'_>, name: &str) -> String {
    format!("{}#refresh", aad(scope, name))
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
///
/// On Windows there is no POSIX mode; files already live under the per-user profile
/// directory (owner-scoped by default), so this is a no-op there.
pub(crate) fn harden_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Set a directory to `0700` (owner-only). No-op on Windows (see [`harden_file`]).
pub(crate) fn harden_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
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
        store
            .set(None, "TOKEN", "super-secret-value", None)
            .expect("set");
        let path = store.dir().join("global/TOKEN.toml");
        let raw = std::fs::read_to_string(&path).expect("read");
        assert!(
            !raw.contains("super-secret-value"),
            "value leaked into {path:?}"
        );
        assert!(raw.contains("ciphertext"));
    }

    #[test]
    fn set_overwrites_value_but_keeps_created_at() {
        let store = scratch("overwrite");
        let first = store.set(None, "K", "v1", None).expect("set");
        let second = store.set(None, "K", "v2", None).expect("overwrite");
        assert_eq!(first.created_at, second.created_at);
        assert!(second.updated_at >= first.updated_at);
        assert_eq!(
            store.reveal(None, "K").expect("reveal").as_deref(),
            Some("v2")
        );
    }

    #[test]
    fn project_scope_is_separate_from_global_and_overrides_on_resolve() {
        let store = scratch("scope");
        store
            .set(None, "SHARED", "global-value", None)
            .expect("global");
        store.set(None, "ONLY_GLOBAL", "g", None).expect("global2");
        store
            .set(Some("proj"), "SHARED", "project-value", None)
            .expect("project");
        store
            .set(Some("proj"), "ONLY_PROJECT", "p", None)
            .expect("project2");

        // list is scope-specific.
        assert_eq!(
            store
                .list(None)
                .expect("g")
                .iter()
                .map(|s| s.name.clone())
                .collect::<Vec<_>>(),
            vec!["ONLY_GLOBAL", "SHARED"]
        );
        assert_eq!(
            store
                .list(Some("proj"))
                .expect("p")
                .iter()
                .map(|s| s.name.clone())
                .collect::<Vec<_>>(),
            vec!["ONLY_PROJECT", "SHARED"]
        );

        // resolve(None) = global only.
        let global = store.resolve(None).expect("resolve global");
        assert_eq!(
            global.get("SHARED").map(String::as_str),
            Some("global-value")
        );
        assert!(!global.contains_key("ONLY_PROJECT"));

        // resolve(project) = global overlaid by project; project wins on SHARED.
        let merged = store.resolve(Some("proj")).expect("resolve merged");
        assert_eq!(
            merged.get("SHARED").map(String::as_str),
            Some("project-value")
        );
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

    // POSIX-only: hardening sets mode bits on Unix; on Windows `harden_*` is a no-op (files
    // already live under the per-user profile), so there is nothing to assert.
    #[cfg(unix)]
    #[test]
    fn files_are_0600_and_dirs_are_0700() {
        use std::os::unix::fs::PermissionsExt as _;
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
        assert_eq!(
            store.reveal(None, "K").expect("reveal").as_deref(),
            Some("g")
        );
        assert!(!store.remove(Some("proj"), "K").expect("remove missing"));
    }

    #[test]
    fn rename_project_re_encrypts_every_secret_into_the_new_scope() {
        let store = scratch("rename-project");
        store
            .set(Some("old"), "API_KEY", "s3cr3t", Some("the key"))
            .expect("set");
        let created = store
            .get(Some("old"), "API_KEY")
            .expect("get")
            .expect("present")
            .created_at;
        let token = OAuthToken {
            provider: "google".to_string(),
            access_token: "ya29.access".to_string(),
            refresh_token: Some("1//refresh".to_string()),
            expires_at: Some(1_800_000_000),
            scope: Some("email".to_string()),
        };
        store
            .set_oauth(Some("old"), "GOOGLE_TOKEN", &token, None)
            .expect("set_oauth");
        store
            .set(Some("other"), "API_KEY", "not mine", None)
            .expect("other project");
        store.set(None, "API_KEY", "global", None).expect("global");

        assert_eq!(store.rename_project("old", "new").expect("rename"), 2);

        // The values decrypt at the new address — which they only can if they were re-encrypted
        // under it, since the AAD binds a ciphertext to its scope.
        assert_eq!(
            store
                .reveal(Some("new"), "API_KEY")
                .expect("reveal")
                .as_deref(),
            Some("s3cr3t")
        );
        assert_eq!(
            store
                .reveal(Some("new"), "GOOGLE_TOKEN")
                .expect("reveal")
                .as_deref(),
            Some("ya29.access")
        );
        assert_eq!(
            store
                .reveal_refresh(Some("new"), "GOOGLE_TOKEN")
                .expect("refresh")
                .as_deref(),
            Some("1//refresh")
        );

        // Metadata travelled unchanged — a rename is not a re-set.
        let moved = store
            .get(Some("new"), "API_KEY")
            .expect("get")
            .expect("present");
        assert_eq!(moved.description.as_deref(), Some("the key"));
        assert_eq!(moved.created_at, created);
        let oauth = store
            .get(Some("new"), "GOOGLE_TOKEN")
            .expect("get")
            .expect("present")
            .oauth
            .expect("oauth info");
        assert_eq!(oauth.provider, "google");
        assert_eq!(oauth.expires_at, Some(1_800_000_000));
        assert!(oauth.has_refresh);

        // The old scope is gone, and no other scope was touched.
        assert!(store.list(Some("old")).expect("list old").is_empty());
        assert!(!store.dir().join("projects/old").exists());
        assert_eq!(
            store
                .reveal(Some("other"), "API_KEY")
                .expect("reveal")
                .as_deref(),
            Some("not mine")
        );
        assert_eq!(
            store.reveal(None, "API_KEY").expect("reveal").as_deref(),
            Some("global")
        );

        // Nothing to move the second time, and renaming onto itself is a no-op.
        assert_eq!(store.rename_project("old", "new").expect("again"), 0);
        assert_eq!(store.rename_project("new", "new").expect("self"), 0);
        assert_eq!(
            store
                .reveal(Some("new"), "API_KEY")
                .expect("reveal")
                .as_deref(),
            Some("s3cr3t")
        );
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
        let meta = store
            .set_oauth(None, "GOOGLE_TOKEN", &token, Some("login"))
            .expect("set_oauth");

        // The view exposes provenance but never a token.
        let info = meta.oauth.expect("oauth info");
        assert_eq!(info.provider, "google");
        assert_eq!(info.expires_at, Some(1_800_000_000));
        assert!(info.has_refresh);

        // The access token is the secret's value — reveal + resolve return it, injecting like text.
        assert_eq!(
            store
                .reveal(None, "GOOGLE_TOKEN")
                .expect("reveal")
                .as_deref(),
            Some("ya29.access")
        );
        assert_eq!(
            store
                .resolve(None)
                .expect("resolve")
                .get("GOOGLE_TOKEN")
                .map(String::as_str),
            Some("ya29.access")
        );
        // The refresh token round-trips through its server-side path.
        assert_eq!(
            store
                .reveal_refresh(None, "GOOGLE_TOKEN")
                .expect("refresh")
                .as_deref(),
            Some("1//refresh")
        );

        // Neither token appears in plaintext on disk.
        let raw =
            std::fs::read_to_string(store.dir().join("global/GOOGLE_TOKEN.toml")).expect("read");
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
        let meta = store
            .set_oauth(None, "GH", &token, None)
            .expect("set_oauth");
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
        assert!(
            store
                .get(None, "K")
                .expect("get")
                .expect("present")
                .oauth
                .is_some()
        );
        // Overwriting with a plain text value drops the OAuth metadata + refresh token.
        store.set(None, "K", "plain", None).expect("set");
        let after = store.get(None, "K").expect("get").expect("present");
        assert!(after.oauth.is_none());
        assert_eq!(store.reveal_refresh(None, "K").expect("refresh"), None);
    }

    #[test]
    fn invalid_names_never_touch_disk() {
        let store = scratch("invalid");
        assert!(matches!(
            store.get(None, "../escape"),
            Err(Error::InvalidName(_))
        ));
        assert!(matches!(
            store.set(Some("a/b"), "K", "v", None),
            Err(Error::InvalidName(_))
        ));
        assert!(matches!(
            store.reveal(None, ".."),
            Err(Error::InvalidName(_))
        ));
        assert!(matches!(
            store.list(Some("../x")),
            Err(Error::InvalidName(_))
        ));
    }

    #[test]
    fn a_moved_ciphertext_fails_to_decrypt() {
        // A value encrypted under global/K is bound there by AAD; copying its file into the
        // project scope must not decrypt as projects/proj/K.
        let store = scratch("aadmove");
        store.set(None, "K", "v", None).expect("set");
        let from = store.dir().join("global/K.toml");
        store
            .ensure_scope_dir(Scope::Project("proj"))
            .expect("scope dir");
        let to = store.dir().join("projects/proj/K.toml");
        std::fs::copy(&from, &to).expect("copy");
        assert!(matches!(
            store.reveal(Some("proj"), "K"),
            Err(Error::Decrypt)
        ));
    }

    // -- Scope::User (ADI-MONO-177 P1.3) --------------------------------------------------

    /// Write a secret directly into `scope`, the way a future `set --user` will, without going
    /// through the two-argument `project: Option<&str>` surface — there is no `Option` shape for
    /// [`Scope::User`].
    fn set_scoped(store: &Secrets, scope: Scope<'_>, name: &str, value: &str) {
        let file = store.manifest_file(scope, name).expect("manifest file");
        store.ensure_scope_dir(scope).expect("scope dir");
        let key = crypto::load_or_create_key(&store.dir()).expect("key");
        let (nonce, ciphertext) =
            crypto::encrypt(&key, &aad(scope, name), value.as_bytes()).expect("encrypt");
        file.save(&Manifest {
            description: None,
            nonce,
            ciphertext,
            created_at: now_unix(),
            updated_at: now_unix(),
            oauth: None,
        })
        .expect("save");
    }

    /// [`Secrets::reveal`] has no `Scope` overload either; this is its test-only mirror.
    fn reveal_scoped(store: &Secrets, scope: Scope<'_>, name: &str) -> Result<String> {
        let manifest: Manifest = store.manifest_file(scope, name)?.load()?;
        let key = crypto::load_or_create_key(&store.dir())?;
        let plaintext = crypto::decrypt(&key, &aad(scope, name), &manifest.nonce, &manifest.ciphertext)?;
        decode_value(plaintext)
    }

    #[test]
    fn a_user_secret_lives_under_users_u_and_round_trips() {
        let store = scratch("user-roundtrip");
        set_scoped(&store, Scope::User("u-aaa"), "CLAUDE_CODE_OAUTH_TOKEN", "sk-mine");

        assert!(
            store
                .dir()
                .join("users/u-aaa/CLAUDE_CODE_OAUTH_TOKEN.toml")
                .exists(),
            "stored at the documented path"
        );
        assert_eq!(
            reveal_scoped(&store, Scope::User("u-aaa"), "CLAUDE_CODE_OAUTH_TOKEN").expect("reveal"),
            "sk-mine"
        );
    }

    #[test]
    fn a_user_ciphertext_fails_to_open_in_global_or_another_users_scope() {
        let store = scratch("user-aadmove");
        set_scoped(&store, Scope::User("u-aaa"), "K", "mine");

        let from = store.dir().join("users/u-aaa/K.toml");

        // Copied into global.
        store.ensure_scope_dir(Scope::Global).expect("scope dir");
        let to_global = store.dir().join("global/K.toml");
        std::fs::copy(&from, &to_global).expect("copy to global");
        assert!(matches!(store.reveal(None, "K"), Err(Error::Decrypt)));

        // Copied into another user's scope.
        store
            .ensure_scope_dir(Scope::User("u-bbb"))
            .expect("scope dir");
        let to_other_user = store.dir().join("users/u-bbb/K.toml");
        std::fs::copy(&from, &to_other_user).expect("copy to another user");
        assert!(matches!(
            reveal_scoped(&store, Scope::User("u-bbb"), "K"),
            Err(Error::Decrypt)
        ));

        // The original is untouched.
        assert_eq!(
            reveal_scoped(&store, Scope::User("u-aaa"), "K").expect("reveal"),
            "mine"
        );
    }

    #[test]
    fn list_all_never_walks_users() {
        let store = scratch("user-listall");
        store.set(None, "G", "g", None).expect("global");
        store.set(Some("proj"), "P", "p", None).expect("project");
        set_scoped(&store, Scope::User("u-aaa"), "SECRET", "mine");

        let all = store.list_all().expect("list_all");
        let names: Vec<&str> = all.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["G", "P"], "no user secret ever appears here");
    }

    #[test]
    fn a_user_id_is_validated_the_same_way_a_project_id_is() {
        let store = scratch("user-validate");
        assert!(matches!(
            store.manifest_file(Scope::User("../escape"), "K"),
            Err(Error::InvalidName(_))
        ));
        assert!(store.manifest_file(Scope::User("u-aaa"), "K").is_ok());
    }

    // -- list_mine / remove_mine (ADI-MONO-178 P1.11) ---------------------------------------

    #[test]
    fn list_mine_is_empty_for_a_user_with_nothing_stored() {
        let store = scratch("list-mine-empty");
        assert!(store.list_mine("u-aaa").expect("list").is_empty());
    }

    #[test]
    fn list_mine_sees_only_this_users_own_names_metadata_only() {
        let store = scratch("list-mine-own");
        set_scoped(&store, Scope::User("u-aaa"), "CLAUDE_CODE_OAUTH_TOKEN", "sk-mine");
        set_scoped(&store, Scope::User("u-bbb"), "OTHER_USERS_KEY", "not-mine");
        store.set(None, "GLOBAL", "g", None).expect("global");

        let mine = store.list_mine("u-aaa").expect("list");
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].name, "CLAUDE_CODE_OAUTH_TOKEN");
        assert_eq!(mine[0].project, None, "a user id is never shown as a project id");
    }

    /// `.settings.toml` (§15 Q1) sits beside a user's own secrets on disk and must never be
    /// listed as one.
    #[test]
    fn list_mine_skips_the_users_own_settings_file() {
        let store = scratch("list-mine-skips-settings");
        set_scoped(&store, Scope::User("u-aaa"), "REAL_SECRET", "value");
        UserSettings::default()
            .save(&store.config, "u-aaa")
            .expect("write the user's own settings file");

        let mine = store.list_mine("u-aaa").expect("list");
        assert_eq!(mine.len(), 1, "{mine:?}");
        assert_eq!(mine[0].name, "REAL_SECRET");
    }

    #[test]
    fn remove_mine_deletes_only_this_users_own_copy() {
        let store = scratch("remove-mine");
        set_scoped(&store, Scope::User("u-aaa"), "K", "mine");
        set_scoped(&store, Scope::User("u-bbb"), "K", "also-mine");

        assert!(store.remove_mine("u-aaa", "K").expect("remove"));
        assert!(!store.remove_mine("u-aaa", "K").expect("remove again"), "already gone");
        assert_eq!(
            reveal_scoped(&store, Scope::User("u-bbb"), "K").expect("reveal"),
            "also-mine",
            "a different user's copy is untouched"
        );
    }
}
