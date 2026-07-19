//! One module per page in the control panel. Each exposes a `*_view(state, …) -> AnyView` entry
//! point the [`App`](crate::App) shell routes to; page-local helpers stay private to their module.

mod agents;
mod dashboards;
mod hive;
mod mesh;
mod ports;
mod project_detail;
mod projects;
mod store_file;
mod tasks;
mod triggers;
mod workspaces;

pub(crate) use agents::{agents_view, poll_watch};
pub(crate) use dashboards::dashboards_view;
pub(crate) use hive::hive_view;
pub(crate) use mesh::mesh_view;
pub(crate) use ports::ports_manager_view;
pub(crate) use project_detail::{load_dir, project_detail_view};
pub(crate) use projects::{project_tree_rows, projects_view};
pub(crate) use store_file::{load_store_file, store_file_view};
pub(crate) use tasks::tasks_view;
pub(crate) use triggers::{poll_trigger_log, triggers_view};
pub(crate) use workspaces::{poll_hook_log, poll_term};
