//! Shared output-formatting and argument-parsing helpers used across the CLI's
//! command groups.

use std::collections::BTreeMap;

use adi_core::{EffectiveStatus, Report, ServiceReport, TaskStatus, contains_json_null};

/// Serialize any value to pretty JSON, degrading to `{}` on the (unreachable) encode failure.
pub(crate) fn print_json<T: serde::Serialize>(value: &T) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string())
    );
}

pub(crate) fn print_report(report: &Report, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(report).unwrap_or_else(|_| "{}".to_string())
        );
        return;
    }
    for svc in &report.services {
        print_human(svc);
    }
}

pub(crate) fn print_service(svc: &ServiceReport, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(svc).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        print_human(svc);
    }
}

pub(crate) fn parse_task_status_opt(value: Option<String>) -> Result<Option<TaskStatus>, String> {
    value.map(|v| parse_task_status(&v)).transpose()
}

fn parse_task_status(value: &str) -> Result<TaskStatus, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "open" | "pending" | "in_progress" => Ok(TaskStatus::Open),
        "done" => Ok(TaskStatus::Done),
        "archived" | "cancelled" => Ok(TaskStatus::Archived),
        _ => Err(format!(
            "unknown task status {value:?}; expected open, done, or archived"
        )),
    }
}

pub(crate) fn parse_effective_status_opt(
    value: Option<String>,
) -> Result<Option<EffectiveStatus>, String> {
    value.map(|v| parse_effective_status(&v)).transpose()
}

fn parse_effective_status(value: &str) -> Result<EffectiveStatus, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "ready" => Ok(EffectiveStatus::Ready),
        "blocked" => Ok(EffectiveStatus::Blocked),
        "done" => Ok(EffectiveStatus::Done),
        "archived" => Ok(EffectiveStatus::Archived),
        _ => Err(format!(
            "unknown effective status {value:?}; expected ready, blocked, done, or archived"
        )),
    }
}

pub(crate) fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub(crate) fn clean_required(name: &str, value: String) -> Result<String, String> {
    clean(Some(value)).ok_or_else(|| format!("{name} is required"))
}

pub(crate) fn clean_tags(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .flat_map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

pub(crate) fn parse_arguments(
    values: Vec<String>,
) -> Result<BTreeMap<String, serde_json::Value>, String> {
    let mut out = BTreeMap::new();
    for raw in values {
        let (key, value) = raw
            .split_once('=')
            .ok_or_else(|| format!("argument {raw:?} must be key=value"))?;
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || value.is_empty() {
            continue;
        }
        let structured = value.starts_with('{') || value.starts_with('[');
        let value = match serde_json::from_str(value) {
            Ok(value) => value,
            Err(error) if structured => {
                return Err(format!("argument {key:?} is invalid JSON: {error}"));
            }
            Err(_) => value.into(),
        };
        if contains_json_null(&value) {
            return Err(format!(
                "argument {key:?} cannot contain null (the manifest store is TOML)"
            ));
        }
        out.insert(key.to_string(), value);
    }
    Ok(out)
}

fn safe_extra_key(key: &str) -> bool {
    key.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

pub(crate) fn parse_extra(values: Vec<String>) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for raw in values {
        let (key, value) = raw
            .split_once('=')
            .ok_or_else(|| format!("extra value {raw:?} must be key=value"))?;
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || value.is_empty() {
            continue;
        }
        if !safe_extra_key(key) {
            return Err(format!(
                "invalid extra key {key:?}: use letters, digits, '_' or '-'"
            ));
        }
        out.insert(key.to_string(), value.to_string());
    }
    Ok(out)
}

fn print_human(svc: &ServiceReport) {
    let state = match (svc.enabled, svc.running) {
        (_, true) => "running",
        (true, false) => "enabled",
        (false, false) => "stopped",
    };
    println!("{} — {} [{state}]", svc.name, svc.detail);
    for action in &svc.actions {
        println!(
            "  {}: {}  (adi-mono {})",
            action.id,
            action.title,
            action.args.join(" ")
        );
    }
}
