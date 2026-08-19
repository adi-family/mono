//! The `db` command group: run SQL against the shared SQLite store, in the global scope or a
//! project's. This is the surface agents reach through the `adi-db` system tool, so it is built for
//! being *called by a program*: `--json` on every read, parameters bound rather than interpolated,
//! and SQL accepted on stdin when it's too long or too quoted to sit in argv.

use std::path::PathBuf;

use adi_core::{Adi, QueryResult, TableInfo};
use clap::Subcommand;

use crate::format::{print_json, resolve_scope, scope_label};

/// How wide a single column may render before it's truncated, so one long text value can't push
/// the rest of the table off the terminal. `--json` is always the untruncated view.
const MAX_CELL_WIDTH: usize = 60;

#[derive(Debug, Subcommand)]
pub(crate) enum DbCommand {
    /// Run a statement and print its rows (`select`, or anything with `returning`). SQL comes from
    /// the argument, or from stdin when it's omitted.
    Query {
        /// The SQL to run. Omit to read it from stdin.
        sql: Option<String>,
        /// Bind a value to the next `?` placeholder. Repeat, in order:
        /// `--param 42 --param hello`. Always prefer this over pasting values into the SQL.
        #[arg(long = "param", value_name = "VALUE")]
        params: Vec<String>,
        /// Operate on the global database (the default when `--project` is omitted).
        #[arg(long)]
        global: bool,
        /// Operate on this project's database.
        #[arg(long)]
        project: Option<String>,
        /// Emit rows as a JSON array of objects.
        #[arg(long)]
        json: bool,
    },
    /// Run a statement for its effect: insert/update/delete, or DDL. Without `--param` the whole
    /// input runs as a batch, so a multi-statement migration lands in one call.
    Exec {
        /// The SQL to run. Omit to read it from stdin.
        sql: Option<String>,
        /// Bind a value to the next `?` placeholder. Using any parameter means the SQL must be a
        /// single statement.
        #[arg(long = "param", value_name = "VALUE")]
        params: Vec<String>,
        #[arg(long)]
        global: bool,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List the tables and views in a scope, with their columns and row counts.
    Tables {
        #[arg(long)]
        global: bool,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Print the `create` statements for a scope — read this before querying a table someone else
    /// created.
    Schema {
        /// Narrow to one table or view. Omit for the whole schema.
        table: Option<String>,
        #[arg(long)]
        global: bool,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List every database that exists in the store — the global one and each project's.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Print a scope's database file path, and nothing else — for a script that wants to open it
    /// directly (`sqlite3 "$(adi-db path)"`).
    Path {
        #[arg(long)]
        global: bool,
        #[arg(long)]
        project: Option<String>,
    },
    /// Snapshot a scope into a standalone `.db` file — a consistent copy taken without stopping
    /// writers. Take one before a destructive migration.
    Backup {
        /// Where to write the snapshot. Must not already exist.
        dest: PathBuf,
        #[arg(long)]
        global: bool,
        #[arg(long)]
        project: Option<String>,
    },
    /// Re-seed the Bun client (`@adi/db`) into the store's `node_modules` and print its directory.
    /// The app does this at startup; this is the manual repair path.
    Client,
}

/// Dispatch a `db` subcommand over the adi-core facade, surfacing any store error as a `String`
/// (like the other command groups) so error families print uniformly.
#[allow(
    clippy::too_many_lines,
    reason = "one arm per subcommand, each short and flat"
)]
pub(crate) fn run_db(adi: Adi, command: DbCommand) -> Result<(), String> {
    let store = adi.db();
    match command {
        DbCommand::Query {
            sql,
            params,
            global,
            project,
            json,
        } => {
            let scope = resolve_scope(global, project)?;
            let sql = resolve_sql(sql)?;
            let result = store
                .query(scope.as_deref(), &sql, &params)
                .map_err(|e| e.to_string())?;
            if json {
                print_json(&result.rows_as_objects());
            } else {
                print_rows(&result);
            }
        }
        DbCommand::Exec {
            sql,
            params,
            global,
            project,
            json,
        } => {
            let scope = resolve_scope(global, project)?;
            let sql = resolve_sql(sql)?;
            let result = store
                .exec(scope.as_deref(), &sql, &params)
                .map_err(|e| e.to_string())?;
            if json {
                print_json(&result);
            } else {
                println!("{} row(s) changed.", result.changes);
                if result.changes > 0 && result.last_insert_rowid > 0 {
                    println!("Last insert rowid: {}.", result.last_insert_rowid);
                }
            }
        }
        DbCommand::Tables {
            global,
            project,
            json,
        } => {
            let scope = resolve_scope(global, project)?;
            let tables = store.tables(scope.as_deref()).map_err(|e| e.to_string())?;
            if json {
                print_json(&tables);
            } else if tables.is_empty() {
                println!(
                    "No tables in the {} database yet.",
                    scope_label(scope.as_deref())
                );
            } else {
                for table in &tables {
                    print_table_info(table);
                }
            }
        }
        DbCommand::Schema {
            table,
            global,
            project,
            json,
        } => {
            let scope = resolve_scope(global, project)?;
            let schema = store
                .schema(scope.as_deref(), table.as_deref())
                .map_err(|e| e.to_string())?;
            if json {
                print_json(&serde_json::json!({ "schema": schema }));
            } else if schema.is_empty() {
                println!(
                    "No schema yet in the {} database.",
                    scope_label(scope.as_deref())
                );
            } else {
                println!("{schema}");
            }
        }
        DbCommand::List { json } => {
            let databases = store.list().map_err(|e| e.to_string())?;
            if json {
                print_json(&databases);
            } else if databases.is_empty() {
                println!("No databases yet — the first write creates one.");
            } else {
                for db in &databases {
                    println!(
                        "{} — {} table(s), {} — {}",
                        scope_label(db.project.as_deref()),
                        db.tables,
                        adi_config::human_bytes(db.bytes),
                        db.path
                    );
                }
            }
        }
        DbCommand::Path { global, project } => {
            let scope = resolve_scope(global, project)?;
            let path = store.path(scope.as_deref()).map_err(|e| e.to_string())?;
            // The path alone, no decoration — safe to capture in a shell substitution.
            println!("{}", path.display());
        }
        DbCommand::Backup {
            dest,
            global,
            project,
        } => {
            let scope = resolve_scope(global, project)?;
            let written = store
                .backup(scope.as_deref(), &dest)
                .map_err(|e| e.to_string())?;
            println!(
                "Backed up the {} database to {}.",
                scope_label(scope.as_deref()),
                written.display()
            );
        }
        DbCommand::Client => {
            let dir = store.ensure_client().map_err(|e| e.to_string())?;
            println!("Seeded @adi/db at {}.", dir.display());
            println!(
                "Import it from any Bun file under the store: import {{ query, run }} from \"@adi/db\";"
            );
        }
    }
    Ok(())
}

/// The SQL to run: the argument, or all of stdin when it was omitted. Reading stdin is what makes
/// a heredoc'd migration or a heavily quoted statement possible without fighting argv escaping.
fn resolve_sql(sql: Option<String>) -> Result<String, String> {
    use std::io::Read as _;

    let text = if let Some(sql) = sql {
        sql
    } else {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("couldn't read SQL from stdin: {e}"))?;
        buf
    };
    if text.trim().is_empty() {
        return Err("no SQL given (pass it as an argument or on stdin)".to_string());
    }
    Ok(text)
}

