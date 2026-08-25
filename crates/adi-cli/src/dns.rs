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
    /// Grant only the DNS route, so this zone's names resolve here (one admin prompt).
    ///
    /// The halves of `install-route`, for an onboarding that asks for one permission at a
    /// time. Both are idempotent, so granting them in sequence lands exactly where
    /// `install-route` would.
    GrantDns,
    /// Grant only the front door, so those names have something answering them.
    GrantNetwork,
    /// Remove the `.adi` route + front-door proxy (one admin prompt).
    RemoveRoute,
}
