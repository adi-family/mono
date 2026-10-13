//! Reading a workflow file's `on` export by actually running it under `bun` — never by parsing
//! the TypeScript text, since `on` is arbitrary JS (`schedule(...)`, a ternary, a computed array)
//! and only a real JS engine evaluates that correctly.
//!
//! Two things are seeded onto disk for this to work, the same way `adi-db` seeds `@adi/db`:
//!
//! * the `@adi/workflows` Bun client (into the store's `node_modules`, so any workflow file's
//!   `import … from "@adi/workflows"` resolves with no install step) — [`CLIENT_FILES`];
//! * this crate's own describe harness, under a *private* module dir (`workflows-engine`, never
//!   `workflows/` itself — that path is what [`crate::discover`] walks for *user* files) —
//!   [`DESCRIBE_SCRIPT`].
//!
//! [`bootstrap`] writes both; [`describe`] runs the harness and parses its JSON, cached by the
//! file's `(mtime, len)` so a tick that finds nothing changed never pays for a `bun` spawn.

use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use adi_config::Config;
use serde::Deserialize;

use crate::error::{Error, Result};

const CLIENT_DIR: &[&str] = &["node_modules", "@adi", "workflows"];
const CLIENT_FILES: &[(&str, &str)] = &[
    (
        "package.json",
        include_str!("../templates/client/package.json"),
    ),
    ("index.ts", include_str!("../templates/client/index.ts")),
];

/// The module dir the describe harness is seeded under — deliberately not `workflows/`, which
/// [`crate::discover`] walks for user files; a describe script sitting there would be discovered
/// as one.
const ENGINE_MODULE: &str = "workflows-engine";
const DESCRIBE_FILE: &str = "describe.ts";
const DESCRIBE_SCRIPT: &str = include_str!("../templates/engine/describe.ts");

/// How long a single `bun run describe.ts` may take before this crate gives up on it. Generous —
/// a cold `bun` start plus a trivial import — but bounded, so one hung or infinite-looping
/// workflow file can never stall a tick forever.
const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Seed the `@adi/workflows` client and the describe harness, rewriting a file only when its
/// content differs (so an unchanged build touches no disk — see `adi-db::ensure_client`, the same
/// pattern).
///
/// # Errors
/// [`Error::Store`] if either directory or file can't be written.
pub fn bootstrap(config: &Config) -> Result<()> {
    let client_dir = CLIENT_DIR
        .iter()
        .fold(config.root().to_path_buf(), |path, seg| path.join(seg));
    write_if_changed_dir(&client_dir, CLIENT_FILES)?;

    let engine_dir = config.module(ENGINE_MODULE).dir().to_path_buf();
    write_if_changed_dir(&engine_dir, &[(DESCRIBE_FILE, DESCRIBE_SCRIPT)])
}

fn write_if_changed_dir(dir: &Path, files: &[(&str, &str)]) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| Error::Store(anyhow::anyhow!("couldn't create {}: {e}", dir.display())))?;
    for (name, body) in files {
        let path = dir.join(name);
        if std::fs::read_to_string(&path).is_ok_and(|current| current == *body) {
            continue;
        }
        std::fs::write(&path, body)
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't write {}: {e}", path.display())))?;
    }
    Ok(())
}

fn describe_script_path(config: &Config) -> PathBuf {
    config.module(ENGINE_MODULE).dir().join(DESCRIBE_FILE)
}

/// One entry of a workflow file's `on` export, as the describe harness prints it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type")]
pub enum TriggerDef {
    #[serde(rename = "schedule")]
    Schedule {
        /// Standard 5-field unix cron — [`crate::cron5::normalize_five_field`] converts it.
        cron: String,
        tz: String,
        #[serde(rename = "catchUp", default)]
        catch_up: Option<String>,
    },
    #[serde(rename = "event")]
    Event {
        pattern: String,
        #[serde(rename = "where", default)]
        where_clause: Option<serde_json::Map<String, serde_json::Value>>,
    },
    #[serde(rename = "manual")]
    Manual,
}

#[derive(Debug, Deserialize)]
struct DescribeOutput {
    #[serde(default)]
    on: Vec<TriggerDef>,
}

struct CacheEntry {
    mtime: SystemTime,
    len: u64,
    triggers: Vec<TriggerDef>,
}

/// Per-process memo of the last successful describe, keyed by path — see the module docs.
fn cache() -> &'static Mutex<HashMap<PathBuf, CacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, CacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Every trigger `path`'s `on` export declares, running it fresh under `bun` unless a cached
/// result for the exact same `(mtime, len)` is already in hand.
///
/// # Errors
/// [`Error::Describe`] if `bun` can't be spawned, times out, exits non-zero (the file threw on
/// import — a syntax error, a bad `@adi/workflows` import, …), or doesn't print the JSON this
/// crate's own harness is seeded to.
pub fn describe(config: &Config, path: &Path) -> Result<Vec<TriggerDef>> {
    let meta = std::fs::metadata(path)
        .map_err(|e| Error::Describe(format!("couldn't stat {}: {e}", path.display())))?;
    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let len = meta.len();

    {
        let guard = cache().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = guard.get(path)
            && entry.mtime == mtime
            && entry.len == len
        {
            return Ok(entry.triggers.clone());
        }
    }

    let triggers = run_describe(config, path)?;
    let mut guard = cache().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.insert(
        path.to_path_buf(),
        CacheEntry {
            mtime,
            len,
            triggers: triggers.clone(),
        },
    );
    Ok(triggers)
}

