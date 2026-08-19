//! `/api/db/*` — the control panel's window onto the shared SQLite store: what databases exist,
//! what's in them, and a place to run SQL.
//!
//! Reading and writing are deliberately different endpoints with different *connections*, not one
//! endpoint that inspects the SQL: `/query` holds a read-only handle, so browsing cannot write no
//! matter what statement is pasted into it, while `/exec` is the explicit write path.

use adi_db::{Db, Error as DbStoreError};

use crate::types::{
    DbColumnDto, DbExecResult, DbInfoDto, DbQuery, DbQueryResult, DbSchema, DbScope, DbState,
    DbTableDto, DbTablesState,
};

use super::response::{FromBody, Response, clean, error, ok_json, require};

/// `GET /api/db` — every database in the store, global first, then each project's.
#[must_use]
pub fn db_state(store: &Db) -> Response {
    match store.list() {
        Ok(list) => ok_json(&DbState {
            databases: list
                .into_iter()
                .map(|d| DbInfoDto {
                    project: d.project,
                    path: d.path,
                    bytes: d.bytes,
                    tables: d.tables,
                })
                .collect(),
        }),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/db/tables` — a scope's tables and views, with columns and live row counts.
///
/// A scope nothing has written to yet reports an empty list rather than 404: on the panel that is
/// an empty database, not a missing one, and the query box below it still has to work.
#[must_use]
pub fn db_tables(store: &Db, body: &[u8]) -> Response {
    let scope = match require::<DbScope>(body) {
        Ok(scope) => scope,
        Err(bad) => return bad,
    };
    let project = clean(scope.project);
    match store.tables(project.as_deref()) {
        Ok(tables) => ok_json(&DbTablesState {
            project,
            tables: tables
                .into_iter()
                .map(|t| DbTableDto {
                    name: t.name,
                    kind: t.kind,
                    rows: t.rows,
                    columns: t
                        .columns
                        .into_iter()
                        .map(|c| DbColumnDto {
                            name: c.name,
                            decl_type: c.decl_type,
                            notnull: c.notnull,
                            pk: c.pk,
                        })
                        .collect(),
                })
                .collect(),
        }),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/db/schema` — the `create` statements for a scope, or just one table's.
#[must_use]
pub fn db_schema(store: &Db, body: &[u8]) -> Response {
    let scope = match require::<DbScope>(body) {
        Ok(scope) => scope,
        Err(bad) => return bad,
    };
    let project = clean(scope.project);
    let table = clean(scope.table);
    match store.schema(project.as_deref(), table.as_deref()) {
        Ok(schema) => ok_json(&DbSchema { project, schema }),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/db/query` — run a statement and return its rows, over a **read-only** connection.
///
/// The connection is the guarantee, not a scan of the SQL: an `update` pasted here is refused by
/// SQLite itself. That also means `insert … returning` belongs on `/exec`, not here.
#[must_use]
pub fn db_query(store: &Db, body: &[u8]) -> Response {
    let req = match require::<DbQuery>(body) {
        Ok(req) => req,
        Err(bad) => return bad,
    };
    let project = clean(req.project);
    let conn = match store.connect_readonly(project.as_deref()) {
        Ok(conn) => conn,
        Err(e) => return Response::from(&e),
    };
    match adi_db::query_on_json(&conn, &req.sql, &req.params) {
        Ok(result) => ok_json(&DbQueryResult {
            columns: result.columns,
            rows: result.rows,
        }),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/db/exec` — run a statement for its effect (DDL, insert/update/delete). With no
/// `params` the whole body runs as a batch, so a multi-statement migration lands in one call.
#[must_use]
pub fn db_exec(store: &Db, body: &[u8]) -> Response {
    let req = match require::<DbQuery>(body) {
        Ok(req) => req,
        Err(bad) => return bad,
    };
    let project = clean(req.project);
    match store.exec_json(project.as_deref(), &req.sql, &req.params) {
        Ok(result) => ok_json(&DbExecResult {
            changes: result.changes,
            last_insert_rowid: result.last_insert_rowid,
        }),
        Err(e) => Response::from(&e),
    }
}

// Map a store error to an HTTP status. A rejected statement is the *caller's* mistake — a typo in
// the SQL or a bad bind — so it is a 400 the panel can show inline, not a 500.
impl From<&DbStoreError> for Response {
    fn from(e: &DbStoreError) -> Self {
        let status = match e {
            DbStoreError::InvalidProject(_) | DbStoreError::Sqlite(_) => 400,
            DbStoreError::NotFound(_) => 404,
            DbStoreError::Config(_) | DbStoreError::Io(_) => 500,
        };
        error(status, &e.to_string())
    }
}

impl FromBody for DbScope {
    const EXPECTED: &'static str = "expected JSON body { \"project\"?: \"…\", \"table\"?: \"…\" }";

    // An empty body is the global scope — the panel's default view asks for exactly that.
    fn on_empty() -> Option<Self> {
        Some(Self::default())
    }
}

impl FromBody for DbQuery {
    const EXPECTED: &'static str =
        "expected JSON body { \"sql\": \"…\", \"project\"?: \"…\", \"params\"?: [\"…\"] }";

    fn is_complete(&self) -> bool {
        !self.sql.trim().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn temp_db() -> Db {
        let root = std::env::temp_dir().join(format!(
            "adi-webapp-api-db-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Db::with_config(adi_config::Config::with_root(root))
    }

    #[test]
    fn exec_then_query_round_trips_and_lists() {
        let store = temp_db();
        let Response { status, body } = db_exec(
            &store,
            br#"{"sql":"create table notes (id integer primary key, body text)"}"#,
        );
        assert_eq!(status, 200, "{body}");

        let Response { status, body } = db_exec(
            &store,
            br#"{"sql":"insert into notes (body) values (?1)","params":["hi"]}"#,
        );
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["changes"], 1);
        assert_eq!(v["last_insert_rowid"], 1);

        let Response { status, body } =
            db_query(&store, br#"{"sql":"select id, body from notes"}"#);
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["columns"][1], "body");
        assert_eq!(v["rows"][0][1], "hi");

        let Response { status, body } = db_state(&store);
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["databases"][0]["tables"], 1);
        assert_eq!(v["databases"][0]["project"], Value::Null);
    }

    #[test]
    fn the_query_endpoint_cannot_write() {
        // The point of the read-only connection: browsing can't mutate, whatever is pasted in.
        let store = temp_db();
        let _ = db_exec(&store, br#"{"sql":"create table t (x int)"}"#);
        let Response { status, body } = db_query(&store, br#"{"sql":"insert into t values (1)"}"#);
        assert_eq!(
            status, 400,
            "a write through /query must be refused: {body}"
        );

        let Response { body, .. } = db_query(&store, br#"{"sql":"select count(*) as n from t"}"#);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["rows"][0][0], 0, "the refused write must not have landed");
    }

    #[test]
    fn tables_of_an_untouched_scope_is_empty_not_missing() {
        let store = temp_db();
        let Response { status, body } = db_tables(&store, b"{}");
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["tables"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn scopes_are_separate_databases() {
        let store = temp_db();
        let _ = db_exec(&store, br#"{"sql":"create table g (x int)"}"#);
        let _ = db_exec(
            &store,
            br#"{"project":"acme","sql":"create table p (x int)"}"#,
        );

        let Response { body, .. } = db_tables(&store, br#"{"project":"acme"}"#);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["tables"][0]["name"], "p");
        assert_eq!(v["tables"].as_array().unwrap().len(), 1);

        let Response { body, .. } = db_state(&store);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["databases"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn json_params_bind_as_their_own_types() {
        // The contract that makes this API usable from a script: send a number as a number. A
        // `Vec<String>` params field would 400 on this body, and quoting `"007"` into an integer
        // column would be worse still.
        let store = temp_db();
        let _ = db_exec(
            &store,
            br#"{"sql":"create table t (id integer, tag text)"}"#,
        );
        let Response { status, body } = db_exec(
            &store,
            br#"{"sql":"insert into t values (?1, ?2)","params":[1,"007"]}"#,
        );
        assert_eq!(status, 200, "{body}");

        // The integer matched an integer column…
        let Response { body, .. } = db_query(
            &store,
            br#"{"sql":"select tag from t where id = ?1","params":[1]}"#,
        );
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["rows"][0][0], "007", "a zero-padded string stayed text");

        // …and null binds SQL NULL rather than the word.
        let Response { status, .. } = db_exec(
            &store,
            br#"{"sql":"insert into t values (?1, ?2)","params":[2,null]}"#,
        );
        assert_eq!(status, 200);
        let Response { body, .. } = db_query(
            &store,
            br#"{"sql":"select count(*) as n from t where tag is null"}"#,
        );
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["rows"][0][0], 1);
    }

    #[test]
    fn a_bad_statement_is_a_400_the_panel_can_show() {
        let store = temp_db();
        let _ = db_exec(&store, br#"{"sql":"create table t (x int)"}"#);
        let Response { status, body } = db_query(&store, br#"{"sql":"select * from nope"}"#);
        assert_eq!(status, 400);
        assert!(
            body.contains("nope"),
            "the SQLite message reaches the caller: {body}"
        );
    }

    #[test]
    fn an_unsafe_project_id_is_refused() {
        let store = temp_db();
        let Response { status, .. } = db_tables(&store, br#"{"project":"../escape"}"#);
        assert_eq!(status, 400);
    }

    #[test]
    fn a_blank_statement_is_refused() {
        let store = temp_db();
        assert_eq!(db_exec(&store, br#"{"sql":"   "}"#).status, 400);
        assert_eq!(db_query(&store, br#"{"sql":""}"#).status, 400);
    }

    #[test]
    fn schema_reports_the_create_statements() {
        let store = temp_db();
        let _ = db_exec(
            &store,
            br#"{"sql":"create table a (x int); create table b (y int);"}"#,
        );
        let Response { body, .. } = db_schema(&store, br#"{"table":"b"}"#);
        let v: Value = serde_json::from_str(&body).unwrap();
        let schema = v["schema"].as_str().unwrap().to_lowercase();
        assert!(schema.contains("create table b"), "{schema}");
        assert!(!schema.contains("create table a"), "{schema}");
    }
}
