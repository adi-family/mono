//! One module per page in the control panel. Each exposes a `*_view(state, …) -> AnyView` entry
//! point the [`App`](crate::App) shell routes to; page-local helpers stay private to their module.

mod agents;
mod dashboards;
mod db;
mod fleet;
mod hive;
mod mesh;
mod meta;
mod onboarding;
mod ports;
mod project_detail;
mod projects;
mod secrets;
mod store_file;
mod tasks;
mod tools;
mod triggers;
mod workspaces;

/// Every table's declared column set, gathered under one name so [`Tables`](crate::state::Tables)
/// can build a state per table in one place. Each set stays defined beside the row builder and
/// comparator that read it — this module only re-exports them, renaming where two pages would
/// otherwise both offer a bare `COLS`.
pub(crate) mod columns {
    pub(crate) use super::agents::COLS as AGENT_COLS;
    pub(crate) use super::agents::{CHAT_COLS, CHAT_RUN_COLS, NEWEST_FIRST, RUN_COLS};
    pub(crate) use super::dashboards::COLS as DASHBOARD_COLS;
    pub(crate) use super::db::{SCOPE_COLS as DB_SCOPE_COLS, TABLE_COLS as DB_TABLE_COLS};
    pub(crate) use super::fleet::COLS as FLEET_COLS;
    pub(crate) use super::hive::COLS as HIVE_COLS;
    pub(crate) use super::mesh::{
        ALLOW_COLS as MESH_ALLOW_COLS, FORWARD_COLS as MESH_FORWARD_COLS,
        PEER_COLS as MESH_PEER_COLS,
    };
    pub(crate) use super::ports::{LEASE_COLS, USED_COLS as USED_PORT_COLS};
    pub(crate) use super::project_detail::{
        FILE_COLS, PROJECT_AGENT_COLS, PROJECT_TASK_COLS, SERVICE_COLS, SUBPROJECT_COLS,
    };
    pub(crate) use super::projects::{
        ARCHIVED_COLS as PROJECT_ARCHIVED_COLS, COLS as PROJECT_COLS,
    };
    pub(crate) use super::secrets::{COLS as SECRET_COLS, PROJECT_COLS as PROJECT_SECRET_COLS};
    pub(crate) use super::tasks::COLS as TASK_COLS;
    pub(crate) use super::tools::{COLS as TOOL_COLS, PROJECT_COLS as PROJECT_TOOL_COLS};
    pub(crate) use super::triggers::{COLS as TRIGGER_COLS, PROJECT_COLS as PROJECT_TRIGGER_COLS};
    pub(crate) use super::workspaces::{HOOK_COLS, WORKSPACE_COLS};
}

pub(crate) use agents::{agents_view, chat_home_view, live_view, poll_watch, reset_chat_home};
pub(crate) use dashboards::dashboards_view;
pub(crate) use db::database_view;
pub(crate) use fleet::fleet_view;
pub(crate) use hive::hive_view;
pub(crate) use mesh::mesh_view;
pub(crate) use meta::{meta_bin_tools, meta_view};
pub(crate) use onboarding::{
    OnboardingForm, onboarding_view, seed_onboarding, start_reconfigure as start_onb_reconfigure,
};
pub(crate) use ports::ports_manager_view;
pub(crate) use project_detail::{load_dir, project_detail_view};
pub(crate) use projects::{project_tree_rows, projects_view};
pub(crate) use secrets::secrets_view;
pub(crate) use store_file::{load_store_file, store_file_view};
pub(crate) use tasks::tasks_view;
pub(crate) use tools::tools_view;
pub(crate) use triggers::{poll_trigger_log, triggers_view};
pub(crate) use workspaces::{poll_hook_log, poll_term};

#[cfg(test)]
mod tests {
    use super::columns as c;

    /// Every column set a table is built from, named as its page names it. Adding a table means
    /// adding it here, so the rules below cover it too.
    const ALL: &[(&str, &[&str])] = &[
        ("agents", c::AGENT_COLS),
        ("chats", c::CHAT_COLS),
        ("chat-runs", c::CHAT_RUN_COLS),
        ("runs", c::RUN_COLS),
        ("dashboards", c::DASHBOARD_COLS),
        ("db-scopes", c::DB_SCOPE_COLS),
        ("db-tables", c::DB_TABLE_COLS),
        ("fleet", c::FLEET_COLS),
        ("hive", c::HIVE_COLS),
        ("leases", c::LEASE_COLS),
        ("used-ports", c::USED_PORT_COLS),
        ("mesh-allow", c::MESH_ALLOW_COLS),
        ("mesh-peers", c::MESH_PEER_COLS),
        ("mesh-forwards", c::MESH_FORWARD_COLS),
        ("projects", c::PROJECT_COLS),
        ("projects-archived", c::PROJECT_ARCHIVED_COLS),
        ("secrets", c::SECRET_COLS),
        ("tasks", c::TASK_COLS),
        ("tools", c::TOOL_COLS),
        ("triggers", c::TRIGGER_COLS),
        ("workspaces", c::WORKSPACE_COLS),
        ("hooks", c::HOOK_COLS),
        ("project-agents", c::PROJECT_AGENT_COLS),
        ("project-secrets", c::PROJECT_SECRET_COLS),
        ("project-tasks", c::PROJECT_TASK_COLS),
        ("project-tools", c::PROJECT_TOOL_COLS),
        ("project-triggers", c::PROJECT_TRIGGER_COLS),
        ("files", c::FILE_COLS),
        ("services", c::SERVICE_COLS),
        ("subprojects", c::SUBPROJECT_COLS),
    ];

    /// A cell builder and a comparator both find their column by its header *text*, and a saved
    /// layout is a list of those texts. Two columns reading the same would be indistinguishable:
    /// one would render the other's cell, and hiding either would hide both.
    #[test]
    fn no_table_declares_the_same_column_twice() {
        for (name, cols) in ALL {
            for (i, col) in cols.iter().enumerate() {
                assert!(
                    !cols[..i].contains(col),
                    "the {name} table declares {col:?} twice"
                );
            }
        }
    }

    /// The blank header is the action column, and there is at most one, last: `body_row` emits a
    /// cell per named column and then appends the row's controls, so a blank anywhere else would
    /// put the buttons in the wrong place.
    #[test]
    fn the_action_column_is_last_when_a_table_has_one() {
        for (name, cols) in ALL {
            let blanks = cols.iter().filter(|h| h.is_empty()).count();
            assert!(
                blanks <= 1,
                "the {name} table declares {blanks} blank headers"
            );
            if blanks == 1 {
                assert_eq!(
                    cols.last().copied(),
                    Some(""),
                    "the {name} table's action column is not last"
                );
            }
        }
    }

    /// A table needs a column to sort by and a menu to arrange, so an empty set is a mistake
    /// rather than a table that happens to show nothing.
    #[test]
    fn every_table_declares_at_least_one_named_column() {
        for (name, cols) in ALL {
            assert!(
                cols.iter().any(|h| !h.is_empty()),
                "the {name} table declares no named columns"
            );
        }
    }

    /// A declared default must name a column its table actually has — otherwise the table opens
    /// sorted by nothing, no header lights up, and Reset returns it there again.
    #[test]
    fn the_declared_default_sort_names_a_real_column() {
        for cols in [c::CHAT_COLS, c::CHAT_RUN_COLS, c::RUN_COLS] {
            assert!(
                cols.contains(&c::NEWEST_FIRST.col),
                "{:?} is not a column of {cols:?}",
                c::NEWEST_FIRST.col
            );
        }
    }
}
