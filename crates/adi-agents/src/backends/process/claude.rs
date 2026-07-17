//! `process:claude` command construction (`claude --print`).

use crate::AgentManifest;

pub(super) const BACKEND_ID: &str = "process:claude";

pub(super) fn argv(manifest: &AgentManifest, message: &str) -> Vec<String> {
    let mut argv = vec!["claude".to_string(), "--print".to_string()];
    push_option(&mut argv, "--model", manifest.model.as_deref());
    push_option(
        &mut argv,
        "--permission-mode",
        manifest.permission_mode.as_deref(),
    );
    push_extra(&mut argv, manifest, "effort", "--effort");
    push_extra(&mut argv, manifest, "output_format", "--output-format");
    push_extra(&mut argv, manifest, "allowed_tools", "--allowed-tools");
    push_extra(
        &mut argv,
        manifest,
        "disallowed_tools",
        "--disallowed-tools",
    );
    push_extra(&mut argv, manifest, "max_budget_usd", "--max-budget-usd");
    push_extra(&mut argv, manifest, "fallback_model", "--fallback-model");
    push_extra(&mut argv, manifest, "add_dir", "--add-dir");

    let prompts = [
        Some(manifest.system_prompt.as_str()),
        manifest
            .extra
            .get("append_system_prompt")
            .map(String::as_str),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>();
    if !prompts.is_empty() {
        argv.extend(["--append-system-prompt".into(), prompts.join("\n\n")]);
    }
    argv.push(run_message(message));
    argv
}

fn push_extra(argv: &mut Vec<String>, manifest: &AgentManifest, key: &str, flag: &str) {
    push_option(argv, flag, manifest.extra.get(key).map(String::as_str));
}

fn push_option(argv: &mut Vec<String>, flag: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        argv.extend([flag.to_string(), value.to_string()]);
    }
}

fn run_message(message: &str) -> String {
    let message = message.trim();
    if message.is_empty() {
        "run".into()
    } else {
        message.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_uses_print_mode_and_process_options() {
        let mut manifest = AgentManifest {
            backend: BACKEND_ID.into(),
            model: Some("sonnet".into()),
            permission_mode: Some("dontAsk".into()),
            system_prompt: "You are a release agent.".into(),
            ..AgentManifest::default()
        };
        manifest.extra.extend([
            ("output_format".into(), "json".into()),
            ("max_budget_usd".into(), "2.5".into()),
            ("effort".into(), "high".into()),
        ]);
        assert_eq!(
            argv(&manifest, "prepare the release"),
            [
                "claude",
                "--print",
                "--model",
                "sonnet",
                "--permission-mode",
                "dontAsk",
                "--effort",
                "high",
                "--output-format",
                "json",
                "--max-budget-usd",
                "2.5",
                "--append-system-prompt",
                "You are a release agent.",
                "prepare the release",
            ]
        );
    }
}
