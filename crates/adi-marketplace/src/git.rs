//! The one external program this crate runs: `git`.
//!
//! An app arrives as a **repository at a pinned commit**, not as a packed artifact, and every
//! property that choice buys lives here:
//!
//! * **The pin is the security story.** A manifest names a 40-hex commit; what lands is that
//!   commit or nothing. A publisher who pushes something else after the operator read the listing
//!   changes nothing about what a later install produces, and moving to a newer commit is an act
//!   somebody takes on purpose ([`move_to`]).
//! * **The clone stays a clone.** `.git` is kept, the branch the pin sits on tracks `origin`, and
//!   the working tree is clean on arrival — so the operator can edit the app, commit their edits,
//!   and `git pull` it later without this crate being in the way.
//!
//! Nothing here knows about dashboards or the store; it is the git verbs, phrased so a failure
//! reads as a sentence rather than as an exit status.

use std::path::Path;
use std::process::Command;

/// Where a working copy stands: the commit checked out, and the branch it is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    /// The commit `HEAD` resolves to — full 40-hex, always, so it compares to a manifest's pin.
    pub commit: String,
    /// The branch that commit sits on, which is what makes a later `git pull` mean something.
    pub branch: String,
}

/// The paths the store owns inside a cloned app, added to the clone's **local** excludes so they
/// never show up as untracked work in `git status` and never travel back upstream.
///
/// `config.toml` is the dashboard's own manifest (its name is the operator's, not the
/// publisher's), `.adi/` holds the hive file this machine wrote for its own paths, and
/// `node_modules/` is a cache. See [`exclude_store_files`].
pub const STORE_OWNED: &[&str] = &["/config.toml", "/.adi/", "/node_modules/"];

/// A dashboard's three generated entry points. The control panel rewrites these **in place**
/// whenever its templates move on — that is how a shell fix reaches dashboards that already
/// exist — and a dashboard's own work lives in `frontend/modules/` and `backend/routes/`, which
/// migration never touches.
///
/// A published app has to ship them (nothing writes a missing one, and two of them are what the
/// hive file runs), so they are tracked *and* rewritten under the operator's feet: the first
/// upgrade after an install would otherwise read as uncommitted work and stop every update. See
/// [`ignore_generated`].
pub const GENERATED: &[&str] = &[
    "frontend/index.html",
    "frontend/index.ts",
    "backend/index.ts",
];

