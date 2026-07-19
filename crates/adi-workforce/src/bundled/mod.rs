//! The bundled capability set. The old repo dlopen'd these as separate
//! cdylib plugins; here they compile straight into the engine and register
//! under the same plugin-id strings, so agent sources written against the
//! old SDK (`sdk.plugin('adi.workforce.runner.claude')`, ...) keep working.

pub mod claude;
pub mod env;
pub mod inbox;
pub mod sandbox;
pub mod shell;
pub mod tasks;

use std::sync::Arc;

use crate::core::{Core, PluginEntry};

/// A [`Core`] with every bundled capability registered. This is the engine
/// bootstrap: what the old `workforce bootstrap()` did by scanning a plugin
/// directory, done statically.
#[must_use]
pub fn core() -> Arc<Core> {
    let mut core = Core::new();

    core.register_plugin(
        "adi.workforce.runner.claude",
        PluginEntry::new().runner("ClaudeCodeApi", claude::ClaudeCodeApi::create),
    );

    core.register_plugin(
        "adi.workforce.capability.shell",
        PluginEntry::new().tool("Shell", shell::ShellTool::create),
    );

    core.register_plugin(
        "adi.workforce.capability.tasks",
        PluginEntry::new()
            .tool("TaskCreate", tasks::TaskCreateTool::create)
            .tool("TaskGet", tasks::TaskGetTool::create)
            .tool("TaskList", tasks::TaskListTool::create)
            .tool("TaskUpdate", tasks::TaskUpdateTool::create)
            .tool("TaskResolve", tasks::TaskResolveTool::create),
    );

    core.register_plugin(
        "adi.workforce.capability.orchestration",
        PluginEntry::new()
            .tool("MessageEmployee", inbox::MessageEmployeeTool::create)
            .trigger("EmployeeMessage", inbox::EmployeeMessageTrigger::create),
    );

    core.register_plugin(
        "adi.workforce.variable.env",
        PluginEntry::new().tool("Env", env::Env::create),
    );

    core.register_plugin(
        "adi.workforce.filesystem.sandbox",
        sandbox::register_plugin(),
    );

    Arc::new(core)
}
