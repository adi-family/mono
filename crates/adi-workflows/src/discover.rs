//! Finding workflow files on disk. Spec section E: `projects/<id>/workflows/*.ts`, or global
//! `workflows/*.ts`. Reading what's *inside* one (its `on` export) is [`crate::describe`]'s job —
//! this module only walks directories.

use std::path::{Path, PathBuf};

use adi_config::Config;

/// The directory name every workflow file lives under, global or per-project.
const WORKFLOWS_DIR: &str = "workflows";
const TS_EXT: &str = "ts";

/// One workflow file found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowFile {
    /// This workflow's id — half of every run key it starts (`${id}@slot`, `${id}@evt:<seq>`).
    /// A global file's id is its bare file stem (`weekly-report.ts` → `weekly-report`); a
    /// per-project file's is `${project}/${stem}`, so two projects (or a project and the global
    /// scope) can each have their own `weekly-report` without colliding.
    pub id: String,
    /// Where the file lives on disk right now — never read directly once a run has started from
    /// it; see [`crate::store::WorkflowStore::create_run`]'s `file_hash` for why.
    pub path: PathBuf,
    /// The project this file is scoped to, or `None` for a global workflow — set on the run's
    /// root task so it lands in the right project's task list.
    pub project: Option<String>,
}

/// Every `.ts` workflow file this store currently has registered: the global `workflows/` dir,
/// then each project's `workflows/` dir, sorted by id. A missing directory contributes nothing —
/// no workflows at all is not an error.
#[must_use]
pub fn workflow_files(config: &Config) -> Vec<WorkflowFile> {
    let mut found = Vec::new();
    found.extend(scan(config.module(WORKFLOWS_DIR).dir(), None));

    let projects_dir = config.projects_dir();
    if let Ok(entries) = std::fs::read_dir(&projects_dir) {
        let mut project_dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        project_dirs.sort();
        for dir in project_dirs {
            let Some(project) = dir.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            found.extend(scan(&dir.join(WORKFLOWS_DIR), Some(project)));
        }
    }

    found.sort_by(|a, b| a.id.cmp(&b.id));
    found
}

/// Every `.ts` file directly inside `dir` (no recursion — a workflow is one file, not a package),
/// tagged with `project`.
fn scan(dir: &Path, project: Option<&str>) -> Vec<WorkflowFile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<WorkflowFile> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(std::ffi::OsStr::to_str) == Some(TS_EXT))
        .filter_map(|path| {
            let stem = path.file_stem()?.to_str()?.to_string();
            let id = match project {
                Some(p) => format!("{p}/{stem}"),
                None => stem,
            };
            Some(WorkflowFile {
                id,
                path,
                project: project.map(String::from),
            })
        })
        .collect();
    files.sort_by(|a, b| a.id.cmp(&b.id));
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-workflows-discover-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn no_workflows_dir_at_all_is_an_empty_list() {
        let config = scratch("empty");
        assert!(workflow_files(&config).is_empty());
    }

    #[test]
    fn a_global_workflow_file_is_found_with_no_project() {
        let config = scratch("global");
        write(
            &config.module("workflows").dir().join("weekly-report.ts"),
            "export const on = [];",
        );
        let found = workflow_files(&config);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "weekly-report");
        assert_eq!(found[0].project, None);
    }

    #[test]
    fn a_project_workflow_file_is_id_qualified_by_its_project() {
        let config = scratch("project");
        write(
            &config.projects_dir().join("demo/workflows/weekly-report.ts"),
            "export const on = [];",
        );
        let found = workflow_files(&config);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "demo/weekly-report");
        assert_eq!(found[0].project.as_deref(), Some("demo"));
    }

    #[test]
    fn the_same_stem_in_two_scopes_never_collides() {
        let config = scratch("collision");
        write(
            &config.module("workflows").dir().join("weekly-report.ts"),
            "export const on = [];",
        );
        write(
            &config.projects_dir().join("demo/workflows/weekly-report.ts"),
            "export const on = [];",
        );
        write(
            &config.projects_dir().join("acme/workflows/weekly-report.ts"),
            "export const on = [];",
        );
        let ids: Vec<String> = workflow_files(&config).into_iter().map(|f| f.id).collect();
        assert_eq!(
            ids,
            vec!["acme/weekly-report", "demo/weekly-report", "weekly-report"]
        );
    }

    #[test]
    fn non_ts_files_and_a_project_with_no_workflows_dir_are_ignored() {
        let config = scratch("ignore");
        write(
            &config.module("workflows").dir().join("notes.md"),
            "not a workflow",
        );
        std::fs::create_dir_all(config.projects_dir().join("empty-project")).unwrap();
        assert!(workflow_files(&config).is_empty());
    }
}
