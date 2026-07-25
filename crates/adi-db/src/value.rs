//! The wire shapes a caller sees — query rows, exec counts, schema listings — and the two
//! conversions between SQLite's dynamic types and JSON that produce them.
//!
//! Everything here is `Serialize`, because the same values are rendered three ways: a table in the
//! terminal, `--json` for an agent to pipe into `jq`, and (later) the control panel's data browser.

use serde::Serialize;

/// The result of a `SELECT` (or any statement with a result set, e.g. `INSERT … RETURNING`).
///
/// Columns are kept beside the rows rather than folded into per-row objects so a renderer can lay
/// out a table without re-deriving the header, and so a query returning zero rows still reports
/// what it *would* have returned. [`rows_as_objects`](Self::rows_as_objects) does the fold for
/// JSON output.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueryResult {
    /// The result columns, in statement order.
    pub columns: Vec<String>,
    /// One entry per row, each aligned to [`columns`](Self::columns).
    pub rows: Vec<Vec<serde_json::Value>>,
}

impl QueryResult {
    /// The rows folded into `{column: value}` objects — the shape `--json` emits, because it is
    /// what `jq` and a TypeScript caller expect.
    #[must_use]
    pub fn rows_as_objects(&self) -> Vec<serde_json::Value> {
        self.rows
            .iter()
            .map(|row| {
                let mut obj = serde_json::Map::with_capacity(self.columns.len());
                for (name, value) in self.columns.iter().zip(row) {
                    obj.insert(name.clone(), value.clone());
                }
                serde_json::Value::Object(obj)
            })
            .collect()
    }

    /// How many rows came back.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the statement produced no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// The result of a statement run for its effect (`INSERT`/`UPDATE`/`DELETE`/DDL).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ExecResult {
    /// Rows changed by the statement — `0` for DDL, and for a multi-statement batch this is the
    /// count from the *last* statement, which is what SQLite itself reports.
    pub changes: u64,
    /// The rowid of the most recent successful insert on this connection, or `0` if there was none.
    pub last_insert_rowid: i64,
}

/// One column of a table or view, as `PRAGMA table_info` describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ColumnInfo {
    /// The column name.
    pub name: String,
    /// Its declared type (`TEXT`, `INTEGER`, …) — empty when the column was declared without one,
    /// which SQLite permits.
    pub decl_type: String,
    /// Whether the column is `NOT NULL`.
    pub notnull: bool,
    /// Whether the column is part of the primary key.
    pub pk: bool,
}

/// One table or view in a database, with its shape and current row count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TableInfo {
    /// The table or view name.
    pub name: String,
    /// `"table"` or `"view"`.
    pub kind: String,
    /// Live row count. Views are counted too, so this can be the cost of running the view.
    pub rows: i64,
    /// The columns, in declaration order.
    pub columns: Vec<ColumnInfo>,
}

/// One database in the store — the global one, or a project's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DbInfo {
    /// The project this database belongs to, or `None` for the global database.
    pub project: Option<String>,
    /// Its absolute path on disk.
    pub path: String,
    /// The size of the main database file in bytes (excluding the `-wal`/`-shm` sidecars).
    pub bytes: u64,
    /// How many tables and views it holds.
    pub tables: usize,
}

/// Convert one SQLite value into JSON.
///
/// Integers and reals stay numeric, `NULL` becomes `null`, and text passes through. A BLOB has no
/// JSON equivalent, so it is base64-encoded into a string — lossless, and the one representation a
/// caller can decode again.
pub(crate) fn to_json(value: rusqlite::types::ValueRef<'_>) -> serde_json::Value {
    use base64::Engine as _;
    use rusqlite::types::ValueRef;

    match value {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Integer(i) => serde_json::Value::from(i),
        ValueRef::Real(f) => serde_json::Number::from_f64(f)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        ValueRef::Text(bytes) => {
            serde_json::Value::from(String::from_utf8_lossy(bytes).into_owned())
        }
        ValueRef::Blob(bytes) => {
            serde_json::Value::from(base64::engine::general_purpose::STANDARD.encode(bytes))
        }
    }
}

/// Bind one JSON value to the SQLite type it already is.
///
/// This is the API path's binder, and unlike [`to_sql`] it needs no guessing: a caller posting
/// JSON has *types*, so a number arrives as a number and the string `"007"` stays exactly that.
/// Booleans become `1`/`0`, SQLite having no boolean of its own. An array or object has no SQLite
/// equivalent, so it binds as its JSON text rather than being silently dropped — which is also how
/// anyone storing JSON in a column wants it.
pub(crate) fn to_sql_json(value: &serde_json::Value) -> rusqlite::types::Value {
    use rusqlite::types::Value;
    use serde_json::Value as Json;

    match value {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Integer(i64::from(*b)),
        Json::Number(n) => n.as_i64().map_or_else(
            || n.as_f64().map_or(Value::Null, Value::Real),
            Value::Integer,
        ),
        Json::String(s) => Value::Text(s.clone()),
        other => Value::Text(other.to_string()),
    }
}

