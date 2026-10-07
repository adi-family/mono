//! Provision the local channels client in the existing per-user Hive supervisor.
//!
//! The user hive runs the client; the front door imports only its route, so even a user-owned
//! front door cannot launch a duplicate. Both use the same leased `channels/client` HTTP port.

use std::path::{Path, PathBuf};

use adi_config::{Config, Flavor};

const MODULE: &str = "channels";
const CONFIG_FILE: &str = "hive.yaml";
const ROUTES_FILE: &str = "routes.yaml";

pub(crate) fn routes_path() -> PathBuf {
    Config::open().module(MODULE).raw_path(ROUTES_FILE)
}

/// Materialize the daemon and attach it to the running hives' configuration.
///
/// Hive reloads these files itself; this never restarts either Hive or DNS. Existing service
/// declarations and imports are preserved. The generated daemon definition follows this build's
/// sibling binary, so upgrades and `ADI_CHANNELS_BIN` development overrides take effect.
///
/// # Errors
/// A configuration cannot be read, parsed, or written.
pub fn ensure() -> Result<(), String> {
    let store = Config::open();
    crate::dashboards::Dashboards::new().ensure_config();
    let binary = crate::dns::sibling_binary("adi-channelsd", "ADI_CHANNELS_BIN");
    ensure_in(&store, Flavor::current(), &binary)
}