/// Spawn `bun run describe.ts <path>`, wait for it with [`DESCRIBE_TIMEOUT`], and parse its
/// stdout — the uncached body of [`describe`].
fn run_describe(config: &Config, path: &Path) -> Result<Vec<TriggerDef>> {
    let script = describe_script_path(config);
    let mut child = Command::new("bun")
        .arg("run")
        .arg(&script)
        .arg(path)
        .env("PATH", adi_config::augmented_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Describe(format!("couldn't spawn bun: {e}")))?;

    let deadline = Instant::now() + DESCRIBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Error::Describe(format!(
                        "{} took longer than {DESCRIBE_TIMEOUT:?} to describe",
                        path.display()
                    )));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(Error::Describe(format!("couldn't wait on bun: {e}"))),
        }
    };

    let mut stdout = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut stdout);
    }
    let mut stderr = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }
    if !status.success() {
        return Err(Error::Describe(format!(
            "bun exited with {status} describing {}: {}",
            path.display(),
            stderr.trim()
        )));
    }
    let parsed: DescribeOutput = serde_json::from_str(stdout.trim()).map_err(|e| {
        Error::Describe(format!(
            "couldn't parse describe output for {}: {e} (got {stdout:?})",
            path.display()
        ))
    })?;
    Ok(parsed.on)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-workflows-describe-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    fn workflow_file(config: &Config, name: &str, body: &str) -> PathBuf {
        let dir = config.module("workflows").dir().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn a_schedule_trigger_round_trips_through_bun() {
        let config = scratch("schedule");
        bootstrap(&config).expect("bootstrap");
        let path = workflow_file(
            &config,
            "report.ts",
            "import { schedule } from \"@adi/workflows\";\n\
             export const on = schedule(\"0 5 * * MON\", \"America/New_York\");\n",
        );

        let triggers = describe(&config, &path).expect("describe");
        assert_eq!(
            triggers,
            vec![TriggerDef::Schedule {
                cron: "0 5 * * MON".into(),
                tz: "America/New_York".into(),
                catch_up: Some("fire".into()),
            }]
        );
    }

    #[test]
    fn an_event_trigger_with_a_where_clause_round_trips() {
        let config = scratch("event");
        bootstrap(&config).expect("bootstrap");
        let path = workflow_file(
            &config,
            "on-task.ts",
            "import { event } from \"@adi/workflows\";\n\
             export const on = event(\"adi.tasks.created\", { project: \"demo\" });\n",
        );

        let triggers = describe(&config, &path).expect("describe");
        let serde_json::Value::Object(expected) =
            serde_json::json!({ "project": "demo" })
        else {
            unreachable!()
        };
        assert_eq!(
            triggers,
            vec![TriggerDef::Event {
                pattern: "adi.tasks.created".into(),
                where_clause: Some(expected),
            }]
        );
    }

    #[test]
    fn several_triggers_on_one_file_are_all_reported() {
        let config = scratch("several");
        bootstrap(&config).expect("bootstrap");
        let path = workflow_file(
            &config,
            "multi.ts",
            "import { schedule, manual } from \"@adi/workflows\";\n\
             export const on = [schedule(\"0 0 * * *\", \"UTC\"), manual()];\n",
        );

        let triggers = describe(&config, &path).expect("describe");
        assert_eq!(triggers.len(), 2);
        assert!(matches!(triggers[1], TriggerDef::Manual));
    }

    #[test]
    fn a_file_with_no_on_export_describes_as_no_triggers() {
        let config = scratch("none");
        bootstrap(&config).expect("bootstrap");
        let path = workflow_file(&config, "bare.ts", "export default 1;\n");
        assert_eq!(describe(&config, &path).expect("describe"), vec![]);
    }

    #[test]
    fn a_syntax_error_is_reported_rather_than_hanging_or_panicking() {
        let config = scratch("broken");
        bootstrap(&config).expect("bootstrap");
        let path = workflow_file(&config, "broken.ts", "export const on = (;\n");
        assert!(matches!(describe(&config, &path), Err(Error::Describe(_))));
    }

    /// A second describe of an unchanged file never spawns `bun` again — proven by editing the
    /// file *without* changing its mtime-visible content being read is impossible to assert
    /// directly, so instead this checks the cheaper, decisive thing: the cached and fresh results
    /// agree, and a genuinely changed file (new mtime+len) is picked up.
    #[test]
    fn a_changed_file_is_redescribed_and_an_unchanged_one_stays_cached() {
        let config = scratch("cache");
        bootstrap(&config).expect("bootstrap");
        let path = workflow_file(
            &config,
            "cached.ts",
            "import { manual } from \"@adi/workflows\";\nexport const on = manual();\n",
        );
        assert_eq!(describe(&config, &path).unwrap(), vec![TriggerDef::Manual]);
        assert_eq!(describe(&config, &path).unwrap(), vec![TriggerDef::Manual]);

        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(
            &path,
            "import { schedule } from \"@adi/workflows\";\nexport const on = schedule(\"0 0 * * *\", \"UTC\");\n",
        )
        .unwrap();
        let updated = describe(&config, &path).unwrap();
        assert_eq!(
            updated,
            vec![TriggerDef::Schedule {
                cron: "0 0 * * *".into(),
                tz: "UTC".into(),
                catch_up: Some("fire".into()),
            }]
        );
    }
}
