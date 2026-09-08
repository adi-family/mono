//! Which upstream each path prefix belongs to, how much of a body the journal keeps, and whether
//! placeholder substitution is available at all.
//!
//! Written out with its defaults on first start (`~/.adi/mono/llm-gateway/config.toml`), so the
//! route table is a file the operator can read and extend rather than a list compiled in here.

use std::collections::BTreeMap;

use adi_config::{Config, Result};
use serde::{Deserialize, Serialize};

/// The store module the settings file lives in: `~/.adi/mono/llm-gateway/`.
pub const MODULE: &str = "llm-gateway";

/// The settings file within that module.
pub const FILE: &str = "config.toml";

/// The service name the port is leased under, when one has to be leased.
pub const SERVICE: &str = "llm-gateway";

/// The port key within that lease.
pub const PORT_KEY: &str = "http";

/// The providers a fresh install knows, as `prefix → upstream base`.
///
/// A base carries no trailing slash and no path of its own: the client's path is appended whole,
/// which is what keeps `/anthropic/v1/messages` and `/openai/v1/chat/completions` working without
/// the gateway knowing either API.
const DEFAULT_ROUTES: [(&str, &str); 4] = [
    ("anthropic", "https://api.anthropic.com"),
    ("openai", "https://api.openai.com"),
    ("gemini", "https://generativelanguage.googleapis.com"),
    ("zai", "https://api.z.ai"),
];

/// How much of a request or response body is kept per journal row.
///
/// A prompt with a few files pasted into it runs to tens of kilobytes and a long answer to a few
/// hundred; a megabyte holds essentially all of both, and caps what one runaway upload can do to
/// the store.
const DEFAULT_MAX_LOGGED_BODY: usize = 1 << 20;

/// The gateway's settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// `prefix → upstream base`, e.g. `anthropic → https://api.anthropic.com`.
    pub routes: BTreeMap<String, String>,
    /// Bodies longer than this are journalled truncated, with a marker saying so.
    pub max_logged_body: usize,
    /// Placeholder substitution (`crate::placeholders`) — off unless switched on here.
    pub placeholders: Placeholders,
}

/// Whether the gateway may rewrite a body on its way past, and what it does when a client expresses
/// no preference.
///
/// **Experimental, and off for a reason.** The substitution changes the prompt a provider sees,
/// which is the one thing this gateway otherwise promises never to do, and it was measured to be a
/// losing trade on this machine: 98.6% of input tokens here are cache reads billed at a tenth, and
/// `full` invalidates that cached prefix to save a percent of a prompt. Read `crate::placeholders`
/// before switching this on — it lists what else can go wrong.
///
/// Switching `enabled` on only makes the `x-adi-placeholders: full|tail` header work;
/// `default_mode` is what a request with no such header gets, and an empty string means nothing.
/// Turning it on for every request at once (`default_mode = "full"`) is the configuration most
/// likely to surprise somebody: prefer the header, one client at a time.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Placeholders {
    /// Master switch. While this is false the header is ignored and every body is forwarded as sent.
    pub enabled: bool,
    /// `full`, `tail`, or empty for no rewrite unless a request asks by header.
    pub default_mode: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            routes: DEFAULT_ROUTES
                .iter()
                .map(|(prefix, base)| ((*prefix).to_string(), (*base).to_string()))
                .collect(),
            max_logged_body: DEFAULT_MAX_LOGGED_BODY,
            placeholders: Placeholders::default(),
        }
    }
}

/// Where one request is bound: the provider that owns it, and the URL to send it to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// The prefix that matched — the name this shows up under in the journal.
    pub provider: String,
    /// The full upstream URL, query included.
    pub url: String,
}

impl Settings {
    /// Load the settings, writing the defaults out on first run.
    ///
    /// # Errors
    /// Fails if the file exists but is unreadable or malformed, or if it cannot be created.
    pub fn load() -> Result<Self> {
        Config::open()
            .module(MODULE)
            .file::<Self>(FILE)
            .load_or_create()
    }

    /// Resolve a request target (`/anthropic/v1/messages?beta=true`) against the route table.
    ///
    /// `None` means the first segment names no configured provider — the client is pointed at a
    /// base URL this gateway does not serve, which is worth an error rather than a guess.
    #[must_use]
    pub fn resolve(&self, target: &str) -> Option<Route> {
        let target = target.strip_prefix('/').unwrap_or(target);
        // The query belongs to the path being forwarded, never to the prefix being matched.
        let (path, query) = target
            .split_once('?')
            .map_or((target, None), |(p, q)| (p, Some(q)));
        let (prefix, rest) = path.split_once('/').unwrap_or((path, ""));
        let base = self.routes.get(prefix)?.trim_end_matches('/');
        let mut url = format!("{base}/{rest}");
        if let Some(query) = query {
            url.push('?');
            url.push_str(query);
        }
        Some(Route {
            provider: prefix.to_string(),
            url,
        })
    }

    /// The prefixes this gateway serves, for the error a client gets when it asks for another.
    #[must_use]
    pub fn prefixes(&self) -> Vec<&str> {
        self.routes.keys().map(String::as_str).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings::default()
    }

    #[test]
    fn appends_the_clients_own_path_to_the_providers_base() {
        let route = settings().resolve("/anthropic/v1/messages").unwrap();
        assert_eq!(route.provider, "anthropic");
        assert_eq!(route.url, "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn carries_the_query_across_and_matches_the_prefix_without_it() {
        let route = settings().resolve("/openai/v1/models?limit=2").unwrap();
        assert_eq!(route.url, "https://api.openai.com/v1/models?limit=2");
        // A bare prefix is the provider's own root, not a 404.
        assert_eq!(
            settings().resolve("/openai?x=1").unwrap().url,
            "https://api.openai.com/?x=1"
        );
    }

    #[test]
    fn a_fresh_install_rewrites_nothing() {
        assert!(!settings().placeholders.enabled);
        assert!(settings().placeholders.default_mode.is_empty());
    }

    #[test]
    fn an_unknown_first_segment_resolves_to_nothing() {
        assert!(settings().resolve("/mistral/v1/chat").is_none());
        assert!(settings().resolve("/").is_none());
    }
}
