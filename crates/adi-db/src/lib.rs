//! adi-db — the shared SQLite store for the adi platform. A pure library (no CLI, no daemon)
//! over the [`adi_config`] store, mirroring [`adi_secrets`](adi_config)'s scoping.
//!
//! One database file per scope, under `~/.adi/mono/db/`:
//!
//! * **global** — `db/global.db`, the database everything shares.
//! * **per-project** — `db/projects/<project-id>.db`, scoped to an `adi-projects` id.
//!
//! Scope is the platform-wide `project: Option<&str>` convention — `None` is global.
//!
//! # Many processes, one file
//!
//! This store is deliberately shared: several agents, their `sh`/`ts` tools, a dashboard backend,
//! and the control panel all reach the same file *at the same time*, from separate processes. Two
//! settings make that safe rather than merely likely to work, and every opener on both sides of
//! the platform applies them (see [`PRAGMAS`], and the seeded `@adi/db` Bun client):
//!
//! * `journal_mode = WAL` — readers never block the writer and the writer never blocks readers,
//!   which is the whole reason concurrent access holds up. It is a property of the *file*, so it
//!   survives across processes once set.
//! * `busy_timeout = 5000` — the remaining writer-vs-writer contention becomes a short wait
//!   instead of an instant `SQLITE_BUSY`.
//!
//! ```
//! # let tmp = std::env::temp_dir().join(format!("adi-db-doctest-{}", std::process::id()));
//! # let _ = std::fs::remove_dir_all(&tmp);
//! use adi_db::Db;
//!
//! # let store = Db::with_config(adi_config::Config::with_root(&tmp));
//! // In real code: let store = Db::open();
//! store.exec(None, "create table notes (id integer primary key, body text)", &[])?;
//! store.exec(None, "insert into notes (body) values (?1)", &["hello".to_string()])?;
//!
//! let found = store.query(None, "select body from notes where id = ?1", &["1".to_string()])?;
//! assert_eq!(found.rows[0][0], serde_json::json!("hello"));
//! assert_eq!(store.tables(None)?[0].name, "notes");
//! # std::fs::remove_dir_all(&tmp).ok();
//! # Ok::<(), adi_db::Error>(())
//! ```

mod error;
mod value;

use std::path::{Path, PathBuf};
use std::time::Duration;

use adi_config::Config;
use rusqlite::{Connection, OpenFlags};

pub use error::{Error, Result};
pub use value::{ColumnInfo, DbInfo, ExecResult, QueryResult, TableInfo};

/// The subdirectory per-project databases live in, and the extension they all carry — needed here
/// only to *enumerate* them in [`Db::list`]. Resolving a single scope's path is
/// [`Config::db_path`]'s job, so the layout has one owner.
const PROJECTS_DIR: &str = "projects";
const DB_EXT: &str = "db";

/// Where the Bun client is seeded: the mono store's own `node_modules`. Every `.ts` the platform
/// runs (a tool's `script.ts`, a dashboard backend, a `ts` trigger) lives *under* the store root,
/// so Bun's upward module resolution finds `@adi/db` from all of them with no install step.
const CLIENT_DIR: &[&str] = &["node_modules", "@adi", "db"];

/// How long a blocked writer waits for the lock before giving up. Long enough to absorb another
/// agent's write, short enough that a genuinely stuck database still surfaces as an error.
const BUSY_TIMEOUT: Duration = Duration::from_millis(5000);

/// The pragmas applied to every read-write connection, here and in the Bun client — see the
/// crate docs for why WAL and `busy_timeout` are what make cross-process sharing safe.
///
/// **`busy_timeout` comes first, and the order is load-bearing.** Setting `journal_mode` takes a
/// lock on the database, so on a connection with no timeout yet it fails outright the moment
/// another process is mid-write or recovering a WAL (`SQLITE_BUSY_RECOVERY`) — a cold open under
/// concurrency dies before it has done anything. With the timeout already in effect, that same
/// statement waits its turn instead.
///
/// `foreign_keys` is per-connection and off by default in SQLite, so declared references would
/// silently not be enforced without it. `synchronous = NORMAL` is the documented companion to WAL:
/// durable across process crashes, trading only the last commits in an OS-level crash for far less
/// fsync traffic.
pub const PRAGMAS: &str = "\
    pragma busy_timeout = 5000;\n\
    pragma journal_mode = WAL;\n\
    pragma foreign_keys = ON;\n\
    pragma synchronous = NORMAL;\n";

