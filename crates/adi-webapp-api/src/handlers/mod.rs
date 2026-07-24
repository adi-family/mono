//! The `/api/*` server surface: the real backend over the [`adi_ports_manager`] port
//! registry. Each handler returns `(status, json_body)`; the host ([`adi-app`](../adi-app))
//! owns the socket and writes the response. Compiled only with the `server` feature,
//! which pulls in the filesystem-backed registry and so is native-only.

mod agents;
mod dashboards;
mod files;
mod fs;
mod health;
mod mesh;
mod meta;
mod ports;
mod projects;
mod response;
mod secrets;
mod services;
mod tasks;
mod tools;
mod triggers;
mod workspaces;

pub use agents::*;
pub use dashboards::*;
pub use files::*;
pub use fs::*;
pub use health::*;
pub use mesh::*;
pub use meta::*;
pub use ports::*;
pub use projects::*;
pub use response::{Response, error};
pub use secrets::*;
pub use services::*;
pub use tasks::*;
pub use tools::*;
pub use triggers::*;
pub use workspaces::*;

#[cfg(test)]
use adi_agents::Agents;
#[cfg(test)]
use adi_ports_manager::Ports;
#[cfg(test)]
use adi_projects::Projects;
#[cfg(test)]
use adi_tasks::Tasks;
#[cfg(test)]
use adi_triggers::Triggers;
#[cfg(test)]
use std::time::Instant;

#[cfg(test)]
mod tests {
    use adi_ports_manager::Config;
    use serde_json::Value;

    use super::*;

    fn temp_manager() -> Ports {
        // Isolated registry per test so we never touch the real one.
        let path = std::env::temp_dir().join(format!(
            "adi-webapp-api-{}-{:?}/registry.json",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        Ports::with_config(Config {
            registry_path: path,
            ..Config::default()
        })
    }

    fn temp_agents() -> Agents {
        let root = std::env::temp_dir().join(format!(
            "adi-webapp-api-agents-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Agents::with_config(adi_config::Config::with_root(root))
    }

    #[test]
    fn create_service_writes_the_hive_yaml_and_reports_it() {
        let store = temp_projects();
        // The auto `http` port is a ports-manager command the detail read executes, so pin
        // command execution to an isolated registry.
        let Response { status, body } = adi_ports_manager::with_ports(temp_manager(), || {
            create_service(
                &store,
                br#"{"project":"demo","name":"api","run":"bun run start","host":"demo.adi"}"#,
                &[],
            )
        });
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["services"][0]["name"], "api");
        assert_eq!(v["services"][0]["host"], "demo.adi");
        assert_eq!(v["services"][0]["run"], "bun run start");
        let text = std::fs::read_to_string(store.hive_path("demo").unwrap()).unwrap();
        assert!(
            text.contains("bash`ports-manager.get('demo/api', 'http')`"),
            "the auto port is written as a ports-manager command, got: {text}"
        );
        assert!(
            text.contains("version"),
            "fields outside `services` survive the rewrite, got: {text}"
        );
    }

    #[test]
    fn create_service_preserves_existing_entries_and_their_port_commands() {
        let store = temp_projects();
        let path = store.hive_path("demo").unwrap();
        std::fs::write(
            &path,
            "services:\n  web:\n    rollout:\n      recreate:\n        ports:\n          http: bash`ports-manager.get('demo/web', 'http')`\n    runner:\n      script:\n        run: bun serve\n",
        )
        .unwrap();
        let Response { status, body } = adi_ports_manager::with_ports(temp_manager(), || {
            create_service(
                &store,
                br#"{"project":"demo","name":"api","run":"cargo run","port":45112}"#,
                &[],
            )
        });
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["services"].as_array().unwrap().len(), 2);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("bash`ports-manager.get('demo/web', 'http')`"),
            "the existing entry's port command survives the rewrite, got: {text}"
        );
        assert!(
            text.contains("45112"),
            "the explicit port is written: {text}"
        );
    }

    #[test]
    fn create_service_refuses_duplicates_bad_names_and_unknown_projects() {
        let store = temp_projects();
        let req = br#"{"project":"demo","name":"api","run":"bun start","port":45113}"#;
        let Response { status, .. } = create_service(&store, req, &[]);
        assert_eq!(status, 200);
        let Response { status, .. } = create_service(&store, req, &[]);
        assert_eq!(status, 409, "the same name again is a conflict");
        let Response { status, .. } = create_service(
            &store,
            br#"{"project":"demo","name":"../evil","run":"x"}"#,
            &[],
        );
        assert_eq!(status, 400, "a path-escaping name is rejected");
        let Response { status, .. } = create_service(
            &store,
            br#"{"project":"nope","name":"api","run":"x","port":45114}"#,
            &[],
        );
        assert_eq!(status, 404, "an unregistered project is rejected");
    }

    #[test]
    fn create_service_writes_a_docker_runner() {
        let store = temp_projects();
        let Response { status, body } = adi_ports_manager::with_ports(temp_manager(), || {
            create_service(
                &store,
                br#"{"project":"demo","name":"web","host":"web.adi","port":45120,
                     "docker":{"image":"nginx:1.27","container_port":80,
                       "volumes":["./site:/usr/share/nginx/html:ro",""],
                       "environment":{"LOG_LEVEL":"debug"},"pull":"missing",
                       "args":["--memory=512m"],"command":["nginx","-g","daemon off;"]}}"#,
                &[],
            )
        });
        assert_eq!(status, 200, "{body}");
        // The read-back view shows a docker service by its image as the "command".
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["services"][0]["name"], "web");
        assert_eq!(v["services"][0]["run"], "docker: nginx:1.27");