/// Render a result set as an aligned table — the human view. Long cells are truncated so one wide
/// value can't wreck the layout; `--json` is the lossless view.
fn print_rows(result: &QueryResult) {
    if result.columns.is_empty() {
        println!("(no result set)");
        return;
    }

    let cells: Vec<Vec<String>> = result
        .rows
        .iter()
        .map(|row| row.iter().map(render_cell).collect())
        .collect();

    let widths: Vec<usize> = result
        .columns
        .iter()
        .enumerate()
        .map(|(index, name)| {
            cells
                .iter()
                .filter_map(|row| row.get(index))
                .map(|cell| cell.chars().count())
                .chain(std::iter::once(name.chars().count()))
                .max()
                .unwrap_or(0)
        })
        .collect();

    let header: Vec<String> = result
        .columns
        .iter()
        .zip(&widths)
        .map(|(name, width)| format!("{name:<width$}"))
        .collect();
    println!("{}", header.join("  ").trim_end());
    println!(
        "{}",
        widths
            .iter()
            .map(|w| "-".repeat(*w))
            .collect::<Vec<_>>()
            .join("  ")
    );

    for row in &cells {
        let line: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(cell, width)| format!("{cell:<width$}"))
            .collect();
        println!("{}", line.join("  ").trim_end());
    }
    println!("({} row(s))", result.rows.len());
}