/// Run one git command in `cwd`, answering its stdout — or the reason it failed, as git wrote it.
///
/// Two things are forced on every invocation and matter more than they look:
/// `GIT_TERMINAL_PROMPT=0`, so a private repository fails in a second instead of blocking forever
/// on a credential prompt nobody can answer inside a web request; and a low-speed cut-off, so a
/// transfer that stalls mid-clone ends rather than hanging the caller (there is no timeout on a
/// child process here).
fn run(cwd: Option<&Path>, args: &[&str]) -> std::result::Result<String, String> {
    let mut command = Command::new("git");
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        // A submodule is a second repository from a URL the manifest never named; the pin says
        // nothing about it, so it is never fetched.
        .args(["-c", "submodule.recurse=false"])
        .args(["-c", "http.lowSpeedLimit=1000", "-c", "http.lowSpeedTime=30"])
        .args(["-c", "advice.detachedHead=false"])
        .args(args);
    let out = command
        .output()
        .map_err(|e| format!("could not run git: {e} — is git installed?"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        // The last non-empty line: git writes progress first and the reason last.
        let reason = stderr
            .lines()
            .map(str::trim)
            .rfind(|line| !line.is_empty())
            .unwrap_or("git failed")
            .to_string();
        return Err(format!("git {}: {reason}", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Whether an argument would be read as a flag rather than as the value it is. Refused rather
/// than escaped: every value here comes from a manifest somebody else publishes, and a repo URL
/// spelled `--upload-pack=…` is not a repo URL.
fn is_flag(value: &str) -> bool {
    value.starts_with('-')
}

/// Whether this machine has git at all — the one dependency an install has outside the store.
#[must_use]
pub fn available() -> bool {
    run(None, &["--version"]).is_ok()
}

/// Clone `repo` into `dest` and leave it standing at `commit`, on a branch that tracks `origin`.
///
/// The tip is never checked out: the clone is `--no-checkout` and the pinned commit is what
/// writes the working tree, so no version other than the pinned one is ever materialized on this
/// machine. `branch` names which branch the pin should sit on (a manifest may pin a commit on
/// something other than the default); absent, the repository's own default branch is used.
///
/// # Errors
/// A sentence for each way it can fail: an argument that reads as a flag, a clone that did not
/// work, a `commit` the repository does not carry (fetched by sha first, in case it is not on any
/// branch), or a checkout that ended somewhere other than the pin.
pub fn clone_pinned(
    repo: &str,
    commit: &str,
    branch: Option<&str>,
    dest: &Path,
) -> std::result::Result<Pin, String> {
    if is_flag(repo) || is_flag(commit) || branch.is_some_and(is_flag) {
        return Err(format!("{repo} is not a repository url"));
    }
    let dest_str = dest.display().to_string();
    run(None, &["clone", "--quiet", "--no-checkout", "--", repo, &dest_str])?;
    pin_to(dest, commit, branch, false)
}

/// Move an already-cloned app onto `commit`: fetch, then fast-forward the branch to the pin.
///
/// **Fast-forward, not reset.** An operator's own commits on top of the app are the point of it
/// being a clone; an update that silently discarded them would make editing an installed app a
/// trap. A branch that cannot fast-forward says so, and `force` — the deliberate second ask — is
/// what resets onto the pin and throws local work away.
///
/// # Errors
/// A sentence for a fetch that failed, a commit the remote does not carry, a branch that has
/// diverged from the pin (without `force`), or a checkout that ended somewhere else.
pub fn move_to(
    dir: &Path,
    commit: &str,
    branch: Option<&str>,
    force: bool,
) -> std::result::Result<Pin, String> {
    if is_flag(commit) || branch.is_some_and(is_flag) {
        return Err(format!("{commit} is not a commit"));
    }
    // Best-effort: a fetch that fails still leaves an update possible when the pin is already
    // here, which is the offline case — the manifest was synced while the network was up.
    let fetched = run(Some(dir), &["fetch", "--quiet", "origin"]);
    if !have_commit(dir, commit) {
        fetch_commit(dir, commit).map_err(|e| match fetched.as_ref().err() {
            // Two failures in a row are one story: the fetch that could not reach the remote is
            // usually *why* the pin is missing, and reporting only the second sends the reader
            // looking at the manifest instead of at the network.
            Some(first) => format!("{first}; {e}"),
            None => e,
        })?;
    }
    if force {
        return pin_to(dir, commit, branch, true);
    }
    let head = head(dir).unwrap_or_default();
    if head == commit.to_ascii_lowercase() {
        return current_pin(dir, commit);
    }
    run(Some(dir), &["merge", "--ff-only", "--quiet", commit]).map_err(|_| {
        format!(
            "this copy cannot fast-forward onto {}: it carries commits the pin does not. Merge or \
             rebase it yourself, or force the update to reset onto the pin and lose them.",
            short(commit)
        )
    })?;
    current_pin(dir, commit)
}

/// Put the working tree on `commit`, on the branch it should sit on, and prove it landed there.
///
/// `checkout -B` rather than a detached checkout: a detached HEAD is a working copy whose
/// `git pull` fails, and the whole point of installing a clone is that the operator can pull it.
/// `force` adds `--force`, which is what throws uncommitted work away — never reached except
/// through the caller that asked for exactly that.
fn pin_to(
    dir: &Path,
    commit: &str,
    branch: Option<&str>,
    force: bool,
) -> std::result::Result<Pin, String> {
    if !have_commit(dir, commit) {
        fetch_commit(dir, commit)?;
    }
    let branch = match branch.map(str::trim).filter(|b| !b.is_empty()) {
        Some(branch) => branch.to_string(),
        None => default_branch(dir),
    };
    let mut args = vec!["checkout", "--quiet"];
    if force {
        args.push("--force");
    }
    args.extend(["-B", &branch, commit, "--"]);
    run(Some(dir), &args)?;
    // A branch named by the manifest may be one the clone never had locally, so it starts with no
    // upstream; give it one when the remote has it. Best-effort — a pin on a branch that exists
    // only in the manifest is still a working install, it just has nothing to pull from.
    let _ = run(
        Some(dir),
        &[
            "branch",
            &format!("--set-upstream-to=origin/{branch}"),
            &branch,
        ],
    );
    current_pin(dir, commit)
}

/// Read where the working copy actually stands and check it against the pin that was asked for.
fn current_pin(dir: &Path, wanted: &str) -> std::result::Result<Pin, String> {
    let commit = head(dir).ok_or_else(|| "the clone has no HEAD commit".to_string())?;
    if commit != wanted.to_ascii_lowercase() {
        return Err(format!(
            "the checkout ended at {} rather than the pinned {}",
            short(&commit),
            short(wanted)
        ));
    }
    Ok(Pin {
        commit,
        branch: branch(dir).unwrap_or_default(),
    })
}

/// Whether `commit` is an object this clone already carries.
fn have_commit(dir: &Path, commit: &str) -> bool {
    run(Some(dir), &["rev-parse", "--verify", "--quiet", &format!("{commit}^{{commit}}")]).is_ok()
}

/// Ask the remote for one commit by sha — the case where a pin is not on any branch (a rewritten
/// branch, a pull-request head). Not every server allows it, so the failure names the pin rather
/// than the protocol.
fn fetch_commit(dir: &Path, commit: &str) -> std::result::Result<(), String> {
    run(Some(dir), &["fetch", "--quiet", "origin", commit])
        .map(|_| ())
        .map_err(|_| {
            format!(
                "the repository does not carry the pinned commit {} — the manifest pins something \
                 this remote will not serve",
                short(commit)
            )
        })?;
    if have_commit(dir, commit) {
        Ok(())
    } else {
        Err(format!(
            "the repository does not carry the pinned commit {}",
            short(commit)
        ))
    }
}

/// The commit `HEAD` resolves to, lowercased, or `None` in a directory that is not a clone.
#[must_use]
pub fn head(dir: &Path) -> Option<String> {
    run(Some(dir), &["rev-parse", "HEAD"])
        .ok()
        .map(|sha| sha.to_ascii_lowercase())
        .filter(|sha| sha.len() == 40)
}

/// The branch the working copy is on, or `None` when it is detached or not a clone.
#[must_use]
pub fn branch(dir: &Path) -> Option<String> {
    run(Some(dir), &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()
        .filter(|b| !b.is_empty())
}

/// The remote this clone came from, as `origin` has it.
#[must_use]
pub fn remote(dir: &Path) -> Option<String> {
    run(Some(dir), &["remote", "get-url", "origin"])
        .ok()
        .filter(|url| !url.is_empty())
}

/// The branch a fresh clone put `HEAD` on — the repository's own default. `main` when the clone
/// somehow has no symbolic HEAD, because a branch name is needed to check out onto.
fn default_branch(dir: &Path) -> String {
    branch(dir).unwrap_or_else(|| "main".to_string())
}

/// Whether the working copy has uncommitted work — the question an update has to ask before it
/// moves anything. Excluded paths (the store's own files) are not work, and do not count.
#[must_use]
pub fn dirty(dir: &Path) -> bool {
    run(Some(dir), &["status", "--porcelain"]).is_ok_and(|out| !out.trim().is_empty())
}

/// Whether the repository tracks `path` — asked about the two files the store owns, because a
/// repository that versions them will fight every update over them.
#[must_use]
pub fn tracks(dir: &Path, path: &str) -> bool {
    run(Some(dir), &["ls-files", "--error-unmatch", "--", path]).is_ok()
}

/// Write the store's own paths into the clone's local excludes (`.git/info/exclude`), so the
/// dashboard's `config.toml`, its `.adi/` and its `node_modules/` never read as untracked work.
///
/// Local, not `.gitignore`: the app's own ignore file belongs to whoever publishes it, and an
/// install has no business committing to their repository.
///
/// # Errors
/// [`std::io::Error`] when the exclude file cannot be written.
pub fn exclude_store_files(dir: &Path) -> std::io::Result<()> {
    let info = dir.join(".git").join("info");
    std::fs::create_dir_all(&info)?;
    let path = info.join("exclude");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut out = existing.clone();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("# Written by the ADI marketplace: these belong to the store, not to the app.\n");
    for entry in STORE_OWNED {
        if !existing.lines().any(|line| line.trim() == *entry) {
            out.push_str(entry);
            out.push('\n');
        }
    }
    std::fs::write(path, out)
}

/// Tell this clone to stop noticing changes to the [`GENERATED`] entry points, so the control
/// panel's own migrations do not read as the operator's uncommitted work.
///
/// `--skip-worktree` rather than an exclude, because these files are *tracked*: an exclude only
/// ever silences an untracked one. Applied per path, and only where the repository actually
/// carries it — an app that (rightly) leaves a generated file out of its history has nothing to
/// skip.
pub fn ignore_generated(dir: &Path) {
    for path in GENERATED {
        if tracks(dir, path) {
            let _ = run(Some(dir), &["update-index", "--skip-worktree", "--", path]);
        }
    }
}

/// Undo [`ignore_generated`] and put the generated files back to what the pin says they are — the
/// state a fetch and a merge can move through.
///
/// The panel's rewrite is discarded here, and that is safe in the one direction it matters: these
/// three files are generated, the panel restamps them again within its next poll, and their
/// contents were never the operator's to keep (the scaffold marks each "do not edit"). Doing this
/// before an update is what keeps a merge from failing on "your local changes would be
/// overwritten" for a file nobody edited.
pub fn unignore_generated(dir: &Path) {
    for path in GENERATED {
        if tracks(dir, path) {
            let _ = run(Some(dir), &["update-index", "--no-skip-worktree", "--", path]);
            let _ = run(Some(dir), &["checkout", "--", path]);
        }
    }
}

/// A commit as it is read out loud: the first seven characters.
#[must_use]
pub fn short(commit: &str) -> String {
    commit.chars().take(7).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A scratch directory of this test's own, under the system temp dir.
    fn scratch(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "adi-marketplace-git-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch root");
        root
    }

    /// A real repository with two commits, standing in for what a publisher hosts. Answers the
    /// path to clone and the two commits, oldest first.
    fn upstream(root: &Path) -> (String, String, String) {
        let dir = root.join("upstream");
        std::fs::create_dir_all(&dir).expect("upstream dir");
        let git = |args: &[&str]| run(Some(&dir), args).unwrap_or_else(|e| panic!("{e}"));
        git(&["init", "--quiet", "-b", "main"]);
        git(&["config", "user.email", "publisher@example"]);
        git(&["config", "user.name", "The Publisher"]);
        std::fs::write(dir.join("app.ts"), "// v1\n").expect("v1");
        // The generated shell every dashboard carries, at the generation this app was published
        // with — what the panel later restamps in place.
        std::fs::create_dir_all(dir.join("frontend")).expect("frontend");
        std::fs::write(dir.join("frontend").join("index.html"), "<!-- gen 2 -->\n").expect("shell");
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "v1"]);
        let first = run(Some(&dir), &["rev-parse", "HEAD"]).expect("sha");
        std::fs::write(dir.join("app.ts"), "// v2\n").expect("v2");
        git(&["commit", "--quiet", "-am", "v2"]);
        let second = run(Some(&dir), &["rev-parse", "HEAD"]).expect("sha");
        (format!("file://{}", dir.display()), first, second)
    }

    #[test]
    fn a_clone_lands_on_the_pin_and_can_still_be_pulled() {
        let root = scratch("pin");
        let (repo, first, _second) = upstream(&root);
        let dest = root.join("app");

        let pin = clone_pinned(&repo, &first, None, &dest).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(pin.commit, first, "the pin is what landed");
        assert_eq!(pin.branch, "main");
        assert_eq!(
            std::fs::read_to_string(dest.join("app.ts")).expect("tree"),
            "// v1\n",
            "the pinned tree, not the tip"
        );
        // Not detached, tracking origin, clean: the three things a `git pull` later needs.
        assert_eq!(branch(&dest).as_deref(), Some("main"));
        assert_eq!(
            run(Some(&dest), &["rev-parse", "--abbrev-ref", "@{u}"]).expect("upstream"),
            "origin/main"
        );
        assert!(!dirty(&dest), "an arriving clone is clean");
        assert_eq!(remote(&dest).as_deref(), Some(repo.as_str()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_update_fast_forwards_onto_the_new_pin() {
        let root = scratch("update");
        let (repo, first, second) = upstream(&root);
        let dest = root.join("app");
        clone_pinned(&repo, &first, None, &dest).expect("clone");

        let pin = move_to(&dest, &second, None, false).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(pin.commit, second);
        assert_eq!(
            std::fs::read_to_string(dest.join("app.ts")).expect("tree"),
            "// v2\n"
        );
        // Idempotent: an update to the pin it is already on is not a failure.
        assert_eq!(move_to(&dest, &second, None, false).expect("again").commit, second);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn local_commits_survive_an_update_that_cannot_fast_forward() {
        let root = scratch("diverged");
        let (repo, first, second) = upstream(&root);
        let dest = root.join("app");
        clone_pinned(&repo, &first, None, &dest).expect("clone");

        // The operator edits the app they installed — the reason it is a clone at all.
        std::fs::write(dest.join("app.ts"), "// mine\n").expect("edit");
        assert!(dirty(&dest), "an edit is uncommitted work");
        for args in [
            &["config", "user.email", "operator@example"][..],
            &["config", "user.name", "The Operator"][..],
            &["commit", "--quiet", "-am", "mine"][..],
        ] {
            run(Some(&dest), args).expect("commit");
        }

        let err = move_to(&dest, &second, None, false).expect_err("refused");
        assert!(err.contains("fast-forward"), "{err}");
        assert_eq!(
            std::fs::read_to_string(dest.join("app.ts")).expect("tree"),
            "// mine\n",
            "a refused update changed nothing"
        );

        // Forcing is the deliberate second ask, and it does what it says.
        let pin = move_to(&dest, &second, None, true).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(pin.commit, second);
        assert_eq!(
            std::fs::read_to_string(dest.join("app.ts")).expect("tree"),
            "// v2\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pin_the_repository_does_not_carry_is_refused_by_name() {
        let root = scratch("nopin");
        let (repo, _first, _second) = upstream(&root);
        let dest = root.join("app");

        let missing = "0".repeat(40);
        let err = clone_pinned(&repo, &missing, None, &dest).expect_err("refused");
        assert!(err.contains(&short(&missing)), "{err}");

        // And an argument that would read as a flag is not a repository url.
        assert!(
            clone_pinned("--upload-pack=touch /tmp/pwned", "abc", None, &root.join("x")).is_err()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_panels_rewrite_of_a_generated_file_is_not_the_operators_work() {
        let root = scratch("generated");
        let (repo, first, second) = upstream(&root);
        let dest = root.join("app");
        clone_pinned(&repo, &first, None, &dest).expect("clone");

        // The app ships the entry point every dashboard has; the panel restamps it in place the
        // first time it sees a dashboard from an older generation.
        assert!(tracks(&dest, "frontend/index.html"), "the app ships its shell");
        ignore_generated(&dest);
        std::fs::write(dest.join("frontend").join("index.html"), "<!-- gen 3 -->\n").expect("panel");
        assert!(
            !dirty(&dest),
            "a generated file the panel rewrote must not stop an update"
        );

        // And an update still moves: the rewrite is dropped, the merge runs, the file goes back
        // to being ignored — the panel restamps it again on its next poll.
        unignore_generated(&dest);
        assert!(!dirty(&dest), "put back to what the pin says it is");
        assert_eq!(
            std::fs::read_to_string(dest.join("frontend").join("index.html")).expect("shell"),
            "<!-- gen 2 -->\n"
        );
        move_to(&dest, &second, None, false).unwrap_or_else(|e| panic!("{e}"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_stores_own_files_are_excluded_from_the_clone() {
        let root = scratch("exclude");
        let (repo, first, _second) = upstream(&root);
        let dest = root.join("app");
        clone_pinned(&repo, &first, None, &dest).expect("clone");

        exclude_store_files(&dest).expect("exclude");
        std::fs::write(dest.join("config.toml"), "name = \"mine\"\n").expect("manifest");
        std::fs::create_dir_all(dest.join(".adi")).expect("adi");
        std::fs::write(dest.join(".adi").join("hive.yaml"), "version: \"1\"\n").expect("hive");
        assert!(
            !dirty(&dest),
            "the store's own files are not the app's uncommitted work"
        );
        assert!(!tracks(&dest, "config.toml"));
        assert!(tracks(&dest, "app.ts"));

        // Writing them twice does not repeat the entries.
        exclude_store_files(&dest).expect("again");
        let raw = std::fs::read_to_string(dest.join(".git").join("info").join("exclude"))
            .expect("exclude file");
        assert_eq!(raw.matches("/config.toml").count(), 1, "{raw}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