/// Convert one CLI-supplied `--param` string into the SQLite value it obviously denotes.
///
/// A parameter arrives from argv as text, but binding everything as TEXT quietly breaks the most
/// common query an agent writes: SQLite compares TEXT `'5'` and INTEGER `5` as *unequal*, so
/// `where id = ?` against an INTEGER column would silently return nothing.
///
/// So a value is coerced only when the conversion is provably lossless — when the parsed number
/// prints back to the exact input string. That keeps `5` an integer and `1.5` a real, while
/// leaving `007`, `1_000`, `+5`, and `1e3` as the text the caller actually typed. The literal
/// `null` (any casing) binds SQL NULL; to store the *word* "null", pass it through stdin-fed SQL
/// instead.
pub(crate) fn to_sql(raw: &str) -> rusqlite::types::Value {
    use rusqlite::types::Value;

    if raw.eq_ignore_ascii_case("null") {
        return Value::Null;
    }
    if let Ok(i) = raw.parse::<i64>()
        && i.to_string() == raw
    {
        return Value::Integer(i);
    }
    if let Ok(f) = raw.parse::<f64>()
        && f.is_finite()
        && f.to_string() == raw
    {
        return Value::Real(f);
    }
    Value::Text(raw.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::types::Value;

    #[test]
    fn rows_fold_into_objects_keyed_by_column() {
        let result = QueryResult {
            columns: vec!["id".into(), "body".into()],
            rows: vec![vec![serde_json::json!(1), serde_json::json!("hi")]],
        };
        assert_eq!(
            result.rows_as_objects(),
            vec![serde_json::json!({ "id": 1, "body": "hi" })]
        );
        assert_eq!(result.len(), 1);
        assert!(!result.is_empty());
    }

    #[test]
    fn params_coerce_only_when_lossless() {
        assert_eq!(to_sql("5"), Value::Integer(5));
        assert_eq!(to_sql("-12"), Value::Integer(-12));
        assert_eq!(to_sql("1.5"), Value::Real(1.5));
        assert_eq!(to_sql("null"), Value::Null);
        assert_eq!(to_sql("NULL"), Value::Null);
        // Anything whose numeric parse wouldn't round-trip stays the text the caller typed —
        // a zip code, a padded id, or a thousands separator must survive intact.
        for raw in ["007", "+5", "1e3", "1_000", " 5", "5 ", "", "hello", "0x10"] {
            assert_eq!(
                to_sql(raw),
                Value::Text(raw.to_string()),
                "{raw:?} must stay text"
            );
        }
    }

    #[test]
    fn json_params_bind_as_the_type_they_already_are() {
        // No guessing on this path: the caller sent types, so `"007"` must stay text and `7` an
        // integer — the exact ambiguity `to_sql` has to work around when the input is argv text.
        assert_eq!(to_sql_json(&serde_json::json!(7)), Value::Integer(7));
        assert_eq!(
            to_sql_json(&serde_json::json!("007")),
            Value::Text("007".into())
        );
        assert_eq!(to_sql_json(&serde_json::json!(1.5)), Value::Real(1.5));
        assert_eq!(to_sql_json(&serde_json::json!(null)), Value::Null);
        assert_eq!(
            to_sql_json(&serde_json::json!("null")),
            Value::Text("null".into())
        );
        // SQLite has no boolean of its own.
        assert_eq!(to_sql_json(&serde_json::json!(true)), Value::Integer(1));
        assert_eq!(to_sql_json(&serde_json::json!(false)), Value::Integer(0));
        // Structured values keep their JSON text rather than vanishing.
        assert_eq!(
            to_sql_json(&serde_json::json!({"a": 1})),
            Value::Text(r#"{"a":1}"#.into())
        );
    }

    #[test]
    fn blobs_become_base64_and_null_becomes_null() {
        use rusqlite::types::ValueRef;
        assert_eq!(to_json(ValueRef::Null), serde_json::Value::Null);
        assert_eq!(to_json(ValueRef::Integer(7)), serde_json::json!(7));
        assert_eq!(to_json(ValueRef::Text(b"hi")), serde_json::json!("hi"));
        assert_eq!(to_json(ValueRef::Blob(b"hi")), serde_json::json!("aGk="));
    }
}
