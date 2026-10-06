//! Project renames span multiple stores and share [`adi_core::rename_project`] with the CLI.

use adi_projects::Projects;
use adi_webapp_api::handlers::{self, Response};
use adi_webapp_api::types::{ProjectRenamed, RenameProject};

pub(crate) fn rename_project(projects: &Projects, body: &[u8]) -> Response {
    let req: RenameProject = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => {
            return handlers::error(
                400,
                &format!(
                    "invalid request body ({e}) — expected JSON body \
                     {{ \"id\": \"…\", \"new_id\": \"…\" }}"
                ),
            );
        }
    };
    let (id, new_id) = (req.id.trim(), req.new_id.trim());
    if id.is_empty() || new_id.is_empty() {
        return handlers::error(400, "a rename needs both the project and its new id");
    }

    // All affected stores must use the registry's configured root.
    let report = match adi_core::rename_project(projects.config(), id, new_id) {
        Ok(report) => report,
        Err(e) => return Response::from(&e),
    };
    let projects = match handlers::projects_state(projects) {
        Ok(state) => state,
        Err(e) => return Response::from(&e),
    };
    handlers::ok_json(&ProjectRenamed {
        id: report.project.id,
        from: report.from,
        subprojects: report.subprojects,
        tools: report.tools,
        agents: report.agents,
        triggers: report.triggers,
        secrets: report.secrets,
        knowledge: report.knowledge,
        database: report.database,
        warnings: report.warnings,
        projects,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Projects {
        let root = std::env::temp_dir().join(format!(
            "adi-app-rename-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Projects::with_config(adi_config::Config::with_root(root))
    }

    #[test]
    fn a_rename_answers_with_the_receipt_and_the_fresh_list() {
        let projects = scratch("ok");
        projects
            .create_with_id("old", Some("Old".into()), None, None)
            .expect("project");
        adi_secrets::Secrets::with_config(projects.config().clone())
            .set(Some("old"), "API_KEY", "s3cr3t", None)
            .expect("secret");

        let response = rename_project(&projects, br#"{"id":" old ","new_id":"new"}"#);
        assert_eq!(response.status, 200, "{}", response.body);
        let got: ProjectRenamed = serde_json::from_str(&response.body).expect("receipt");
        assert_eq!(got.id, "new");
        assert_eq!(got.from, "old");
        assert_eq!(got.secrets, 1);
        assert!(got.warnings.is_empty(), "{:?}", got.warnings);
        let ids: Vec<&str> = got
            .projects
            .projects
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        assert_eq!(ids, ["new"]);
    }

    #[test]
    fn a_body_that_names_nothing_is_a_400_and_a_taken_id_is_a_409() {
        let projects = scratch("refused");
        projects
            .create_with_id("old", None, None, None)
            .expect("old");
        projects
            .create_with_id("taken", None, None, None)
            .expect("taken");

        for body in [
            &b"not json"[..],
            br#"{"id":"old"}"#,
            br#"{"id":"","new_id":"new"}"#,
            br#"{"id":"old","new_id":"   "}"#,
        ] {
            assert_eq!(rename_project(&projects, body).status, 400, "{body:?}");
        }
        assert_eq!(
            rename_project(&projects, br#"{"id":"old","new_id":"taken"}"#).status,
            409
        );
        assert_eq!(
            rename_project(&projects, br#"{"id":"old","new_id":"../escape"}"#).status,
            400
        );
        assert_eq!(
            rename_project(&projects, br#"{"id":"ghost","new_id":"new"}"#).status,
            404
        );
        assert!(projects.get("old").expect("get").is_some(), "nothing moved");
    }
}