/// The Bun client's two files, embedded so a build always seeds the version that matches it.
const CLIENT_FILES: &[(&str, &str)] = &[
    (
        "package.json",
        include_str!("../templates/client/package.json"),
    ),
    ("index.ts", include_str!("../templates/client/index.ts")),
];

/// The database store: resolves, opens, and queries the per-scope SQLite files under the `db`
/// module dir. Cheap to clone; all state is on disk.
#[derive(Debug, Clone)]
pub struct Db {
    config: Config,
}

impl Default for Db {
    fn default() -> Self {
        Self::open()
    }
}

impl Db {
    /// Open the store backed by the standard config (`~/.adi/mono`, honoring `$ADI_DIR`).
    #[must_use]
    pub fn open() -> Self {
        Self {
            config: Config::open(),
        }
    }

    /// Open the store backed by a caller-supplied [`Config`] — for tests or alternate installs.
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self { config }
    }

    /// The config this store reads from.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The `db` directory: `~/.adi/mono/db`.
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.config.db_dir()
    }

    /// Where a scope's database file lives — `db/global.db`, or `db/projects/<id>.db`. Returns the
    /// path even if nothing has created it yet.
    ///
    /// # Errors
    /// [`Error::InvalidProject`] for an unsafe project id — the security boundary before the id is
    /// joined onto the store path.
    pub fn path(&self, project: Option<&str>) -> Result<PathBuf> {
        self.config
            .db_path(project)
            .ok_or_else(|| Error::InvalidProject(project.unwrap_or_default().to_string()))
    }

    /// Open a read-write connection to a scope, creating the file (and `db/`) if this is the first
    /// touch, and applying [`PRAGMAS`].
    ///
    /// # Errors
    /// [`Error::InvalidProject`] for an unsafe project id, [`Error::Io`] if `db/` can't be created,
    /// or [`Error::Sqlite`] if SQLite refuses the file.
    pub fn connect(&self, project: Option<&str>) -> Result<Connection> {
        let path = self.path(project)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        conn.execute_batch(PRAGMAS)?;
        Ok(conn)
    }

    /// Open a **read-only** connection to a scope — the safe path for browsing (the control panel's
    /// data view), where the connection itself, not a guess at what the SQL does, is what prevents
    /// a write.
    ///
    /// # Errors
    /// [`Error::InvalidProject`] for an unsafe project id, [`Error::NotFound`] when the database
    /// doesn't exist (read-only never creates one), or [`Error::Sqlite`] on an open failure.
    pub fn connect_readonly(&self, project: Option<&str>) -> Result<Connection> {
        let path = self.path(project)?;
        if !path.exists() {
            return Err(Error::NotFound(path.display().to_string()));
        }
        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Ok(conn)
    }

    /// Run a statement and collect its result set — `SELECT`, or anything else with `RETURNING`.
    ///
    /// `params` bind to the statement's `?` placeholders in order. Each is coerced from its argv
    /// text to the SQLite type it plainly denotes, but only when that conversion round-trips
    /// exactly, so `5` binds as an integer while `007` stays the text it was.
    ///
    /// # Errors
    /// [`Error::InvalidProject`] for an unsafe project id, or [`Error::Sqlite`] when SQLite rejects
    /// the statement.
    pub fn query(
        &self,
        project: Option<&str>,
        sql: &str,
        params: &[String],
    ) -> Result<QueryResult> {
        let conn = self.connect(project)?;
        query_on(&conn, sql, params)
    }

    /// Run a statement for its effect: `INSERT`/`UPDATE`/`DELETE`, or DDL.
    ///
    /// With no `params` the whole string runs as a batch, so a multi-statement migration or a
    /// `create table …; create index …;` pair lands in one call. With `params`, it must be a single
    /// statement — that is SQLite's own rule for a parameterized prepare.
    ///
    /// # Errors
    /// [`Error::InvalidProject`] for an unsafe project id, or [`Error::Sqlite`] when SQLite rejects
    /// a statement (in a batch, everything before the failure has already run).
    pub fn exec(&self, project: Option<&str>, sql: &str, params: &[String]) -> Result<ExecResult> {
        let bound = params.iter().map(|p| value::to_sql(p)).collect();
        self.exec_bound(project, sql, bound)
    }

    /// [`exec`](Self::exec), for a caller whose parameters are already typed — the JSON API path.
    /// Each value binds as the SQLite type it *is*, with none of the argv-text guessing
    /// [`exec`](Self::exec) has to do.
    ///
    /// # Errors
    /// Identical to [`exec`](Self::exec).
    pub fn exec_json(
        &self,
        project: Option<&str>,
        sql: &str,
        params: &[serde_json::Value],
    ) -> Result<ExecResult> {
        let bound = params.iter().map(value::to_sql_json).collect();
        self.exec_bound(project, sql, bound)
    }

    /// The shared body of [`exec`](Self::exec) and [`exec_json`](Self::exec_json), once the
    /// parameters are bound — the two differ only in how they got there.
    fn exec_bound(
        &self,
        project: Option<&str>,
        sql: &str,
        bound: Vec<rusqlite::types::Value>,
    ) -> Result<ExecResult> {
        let conn = self.connect(project)?;
        if bound.is_empty() {
            conn.execute_batch(sql)?;
        } else {
            conn.execute(sql, rusqlite::params_from_iter(bound))?;
        }
        Ok(ExecResult {
            changes: conn.changes(),
            last_insert_rowid: conn.last_insert_rowid(),
        })
    }

    /// Every table and view in a scope, with its columns and live row count. SQLite's own
    /// `sqlite_*` tables are internal bookkeeping, so they're left out.
    ///
    /// # Errors
    /// [`Error::InvalidProject`] for an unsafe project id, or [`Error::Sqlite`] on a read failure.
    pub fn tables(&self, project: Option<&str>) -> Result<Vec<TableInfo>> {
        let conn = self.connect(project)?;
        tables_on(&conn)
    }

    /// The `CREATE` statements for a scope — the whole schema, or just `table`'s. This is what an
    /// agent reads before writing a query against a table someone else created.
    ///
    /// # Errors
    /// [`Error::InvalidProject`] for an unsafe project id, or [`Error::Sqlite`] on a read failure.
    pub fn schema(&self, project: Option<&str>, table: Option<&str>) -> Result<String> {
        let conn = self.connect(project)?;
        let mut stmt = conn.prepare(
            "select sql from sqlite_master
              where sql is not null and name not like 'sqlite_%'
                and (?1 is null or name = ?1)
              order by case type when 'table' then 0 when 'view' then 1 else 2 end, name",
        )?;
        let statements: Vec<String> = stmt
            .query_map([table], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(statements
            .iter()
            .map(|s| format!("{};", s.trim_end_matches(';')))
            .collect::<Vec<_>>()
            .join("\n\n"))
    }

    /// Every database that exists in the store — the global one, then each project's, sorted by id.
    /// A scope nothing has written to yet has no file and is simply absent.
    ///
    /// # Errors
    /// [`Error::Io`] if the `db/projects` directory can't be listed.
    pub fn list(&self) -> Result<Vec<DbInfo>> {
        let mut out = Vec::new();
        let global = self.path(None)?;
        if global.exists() {
            out.push(self.describe(None, &global));
        }

        let projects_dir = self.dir().join(PROJECTS_DIR);
        let mut ids: Vec<String> = match std::fs::read_dir(&projects_dir) {
            Ok(entries) => entries
                .filter_map(std::result::Result::ok)
                .filter_map(|e| {
                    let name = e.file_name().into_string().ok()?;
                    name.strip_suffix(&format!(".{DB_EXT}")).map(String::from)
                })
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(Error::Io(e)),
        };
        ids.sort();
        for id in ids {
            let path = projects_dir.join(format!("{id}.{DB_EXT}"));
            out.push(self.describe(Some(id), &path));
        }
        Ok(out)
    }

    /// Snapshot a scope into a standalone `.db` file at `dest`, via `VACUUM INTO` — a consistent
    /// copy taken without stopping writers, and compacted on the way out. Refuses to overwrite.
    ///
    /// # Errors
    /// [`Error::InvalidProject`] for an unsafe project id, [`Error::Io`] if `dest` already exists,
    /// or [`Error::Sqlite`] if the snapshot fails.
    pub fn backup(&self, project: Option<&str>, dest: &Path) -> Result<PathBuf> {
        if dest.exists() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{} already exists", dest.display()),
            )));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = self.connect(project)?;
        conn.execute("vacuum into ?1", [dest.to_string_lossy().as_ref()])?;
        Ok(dest.to_path_buf())
    }

    /// Follow a project rename into this store: move `db/projects/<from>.db` to
    /// `db/projects/<to>.db`, with the WAL and shared-memory sidecars SQLite may have left beside
    /// it. Returns `false` when the project never had a database — the common case, and not a
    /// failure.
    ///
    /// A plain file move, deliberately: nothing inside a SQLite file names its own path, and the
    /// scope a database belongs to is which file it is. Refuses to overwrite an existing database
    /// at `to` rather than merge or clobber two projects' data.
    ///
    /// # Errors
    /// [`Error::InvalidProject`] for an unsafe id on either side, or [`Error::Io`] if `to` already
    /// has a database or the move fails.
    pub fn rename_project(&self, from: &str, to: &str) -> Result<bool> {
        let source = self.path(Some(from))?;
        let target = self.path(Some(to))?;
        if from == to || !source.exists() {
            return Ok(false);
        }
        if target.exists() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{} already exists", target.display()),
            )));
        }
        std::fs::rename(&source, &target)?;
        // The sidecars only exist while a connection is (or was) open; each is named after the
        // database file, so each has to follow it or SQLite would read a stale journal.
        for suffix in ["-wal", "-shm"] {
            let sidecar = sibling(&source, suffix);
            if sidecar.exists() {
                std::fs::rename(sidecar, sibling(&target, suffix))?;
            }
        }
        Ok(true)
    }

    /// The environment a run gets so its `ts` code and shell find the right database without being
    /// told — see [`Config::db_env`], which the launching crates call directly (they need the two
    /// variables, not a SQLite dependency).
    #[must_use]
    pub fn env(&self, project: Option<&str>) -> Vec<(String, String)> {
        self.config.db_env(project)
    }

    /// Seed the Bun client into the store's `node_modules/@adi/db`, so `import … from "@adi/db"`
    /// resolves from every `.ts` the platform runs. Rewrites a file only when its content differs,
    /// so an upgraded build ships its matching client while an unchanged one touches no disk.
    /// Returns the package directory.
    ///
    /// # Errors
    /// [`Error::Io`] if the package directory or its files can't be written.
    pub fn ensure_client(&self) -> Result<PathBuf> {
        let dir = CLIENT_DIR
            .iter()
            .fold(self.config.root().to_path_buf(), |path, seg| path.join(seg));
        std::fs::create_dir_all(&dir)?;
        for (name, body) in CLIENT_FILES {
            let path = dir.join(name);
            if std::fs::read_to_string(&path).is_ok_and(|current| current == *body) {
                continue;
            }
            std::fs::write(&path, body)?;
        }
        Ok(dir)
    }

    /// Bring the store up: create the global database (with [`PRAGMAS`] applied, so WAL is set on
    /// the file from the start) and seed the Bun client. Idempotent — the platform calls this on
    /// every app start.
    ///
    /// # Errors
    /// [`Error::Sqlite`] if the global database can't be created, or [`Error::Io`] if the client
    /// can't be seeded.
    pub fn bootstrap(&self) -> Result<()> {
        self.connect(None)?;
        self.ensure_client()?;
        Ok(())
    }

    /// Describe one on-disk database for [`list`](Self::list). Size and table count are best-effort:
    /// a file being written concurrently, or one that isn't a database at all, reports zeros rather
    /// than failing the whole listing.
    fn describe(&self, project: Option<String>, path: &Path) -> DbInfo {
        let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let tables = self
            .connect_readonly(project.as_deref())
            .and_then(|conn| tables_on(&conn))
            .map(|t| t.len())
            .unwrap_or(0);
        DbInfo {
            project,
            path: path.display().to_string(),
            bytes,
            tables,
        }
    }
}