/// One JSON value as a table cell: strings unquoted, `NULL` spelled out so it reads differently
/// from an empty string, and anything overlong ellipsized.
fn render_cell(value: &serde_json::Value) -> String {
    let raw = match value {
        serde_json::Value::Null => "NULL".to_string(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    // Newlines would break the row alignment, so flatten them into the cell.
    let flat = raw.replace(['\n', '\r'], " ");
    if flat.chars().count() > MAX_CELL_WIDTH {
        let kept: String = flat.chars().take(MAX_CELL_WIDTH - 1).collect();
        format!("{kept}…")
    } else {
        flat
    }
}

/// One table's shape, as the `tables` command prints it.
fn print_table_info(table: &TableInfo) {
    println!("{} [{}] — {} row(s)", table.name, table.kind, table.rows);
    for column in &table.columns {
        let mut flags = Vec::new();
        if column.pk {
            flags.push("pk");
        }
        if column.notnull {
            flags.push("not null");
        }
        let decl = if column.decl_type.is_empty() {
            String::new()
        } else {
            format!(" {}", column.decl_type)
        };
        let suffix = if flags.is_empty() {
            String::new()
        } else {
            format!(" ({})", flags.join(", "))
        };
        println!("  {}{decl}{suffix}", column.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_flags_resolve_and_conflict() {
        assert_eq!(resolve_scope(false, None).expect("global"), None);
        assert_eq!(resolve_scope(true, None).expect("explicit global"), None);
        assert_eq!(
            resolve_scope(false, Some("acme".into())).expect("project"),
            Some("acme".to_string())
        );
        assert!(
            resolve_scope(true, Some("acme".into())).is_err(),
            "both is a conflict"
        );
    }

    #[test]
    fn cells_render_null_distinctly_and_stay_within_the_width_cap() {
        assert_eq!(render_cell(&serde_json::Value::Null), "NULL");
        // An empty string must not look like NULL.
        assert_eq!(render_cell(&serde_json::json!("")), "");
        assert_eq!(render_cell(&serde_json::json!(42)), "42");
        assert_eq!(render_cell(&serde_json::json!("a\nb")), "a b");

        let long = render_cell(&serde_json::json!("x".repeat(200)));
        assert_eq!(long.chars().count(), MAX_CELL_WIDTH);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn sql_is_required_from_somewhere() {
        assert!(
            resolve_sql(Some("  ".to_string())).is_err(),
            "blank SQL is refused"
        );
        assert_eq!(
            resolve_sql(Some("select 1".into())).expect("sql"),
            "select 1"
        );
    }

    #[test]
    fn byte_sizes_pick_a_readable_unit() {
        assert_eq!(adi_config::human_bytes(512), "512 B");
        assert_eq!(adi_config::human_bytes(4096), "4.1 kB");
        assert_eq!(adi_config::human_bytes(5_500_000), "5.5 MB");
    }
}
