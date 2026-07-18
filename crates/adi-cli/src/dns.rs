//! The `dns` command group: subcommand surface for the DNS resolver. Dispatch stays
//! inline in `main` since every arm is a direct call on the `adi.dns()` facade.

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub(crate) enum DnsCommand {
    /// Enable the DNS resolver (installs the route + front-door proxy on first enable).
    Enable,
    /// Disable the DNS resolver (leaves the route + front-door proxy in place).
    Disable,
    /// Show live DNS status.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Install the `.adi` route + front-door proxy (one admin prompt).
    InstallRoute,
    /// Remove the `.adi` route + front-door proxy (one admin prompt).
    RemoveRoute,
}
