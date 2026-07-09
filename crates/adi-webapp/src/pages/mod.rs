//! One module per page in the control panel. Each exposes a `*_view(state, …) -> AnyView` entry
//! point the [`App`](crate::App) shell routes to; page-local helpers stay private to their module.

mod agents;
mod hive;
mod mesh;
mod overview;
mod ports;
mod project_detail;
mod projects;
mod tasks;

pub(crate) use agents::agents_view;
pub(crate) use hive::hive_view;
pub(crate) use mesh::mesh_view;
pub(crate) use overview::overview_view;
pub(crate) use ports::ports_manager_view;
pub(crate) use project_detail::{load_dir, project_detail_view};
pub(crate) use projects::projects_view;
pub(crate) use tasks::tasks_view;
