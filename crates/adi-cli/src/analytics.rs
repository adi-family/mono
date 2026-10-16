//! The `analytics` command group: usage events on/off and this install's id (ADI-MONO-157) — the
//! CLI twin of the panel's "Usage events" settings page.
//!
//! No `adi` facade, like `mesh`/`marketplace`: this is `adi-usage`'s own settings/state under the
//! store the resolved flavour already points at, not platform state `Adi` owns.

use clap::Subcommand;
use serde_json::json;

use crate::format::print_json;

#[derive(Debug, Clone, Copy, Subcommand)]
pub(crate) enum AnalyticsCommand {
    /// Turn usage events on.
    Enable,
    /// Turn usage events off. An existing install id is left on disk, dormant — it is reused,
    /// not re-minted, if this is switched back on. `forget` is what actually clears it.
    Disable,
    /// Whether usage events are actually on right now, and why — an env override, if either
    /// `ADI_ANALYTICS` or `DO_NOT_TRACK` is set, wins over the stored setting.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Print this install's id, or say there isn't one yet.
    Id,
    /// Clear this install's id. A fresh one is minted only the next time usage events run while
    /// on — never by this command.
    Forget,
}

pub(crate) fn run_analytics(command: AnalyticsCommand) -> Result<(), String> {
    let module = adi_usage::module(&adi_config::Config::open());
    match command {
        AnalyticsCommand::Enable => {
            adi_usage::set_enabled(&module, true).map_err(|e| e.to_string())?;
            println!("usage events: on");
        }
        AnalyticsCommand::Disable => {
            adi_usage::set_enabled(&module, false).map_err(|e| e.to_string())?;
            println!("usage events: off");
        }
        AnalyticsCommand::Status { json: as_json } => print_status(&module, as_json),
        AnalyticsCommand::Id => match adi_usage::State::load(&module).install_id {
            Some(id) => println!("{id}"),
            None => {
                println!("no install id yet — minted the first time usage events run while on");
            }
        },
        AnalyticsCommand::Forget => {
            adi_usage::forget(&module).map_err(|e| e.to_string())?;
            println!("install id forgotten");
        }
    }
    Ok(())
}

fn print_status(module: &adi_config::Module, as_json: bool) {
    let persisted = adi_usage::Settings::load(module).enabled;
    let enabled = adi_usage::effective(module);
    let env_override = adi_usage::env_override().map(|(name, _)| name.to_string());
    let install_id = adi_usage::State::load(module).install_id;
    if as_json {
        print_json(&json!({
            "enabled": enabled,
            "persisted": persisted,
            "env_override": env_override,
            "install_id": install_id,
        }));
        return;
    }
    println!("usage events: {}", if enabled { "on" } else { "off" });
    if let Some(name) = &env_override {
        println!("  overridden by {name} in the environment");
    }
    println!("  stored setting: {}", if persisted { "on" } else { "off" });
    match &install_id {
        Some(id) => println!("  install id: {id}"),
        None => println!("  install id: none yet"),
    }
}
