//! `process:codex` command construction (`codex exec`).

use crate::AgentManifest;

pub(super) const BACKEND_ID: &str = "process:codex";

pub(super) fn argv(manifest: &AgentManifest, message: &str) -> Vec<String> {
    let mut argv = vec!["codex".to_string()];
    push_option(&mut argv, "--model", manifest.model.as_deref());
    push_extra(&mut argv, manifest, "sandbox", "--sandbox");
    push_extra(&mut argv, manifest, "approval", "--ask-for-approval");
    push_extra(&mut argv, manifest, "working_dir", "--cd");
    push_extra(&mut argv, manifest, "add_dir", "--add-dir");
    if extra_bool(manifest, "web_search") {
        argv.push("--search".into());
    }
    if let Some(effort) = extra_value(manifest, "reasoning_effort") {
        argv.extend([
            "--config".into(),
            format!("model_reasoning_effort={effort}"),
        ]);
    }

    argv.push("exec".into());
    argv.extend(["--color".into(), "never".into()]);
    if extra_bool(manifest, "skip_git_repo_check") {
        argv.push("--skip-git-repo-check".into());
    }
    if extra_bool(manifest, "json_events") {
        argv.push("--json".into());
    }
    argv.push(run_prompt(manifest, message));
    argv
}

fn push_extra(argv: &mut Vec<String>, manifest: &AgentManifest, key: &str, flag: &str) {
    push_option(argv, flag, extra_value(manifest, key));
}

fn push_option(argv: &mut Vec<String>, flag: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        argv.extend([flag.to_string(), value.to_string()]);
    }
}

fn extra_value<'a>(manifest: &'a AgentManifest, key: &str) -> Option<&'a str> {
    manifest.extra.get(key).map(String::as_str)
}

fn extra_bool(manifest: &AgentManifest, key: &str) -> bool {
    extra_value(manifest, key).is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn run_prompt(manifest: &AgentManifest, message: &str) -> String {
    let system = manifest.system_prompt.trim();
    let message = message.trim();
    match (system.is_empty(), message.is_empty()) {
        (true, true) => "run".into(),
        (true, false) => message.into(),
        (false, true) => system.into(),
        (false, false) => format!("{system}\n\n{message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_puts_global_options_before_exec_and_never_opens_a_tui() {
        let mut manifest = AgentManifest {
            backend: BACKEND_ID.into(),
            model: Some("gpt-5-codex".into()),
            system_prompt: "Work carefully.".into(),
            ..AgentManifest::default()
        };
        manifest.extra.extend([
            ("sandbox".into(), "workspace-write".into()),
            ("approval".into(), "never".into()),
            ("reasoning_effort".into(), "high".into()),
            ("skip_git_repo_check".into(), "true".into()),
            ("json_events".into(), "true".into()),
        ]);
        assert_eq!(
            argv(&manifest, "fix the tests"),
            [
                "codex",
                "--model",
                "gpt-5-codex",
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "never",
                "--config",
                "model_reasoning_effort=high",
                "exec",
                "--color",
                "never",
                "--skip-git-repo-check",
                "--json",
                "Work carefully.\n\nfix the tests",
            ]
        );
    }
}
