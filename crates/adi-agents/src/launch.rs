//! What an agent run is launched *into*: the `PATH` it resolves its commands on.
//!
//! A run started by the daemon inherits launchd's minimal environment — a bare `PATH` like
//! `/usr/bin:/bin`, without the user's Homebrew, cargo, or bun dirs, let alone a project's pinned
//! toolchain. Every executor therefore rebuilds `PATH` before it spawns. This is the one place that
//! build lives, so the detached and pty executors cannot drift apart on what a run can find.

use std::path::{Path, PathBuf};

/// The `PATH` an agent run gets, in resolution order:
///
/// 1. the agent's own `.bin` — its enabled tools, so it runs them by name;
/// 2. the dirs its manifest `declared` (see [`AgentManifest::path`](crate::AgentManifest::path)),
///    so an agent can pin a toolchain ahead of the machine's default one;
/// 3. the standard user and system tool dirs launchd's minimal `PATH` misses;
/// 4. whatever `PATH` this process already had.
///
/// Joined with the platform separator, so it is right on Windows too.
pub(crate) fn run_path(bin_dir: Option<&Path>, declared: &[String]) -> String {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(bin) = bin_dir {
        dirs.push(bin.to_path_buf());
    }
    dirs.extend(declared.iter().filter_map(|dir| expand_home(dir)));
    #[cfg(unix)]
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        // `.bun/bin` belongs here for the same reason `adi_config::launch_env` lists it: a project
        // whose scripts run under bun has nothing else to find it by.
        for dir in [".local/bin", "bin", ".cargo/bin", ".bun/bin"] {
            dirs.push(home.join(dir));
        }
    }
    #[cfg(unix)]
    for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        dirs.push(PathBuf::from(dir));
    }
    if let Some(existing) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(dirs)
        .map(|joined| joined.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// One declared `path` entry as a directory: a leading `~`, `$HOME`, or `${HOME}` expanded against
/// the current `HOME`, everything else taken as written. `None` for a blank entry, and for a
/// `~`/`$HOME` entry when this process has no `HOME` — a run's `PATH` is better one dir short than
/// carrying a literal `$HOME/...` that resolves to nothing.
fn expand_home(dir: &str) -> Option<PathBuf> {
    let dir = dir.trim();
    if dir.is_empty() {
        return None;
    }
    // `${HOME}` before `$HOME`, or the longer form would match the shorter prefix and leave `{}`.
    for prefix in ["~", "${HOME}", "$HOME"] {
        let Some(rest) = dir.strip_prefix(prefix) else {
            continue;
        };
        // Only a whole leading segment counts: `~/x` expands, `~someone/x` is a literal path.
        if !rest.is_empty() && !rest.starts_with('/') {
            break;
        }
        let home = PathBuf::from(std::env::var_os("HOME")?);
        let rest = rest.trim_start_matches('/');
        return Some(if rest.is_empty() {
            home
        } else {
            home.join(rest)
        });
    }
    Some(PathBuf::from(dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The declared dirs sit between the agent's own `.bin` and every standard dir — an agent can
    /// outrank the machine's `node` without ever outranking its own tools.
    #[test]
    fn declared_dirs_rank_under_the_agent_bin_and_over_the_system() {
        let path = run_path(
            Some(Path::new("/store/.bin/solver")),
            &["/opt/node22/bin".to_string()],
        );
        let dirs: Vec<_> = std::env::split_paths(&path).collect();
        let at = |needle: &str| dirs.iter().position(|d| d == Path::new(needle));

        let own_bin = at("/store/.bin/solver").expect("the agent's own .bin");
        let declared = at("/opt/node22/bin").expect("the declared dir");
        assert!(own_bin < declared, "{path}");
        #[cfg(unix)]
        {
            let system = at("/usr/bin").expect("the system dir");
            assert!(declared < system, "{path}");
        }
    }

    /// `~` and `$HOME` are what a manifest is actually written with — TOML has no shell to expand
    /// them, so this does.
    #[test]
    fn home_prefixes_expand_and_a_literal_path_survives() {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return;
        };
        for entry in ["~/.nvm/bin", "$HOME/.nvm/bin", "${HOME}/.nvm/bin"] {
            assert_eq!(expand_home(entry), Some(home.join(".nvm/bin")), "{entry}");
        }
        assert_eq!(expand_home("~"), Some(home));
        assert_eq!(
            expand_home("/opt/node22/bin"),
            Some("/opt/node22/bin".into())
        );
        // Not a home prefix — `~someone` is a path in its own right, not this process's `HOME`.
        assert_eq!(expand_home("~someone/bin"), Some("~someone/bin".into()));
    }

    #[test]
    fn blank_entries_are_dropped() {
        assert_eq!(expand_home("   "), None);
        let path = run_path(None, &[String::new(), "  ".to_string()]);
        assert!(
            !std::env::split_paths(&path).any(|d| d.as_os_str().is_empty()),
            "{path}"
        );
    }

    /// Under launchd the inherited `PATH` is `/usr/bin:/bin` and nothing else — the standard dirs
    /// have to come from here, or a run cannot find the tools it was told to use.
    #[test]
    #[cfg(unix)]
    fn the_standard_tool_dirs_are_present() {
        let path = run_path(None, &[]);
        let dirs: Vec<_> = std::env::split_paths(&path).collect();
        for dir in ["/opt/homebrew/bin", "/usr/local/bin"] {
            assert!(dirs.iter().any(|d| d == Path::new(dir)), "{dir} in {path}");
        }
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            for dir in [".local/bin", ".cargo/bin", ".bun/bin"] {
                assert!(dirs.contains(&home.join(dir)), "{dir} in {path}");
            }
        }
    }
}
