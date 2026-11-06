//! Lazy, backend-owned Codex state. Never shares sessions or writer locks with desktop.
use crate::error::Result;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

pub(crate) fn prepare(
    config: &adi_config::Config,
    backend: &str,
    env: &mut Vec<(String, String)>,
) -> Result<()> {
    let key = Sha256::digest(backend.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let root = config.root().join("codex-homes");
    fs::create_dir_all(&root)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(format!("{key}.lock")))?;
    lock.lock()?;
    let home = root.join(&key);
    if !home.exists() {
        let source = env
            .iter()
            .rev()
            .find(|(k, _)| k == "CODEX_HOME")
            .map(|(_, v)| PathBuf::from(v))
            .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
            .unwrap_or_else(|| adi_config::home().join(".codex"));
        let staging = root.join(format!(".{key}.{}", uuid::Uuid::new_v4()));
        fs::create_dir(&staging)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&staging, fs::Permissions::from_mode(0o700))?;
        }
        // Copy once: subsequent login/refresh/config changes belong to this backend.
        // No symlinks: credentials and state must not mutate the desktop's files.
        if source.join("auth.json").is_file() {
            fs::copy(source.join("auth.json"), staging.join("auth.json"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(staging.join("auth.json"), fs::Permissions::from_mode(0o600))?;
            }
        }
        // Preserve explicit execution policy without importing paths to shared state.
        let mut initial = toml::Table::new();
        initial.insert("cli_auth_credentials_store".into(), "file".into());
        match fs::read_to_string(source.join("config.toml")) {
            Ok(text) => {
                let settings: toml::Table = toml::from_str(&text).map_err(|e| {
                    crate::error::Error::Arguments(format!("invalid source Codex config: {e}"))
                })?;
                for key in ["approval_policy", "sandbox_mode"] {
                    if let Some(value) = settings.get(key) {
                        initial.insert(key.into(), value.clone());
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        fs::write(
            staging.join("config.toml"),
            toml::to_string(&initial).map_err(|e| crate::error::Error::Arguments(e.to_string()))?,
        )?;
        fs::write(staging.join("backend.txt"), backend)?;
        fs::write(
            staging.join("import-home.txt"),
            source.to_string_lossy().as_bytes(),
        )?;
        fs::rename(&staging, &home)?;
    }
    env.retain(|(k, _)| k != "CODEX_HOME" && k != "CODEX_SQLITE_HOME");
    env.push(("CODEX_HOME".into(), home.to_string_lossy().into_owned()));
    env.push((
        "CODEX_SQLITE_HOME".into(),
        home.to_string_lossy().into_owned(),
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn isolated_idempotent_and_concurrent() {
        let dir = std::env::temp_dir().join(format!("adi-codex-homes-{}", uuid::Uuid::new_v4()));
        let source = dir.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("auth.json"), "original").unwrap();
        fs::write(
            source.join("config.toml"),
            r#"approval_policy = "never"
sandbox_mode = "danger-full-access"
sqlite_home = "/desktop/state"
"#,
        )
        .unwrap();
        let config = adi_config::Config::with_root(dir.join("adi"));
        let make = || vec![("CODEX_HOME".into(), source.to_string_lossy().into_owned())];
        let mut a = make();
        prepare(&config, "a", &mut a).unwrap();
        let home = PathBuf::from(&a[0].1);
        let initial: toml::Table =
            toml::from_str(&fs::read_to_string(home.join("config.toml")).unwrap()).unwrap();
        assert_eq!(initial["approval_policy"].as_str(), Some("never"));
        assert_eq!(initial["sandbox_mode"].as_str(), Some("danger-full-access"));
        assert!(!initial.contains_key("sqlite_home"));
        fs::write(home.join("auth.json"), "refreshed").unwrap();
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let config = &config;
                let make = &make;
                scope.spawn(move || {
                    let mut env = make();
                    prepare(config, "a", &mut env).unwrap();
                });
            }
        });
        assert_eq!(
            fs::read_to_string(home.join("auth.json")).unwrap(),
            "refreshed"
        );
        let mut b = make();
        prepare(&config, "b", &mut b).unwrap();
        assert_ne!(a, b);
        assert!(!home.join("sessions").exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
