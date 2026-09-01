//! The `secrets` command group: set / get / list / remove encrypted secrets in the global or
//! a per-project scope. Values are stored encrypted; only `get --reveal` ever prints one.

use adi_core::{Adi, Secret};
use clap::Subcommand;

use crate::format::{print_json, resolve_scope, scope_label};

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
            json,
        } => {
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
        } => {
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
    }
    Ok(())
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
