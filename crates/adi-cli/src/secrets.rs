//! The `secrets` command group: set / get / list / remove encrypted secrets in the global or
//! a per-project scope. Values are stored encrypted; only `get --reveal` ever prints one.
//!
//! `policy`, `leases`, and `--user --node` on `rm`/`set`, are the BYO-credentials verbs
//! (`docs/byo-credentials.md` §4.4, ADI-MONO-178 P1.12 / ADI-MONO-180 P2.5): a user's own scope
//! on a paired server, reached through this machine's local mesh gateway the way `docs/fleet.md`
//! §10's dashboard transfer addresses one — `127.0.0.1:<gateway port>` with
//! `Host: app.<node>.n.adi`. `set --user` seals the value on this device before it ever leaves
//! (§8, P2.1/P2.2) — the server stores exactly what it is handed and never decrypts it. There is
//! no `--reveal` or `read` for `--user`: a user scope is write-only everywhere (§7).

use adi_core::{Adi, Missing, Secret, Secrets, UserSecretsPolicy};
use clap::Subcommand;

use crate::format::{print_json, resolve_scope, scope_label};

/// How long a `--node` request waits before giving up — generous for a loopback hop plus a QUIC
/// round trip to the peer, short enough that an unreachable or unpaired node fails as that rather
/// than hanging the shell.
pub(crate) const NODE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(Debug, Subcommand)]
pub(crate) enum SecretsCommand {
    /// List secret names in a scope — metadata only, never values. Omit `--project` for global.
    List {
        /// Operate on global secrets (the default when `--project` is omitted).
        #[arg(long)]
        global: bool,
        /// Operate on this project's secrets.
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Set (create or overwrite) a secret. The value comes from `--value`, or from stdin when
    /// that's omitted (e.g. `printf %s "$V" | adi-mono secrets set FOO`), so it needn't sit in
    /// shell history.
    Set {
        /// The secret key name — also the env-var name it injects into runs as.
        name: String,
        /// The value. If omitted, it's read from stdin.
        value: Option<String>,
        /// An optional one-line description of what the secret is for.
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        global: bool,
        #[arg(long)]
        project: Option<String>,
        /// Set this value in *your own* scope on `--node`, not this machine's — sealed on this
        /// device before it leaves (`docs/byo-credentials.md` §7, §8).
        #[arg(long)]
        user: bool,
        /// The paired server to reach, over this machine's local mesh gateway. Required with
        /// `--user` — there is no user scope on your own machine.
        #[arg(long)]
        node: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Show one secret's metadata — or its decrypted value with `--reveal`.
    Get {
        name: String,
        /// Print the decrypted value instead of metadata.
        #[arg(long)]
        reveal: bool,
        #[arg(long)]
        global: bool,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Print one secret's decrypted value — the raw bytes, nothing else — so it can be consumed
    /// directly (`GMAIL=$(adi-mono secrets read GMAIL_TOKEN)`), the way `op read` works. This is
    /// the value-returning primitive an agent uses; unlike `get`, no `--reveal` flag is needed.
    Read {
        name: String,
        #[arg(long)]
        global: bool,
        #[arg(long)]
        project: Option<String>,
    },
    /// Delete a secret from a scope.
    Rm {
        name: String,
        #[arg(long)]
        global: bool,
        #[arg(long)]
        project: Option<String>,
        /// Delete this value from *your own* scope on `--node`, not this machine's.
        #[arg(long)]
        user: bool,
        /// The paired server to reach, over this machine's local mesh gateway. Required with
        /// `--user` — there is no user scope on your own machine.
        #[arg(long)]
        node: Option<String>,
    },
    /// Show or edit the server's own `[user_secrets]` policy
    /// (`docs/byo-credentials.md` §4.1) — or, with `--user --node`, one paired server's own
    /// `.settings.toml` for *you* (§4.2).
    Policy {
        /// Edit your own policy on `--node` instead of this server's.
        #[arg(long)]
        user: bool,
        /// The paired server to reach, over this machine's local mesh gateway. Required with
        /// `--user`.
        #[arg(long)]
        node: Option<String>,
        /// Turn the user-secrets feature on. Local only (`--user` has no `enabled` of its own).
        #[arg(long)]
        enable: bool,
        #[arg(long)]
        disable: bool,
        /// The env-var names a user may supply, replacing the list. Local only.
        #[arg(long, value_delimiter = ',')]
        overridable: Option<Vec<String>>,
        /// `server` | `wait` | `fail`. On `--user`, may only tighten the server's own (§4.2).
        #[arg(long)]
        missing: Option<String>,
        /// The longest lease a device may ask for (`"72h"`). Local only.
        #[arg(long = "max-lease")]
        max_lease: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Live leases (`docs/byo-credentials.md` §7, §9, ADI-MONO-180 P2.5): on a paired server
    /// (`--node`), the caller's own; this device's own admin listing of every live lease on its
    /// own server isn't wired up yet (`--node` is required here for now).
    Leases {
        #[arg(long)]
        node: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

/// Dispatch a `secrets` subcommand over the adi-core facade, surfacing any store error as a
/// `String` (like the other command groups) so error families print uniformly.
pub(crate) fn run_secrets(adi: Adi, command: SecretsCommand) -> Result<(), String> {
    let store = adi.secrets();
    match command {
        SecretsCommand::List {
            global,
            project,
            json,
        } => {
            let scope = resolve_scope(global, project)?;
            let secrets = store.list(scope.as_deref()).map_err(|e| e.to_string())?;
            if json {
                print_json(&secrets);
            } else if secrets.is_empty() {
                println!("No secrets in {}.", scope_label(scope.as_deref()));
            } else {
                for secret in &secrets {
                    print_secret(secret);
                }
            }
        }
        SecretsCommand::Set {
            name,
            value,
            description,
            global,
            project,
            user,
            node,
            json,
        } => {
            if user {
                let node = require_node(node.as_deref())?;
                let value = match value {
                    Some(v) => v,
                    None => read_stdin_value()?,
                };
                let answer = set_user_secret(&node, &name, &value, description.as_deref())?;
                if json {
                    print_json(&answer);
                } else {
                    println!("{node}: set {name} in your own scope there.");
                }
                return Ok(());
            }
            let scope = resolve_scope(global, project)?;
            let value = match value {
                Some(v) => v,
                None => read_stdin_value()?,
            };
            let secret = store
                .set(scope.as_deref(), &name, &value, description.as_deref())
                .map_err(|e| e.to_string())?;
            if json {
                print_json(&secret);
            } else {
                println!(
                    "Set secret {} in {}.",
                    secret.name,
                    scope_label(scope.as_deref())
                );
            }
        }
        SecretsCommand::Get {
            name,
            reveal,
            global,
            project,
            json,
        } => {
            let scope = resolve_scope(global, project)?;
            if reveal {
                let value = store
                    .reveal(scope.as_deref(), &name)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("no such secret: {name}"))?;
                if json {
                    print_json(&serde_json::json!({ "name": name, "value": value }));
                } else {
                    // The value alone, no trailing decoration — safe to pipe.
                    println!("{value}");
                }
            } else {
                let secret = store
                    .get(scope.as_deref(), &name)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("no such secret: {name}"))?;
                if json {
                    print_json(&secret);
                } else {
                    print_secret(&secret);
                }
            }
        }
        SecretsCommand::Read {
            name,
            global,
            project,
        } => {
            use std::io::Write as _;
            let scope = resolve_scope(global, project)?;
            let value = store
                .reveal(scope.as_deref(), &name)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no such secret: {name}"))?;
            // The exact bytes, no added newline — faithful for a shell capture or an agent read.
            print!("{value}");
            let _ = std::io::stdout().flush();
        }
        SecretsCommand::Rm {
            name,
            global,
            project,
            user,
            node,
        } => {
            if user {
                let node = require_node(node.as_deref())?;
                let answer = node_request(
                    &node,
                    "POST",
                    "/api/secrets/mine/remove",
                    Some(&serde_json::json!({ "name": name })),
                )?;
                println!("{node}: removed {name} from your own scope there.");
                let _ = answer;
                return Ok(());
            }
            let scope = resolve_scope(global, project)?;
            if store
                .remove(scope.as_deref(), &name)
                .map_err(|e| e.to_string())?
            {
                println!(
                    "Deleted secret {name} from {}.",
                    scope_label(scope.as_deref())
                );
            } else {
                println!("No such secret: {name}.");
            }
        }
        SecretsCommand::Policy {
            user,
            node,
            enable,
            disable,
            overridable,
            missing,
            max_lease,
            json,
        } => run_policy(
            &adi.secrets(),
            user,
            node,
            enable,
            disable,
            overridable,
            missing,
            max_lease,
            json,
        )?,
        SecretsCommand::Leases { node, json } => run_leases(node, json)?,
    }
    Ok(())
}

/// `secrets leases --node <server>` — the caller's own live leases there (§7). The local,
/// no-`--node` admin listing of every lease on this machine's own server has no established way
/// for this CLI to reach a *running* `adi-app` process yet (leases live only in its memory, never
/// on disk) — refused for now rather than guessed at.
fn run_leases(node: Option<String>, json: bool) -> Result<(), String> {
    let Some(node) = node else {
        return Err(
            "the local admin listing isn't available from this CLI yet — pass --node <server> \
             for your own leases there"
                .to_string(),
        );
    };
    let answer = node_request(&node, "GET", "/api/leases", None)?;
    if json {
        print_json(&answer);
    } else {
        let leases = answer["leases"].as_array().cloned().unwrap_or_default();
        if leases.is_empty() {
            println!("{node}: no live leases.");
        }
        for lease in leases {
            println!(
                "{node}: {} ({}) — {} — lease {} — last seen {}s ago",
                lease["user"].as_str().unwrap_or("?"),
                lease["petname"].as_str().unwrap_or("?"),
                lease["names"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", "))
                    .unwrap_or_default(),
                lease["lease"].as_str().unwrap_or("?"),
                lease["last_seen_secs_ago"].as_u64().unwrap_or(0),
            );
        }
    }
    Ok(())
}

/// `--node` needs `--user` to make sense of, and the reverse too: there is no user scope on
/// your own machine to reach with it (`docs/byo-credentials.md` §4.4). Reused by `mesh
/// share`/`unlock`/`lock` (ADI-MONO-180 P2.5), which need exactly the same node for exactly the
/// same reason, even though none of them takes a `--user` flag of their own.
pub(crate) fn require_node(node: Option<&str>) -> Result<String, String> {
    node.map(str::to_string)
        .ok_or_else(|| "--user needs --node: there is no user scope on your own machine".to_string())
}

/// `secrets policy`, local and `--user --node` both.
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
fn run_policy(
    store: &Secrets,
    user: bool,
    node: Option<String>,
    enable: bool,
    disable: bool,
    overridable: Option<Vec<String>>,
    missing: Option<String>,
    max_lease: Option<String>,
    json: bool,
) -> Result<(), String> {
    if user {
        let node = require_node(node.as_deref())?;
        if let Some(missing) = missing {
            let answer = node_request(
                &node,
                "POST",
                "/api/secrets/mine/settings",
                Some(&serde_json::json!({ "missing": missing })),
            )?;
            print_policy_answer(&node, &answer, json);
        } else {
            let answer = node_request(&node, "GET", "/api/secrets/mine", None)?;
            print_policy_answer(&node, &answer, json);
        }
        return Ok(());
    }
    if enable && disable {
        return Err("--enable and --disable are mutually exclusive".to_string());
    }
    let mut policy = UserSecretsPolicy::load(store.config());
    let edited = enable || disable || overridable.is_some() || missing.is_some() || max_lease.is_some();
    if edited {
        if enable {
            policy.enabled = true;
        }
        if disable {
            policy.enabled = false;
        }
        if let Some(overridable) = overridable {
            policy.overridable = overridable
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
        if let Some(missing) = &missing {
            policy.missing = parse_missing(missing)?;
        }
        if let Some(max_lease) = max_lease {
            policy.max_lease = Some(max_lease);
        }
        policy.save(store.config()).map_err(|e| e.to_string())?;
    }
    if json {
        print_json(&serde_json::json!({
            "enabled": policy.enabled,
            "overridable": policy.overridable,
            "missing": missing_label(policy.missing),
            "max_lease": policy.max_lease,
        }));
    } else {
        println!(
            "user_secrets: {} — overridable: {} — missing: {}{}",
            if policy.enabled { "enabled" } else { "disabled" },
            if policy.overridable.is_empty() {
                "(none)".to_string()
            } else {
                policy.overridable.join(", ")
            },
            missing_label(policy.missing),
            policy
                .max_lease
                .as_deref()
                .map(|l| format!(" — max_lease: {l}"))
                .unwrap_or_default(),
        );
    }
    Ok(())
}

fn missing_label(missing: Missing) -> &'static str {
    match missing {
        Missing::Server => "server",
        Missing::Wait => "wait",
        Missing::Fail => "fail",
    }
}

fn parse_missing(text: &str) -> Result<Missing, String> {
    match text {
        "server" => Ok(Missing::Server),
        "wait" => Ok(Missing::Wait),
        "fail" => Ok(Missing::Fail),
        other => Err(format!("unrecognised --missing value: {other} (want server, wait or fail)")),
    }
}

fn print_policy_answer(node: &str, answer: &serde_json::Value, json: bool) {
    if json {
        print_json(answer);
    } else {
        println!("{node}: {answer}");
    }
}

/// This device's own username for `node` — the credential's non-secret half
/// (`adi_mesh::auth::Credential::user`, "stored in the clear"). The password is never kept
/// anywhere after pairing (`docs/fleet.md` §3.1: "neither side stores it; both store its
/// verifier"), so [`read_password`] asks for it fresh every time.
pub(crate) fn node_username(node: &str) -> Result<String, String> {
    adi_mesh::node_api::node_username(node)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("not paired with {node} — see `adi-mono mesh list`"))
}

/// Prompt for `node`'s password on stderr, so a piped stdout stays clean of it. Plain text, not
/// hidden — the same trade `secrets set`'s stdin convenience already makes, and there is no
/// terminal-echo crate in this binary's dependency tree to hide it with.
pub(crate) fn read_password(node: &str) -> Result<String, String> {
    use std::io::Write as _;
    eprint!("Password for {node}: ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let trimmed = line.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        return Err("a password is required".to_string());
    }
    Ok(trimmed.to_string())
}

/// One request to `node`'s own `app` service, through this machine's local mesh gateway —
/// `docs/fleet.md` §10's addressing (`adi_mesh::node_api`, lifted out of this module when the
/// BYO-credentials `mesh` verbs needed the same capability, ADI-MONO-180 P2.5).
pub(crate) fn node_request(
    node: &str,
    method: &str,
    path: &str,
    body: Option<&serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let username = node_username(node)?;
    let password = read_password(node)?;
    let client = adi_mesh::node_api::gateway_client(NODE_TIMEOUT).map_err(|e| e.to_string())?;
    adi_mesh::node_api::node_request(&client, node, &username, &password, method, path, body)
        .map_err(|e| e.to_string())
}

/// Seal `value` on this device and upload it to `node`'s own scope (`docs/byo-credentials.md`
/// §8, ADI-MONO-180 P2.1/P2.2): a fresh DEK seals the value, this device's own key-wrapping key
/// (`adi_mesh::devicekey`) wraps the DEK, and only the sealed result crosses the wire. `name` is
/// the AAD label — stable for as long as this device calls `node` by this petname, and never
/// checked by the server, which never opens either seal (§8's whole point).
fn set_user_secret(
    node: &str,
    name: &str,
    value: &str,
    description: Option<&str>,
) -> Result<serde_json::Value, String> {
    let wrap_key = adi_mesh::devicekey::load_or_create().map_err(|e| e.to_string())?;
    let dek = adi_secrets::wrap::generate_dek();
    let (nonce, ciphertext) =
        adi_secrets::wrap::seal_value(&dek, node, name, value.as_bytes()).map_err(|e| e.to_string())?;
    let (wrap_nonce, wrapped) =
        adi_secrets::wrap::wrap_dek(&wrap_key, node, name, &dek).map_err(|e| e.to_string())?;
    let body = serde_json::json!({
        "name": name,
        "description": description,
        "nonce": nonce,
        "ciphertext": ciphertext,
        "wraps": [{
            "kind": adi_mesh::devicekey::KIND,
            "nonce": wrap_nonce,
            "wrapped": wrapped,
        }],
    });
    node_request(node, "POST", "/api/secrets/mine/set", Some(&body))
}

/// Read a secret value from stdin, dropping a single trailing newline so a piped
/// `echo`/`printf` doesn't smuggle one into the stored value.
fn read_stdin_value() -> Result<String, String> {
    use std::io::Read as _;
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| e.to_string())?;
    let trimmed = buf.strip_suffix('\n').unwrap_or(&buf);
    Ok(trimmed.strip_suffix('\r').unwrap_or(trimmed).to_string())
}

/// Print a secret's metadata as a human line plus its description — never the value.
fn print_secret(secret: &Secret) {
    println!(
        "{} [{}]",
        secret.name,
        scope_label(secret.project.as_deref())
    );
    if let Some(description) = &secret.description {
        println!("  {description}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory as _, Parser};

    /// A parser wrapper, so the group's argv surface can be exercised without the whole
    /// `adi-mono` command tree — the same harness `mesh`'s own tests use.
    #[derive(Debug, Parser)]
    #[command(name = "secrets")]
    struct Harness {
        #[command(subcommand)]
        command: SecretsCommand,
    }

    fn command_line(words: &[&str]) -> Vec<String> {
        std::iter::once("secrets")
            .chain(words.iter().copied())
            .map(ToString::to_string)
            .collect()
    }

    fn parse(words: &[&str]) -> SecretsCommand {
        Harness::try_parse_from(command_line(words))
            .unwrap_or_else(|e| panic!("{words:?} should parse: {e}"))
            .command
    }

    #[test]
    fn the_command_tree_is_well_formed() {
        Harness::command().debug_assert();
    }

    #[test]
    fn rm_takes_user_and_node() {
        match parse(&["rm", "CLAUDE_CODE_OAUTH_TOKEN", "--user", "--node", "box-b"]) {
            SecretsCommand::Rm { name, user, node, .. } => {
                assert_eq!(name, "CLAUDE_CODE_OAUTH_TOKEN");
                assert!(user);
                assert_eq!(node, Some("box-b".to_string()));
            }
            other => panic!("not an Rm: {other:?}"),
        }
    }

    #[test]
    fn set_takes_user_and_node() {
        match parse(&["set", "K", "v", "--user", "--node", "box-b"]) {
            SecretsCommand::Set { name, value, user, node, .. } => {
                assert_eq!(name, "K");
                assert_eq!(value, Some("v".to_string()));
                assert!(user);
                assert_eq!(node, Some("box-b".to_string()));
            }
            other => panic!("not a Set: {other:?}"),
        }
    }

    #[test]
    fn policy_takes_every_server_side_flag_and_user_node() {
        match parse(&[
            "policy",
            "--enable",
            "--overridable",
            "CLAUDE_CODE_OAUTH_TOKEN,ANTHROPIC_API_KEY",
            "--missing",
            "fail",
            "--max-lease",
            "72h",
        ]) {
            SecretsCommand::Policy {
                user,
                node,
                enable,
                disable,
                overridable,
                missing,
                max_lease,
                ..
            } => {
                assert!(!user);
                assert_eq!(node, None);
                assert!(enable);
                assert!(!disable);
                assert_eq!(
                    overridable,
                    Some(vec![
                        "CLAUDE_CODE_OAUTH_TOKEN".to_string(),
                        "ANTHROPIC_API_KEY".to_string()
                    ])
                );
                assert_eq!(missing, Some("fail".to_string()));
                assert_eq!(max_lease, Some("72h".to_string()));
            }
            other => panic!("not a Policy: {other:?}"),
        }

        match parse(&["policy", "--user", "--node", "box-b", "--missing", "wait"]) {
            SecretsCommand::Policy { user, node, missing, .. } => {
                assert!(user);
                assert_eq!(node, Some("box-b".to_string()));
                assert_eq!(missing, Some("wait".to_string()));
            }
            other => panic!("not a Policy: {other:?}"),
        }
    }

    #[test]
    fn leases_takes_an_optional_node() {
        match parse(&["leases"]) {
            SecretsCommand::Leases { node, json } => {
                assert!(node.is_none() && !json);
            }
            other => panic!("not a Leases: {other:?}"),
        }
        match parse(&["leases", "--node", "box-b", "--json"]) {
            SecretsCommand::Leases { node, json } => {
                assert_eq!(node, Some("box-b".to_string()));
                assert!(json);
            }
            other => panic!("not a Leases: {other:?}"),
        }
    }

    #[test]
    fn run_leases_without_node_refuses_before_any_network_call() {
        let err = run_leases(None, false).expect_err("refused");
        assert!(err.contains("--node"), "{err}");
    }

    #[test]
    fn require_node_refuses_user_with_no_node() {
        assert!(require_node(None).is_err());
        assert_eq!(require_node(Some("box-b")).unwrap(), "box-b");
    }

    #[test]
    fn missing_round_trips_through_its_wire_spelling() {
        for (text, missing) in [
            ("server", Missing::Server),
            ("wait", Missing::Wait),
            ("fail", Missing::Fail),
        ] {
            assert_eq!(parse_missing(text).unwrap(), missing);
            assert_eq!(missing_label(missing), text);
        }
        assert!(parse_missing("nonsense").is_err());
    }

    fn scratch(tag: &str) -> Secrets {
        let root = std::env::temp_dir().join(format!(
            "adi-cli-secrets-policy-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Secrets::with_config(adi_config::Config::with_root(root))
    }

    /// `secrets policy` with no edit flags only reads — the fresh store's defaults, since it is
    /// never written just by being asked about.
    #[test]
    fn policy_with_no_edit_flags_only_reads() {
        let store = scratch("read-only");
        run_policy(&store, false, None, false, false, None, None, None, true)
            .expect("read");
        assert!(!UserSecretsPolicy::load(store.config()).enabled);
    }

    /// The local admin path round-trips every field in one call, and a blank entry in
    /// `--overridable` is dropped the same way the HTTP route drops one.
    #[test]
    fn policy_local_edit_round_trips_every_field() {
        let store = scratch("edit-round-trip");
        run_policy(
            &store,
            false,
            None,
            true,
            false,
            Some(vec!["CLAUDE_CODE_OAUTH_TOKEN".to_string(), "  ".to_string()]),
            Some("fail".to_string()),
            Some("72h".to_string()),
            true,
        )
        .expect("edit");

        let policy = UserSecretsPolicy::load(store.config());
        assert!(policy.enabled);
        assert_eq!(policy.overridable, vec!["CLAUDE_CODE_OAUTH_TOKEN".to_string()]);
        assert_eq!(policy.missing, Missing::Fail);
        assert_eq!(policy.max_lease, Some("72h".to_string()));

        // `--disable` turns it back off without touching the rest.
        run_policy(&store, false, None, false, true, None, None, None, true)
            .expect("disable");
        let policy = UserSecretsPolicy::load(store.config());
        assert!(!policy.enabled);
        assert_eq!(policy.missing, Missing::Fail, "untouched by a disable-only call");
    }

    #[test]
    fn policy_refuses_enable_and_disable_together() {
        let store = scratch("enable-disable-conflict");
        assert!(
            run_policy(&store, false, None, true, true, None, None, None, false).is_err()
        );
    }

    #[test]
    fn policy_user_without_node_is_refused_before_any_network_call() {
        let store = scratch("user-no-node");
        let err = run_policy(&store, true, None, false, false, None, None, None, false)
            .expect_err("refused");
        assert!(err.contains("--node"), "{err}");
    }
}
