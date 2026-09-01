//! Who a scoped command runs as.
//!
//! Both `knowledge` and `facts` address their bases at the same three isolation levels, so both
//! answer the same question the same way — the flags first, then the run environment, then
//! nobody in particular, which means the owner of the store. It lives here rather than in either
//! command group because two copies of this rule would be two chances for them to disagree, and
//! a caller that learned `--as-agent` from one would have to relearn it for the other.

use adi_knowledge::Reader;

use crate::format::clean;

/// Who a command runs as, or `None` for the owner of the store.
///
/// Split out from the store-building call so it can be tested without a test reaching into the
/// process environment every other test shares.
pub(crate) fn identity(
    as_agent: Option<String>,
    as_project: Option<String>,
    env_agent: Option<String>,
    env_project: Option<String>,
) -> Option<Reader> {
    let agent = clean(as_agent).or_else(|| clean(env_agent));
    let project = clean(as_project).or_else(|| clean(env_project));
    if agent.is_none() && project.is_none() {
        return None;
    }
    Some(Reader {
        agent,
        project,
        admin: false,
    })
}

/// The reader for a command, reading `ADI_AGENT` / `ADI_PROJECT` when no flag states one.
///
/// `root` is answered **first** — before the flags and before the environment. A run cannot
/// unset a variable its launcher exported, so a root check that came second would scope the
/// caller straight back into the isolation it asked to step out of, and writing into another
/// agent's base — the one thing the flag exists for — would stay impossible.
pub(crate) fn reader_for(
    as_agent: Option<String>,
    as_project: Option<String>,
    root: bool,
) -> Option<Reader> {
    if root {
        return None;
    }
    identity(
        as_agent,
        as_project,
        std::env::var("ADI_AGENT").ok(),
        std::env::var("ADI_PROJECT").ok(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fallback that makes the system tool work: a run already carries who it is.
    #[test]
    fn the_run_environment_supplies_an_identity_no_flag_stated() {
        let from_env = identity(None, None, Some("solver".into()), Some("acme".into()))
            .expect("an agent run is somebody");
        assert_eq!(from_env.agent.as_deref(), Some("solver"));
        assert_eq!(from_env.project.as_deref(), Some("acme"));
        assert!(!from_env.admin);

        // A stated flag beats the environment — that is what makes `--as-agent` useful for
        // inspecting another agent's view from inside a run.
        let stated =
            identity(Some("reviewer".into()), None, Some("solver".into()), None).expect("stated");
        assert_eq!(stated.agent.as_deref(), Some("reviewer"));

        // A person's shell has neither, and stays the owner of the store.
        assert_eq!(identity(None, None, None, None), None);
        // …and an empty variable is not an identity, which is how an exported-but-blank
        // `ADI_PROJECT` avoids hiding every project base from a run that has no project.
        assert_eq!(
            identity(None, None, Some(String::new()), Some("  ".into())),
            None
        );
    }

    #[test]
    fn root_is_the_owner_whatever_the_flags_and_the_environment_say() {
        assert_eq!(
            reader_for(Some("solver".into()), Some("acme".into()), true),
            None,
            "root is nobody in particular"
        );
        let scoped = reader_for(Some("solver".into()), None, false).expect("scoped");
        assert!(!scoped.admin);
        assert_eq!(scoped.agent.as_deref(), Some("solver"));
    }
}