fn ensure_in(store: &Config, flavor: &Flavor, binary: &str) -> Result<(), String> {
    let module = store.module(MODULE);
    let path = module.raw_path(CONFIG_FILE);
    let old = read_optional(&path)?;
    let mut environment = std::collections::BTreeMap::<String, String>::new();
    if let Some(raw) = old.as_deref() {
        let previous: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(raw).map_err(|e| e.to_string())?;
        for key in ["ADI_CHANNEL_ROUTER_URL", "ADI_CHANNEL_ROUTER_ADMIN_SECRET"] {
            if let Some(value) =
                previous["services"]["client"]["environment"]["static"][key].as_str()
            {
                environment.insert(key.to_string(), value.to_string());
            }
        }
    }
    environment.extend(
        flavor
            .env()
            .into_iter()
            .map(|(key, value)| (key.to_string(), value)),
    );
    for key in ["ADI_CHANNEL_ROUTER_URL", "ADI_CHANNEL_ROUTER_ADMIN_SECRET"] {
        if let Ok(value) = std::env::var(key) {
            environment.insert(key.to_string(), value);
        }
    }
    let rendered = render(flavor, Some(binary), environment, crate::VERSION)?;
    if old.as_deref() != Some(&rendered) {
        module
            .write_raw(CONFIG_FILE, rendered.as_bytes())
            .map_err(|e| e.to_string())?;
    }

    // A separate route-only artifact is necessary for hand-managed front doors that also run
    // other services. Do not rely on their routes_only flag or copy secrets into that artifact.
    let routes = render(flavor, None, Default::default(), crate::VERSION)?;
    let routes_path = module.raw_path(ROUTES_FILE);
    if read_optional(&routes_path)?.as_deref() != Some(&routes) {
        module
            .write_raw(ROUTES_FILE, routes.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    for (target, import) in [
        (store.module("dashboards").raw_path(CONFIG_FILE), path),
        (crate::dns::front_door_in_use(store), routes_path),
    ] {
        if let Some(raw) = read_optional(&target)? {
            let updated = with_import(&raw, &import.to_string_lossy())?;
            if updated != raw {
                // Config's raw writer provides an atomic replacement and private file mode.
                let parent = target.parent().ok_or("Hive config has no parent")?;
                let name = target
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or("invalid Hive config path")?;
                Config::with_root(parent)
                    .module("")
                    .write_raw(name, updated.as_bytes())
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

fn read_optional(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(Some(raw)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn render(
    flavor: &Flavor,
    binary: Option<&str>,
    mut environment: std::collections::BTreeMap<String, String>,
    version: &str,
) -> Result<String, String> {
    let mut document = serde_json::json!({
        "version": "1",
        "services": {
            "client": {
                "proxy": { "host": format!("channels.{}", flavor.domain) },
                "start": "always",
                "restart": "always"
            }
        }
    });
    if let Some(binary) = binary {
        // A release replaces the executable at the same path. Carry its version in the runner
        // spec so Hive restarts this client on upgrade, while repeated `up` stays a no-op.
        environment.insert("ADI_CHANNELS_VERSION".to_string(), version.to_string());
        document["services"]["client"]["runner"] =
            serde_json::json!({ "script": { "run": shell_command(binary) } });
        document["services"]["client"]["environment"] =
            serde_json::json!({ "static": environment });
    }
    serde_yaml_ng::to_string(&document)
        .map(|yaml| {
            format!("# Managed by adi-core: channels/client gets its HTTP port from Hive.\n{yaml}")
        })
        .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
fn shell_command(binary: &str) -> String {
    format!("exec '{}'", binary.replace('\'', "'\\''"))
}

#[cfg(windows)]
fn shell_command(binary: &str) -> String {
    format!("\"{}\"", binary.replace('"', "\"\""))
}

/// Add a top-level import while retaining comments and all unrelated YAML verbatim.
pub(crate) fn with_import(raw: &str, import: &str) -> Result<String, String> {
    // Parse only what this edit owns. Hive's typed service maps accept configurations (such as
    // duplicate HTTP port keys) that Value rejects; unrelated content must remain untouched.
    let document = read_import_document(raw)?;
    let encoded = serde_json::to_string(import).map_err(|e| e.to_string())?;
    let Some(imports) = document.imports else {
        let updated = format!("{raw}\nimports:\n  - {encoded}\n");
        read_import_document(&updated)?;
        return Ok(updated);
    };
    if imports.iter().any(|value| value == import) {
        return Ok(raw.to_string());
    }
    let mut offset = 0;
    for line in raw.split_inclusive('\n') {
        if let Some(value) = line.strip_prefix("imports:") {
            let value = value.trim();
            let mut updated = raw.to_string();
            if value.is_empty() || value.starts_with('#') {
                let indent = raw[offset + line.len()..]
                    .lines()
                    .find(|next| {
                        let trimmed = next.trim();
                        !trimmed.is_empty() && !trimmed.starts_with('#')
                    })
                    .filter(|next| next.trim_start().starts_with('-'))
                    .map_or("  ", |next| &next[..next.len() - next.trim_start().len()]);
                let entry = format!(
                    "{}{indent}- {encoded}\n",
                    if line.ends_with('\n') { "" } else { "\n" }
                );
                updated.insert_str(offset + line.len(), &entry);
            } else if value.starts_with('[') {
                let mut entries = imports.clone();
                entries.push(import.to_string());
                let entries = serde_json::to_string(&entries).map_err(|e| e.to_string())?;
                updated.replace_range(
                    offset..offset + line.len(),
                    &format!("imports: {entries}\n"),
                );
            } else {
                return Err("unsupported Hive imports declaration".to_string());
            }
            // Refuse an edit if an unusual YAML layout made the surgical insertion invalid.
            read_import_document(&updated)?;
            return Ok(updated);
        }
        offset += line.len();
    }
    Err("could not locate the top-level Hive imports declaration".to_string())
}

#[derive(serde::Deserialize)]
struct ImportDocument {
    // Distinguish an absent field from an invalid `imports: null`, and let derive reject a
    // duplicate top-level imports key while ignoring all unrelated document fields.
    #[serde(default, deserialize_with = "read_imports")]
    imports: Option<Vec<String>>,
}

fn read_import_document(raw: &str) -> Result<ImportDocument, String> {
    // Commands may contain commas inside flow maps. Use Hive's own lexical preprocessing, but
    // never evaluate a command or reserve a port while inspecting unrelated imports.
    let (yaml, _) = adi_ports_manager::preprocess(raw);
    serde_yaml_ng::from_str(&yaml).map_err(|e| e.to_string())
}

fn read_imports<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error> {
    <Vec<String> as serde::Deserialize>::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_is_an_always_running_service_with_a_managed_port_and_flavor_domain() {
        let flavor = Flavor::for_id("dev");
        let raw = render(
            &flavor,
            Some("/tmp/ADI Dev/adi-channelsd"),
            Default::default(),
            crate::VERSION,
        )
        .unwrap();
        let hive: adi_hive::config::Hive = serde_yaml_ng::from_str(&raw).unwrap();
        let client = &hive.services["client"];
        assert_eq!(client.start_policy(), adi_hive::config::StartPolicy::Always);
        assert_eq!(
            client.proxy.as_ref().unwrap().host,
            format!("channels.{}", flavor.domain)
        );
        assert!(client.rollout.is_none(), "Hive must allocate the port");
        assert_eq!(client.restart.as_deref(), Some("always"));
        assert!(
            client
                .runner
                .as_ref()
                .unwrap()
                .script
                .as_ref()
                .unwrap()
                .run
                .contains("ADI Dev/adi-channelsd")
        );
    }

    #[test]
    fn releases_change_the_runner_spec_without_changing_routes() {
        let flavor = Flavor::for_id("dev");
        let render_client = |version| {
            render(
                &flavor,
                Some("/tmp/adi-channelsd"),
                Default::default(),
                version,
            )
            .unwrap()
        };
        let old = render_client("1.0.0");
        assert_eq!(old, render_client("1.0.0"), "same release is idempotent");
        let parse = |raw: &str| serde_yaml_ng::from_str::<adi_hive::config::Hive>(raw).unwrap();
        let old_hive = parse(&old);
        let new_hive = parse(&render_client("1.0.1"));
        let old_env = &old_hive.services["client"]
            .environment
            .as_ref()
            .unwrap()
            .static_env;
        let new_env = &new_hive.services["client"]
            .environment
            .as_ref()
            .unwrap()
            .static_env;
        assert_eq!(old_env["ADI_CHANNELS_VERSION"], "1.0.0");
        assert_eq!(new_env["ADI_CHANNELS_VERSION"], "1.0.1");
        assert_ne!(old_env, new_env);

        let old_runners = old_hive.runners(Path::new("/tmp"));
        let new_runners = new_hive.runners(Path::new("/tmp"));
        assert_eq!(old_runners.len(), new_runners.len());
        // A root Hive intentionally produces no runners. The parsed environment assertions
        // above still cover root test environments; ordinary users also exercise spec equality.
        if let Some(old_runner) = old_runners.first() {
            assert_eq!(old_runners.len(), 1);
            assert_eq!(
                old_runners,
                parse(&render_client("1.0.0")).runners(Path::new("/tmp"))
            );
            let new_runner = &new_runners[0];
            assert_ne!(
                old_runner, new_runner,
                "Hive must restart the upgraded client"
            );
            assert_eq!(
                old_runner.run, new_runner.run,
                "the binary path is unchanged"
            );
        }

        let render_routes = |version| {
            render(
                &flavor,
                None,
                [(
                    "ADI_CHANNEL_ROUTER_ADMIN_SECRET".to_string(),
                    "private".to_string(),
                )]
                .into(),
                version,
            )
            .unwrap()
        };
        let routes = render_routes("1.0.0");
        assert_eq!(routes, render_routes("1.0.1"));
        let routes: serde_yaml_ng::Value = serde_yaml_ng::from_str(&routes).unwrap();
        assert!(routes["services"]["client"]["runner"].is_null());
        assert!(routes["services"]["client"]["environment"].is_null());
    }

    #[test]
    fn importing_preserves_existing_services_and_comments_and_is_idempotent() {
        for raw in [
            "# operator notes\nimports:\n  - /existing/hive.yaml\nservices: {custom: {start: always}}\n",
            "# operator notes\nimports:\n- /existing/hive.yaml\nservices: {custom: {start: always}}\n",
            "# operator notes\nimports: [/existing/hive.yaml]\nservices: {custom: {start: always}}\n",
            "# operator notes\nservices: {custom: {start: always}}\n",
        ] {
            let updated = with_import(raw, "/store/channels/hive.yaml").unwrap();
            assert!(updated.contains("# operator notes"));
            assert!(updated.contains("services: {custom: {start: always}}"));
            let parsed: serde_yaml_ng::Value = serde_yaml_ng::from_str(&updated).unwrap();
            assert!(
                parsed["imports"]
                    .as_sequence()
                    .unwrap()
                    .iter()
                    .any(|v| v.as_str() == Some("/store/channels/hive.yaml"))
            );
            assert_eq!(
                with_import(&updated, "/store/channels/hive.yaml").unwrap(),
                updated
            );
        }
    }

    #[test]
    fn importing_accepts_hive_service_maps_without_rewriting_them() {
        let service = "# hand-managed\nservices:\n  app:\n    rollout:\n      recreate:\n        ports: {http: 8000, http: 8001}\n";
        for raw in [service.to_string(), format!("imports: []\n{service}")] {
            let updated = with_import(&raw, "/store/channels/routes.yaml").unwrap();
            assert!(
                updated.contains(service),
                "unrelated YAML stays byte-for-byte intact"
            );
            let parsed: adi_hive::config::Hive = serde_yaml_ng::from_str(&updated).unwrap();
            assert_eq!(parsed.imports, ["/store/channels/routes.yaml"]);
            assert_eq!(
                with_import(&updated, "/store/channels/routes.yaml").unwrap(),
                updated
            );
        }
        for invalid in [
            "imports: []\nimports: []\n",
            "imports: null\n",
            "imports: {invalid: shape}\n",
        ] {
            assert!(with_import(invalid, "/store/channels/routes.yaml").is_err());
        }
    }

    #[test]
    fn importing_preserves_unevaluated_port_commands_in_flow_maps() {
        let service = "# managed port\nservices:\n  app:\n    rollout:\n      recreate:\n        ports: { http: bash`ports-manager.get('app', 'http')` }\n";
        for raw in [service.to_string(), format!("imports: []\n{service}")] {
            let updated = with_import(&raw, "/store/channels/routes.yaml").unwrap();
            assert!(updated.contains(service));
            assert_eq!(
                read_import_document(&updated).unwrap().imports.unwrap(),
                ["/store/channels/routes.yaml"]
            );
            assert_eq!(
                with_import(&updated, "/store/channels/routes.yaml").unwrap(),
                updated
            );
        }
    }

    #[test]
    fn both_hives_import_the_same_daemon_and_share_its_port_lease() {
        let root = std::env::temp_dir().join(format!(
            "adi-core-channels-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let store = Config::with_root(&root);
        let supervisor = store.module("dashboards");
        supervisor
            .write_raw(
                CONFIG_FILE,
                b"proxy: {bind: ['127.0.0.1:0']}\nimports: []\n",
            )
            .unwrap();
        let frontdoor = store.module("hive");
        frontdoor
            .write_raw(CONFIG_FILE, b"# custom frontdoor\nservices: {}\n")
            .unwrap();
        let flavor = Flavor::for_id("dev");
        ensure_in(&store, &flavor, "/tmp/adi-channelsd").unwrap();
        let before = std::fs::read(supervisor.raw_path(CONFIG_FILE)).unwrap();
        ensure_in(&store, &flavor, "/tmp/adi-channelsd").unwrap();
        assert_eq!(
            before,
            std::fs::read(supervisor.raw_path(CONFIG_FILE)).unwrap()
        );

        let ports = adi_ports_manager::Ports::with_config(adi_ports_manager::Config {
            registry_path: root.join("ports.json"),
            ..Default::default()
        });
        let mut running = adi_hive::config::Hive::load(&supervisor.raw_path(CONFIG_FILE)).unwrap();
        let mut routing = adi_hive::config::Hive::load(&frontdoor.raw_path(CONFIG_FILE)).unwrap();
        let leased = running.allocate_missing_ports(&ports);
        assert_eq!(leased.len(), 1);
        assert_eq!(leased[0].0, "channels/client");
        assert_eq!(routing.allocate_missing_ports(&ports), leased);
        assert!(running.services["channels/client"].runner.is_some());
        assert!(routing.services["channels/client"].runner.is_none());
        assert_eq!(routing.resolve().routes[0].service, "channels/client");
        assert_eq!(
            routing.resolve().routes[0].host,
            format!("channels.{}", flavor.domain)
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