/// Run `sql` on an existing connection and collect the result set — the shared body behind
/// [`Db::query`] and any caller holding its own [`Connection`] (a read-only one, say).
///
/// # Errors
/// [`Error::Sqlite`] when SQLite rejects the statement or a row can't be read.
pub fn query_on(conn: &Connection, sql: &str, params: &[String]) -> Result<QueryResult> {
    query_bound(conn, sql, params.iter().map(|p| value::to_sql(p)).collect())
}

/// [`query_on`], for a caller whose parameters are already typed — the JSON API path. Each value
/// binds as the SQLite type it *is*, with none of the argv-text guessing [`query_on`] must do.
///
/// # Errors
/// Identical to [`query_on`].
pub fn query_on_json(
    conn: &Connection,
    sql: &str,
    params: &[serde_json::Value],
) -> Result<QueryResult> {
    query_bound(conn, sql, params.iter().map(value::to_sql_json).collect())
}

/// A database file's sidecar path — `<db>-wal`, `<db>-shm`. SQLite names them by appending to the
/// whole file name (extension included), which is why this is a suffix on the file name rather
/// than a change of extension.
fn sibling(db: &Path, suffix: &str) -> PathBuf {
    let mut name = db.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// The shared body of [`query_on`] and [`query_on_json`], once the parameters are bound.
fn query_bound(
    conn: &Connection,
    sql: &str,
    bound: Vec<rusqlite::types::Value>,
) -> Result<QueryResult> {
    let mut stmt = conn.prepare(sql)?;
    let columns: Vec<String> = stmt.column_names().into_iter().map(String::from).collect();

    let mut collected = Vec::new();
    let mut rows = stmt.query(rusqlite::params_from_iter(bound))?;
    while let Some(row) = rows.next()? {
        let mut cells = Vec::with_capacity(columns.len());
        for index in 0..columns.len() {
            cells.push(value::to_json(row.get_ref(index)?));
        }
        collected.push(cells);
    }
    Ok(QueryResult {
        columns,
        rows: collected,
    })
}

/// List the tables and views on an existing connection — the shared body behind [`Db::tables`].
///
/// # Errors
/// [`Error::Sqlite`] on a read failure.
pub fn tables_on(conn: &Connection) -> Result<Vec<TableInfo>> {
    let mut stmt = conn.prepare(
        "select name, type from sqlite_master
          where type in ('table', 'view') and name not like 'sqlite_%'
          order by name",
    )?;
    let entries: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    drop(stmt);

    let mut out = Vec::with_capacity(entries.len());
    for (name, kind) in entries {
        let mut columns_stmt =
            conn.prepare("select name, type, \"notnull\", pk from pragma_table_info(?1)")?;
        let columns: Vec<ColumnInfo> = columns_stmt
            .query_map([&name], |row| {
                Ok(ColumnInfo {
                    name: row.get(0)?,
                    decl_type: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    notnull: row.get::<_, i64>(2)? != 0,
                    pk: row.get::<_, i64>(3)? != 0,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        drop(columns_stmt);

        // A name out of sqlite_master can't be bound as a parameter, so it is quoted as an
        // identifier — doubling any embedded quote, the SQL-standard escape.
        let quoted = name.replace('"', "\"\"");
        let rows = conn
            .query_row(&format!("select count(*) from \"{quoted}\""), [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap_or(0);

        out.push(TableInfo {
            name,
            kind,
            rows,
            columns,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Db {
        let root = std::env::temp_dir().join(format!(
            "adi-db-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Db::with_config(Config::with_root(root))
    }

    #[test]
    fn scopes_resolve_to_their_own_files() {
        let store = scratch("paths");
        assert!(store.path(None).expect("global").ends_with("db/global.db"));
        assert!(
            store
                .path(Some("acme"))
                .expect("project")
                .ends_with("db/projects/acme.db")
        );
    }

    #[test]
    fn rename_project_moves_the_database_and_its_sidecars() {
        let store = scratch("rename-project");
        store
            .exec(Some("old"), "create table t (body text)", &[])
            .expect("ddl");
        store
            .exec(Some("old"), "insert into t values ('kept')", &[])
            .expect("insert");
        assert!(store.path(Some("old")).expect("path").exists());

        assert!(store.rename_project("old", "new").expect("rename"));
        assert!(!store.path(Some("old")).expect("old path").exists());
        let rows = store
            .query(Some("new"), "select body from t", &[])
            .expect("query");
        assert_eq!(rows.rows.len(), 1);
        assert_eq!(rows.rows[0][0], serde_json::json!("kept"));
        // No `-wal` left orphaned beside the file that moved away.
        assert!(!sibling(&store.path(Some("old")).expect("old path"), "-wal").exists());

        // A project with no database of its own is a no-op, not a failure.
        assert!(!store.rename_project("never-used", "elsewhere").expect("no db"));
        // And a rename onto an occupied id refuses rather than clobbering it.
        store
            .exec(Some("other"), "create table t (body text)", &[])
            .expect("other");
        assert!(store.rename_project("new", "other").is_err());
    }

    #[test]
    fn an_unsafe_project_id_never_touches_disk() {
        let store = scratch("badproject");
        for id in ["../escape", "..", "a/b", "with space", ""] {
            assert!(
                matches!(store.path(Some(id)), Err(Error::InvalidProject(_))),
                "{id:?} should be rejected"
            );
        }
    }

    #[test]
    fn exec_then_query_round_trips_through_the_global_database() {
        let store = scratch("roundtrip");
        store
            .exec(
                None,
                "create table t (id integer primary key, body text)",
                &[],
            )
            .expect("ddl");
        let inserted = store
            .exec(
                None,
                "insert into t (body) values (?1)",
                &["hi".to_string()],
            )
            .expect("insert");
        assert_eq!(inserted.changes, 1);
        assert_eq!(inserted.last_insert_rowid, 1);

        let found = store
            .query(None, "select id, body from t", &[])
            .expect("select");
        assert_eq!(found.columns, vec!["id", "body"]);
        assert_eq!(
            found.rows,
            vec![vec![serde_json::json!(1), serde_json::json!("hi")]]
        );
        assert_eq!(
            found.rows_as_objects(),
            vec![serde_json::json!({ "id": 1, "body": "hi" })]
        );
    }

    #[test]
    fn a_numeric_param_binds_as_a_number_not_text() {
        // The regression this guards: SQLite compares TEXT '1' and INTEGER 1 as unequal, so binding
        // every argv param as text would make `where id = ?` silently match nothing.
        let store = scratch("params");
        store
            .exec(None, "create table t (id integer, tag text)", &[])
            .expect("ddl");
        store
            .exec(None, "insert into t values (1, '007')", &[])
            .expect("insert");

        let hit = store
            .query(None, "select tag from t where id = ?1", &["1".to_string()])
            .expect("query");
        assert_eq!(
            hit.rows.len(),
            1,
            "an integer param must match an integer column"
        );

        // And the converse: a zero-padded string stays text, so it still matches a text column.
        let padded = store
            .query(
                None,
                "select id from t where tag = ?1",
                &["007".to_string()],
            )
            .expect("query");
        assert_eq!(padded.rows.len(), 1, "'007' must stay text");
    }

    #[test]
    fn exec_without_params_runs_a_whole_batch() {
        let store = scratch("batch");
        store
            .exec(
                None,
                "create table a (x int); create table b (y int); insert into b values (1);",
                &[],
            )
            .expect("batch");
        let names: Vec<String> = store
            .tables(None)
            .expect("tables")
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn tables_report_columns_and_row_counts() {
        let store = scratch("tables");
        store
            .exec(
                None,
                "create table notes (id integer primary key, body text not null);
                 insert into notes (body) values ('one'), ('two');
                 create view recent as select * from notes;",
                &[],
            )
            .expect("setup");

        let tables = store.tables(None).expect("tables");
        let notes = tables.iter().find(|t| t.name == "notes").expect("notes");
        assert_eq!(notes.kind, "table");
        assert_eq!(notes.rows, 2);
        assert_eq!(notes.columns.len(), 2);
        assert!(notes.columns[0].pk, "id is the primary key");
        assert_eq!(notes.columns[1].decl_type, "TEXT");
        assert!(notes.columns[1].notnull);

        let view = tables.iter().find(|t| t.name == "recent").expect("view");
        assert_eq!(view.kind, "view");
        assert_eq!(view.rows, 2);
    }

    #[test]
    fn schema_prints_create_statements_and_can_narrow_to_one_table() {
        let store = scratch("schema");
        store
            .exec(None, "create table a (x int); create table b (y int);", &[])
            .expect("ddl");
        // SQLite stores the statement with its `CREATE TABLE` prefix normalized to uppercase,
        // whatever the caller typed — so compare case-insensitively.
        let all = store.schema(None, None).expect("schema").to_lowercase();
        assert!(all.contains("create table a"), "got {all}");
        assert!(all.contains("create table b"), "got {all}");

        let one = store.schema(None, Some("b")).expect("one").to_lowercase();
        assert!(one.contains("create table b"), "got {one}");
        assert!(!one.contains("create table a"), "got {one}");
    }

    #[test]
    fn scopes_are_isolated_and_listed_once_they_exist() {
        let store = scratch("scopes");
        assert!(
            store.list().expect("empty").is_empty(),
            "nothing exists yet"
        );

        store
            .exec(None, "create table g (x int)", &[])
            .expect("global");
        store
            .exec(Some("acme"), "create table p (x int)", &[])
            .expect("project");

        // A table in one scope is invisible in the other.
        assert_eq!(store.tables(None).expect("global").len(), 1);
        assert_eq!(store.tables(Some("acme")).expect("acme")[0].name, "p");

        let listed = store.list().expect("list");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].project, None);
        assert_eq!(listed[0].tables, 1);
        assert!(listed[0].bytes > 0);
        assert_eq!(listed[1].project.as_deref(), Some("acme"));
    }

    #[test]
    fn busy_timeout_is_set_before_journal_mode() {
        // Not cosmetic: `journal_mode` takes a lock, so with no timeout in effect yet it fails
        // outright when another process is mid-write or recovering a WAL. Reordering these two
        // lines makes a cold open under concurrency die with SQLITE_BUSY_RECOVERY — which only
        // ever shows up under real contention, so guard it here.
        let timeout = PRAGMAS.find("busy_timeout").expect("busy_timeout pragma");
        let journal = PRAGMAS.find("journal_mode").expect("journal_mode pragma");
        assert!(
            timeout < journal,
            "busy_timeout must be set before journal_mode"
        );
    }

    #[test]
    fn wal_is_enabled_on_the_file() {
        // The load-bearing setting for cross-process sharing; assert it rather than trust it.
        let store = scratch("wal");
        let conn = store.connect(None).expect("connect");
        let mode: String = conn
            .query_row("pragma journal_mode", [], |row| row.get(0))
            .expect("journal_mode");
        assert_eq!(mode, "wal");
    }

    #[test]
    fn a_readonly_connection_refuses_writes() {
        let store = scratch("readonly");
        store
            .exec(None, "create table t (x int)", &[])
            .expect("ddl");
        let conn = store.connect_readonly(None).expect("readonly");
        assert!(
            conn.execute("insert into t values (1)", []).is_err(),
            "a read-only connection must refuse a write"
        );
        // Reads still work through it.
        assert!(
            query_on(&conn, "select * from t", &[])
                .expect("read")
                .is_empty()
        );
    }

    #[test]
    fn a_readonly_connection_to_a_missing_database_is_not_found() {
        let store = scratch("readonlymissing");
        assert!(matches!(
            store.connect_readonly(None),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn backup_snapshots_the_scope_and_refuses_to_overwrite() {
        let store = scratch("backup");
        store
            .exec(
                None,
                "create table t (x int); insert into t values (42);",
                &[],
            )
            .expect("setup");
        let dest = store.dir().join("snapshots").join("global.db");
        store.backup(None, &dest).expect("backup");

        // The snapshot is a standalone database: openable on its own, with the data in it.
        let conn = Connection::open(&dest).expect("open copy");
        let rows = query_on(&conn, "select x from t", &[]).expect("read copy");
        assert_eq!(rows.rows, vec![vec![serde_json::json!(42)]]);

        assert!(store.backup(None, &dest).is_err(), "must not overwrite");
    }

    #[test]
    fn env_points_at_the_scope_and_degrades_to_global() {
        let store = scratch("env");
        let global = store.env(None);
        assert_eq!(global[0].0, "ADI_DB");
        assert!(global[0].1.ends_with("global.db"));
        assert!(store.env(Some("acme"))[0].1.ends_with("acme.db"));
        // A launch must never be blocked by a bad id — it falls back to global.
        assert!(store.env(Some("../evil"))[0].1.ends_with("global.db"));
    }

    #[test]
    fn bootstrap_creates_the_global_database_and_seeds_the_bun_client() {
        let store = scratch("bootstrap");
        store.bootstrap().expect("bootstrap");
        assert!(store.path(None).expect("path").exists());

        let client = store.config.root().join("node_modules/@adi/db");
        assert!(client.join("package.json").exists());
        let index = std::fs::read_to_string(client.join("index.ts")).expect("index.ts");
        assert!(
            index.contains("bun:sqlite"),
            "the client uses bun's built-in sqlite"
        );

        // Idempotent, and it repairs a client someone edited or truncated.
        std::fs::write(client.join("index.ts"), "tampered").expect("tamper");
        store.bootstrap().expect("rerun");
        assert!(
            std::fs::read_to_string(client.join("index.ts"))
                .expect("reread")
                .contains("bun:sqlite")
        );
    }
}
