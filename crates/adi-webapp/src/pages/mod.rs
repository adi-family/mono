//! One module per page in the control panel. Each exposes a `*_view(state, …) -> AnyView` entry
//! point the [`App`](crate::App) shell routes to; page-local helpers stay private to their module.

mod agents;
mod dashboards;
mod db;
mod hive;
mod mesh;
mod meta;
mod ports;
mod project_detail;
mod projects;
mod secrets;
mod store_file;
mod tasks;
mod tools;
mod triggers;
mod workspaces;

pub(crate) use agents::{agents_view, chat_home_view, live_view, poll_watch};
pub(crate) use dashboards::dashboards_view;
pub(crate) use db::database_view;
pub(crate) use hive::{COLS as HIVE_COLS, hive_view};
pub(crate) use mesh::mesh_view;
pub(crate) use meta::meta_view;
pub(crate) use ports::ports_manager_view;
pub(crate) use project_detail::{SERVICE_COLS, load_dir, project_detail_view};
pub(crate) use projects::{project_tree_rows, projects_view};
pub(crate) use secrets::secrets_view;
pub(crate) use store_file::{load_store_file, store_file_view};
pub(crate) use tasks::tasks_view;
pub(crate) use tools::tools_view;
pub(crate) use triggers::{poll_trigger_log, triggers_view};
pub(crate) use workspaces::{poll_hook_log, poll_term};