        let text = std::fs::read_to_string(store.hive_path("demo").unwrap()).unwrap();
        assert!(text.contains("docker:"), "a docker runner is written: {text}");
        assert!(text.contains("image: nginx:1.27"), "got: {text}");
        assert!(text.contains("http: 80"), "container port mapping: {text}");
        assert!(text.contains("pull: missing"), "got: {text}");
        assert!(text.contains("LOG_LEVEL: debug"), "got: {text}");
        assert!(text.contains("--memory=512m"), "got: {text}");
        assert!(
            !text.contains("script:"),
            "a docker service must not also write a script runner: {text}"
        );
        // Blank volume entries are dropped.
        assert!(
            text.contains("./site:/usr/share/nginx/html:ro"),
            "got: {text}"
        );
    }

    #[test]
    fn create_service_requires_a_run_or_a_docker_image() {
        let store = temp_projects();
        // Neither a run command nor a docker image → 400.
        let Response { status, .. } =
            create_service(&store, br#"{"project":"demo","name":"x"}"#, &[]);
        assert_eq!(status, 400);
        // A docker block with a blank image → 400.
        let Response { status, .. } = create_service(
            &store,
            br#"{"project":"demo","name":"x","docker":{"image":"  "}}"#,
            &[],
        );
        assert_eq!(status, 400);
    }

    /// An isolated config rooted in a temp dir, so a dashboards test never touches the real store.
    fn temp_dashboards_cfg() -> adi_config::Config {
        let root = std::env::temp_dir().join(format!(
            "adi-webapp-api-dash-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_config::Config::with_root(root)
    }

    #[test]
    fn dashboard_archive_parks_hive_and_restore_returns_it() {
        let cfg = temp_dashboards_cfg();
        let ports = temp_manager();

        // Scaffold one dashboard; its live hive file is what the supervisor globs.
        let Response { status, body } = create_dashboard(&cfg, &ports, br#"{"name":"Metrics"}"#);
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        let id = v["id"].as_str().unwrap().to_string();
        let dir = cfg.module("dashboards").dir().join(&id);
        assert!(dir.join(".adi/hive.yaml").is_file());

        let ref_body = format!(r#"{{"id":{}}}"#, serde_json::to_string(&id).unwrap());

        // Archive: manifest records archived_at, the hive file is parked out of the glob, and the
        // fresh listing flags the dashboard.
        let Response { status, body } = archive_dashboard(&cfg, &ports, &[], ref_body.as_bytes());
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        let d = &v["dashboards"][0];
        assert_eq!(d["id"], id);
        assert!(d["archived_at"].as_u64().is_some(), "archived_at set: {body}");
        assert!(!dir.join(".adi/hive.yaml").exists(), "live hive file is parked");
        assert!(dir.join(".adi/hive.yaml.archived").is_file(), "parked file present");
        let toml = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        assert!(toml.contains("archived_at ="), "manifest records archive: {toml}");

        // Restore: the hive file returns to the glob and archived_at clears.
        let Response { status, body } = unarchive_dashboard(&cfg, &ports, &[], ref_body.as_bytes());
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(
            v["dashboards"][0]["archived_at"].is_null(),
            "archived_at cleared: {body}"
        );
        assert!(dir.join(".adi/hive.yaml").is_file(), "hive file restored");
        assert!(!dir.join(".adi/hive.yaml.archived").exists(), "parked file removed");

        // Bad ids: an unknown or path-escaping id is a 404; a blank or unparseable body is a 400.
        assert_eq!(
            archive_dashboard(&cfg, &ports, &[], br#"{"id":"ghost"}"#).status,
            404
        );
        assert_eq!(
            archive_dashboard(&cfg, &ports, &[], br#"{"id":"../evil"}"#).status,
            404
        );
        assert_eq!(
            archive_dashboard(&cfg, &ports, &[], br#"{"id":""}"#).status,
            400
        );
        assert_eq!(archive_dashboard(&cfg, &ports, &[], b"not json").status, 400);
    }

    #[test]
    fn dashboard_delete_requires_archived_then_removes_dir() {
        let cfg = temp_dashboards_cfg();
        let ports = temp_manager();

        let Response { body, .. } = create_dashboard(&cfg, &ports, br#"{"name":"Metrics"}"#);
        let v: Value = serde_json::from_str(&body).unwrap();
        let id = v["id"].as_str().unwrap().to_string();
        let dir = cfg.module("dashboards").dir().join(&id);
        let ref_body = format!(r#"{{"id":{}}}"#, serde_json::to_string(&id).unwrap());

        // A live dashboard can't be deleted — it must be archived first, so the running servers
        // are never pulled out from under themselves.
        let Response { status, .. } = delete_dashboard(&cfg, &ports, &[], ref_body.as_bytes());
        assert_eq!(status, 409, "delete refused while live");
        assert!(dir.is_dir(), "directory left untouched");

        // Archive, then delete removes the whole directory and drops it from the listing.
        let _ = archive_dashboard(&cfg, &ports, &[], ref_body.as_bytes());
        let Response { status, body } = delete_dashboard(&cfg, &ports, &[], ref_body.as_bytes());
        assert_eq!(status, 200, "{body}");
        assert!(!dir.exists(), "directory removed");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(
            v["dashboards"].as_array().unwrap().is_empty(),
            "listing is empty: {body}"
        );

        assert_eq!(
            delete_dashboard(&cfg, &ports, &[], br#"{"id":"ghost"}"#).status,
            404
        );
        assert_eq!(
            delete_dashboard(&cfg, &ports, &[], br#"{"id":""}"#).status,
            400
        );
    }

    /// An isolated tasks store rooted in a temp dir, so a tasks test never touches the real one.
    fn temp_tasks() -> Tasks {
        let root = std::env::temp_dir().join(format!(
            "adi-webapp-api-tasks-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Tasks::with_config(adi_config::Config::with_root(root))
    }

    #[test]
    fn delete_task_removes_it_and_reparents_children() {
        let store = temp_tasks();

        // A parent with one child.
        let _ = create_task(&store, br#"{"title":"parent"}"#);
        let Response { body, .. } = tasks(&store);
        let v: Value = serde_json::from_str(&body).unwrap();
        let parent_id = v["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["title"] == "parent")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let child_body = format!(
            r#"{{"title":"child","parent":{}}}"#,
            serde_json::to_string(&parent_id).unwrap()
        );
        let _ = create_task(&store, child_body.as_bytes());

        // Deleting the parent removes it and reparents the child to the root (no dangling link).
        let ref_body = format!(r#"{{"id":{}}}"#, serde_json::to_string(&parent_id).unwrap());
        let Response { status, body } = delete_task(&store, ref_body.as_bytes());
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        let list = v["tasks"].as_array().unwrap();
        assert!(
            !list.iter().any(|t| t["id"] == parent_id.as_str()),
            "parent is gone: {body}"
        );
        let child = list.iter().find(|t| t["title"] == "child").unwrap();
        assert!(child["parent"].is_null(), "child reparented to root: {body}");

        assert_eq!(delete_task(&store, br#"{"id":"ghost"}"#).status, 404);
        assert_eq!(delete_task(&store, br#"{"id":""}"#).status, 400);
    }

    #[test]
    fn health_reports_ok_and_identity() {
        let Response { status, body } = health("adi-app", "1.2.3", Instant::now());
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["service"], "adi-app");
        assert_eq!(v["version"], "1.2.3");
    }

    #[test]
    fn reserve_then_ports_lists_the_lease() {
        let m = temp_manager();
        let Response { status, body } = reserve(&m, br#"{"service":"web","key":"http"}"#);
        assert_eq!(status, 200);
        let reserved: Value = serde_json::from_str(&body).unwrap();
        let port = reserved["port"].as_u64().unwrap();

        let Response { status, body } = ports(&m);
        assert_eq!(status, 200);
        let listed: Value = serde_json::from_str(&body).unwrap();
        let leases = listed["leases"].as_array().unwrap();
        assert_eq!(leases.len(), 1);
        assert_eq!(leases[0]["service"], "web");
        assert_eq!(leases[0]["port"].as_u64().unwrap(), port);
    }

    #[test]
    fn reserve_is_idempotent_over_the_api() {
        let m = temp_manager();
        let Response { body: first, .. } = reserve(&m, br#"{"service":"web","key":"http"}"#);
        let Response { body: again, .. } = reserve(&m, br#"{"service":"web","key":"http"}"#);
        let a: Value = serde_json::from_str(&first).unwrap();
        let b: Value = serde_json::from_str(&again).unwrap();
        assert_eq!(a["port"], b["port"]);
    }

    #[test]
    fn release_frees_the_lease() {
        let m = temp_manager();
        let _ = reserve(&m, br#"{"service":"web","key":"http"}"#);
        let Response { status, body } = release(&m, br#"{"service":"web","key":"http"}"#);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(v["freed"].is_number());

        let Response { body, .. } = ports(&m);
        let listed: Value = serde_json::from_str(&body).unwrap();
        assert!(listed["leases"].as_array().unwrap().is_empty());
    }

    #[test]
    fn bad_body_is_a_400() {
        let m = temp_manager();
        assert_eq!(reserve(&m, b"not json").status, 400);
        assert_eq!(reserve(&m, br#"{"service":"","key":"x"}"#).status, 400);
    }

    #[test]
    fn agents_response_includes_form_schema() {
        let store = temp_agents();
        let Response { status, body } = agents(&store);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();

        let backends = v["form"]["backends"].as_array().unwrap();
        assert!(
            backends
                .iter()
                .any(|b| b["id"] == "pty:claude" && b["executor"] == "pty")
        );
        assert!(
            backends
                .iter()
                .any(|b| b["id"] == "harness:adi" && b["executor"] == "harness")
        );
        assert!(
            backends
                .iter()
                .any(|b| b["id"] == "wasm:loop-script" && b["executor"] == "wasm")
        );

        let fields = v["form"]["fields"].as_array().unwrap();
        assert!(fields.iter().any(|f| f["name"] == "api_key_env"));
        assert!(fields.iter().any(|f| {
            f["name"] == "wasm"
                && f["backend_ids"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|id| id == "wasm:loop-script")
        }));
        assert!(fields.iter().any(|f| {
            f["name"] == "permission_mode"
                && f["backend_ids"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|id| id == "pty:claude")
        }));
        for name in [
            "effort",
            "sandbox",
            "approval",
            "thinking",
            "num_ctx",
            "max_tokens",
        ] {
            assert!(
                fields.iter().any(|f| f["name"] == name),
                "missing field {name}"
            );
        }
        for (field, excluded_backend) in [
            ("tools", "pty:codex"),
            ("max_turns", "process:claude"),
            ("fallback_model", "pty:claude"),
            ("skip_git_repo_check", "pty:codex"),
        ] {
            let ids = fields.iter().find(|f| f["name"] == field).unwrap()["backend_ids"]
                .as_array()
                .unwrap();
            assert!(!ids.iter().any(|id| id == excluded_backend));
        }
        // Temperature applies only where a non-default value is safe (the Gemini and Ollama
        // providers) — not the reasoning / current-model providers where it 400s.
        let temperature = fields.iter().find(|f| f["name"] == "temperature").unwrap();
        let providers = temperature["providers"].as_array().unwrap();
        assert!(providers.iter().any(|p| p == "ollama"));
        assert!(!providers.iter().any(|p| p == "anthropic"));
    }

    #[test]
    fn agents_report_runnable_for_pty_and_process_backends() {
        let store = temp_agents();
        let _ = save_agent(&store, br#"{"name":"solver","backend":"pty:claude"}"#);
        let _ = save_agent(&store, br#"{"name":"reviewer","backend":"process:codex"}"#);
        let _ = save_agent(&store, br#"{"name":"looper","backend":"harness:adi"}"#);

        let Response { status, body } = agents(&store);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        let list = v["agents"].as_array().unwrap();
        let looper = list.iter().find(|a| a["name"] == "looper").unwrap();
        let reviewer = list.iter().find(|a| a["name"] == "reviewer").unwrap();
        let solver = list.iter().find(|a| a["name"] == "solver").unwrap();
        assert_eq!(looper["runnable"], false);
        assert_eq!(reviewer["runnable"], true);
        assert_eq!(solver["runnable"], true);
        assert_eq!(looper["running"], false);
        assert_eq!(reviewer["running"], false);
    }

    #[test]
    fn run_of_a_missing_agent_is_404() {
        let store = temp_agents();
        let Response { status, .. } = run_agent(&store, br#"{"name":"ghost"}"#);
        assert_eq!(status, 404);
    }

    #[test]
    fn agent_code_reads_and_writes_the_src_argument_file() {
        let store = temp_agents();

        let _ = save_agent(&store, br#"{"name":"emp","backend":"wasm:loop-script"}"#);
        assert_eq!(agent_code(&store, br#"{"name":"emp"}"#).status, 400);
        assert_eq!(agent_code(&store, br#"{"name":"ghost"}"#).status, 404);

        let src = std::env::temp_dir().join(format!(
            "adi-webapp-api-agent-code-{}.ts",
            std::process::id()
        ));
        std::fs::write(&src, "export const main = () => {};\n").unwrap();
        let body = format!(
            r#"{{"name":"emp","backend":"wasm:loop-script","arguments":{{"src":{}}}}}"#,
            serde_json::to_string(&src.display().to_string()).unwrap()
        );
        let _ = save_agent(&store, body.as_bytes());

        let Response { status, body } = agent_code(&store, br#"{"name":"emp"}"#);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["code"], "export const main = () => {};\n");
        assert_eq!(v["path"], src.display().to_string());

        let save = serde_json::json!({"name": "emp", "code": "// edited\n"}).to_string();
        let Response { status, .. } = save_agent_code(&store, save.as_bytes());
        assert_eq!(status, 200);
        assert_eq!(std::fs::read_to_string(&src).unwrap(), "// edited\n");
        let _ = std::fs::remove_file(&src);
    }

    #[test]
    fn peek_reports_not_running_for_a_sessionless_agent() {
        let store = temp_agents();
        let _ = save_agent(&store, br#"{"name":"solver","backend":"pty:claude"}"#);

        let Response { status, body } = peek_agent(&store, br#"{"name":"solver"}"#);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["running"], false);
        assert_eq!(v["output"], "");
        // A pty session has no external attach command; the live view is the control panel.
        assert_eq!(v["attach"], "");

        assert_eq!(peek_agent(&store, br#"{"name":"ghost"}"#).status, 404);
    }

    #[test]
    fn send_keys_validates_body_and_run_state() {
        let store = temp_agents();
        let _ = save_agent(&store, br#"{"name":"solver","backend":"pty:claude"}"#);

        assert_eq!(
            send_agent_keys(&store, br#"{"name":"ghost","key":"Enter"}"#).status,
            404
        );
        assert_eq!(send_agent_keys(&store, br#"{"name":"solver"}"#).status, 400);

        let Response { status, body } =
            send_agent_keys(&store, br#"{"name":"solver","text":"hi"}"#);
        assert_eq!(status, 409);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(v["error"].as_str().unwrap().contains("isn't running"));
    }

    #[test]
    fn stop_is_idempotent_and_404s_unknown() {
        let store = temp_agents();
        let _ = save_agent(&store, br#"{"name":"solver","backend":"pty:claude"}"#);

        let Response { status, body } = stop_agent(&store, br#"{"name":"solver"}"#);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(
            v["agents"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["name"] == "solver")
        );
        assert_eq!(stop_agent(&store, br#"{"name":"ghost"}"#).status, 404);
    }

    #[test]
    fn run_of_an_unrunnable_backend_is_400() {
        let store = temp_agents();
        let _ = save_agent(&store, br#"{"name":"looper","backend":"harness:adi"}"#);
        let Response { status, body } = run_agent(&store, br#"{"name":"looper"}"#);
        assert_eq!(status, 400);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(v["error"].as_str().unwrap().contains("can't be run yet"));
    }

    #[test]
    fn save_agent_round_trips_backend_settings() {
        let store = temp_agents();
        let Response { status, body } = save_agent(
            &store,
            br#"{
                "name":"api-solver",
                "backend":"api:openai",
                "arguments":{
                    "system_prompt":"Solve carefully",
                    "tools":"tasks,projects",
                    "model":"gpt-5-codex",
                    "permission_mode":"plan",
                    "temperature":0.2,
                    "max_turns":12,
                    "resume":true,
                    "api_key_env":"OPENAI_API_KEY",
                    "base_url":"http://localhost:11434",
                    "bad key":"drop",
                    "empty":"",
                    "cloud_manifest":{
                        "region":"eu-west-1",
                        "replicas":2,
                        "capabilities":["files", "tasks"]
                    }
                }
            }"#,
        );
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        let agent = &v["agents"].as_array().unwrap()[0];
        assert_eq!(agent["arguments"]["system_prompt"], "Solve carefully");
        assert_eq!(agent["arguments"]["tools"], "tasks,projects");
        assert_eq!(agent["arguments"]["model"], "gpt-5-codex");
        assert_eq!(agent["arguments"]["permission_mode"], "plan");
        assert_eq!(agent["arguments"]["temperature"], 0.2);
        assert_eq!(agent["arguments"]["max_turns"], 12);
        assert_eq!(agent["arguments"]["resume"], true);
        assert_eq!(agent["arguments"]["cloud_manifest"]["replicas"], 2);
        assert_eq!(
            agent["arguments"]["cloud_manifest"]["capabilities"][1],
            "tasks"
        );
        assert_eq!(agent["arguments"]["api_key_env"], "OPENAI_API_KEY");
        assert_eq!(agent["arguments"]["base_url"], "http://localhost:11434");
        assert_eq!(agent["arguments"]["bad key"], "drop");
        assert_eq!(agent["arguments"]["empty"], "");
        for flattened in [
            "system_prompt",
            "tools",
            "model",
            "permission_mode",
            "temperature",
            "max_turns",
            "extra",
        ] {
            assert!(
                agent.get(flattened).is_none(),
                "flattened field {flattened}"
            );
        }
    }

    #[test]
    fn save_agent_rejects_unknown_arguments_for_built_in_backends() {
        let store = temp_agents();
        let Response { status, body } = save_agent(
            &store,
            br#"{
                "name":"typo",
                "backend":"process:codex",
                "arguments":{"max_truns":12}
            }"#,
        );
        assert_eq!(status, 400);
        assert!(body.contains("max_truns"), "{body}");
    }

    // ---- triggers ----------------------------------------------------------------------

    fn temp_triggers() -> Triggers {
        let root = std::env::temp_dir().join(format!(
            "adi-webapp-api-triggers-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Triggers::with_config(adi_config::Config::with_root(root))
    }

    /// Saving is what steers the background supervisor, so the handlers take one. These tests
    /// exercise the store side only, so they hand over a supervisor that runs nothing.
    fn inert_supervisor(store: &Triggers) -> std::sync::Arc<adi_triggers::Supervisor> {
        adi_triggers::Supervisor::inert(store.clone())
    }

    #[test]
    fn triggers_response_includes_the_kind_options() {
        let store = temp_triggers();
        let Response { status, body } = triggers(&store);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(v["triggers"].as_array().unwrap().is_empty());
        let kinds: Vec<&str> = v["kinds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k["id"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["webhook", "background", "event"]);
    }

    #[test]
    fn save_trigger_persists_event_kind_and_patterns() {
        let store = temp_triggers();
        let Response { status, body } = save_trigger(
            &store,
            &inert_supervisor(&store),
            br#"{
                "name":"on-task",
                "kind":"event",
                "code":"echo $ADI_EVENT",
                "events":[" adi.tasks.* ", "", "adi.agents.**"]
            }"#,
        );
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        let t = &v["triggers"].as_array().unwrap()[0];
        assert_eq!(t["kind"], "event");
        // Blank patterns are dropped; the rest are trimmed and preserved in order.
        let events: Vec<&str> = t["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e.as_str().unwrap())
            .collect();
        assert_eq!(events, ["adi.tasks.*", "adi.agents.**"]);
    }

    fn temp_events() -> adi_events::Events {
        let root = std::env::temp_dir().join(format!(
            "adi-webapp-api-events-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_events::Events::with_config(adi_config::Config::with_root(root))
    }

    #[test]
    fn emit_event_publishes_onto_the_bus() {
        let bus = temp_events();
        let Response { status, body } = emit_event(
            &bus,
            br#"{ "name":"adi.tasks.created", "payload":"{\"id\":\"t1\"}" }"#,
        );
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["event"], "adi.tasks.created");

        let spooled = bus.drain().unwrap();
        assert_eq!(spooled.len(), 1);
        assert_eq!(spooled[0].record.name, "adi.tasks.created");
        assert_eq!(spooled[0].record.payload, r#"{"id":"t1"}"#);

        // An empty name is a 400, not a spooled empty event.
        let Response { status, .. } = emit_event(&bus, br#"{ "name":"  " }"#);
        assert_eq!(status, 400);
    }

    #[test]
    fn save_trigger_round_trips_and_cleans_extras() {
        let store = temp_triggers();
        let Response { status, body } = save_trigger(
            &store,
            &inert_supervisor(&store),
            br#"{
                "name":"deploy-hook",
                "kind":"webhook",
                "code":"echo deployed",
                "description":" redeploy on push ",
                "project":" demo ",
                "extra":{ "secret":" s3cr3t ", "bad key":"drop", "empty":"" }
            }"#,
        );
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        let t = &v["triggers"].as_array().unwrap()[0];
        assert_eq!(t["name"], "deploy-hook");
        assert_eq!(t["kind"], "webhook");
        assert_eq!(t["enabled"], true);
        assert_eq!(t["project"], "demo");
        assert_eq!(t["description"], "redeploy on push");
        assert_eq!(t["extra"]["secret"], "s3cr3t");
        assert!(t["extra"]["bad key"].is_null());
        assert!(t["extra"]["empty"].is_null());
        assert!(t["last_fired_at"].is_null());

        assert_eq!(
            save_trigger(
                &store,
                &inert_supervisor(&store),
                br#"{"name":"x","kind":""}"#
            )
            .status,
            400
        );
        assert_eq!(
            save_trigger(&store, &inert_supervisor(&store), b"not json").status,
            400
        );
    }

    #[test]
    fn fire_validates_the_target() {
        let store = temp_triggers();
        assert_eq!(fire_trigger(&store, br#"{"name":"ghost"}"#).status, 404);
        let _ = save_trigger(
            &store,
            &inert_supervisor(&store),
            br#"{"name":"idle","kind":"background"}"#,
        );
        assert_eq!(fire_trigger(&store, br#"{"name":"idle"}"#).status, 400);
    }

    #[test]
    fn log_of_a_never_fired_trigger_is_empty_not_an_error() {
        let store = temp_triggers();
        let _ = save_trigger(
            &store,
            &inert_supervisor(&store),
            br#"{"name":"idle","kind":"background","code":"true"}"#,
        );
        let Response { status, body } = trigger_log(&store, br#"{"name":"idle"}"#);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["fired"], false);
        assert_eq!(v["output"], "");
        assert_eq!(trigger_log(&store, br#"{"name":"ghost"}"#).status, 404);
    }

    #[test]
    fn hook_gates_on_kind_enabled_and_secret() {
        let store = temp_triggers();
        let _ = save_trigger(
            &store,
            &inert_supervisor(&store),
            br#"{"name":"background-only","kind":"background","code":"true"}"#,
        );
        let _ = save_trigger(
            &store,
            &inert_supervisor(&store),
            br#"{"name":"paused","kind":"webhook","code":"true","enabled":false}"#,
        );
        let _ = save_trigger(
            &store,
            &inert_supervisor(&store),
            br#"{"name":"locked","kind":"webhook","code":"true","extra":{"secret":"s3"}}"#,
        );

        // Unknown, unsafe, and background names all answer the same 404.
        assert_eq!(hook_trigger(&store, "ghost", "", b"").status, 404);
        assert_eq!(hook_trigger(&store, "../etc", "", b"").status, 404);
        assert_eq!(hook_trigger(&store, "background-only", "", b"").status, 404);
        assert_eq!(hook_trigger(&store, "paused", "", b"").status, 403);
        assert_eq!(hook_trigger(&store, "locked", "", b"").status, 403);
        assert_eq!(
            hook_trigger(&store, "locked", "secret=wrong", b"").status,
            403
        );
        let Response { status, body } =
            hook_trigger(&store, "locked", "x=1&secret=s3", b"{\"ref\":\"main\"}");
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["trigger"], "locked");
    }

    /// A webhook with a `trigger_on` allowlist only fires for a request body naming an allowed
    /// project; a body naming another project (or none) is refused with a 403.
    #[test]
    fn hook_gates_on_trigger_on_project() {
        let store = temp_triggers();
        let _ = save_trigger(
            &store,
            &inert_supervisor(&store),
            br#"{"name":"scoped","kind":"webhook","code":"true","trigger_on":["alpha"]}"#,
        );

        // No project, and the wrong project, are both refused.
        assert_eq!(hook_trigger(&store, "scoped", "", b"{}").status, 403);
        assert_eq!(
            hook_trigger(&store, "scoped", "", br#"{"project":"beta"}"#).status,
            403
        );
        // The allowed project fires it.
        let Response { status, body } =
            hook_trigger(&store, "scoped", "", br#"{"project":"alpha"}"#);
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["trigger"], "scoped");
    }

    // ---- files -----------------------------------------------------------------------

    /// A projects store rooted in an isolated temp dir, with a registered `demo` project whose
    /// `.adi/hive.yaml` exists (mirroring the real on-disk layout).
    fn temp_projects() -> Projects {
        let root = std::env::temp_dir().join(format!(
            "adi-webapp-api-files-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        let store = Projects::with_config(adi_config::Config::with_root(&root));
        store
            .create_with_id("demo", Some("Demo".into()), None, None)
            .unwrap();
        let hive = store.hive_path("demo").unwrap();
        std::fs::create_dir_all(hive.parent().unwrap()).unwrap();
        std::fs::write(&hive, b"version: \"1\"\n").unwrap();
        store
    }

    #[test]
    fn list_files_shows_the_project_tree() {
        let store = temp_projects();
        let Response { status, body } = list_files(&store, br#"{"id":"demo","path":""}"#);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["path"], "");
        assert!(v["parent"].is_null());
        let names: Vec<&str> = v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&".adi"));
        assert!(names.contains(&"config.toml"));

        let Response { body, .. } = list_files(&store, br#"{"id":"demo","path":".adi"}"#);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["path"], ".adi");
        assert_eq!(v["parent"], "");
    }

    #[test]
    fn read_then_write_round_trips_the_hive_file() {
        let store = temp_projects();
        let Response { status, body } =
            read_file(&store, br#"{"id":"demo","path":".adi/hive.yaml"}"#);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["content"], "version: \"1\"\n");

        let Response { status, body } = write_file(
            &store,
            br#"{"id":"demo","path":".adi/hive.yaml","content":"version: \"2\"\n"}"#,
        );
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["content"], "version: \"2\"\n");

        let Response { body, .. } = read_file(&store, br#"{"id":"demo","path":".adi/hive.yaml"}"#);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["content"], "version: \"2\"\n");
    }

    #[test]
    fn escaping_paths_are_refused_with_400() {
        let store = temp_projects();
        assert_eq!(
            list_files(&store, br#"{"id":"demo","path":".."}"#).status,
            400
        );
        assert_eq!(
            read_file(&store, br#"{"id":"demo","path":"../../secret"}"#).status,
            400
        );
        assert_eq!(
            write_file(&store, br#"{"id":"demo","path":"../evil","content":"x"}"#).status,
            400
        );
    }

    #[test]
    fn unregistered_project_is_a_404() {
        let store = temp_projects();
        assert_eq!(
            list_files(&store, br#"{"id":"ghost","path":""}"#).status,
            404
        );
        assert_eq!(
            list_files(&store, br#"{"id":"../x","path":""}"#).status,
            400
        );
    }

    #[test]
    fn reading_a_missing_file_is_a_404() {
        let store = temp_projects();
        assert_eq!(
            read_file(&store, br#"{"id":"demo","path":"nope.txt"}"#).status,
            404
        );
    }

    // ---- workspaces & project hooks ----------------------------------------------------

    /// Poll until `cond` holds (hook runs are detached), up to ~5s.
    fn wait_until(cond: impl Fn() -> bool) -> bool {
        for _ in 0..250 {
            if cond() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn workspaces_state_starts_empty_with_init_next() {
        let store = temp_projects();
        let Response { status, body } = workspaces_state(&store, br#"{"id":"demo"}"#);
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["id"], "demo");
        assert_eq!(v["workspaces"].as_array().unwrap().len(), 0);
        assert_eq!(v["hooks"].as_array().unwrap().len(), 0);
        assert_eq!(v["next_hook"], "init");
        assert_eq!(v["has_init_hook"], false);

        assert_eq!(workspaces_state(&store, br#"{"id":"ghost"}"#).status, 404);
        assert_eq!(workspaces_state(&store, br#"{"id":""}"#).status, 400);
    }

    #[test]
    fn create_project_hook_materializes_a_template_once() {
        let store = temp_projects();
        let Response { status, body } =
            create_project_hook(&store, br#"{"id":"demo","name":"init","template":"init"}"#);
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["has_init_hook"], true);
        assert_eq!(v["hooks"][0]["name"], "init");
        assert_eq!(v["hooks"][0]["status"], "never");
        let file = store.project_dir("demo").unwrap().join(".adi/hooks/init");
        assert!(file.is_file());

        assert_eq!(
            create_project_hook(&store, br#"{"id":"demo","name":"init"}"#).status,
            409
        );
        assert_eq!(
            create_project_hook(&store, br#"{"id":"demo","name":"x","template":"nope"}"#).status,
            400
        );
    }

    #[test]
    fn create_workspace_without_an_init_hook_is_a_409() {
        let store = temp_projects();
        let Response { status, body } = create_workspace(&store, br#"{"id":"demo","name":"main"}"#);
        assert_eq!(status, 409, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(
            v["error"].as_str().unwrap().contains("init"),
            "message should point at the missing init hook: {body}"
        );
    }

    #[test]
    fn create_workspace_runs_the_init_hook_to_ready() {
        let store = temp_projects();
        let dir = store.project_dir("demo").unwrap();
        std::fs::create_dir_all(dir.join(".adi/hooks")).unwrap();
        std::fs::write(
            dir.join(".adi/hooks/init"),
            "mkdir \"$ADI_WORKSPACE_DIR\"\n",
        )
        .unwrap();

        let Response { status, body } = create_workspace(&store, br#"{"id":"demo","name":"main"}"#);
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(v["message"].as_str().unwrap().contains("init"));
        let ws = &v["state"]["workspaces"][0];
        assert_eq!(ws["name"], "main");
        assert_eq!(ws["kind"], "init");
        assert_eq!(ws["primary"], true);
        assert!(ws["pid"].as_u64().is_some());

        assert!(wait_until(|| dir.join("workspaces/main").is_dir()));
        assert!(wait_until(|| {
            let Response { body, .. } = workspaces_state(&store, br#"{"id":"demo"}"#);
            let v: Value = serde_json::from_str(&body).unwrap();
            v["workspaces"][0]["status"] == "ready" && v["next_hook"] == "workspace"
        }));
        assert_eq!(
            create_workspace(&store, br#"{"id":"demo","name":"main"}"#).status,
            409
        );
    }

    #[test]
    fn local_workspace_links_and_remove_leaves_files() {
        let store = temp_projects();
        let dir = store.project_dir("demo").unwrap();
        let linked = dir.join("elsewhere");
        std::fs::create_dir_all(&linked).unwrap();

        let body = format!(
            r#"{{"id":"demo","name":"home","path":{:?},"local":true}}"#,
            linked.to_str().unwrap()
        );
        let Response { status, body: resp } = create_workspace(&store, body.as_bytes());
        assert_eq!(status, 200, "{resp}");
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["state"]["workspaces"][0]["status"], "local");
        assert_eq!(v["state"]["next_hook"], "init");

        let Response { status, body: resp } =
            remove_workspace(&store, br#"{"id":"demo","name":"home"}"#);
        assert_eq!(status, 200, "{resp}");
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["workspaces"].as_array().unwrap().len(), 0);
        assert!(linked.is_dir(), "remove must never delete files");

        assert_eq!(
            remove_workspace(&store, br#"{"id":"demo","name":"home"}"#).status,
            404
        );
    }

    #[test]
    fn workspace_terminal_endpoints_gate_on_the_registry() {
        let store = temp_projects();
        assert_eq!(
            peek_workspace_terminal(&store, br#"{"id":"demo","name":"main"}"#).status,
            404
        );
        assert_eq!(
            open_workspace_terminal(&store, br#"{"id":"demo","name":"main"}"#).status,
            404
        );
        assert_eq!(
            kill_workspace_terminal(&store, br#"{"id":"ghost","name":"main"}"#).status,
            404
        );

        // A registered workspace whose directory is gone can't host a terminal (400 from
        // the NotADir guard), but peek still answers a not-running snapshot.
        let dir = store.project_dir("demo").unwrap();
        let linked = dir.join("linked");
        std::fs::create_dir_all(&linked).unwrap();
        let body = format!(
            r#"{{"id":"demo","name":"gone","path":{:?},"local":true}}"#,
            linked.to_str().unwrap()
        );
        assert_eq!(create_workspace(&store, body.as_bytes()).status, 200);
        std::fs::remove_dir_all(&linked).unwrap();
        assert_eq!(
            open_workspace_terminal(&store, br#"{"id":"demo","name":"gone"}"#).status,
            400
        );
        let Response { status, body } =
            peek_workspace_terminal(&store, br#"{"id":"demo","name":"gone"}"#);
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["running"], false);
        // A pty session has no external attach command — the terminal is viewed in the panel.
        assert_eq!(v["attach"], "", "{body}");
    }

    #[test]
    fn manual_hook_run_and_log_round_trip() {
        let store = temp_projects();
        let dir = store.project_dir("demo").unwrap();
        std::fs::create_dir_all(dir.join(".adi/hooks")).unwrap();
        std::fs::write(
            dir.join(".adi/hooks/greet"),
            "printf 'hi from %s\\n' \"$ADI_PROJECT_NAME\"\n",
        )
        .unwrap();

        let Response { status, body } =
            run_project_hook(&store, br#"{"id":"demo","name":"greet"}"#);
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(v["message"].as_str().unwrap().contains("pid"));

        assert!(wait_until(|| {
            let Response { body, .. } =
                project_hook_log(&store, br#"{"id":"demo","name":"greet"}"#);
            let v: Value = serde_json::from_str(&body).unwrap();
            v["status"] == "ok"
        }));
        let Response { status, body } =
            project_hook_log(&store, br#"{"id":"demo","name":"greet"}"#);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ran"], true);
        assert_eq!(v["exit_code"], 0);
        assert!(v["output"].as_str().unwrap().contains("hi from Demo"));

        assert_eq!(
            run_project_hook(&store, br#"{"id":"demo","name":"ghost"}"#).status,
            404
        );
        // Lifecycle hooks are refused with a pointer at the workspace-create path — a bare
        // run would see an empty $ADI_WORKSPACE_DIR and fail confusingly inside git.
        let Response { status, body } = run_project_hook(&store, br#"{"id":"demo","name":"init"}"#);
        assert_eq!(status, 409, "{body}");
        assert!(body.contains("Add workspace"), "{body}");
        assert_eq!(
            project_hook_log(&store, br#"{"id":"demo","name":"ghost"}"#).status,
            404
        );
    }
}
