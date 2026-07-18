//! The `update` command group: check/run/status plus enable/disable for the background
//! auto-update agent, dispatched over the adi-core update facade.

use adi_core::{Adi, RunOutcome, Service, Updater};
use clap::Subcommand;

use crate::format::print_json;

// Carries only flags, so it's `Copy` (which also satisfies pedantic's pass-by-value lint).
#[derive(Debug, Clone, Copy, Subcommand)]
pub(crate) enum UpdateCommand {
    /// Fetch the release manifest and compare against the installed version (no install).
    Check {
        #[arg(long)]
        json: bool,
    },
    /// Download, verify, and install the latest version if newer, then restart services.
    Run {
        /// Reinstall even when the published version isn't newer.
        #[arg(long)]
        force: bool,
        /// Swap the bundle but leave running services on the old binaries.
        #[arg(long)]
        no_restart: bool,
        /// Exit 0 on errors too (offline is normal) — what the background agent runs.
        #[arg(long)]
        quiet: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show the updater's persisted last check/install record.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Enable the background auto-update agent (periodic check + install).
    Enable,
    /// Disable the background auto-update agent.
    Disable,
}

/// Dispatch an `update` subcommand over the adi-core update facade.
pub(crate) fn run_update(adi: Adi, command: UpdateCommand) {
    match command {
        UpdateCommand::Check { json } => match adi.update().check() {
            Ok(check) => {
                if json {
                    print_json(&check);
                } else {
                    println!("Installed: {}", check.installed);
                    println!("Latest:    {}", check.latest);
                    if check.update_available {
                        println!("Update available — run `adi-mono update run` to install.");
                    } else {
                        println!("Up to date.");
                    }
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        },
        UpdateCommand::Run {
            force,
            no_restart,
            quiet,
            json,
        } => match adi.update().run(force, !no_restart) {
            Ok(outcome) => {
                if json {
                    print_json(&outcome);
                } else {
                    match outcome {
                        RunOutcome::UpToDate { installed, .. } => {
                            println!("Up to date ({installed}).");
                        }
                        RunOutcome::Installed {
                            from,
                            to,
                            restarted,
                        } => {
                            println!(
                                "Updated {from} → {to}{}.",
                                if restarted {
                                    "; services restarted"
                                } else {
                                    " (services not restarted)"
                                }
                            );
                        }
                    }
                }
            }
            Err(e) => {
                // In quiet (background-agent) mode an unreachable manifest is routine —
                // log it and exit 0 so launchd doesn't treat the tick as a failure.
                eprintln!("error: {e}");
                if !quiet {
                    std::process::exit(1);
                }
            }
        },
        UpdateCommand::Status { json } => {
            let state = adi.update().state();
            if json {
                print_json(&state);
            } else if state.last_check_unix.is_none() {
                println!("Never checked for updates.");
            } else {
                println!(
                    "{} — installed {}, latest {} (last check {} unix)",
                    state.last_outcome.as_deref().unwrap_or("unknown"),
                    state.installed_version.as_deref().unwrap_or("?"),
                    state.latest_version.as_deref().unwrap_or("?"),
                    state.last_check_unix.unwrap_or(0),
                );
                if let Some(err) = &state.last_error {
                    println!("  last error: {err}");
                }
            }
        }
        UpdateCommand::Enable => Updater::new().enable(),
        UpdateCommand::Disable => Updater::new().disable(),
    }
}
