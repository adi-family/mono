//! Configuration for the external channel router; the local daemon is discovered through Hive.

const ROUTER_URL_ENV: &str = "ADI_CHANNEL_ROUTER_URL";
const DEFAULT_ROUTER_URL: &str = "https://hooks.withadi.dev";
const ROUTER_ADMIN_SECRET_ENV: &str = "ADI_CHANNEL_ROUTER_ADMIN_SECRET";

/// The externally deployed router, or an operator's explicit development override.
#[must_use]
pub fn router_url() -> String {
    std::env::var(ROUTER_URL_ENV).unwrap_or_else(|_| DEFAULT_ROUTER_URL.to_string())
}

/// Optional operator secret. Ordinary nodes use open registration; empty values mean absent.
#[must_use]
pub fn router_admin_secret() -> Option<String> {
    std::env::var(ROUTER_ADMIN_SECRET_ENV)
        .ok()
        .filter(|secret| !secret.is_empty())
}
