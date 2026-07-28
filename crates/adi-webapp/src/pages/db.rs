//! The Database page: the shared SQLite store — which databases exist, what's in the open one,
//! and a console to run SQL against it.
//!
//! Reading and writing are two buttons, not one, because they are two different connections on the
//! server: **Run** holds a read-only handle (so browsing can't mutate, whatever is typed), while
//! **Execute** is the deliberate write. The distinction is the safety property, so the UI states it
//! rather than hiding it behind a single "go".

use adi_webapp_api::types::{DbQueryResult, DbTableDto};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::highlight::Lang;
use crate::state::{DbConsole, Flash, State, load};
use crate::ui::{code_editor, code_viewer, data_table, placeholder_row, updated_text};

/// How many rows a table-click previews. A browse shouldn't pull a million rows into the DOM; the
/// console is right there for anyone who wants more.
const PREVIEW_LIMIT: usize = 50;

/// The Database page: the scope picker, the open scope's tables, and the SQL console.
pub(crate) fn database_view(state: State, console: DbConsole) -> AnyView {
    let State {
        db,
        flash,
        secs_since,
        ..
    } = state;

    // Load the open scope's tables whenever the scope changes (and on first render).
    Effect::new(move |_| {
        let project = console.project.get();
        let scope = (!project.is_empty()).then_some(project);
        spawn_local(async move {
            match fetch::db_tables(scope).await {
                Ok(tables) => console.tables.set(Some(tables)),
                Err(e) => {
                    console.tables.set(None);
                    console.error.set(Some(e));
                }
            }
        });
    });

    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Databases"</h2>
                <span class="adi-chip adi-mono" title="Databases in the store">
                    {move || db.get().map_or_else(|| "\u{2014}".to_string(),
                        |d| d.databases.len().to_string())}
                </span>
                <span class="adi-spacer"></span>
                <span class="adi-updated">{move || updated_text(db, secs_since)}</span>
            </div>
            {data_table(&["Scope", "Tables", "Size", "Path"], move || scope_rows(state, console))}
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Tables"</h2>
                <span class="adi-chip adi-mono">{move || scope_label(console)}</span>
                <span class="adi-spacer"></span>
                <span class="adi-updated">
                    {move || console.tables.get().map_or(String::new(),
                        |t| format!("{} table(s)", t.tables.len()))}
                </span>
            </div>
            {data_table(&["Name", "Rows", "Columns", ""], move || table_rows(console))}
        </section>

        // The schema panel, open only once a table's `Schema` is clicked.
        {move || (!console.schema.get().is_empty()).then(|| view! {
            <section class="adi-panel">
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">"Schema"</h2>
                    <span class="adi-spacer"></span>
                    <button class="adi-btn adi-btn--link" type="button"
                        on:click=move |_| console.schema.set(String::new())>"Close"</button>
                </div>
                {code_viewer(|| Lang::Sql, console.schema, "", "db-schema")}
            </section>
        })}

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"SQL"</h2>
                <span class="adi-spacer"></span>
                <span class="adi-updated">
                    "Run reads over a read-only connection; Execute writes."
                </span>
            </div>

            {code_editor(|| Lang::Sql, console.sql, "adi-code--form", "db-sql")}

            <div class="adi-form">
                <button class="adi-btn adi-btn--primary" type="button"
                    prop:disabled=move || console.busy.get()
                    on:click=move |_| run_sql(state, console, false)>
                    "Run"
                </button>
                <button class="adi-btn" type="button"
                    prop:disabled=move || console.busy.get()
                    on:click=move |_| run_sql(state, console, true)>
                    "Execute"
                </button>
                <span class="adi-spacer"></span>
                <span class="adi-chip adi-mono">{move || result_summary(console)}</span>
            </div>

            {move || console.error.get().map(|e| view! {
                <div class="adi-flash" data-kind="err">{e}</div>
            })}

            {move || console.rows.get().map(result_table)}
            {flash_or_nothing(flash)}
        </section>
    }
    .into_any()
}

/// The databases table: one row per scope, the open one marked. Clicking a row opens that scope.
fn scope_rows(state: State, console: DbConsole) -> AnyView {
    let Some(listing) = state.db.get() else {
        return placeholder_row(4, "Loading…");
    };
    if listing.databases.is_empty() {
        return placeholder_row(
            4,
            "No databases yet — the first write creates one. Try a `create table` below.",
        );
    }

    listing
        .databases
        .into_iter()
        .map(|d| {
            let id = d.project.clone().unwrap_or_default();
            let label = d.project.clone().unwrap_or_else(|| "global".to_string());
            let open = console.project.get() == id;
            let target = id.clone();
            view! {
                <tr>
                    <td>
                        <button class="adi-btn adi-btn--link adi-mono" on:click=move |_| {
                            if console.project.get_untracked() != target {
                                console.project.set(target.clone());
                                console.tables.set(None);
                                console.schema.set(String::new());
                                console.clear_result();
                            }
                        }>{label}</button>
                        {open.then(|| view! { <span class="adi-chip">"open"</span> })}
                    </td>
                    <td class="adi-mono">{d.tables.to_string()}</td>
                    <td class="adi-mono adi-muted">{human_bytes(d.bytes)}</td>
                    <td class="adi-mono adi-muted">{d.path}</td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// The open scope's tables: shape, live row count, and a preview button per table.
fn table_rows(console: DbConsole) -> AnyView {
    let Some(state) = console.tables.get() else {
        return placeholder_row(4, "Loading…");
    };
    if state.tables.is_empty() {
        return placeholder_row(4, "This database has no tables yet.");
    }

    state
        .tables
        .into_iter()
        .map(|t| {
            let name = t.name.clone();
            let schema_of = t.name.clone();
            let columns = column_summary(&t);
            let kind = (t.kind == "view").then(|| view! { <span class="adi-chip">"view"</span> });
            view! {
                <tr>
                    <td class="adi-mono">{t.name.clone()} {kind}</td>
                    <td class="adi-mono">{t.rows.to_string()}</td>
                    <td class="adi-muted">{columns}</td>
                    <td class="adi-table__actions">
                        <button class="adi-btn adi-btn--link" on:click=move |_| {
                            // Quote the identifier the way SQLite does, so a name with a space or
                            // a keyword still parses.
                            let quoted = name.replace('"', "\"\"");
                            console.sql.set(format!(
                                "select * from \"{quoted}\" limit {PREVIEW_LIMIT}"
                            ));
                        }>"Preview"</button>
                        <button class="adi-btn adi-btn--link" on:click=move |_| {
                            let table = schema_of.clone();
                            spawn_local(async move {
                                match fetch::db_schema(console.scope(), Some(table)).await {
                                    Ok(s) => console.schema.set(s.schema),
                                    Err(e) => console.error.set(Some(e)),
                                }
                            });
                        }>"Schema"</button>
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// A table's columns as one line: `id INTEGER pk, body TEXT not null, …`.
fn column_summary(table: &DbTableDto) -> String {
    table
        .columns
        .iter()
        .map(|c| {
            let mut text = c.name.clone();
            if !c.decl_type.is_empty() {
                text.push(' ');
                text.push_str(&c.decl_type);
            }
            if c.pk {
                text.push_str(" pk");
            }
            text
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Run the console's SQL — as a write when `write`, else as a read. Refreshes the shell's database
/// listing afterwards, so a `create table` shows up in the tables panel without a manual reload.
fn run_sql(state: State, console: DbConsole, write: bool) {
    let sql = console.sql.get_untracked().trim().to_string();
    if sql.is_empty() {
        return;
    }
    let scope = console.scope();
    console.clear_result();
    console.busy.set(true);

    spawn_local(async move {
        if write {
            match fetch::db_exec(scope.clone(), sql).await {
                Ok(result) => {
                    console.exec.set(Some(result));
                    state.flash.set(Some(Flash::ok(format!(
                        "{} row(s) changed.",
                        result.changes
                    ))));
                }
                Err(e) => console.error.set(Some(e)),
            }
        } else {
            match fetch::db_query(scope.clone(), sql).await {
                Ok(result) => console.rows.set(Some(result)),
                Err(e) => console.error.set(Some(e)),
            }
        }

        // A write can create or drop tables, and even a read is worth re-listing after (another
        // process may have changed the store). Both panels re-read from one refresh.
        if let Ok(tables) = fetch::db_tables(scope).await {
            console.tables.set(Some(tables));
        }
        load(state).await;
        console.busy.set(false);
    });
}

/// The result set as a table. Columns come from the statement, so an empty result still shows its
/// shape rather than collapsing to nothing.
fn result_table(result: DbQueryResult) -> AnyView {
    if result.columns.is_empty() {
        return view! { <p class="adi-muted">"(no result set)"</p> }.into_any();
    }
    let headers = result.columns.clone();
    let count = result.rows.len();
    view! {
        <div class="adi-tablewrap">
            <table class="adi-table">
                <thead>
                    <tr>{headers.into_iter()
                        .map(|h| view! { <th class="adi-mono">{h}</th> })
                        .collect::<Vec<_>>()}</tr>
                </thead>
                <tbody>
                    {if count == 0 {
                        placeholder_row(99, "No rows.")
                    } else {
                        result.rows.into_iter()
                            .map(|row| view! {
                                <tr>{row.iter().map(|cell| view! {
                                    <td class="adi-mono">{cell_text(cell)}</td>
                                }).collect::<Vec<_>>()}</tr>
                            })
                            .collect::<Vec<_>>()
                            .into_any()
                    }}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}

/// One JSON cell as display text: strings unquoted, `NULL` spelled out so it reads differently
/// from an empty string.
fn cell_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "NULL".to_string(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The one-line summary beside the buttons: row count for a read, change count for a write.
fn result_summary(console: DbConsole) -> String {
    if console.busy.get() {
        return "running…".to_string();
    }
    if let Some(rows) = console.rows.get() {
        return format!("{} row(s)", rows.rows.len());
    }
    if let Some(exec) = console.exec.get() {
        return if exec.changes > 0 && exec.last_insert_rowid > 0 {
            format!("{} changed, rowid {}", exec.changes, exec.last_insert_rowid)
        } else {
            format!("{} changed", exec.changes)
        };
    }
    String::new()
}

/// The open scope's label for the panel heading.
fn scope_label(console: DbConsole) -> String {
    let project = console.project.get();
    if project.is_empty() {
        "global".to_string()
    } else {
        project
    }
}

/// The shared flash line, shown under the console like every other page's form.
fn flash_or_nothing(flash: RwSignal<Option<Flash>>) -> AnyView {
    view! {
        <div class="adi-flash" data-kind=move || flash.get().map_or("none", |f| f.kind)>
            {move || flash.get().map(|f| f.msg).unwrap_or_default()}
        </div>
    }
    .into_any()
}

/// A byte count in the largest unit that keeps it readable.
fn human_bytes(bytes: u64) -> String {
    #[allow(
        clippy::cast_precision_loss,
        reason = "display only; a database size never needs full precision"
    )]
    let value = bytes as f64;
    for (unit, scale) in [("GB", 1e9), ("MB", 1e6), ("kB", 1e3)] {
        if value >= scale {
            return format!("{:.1} {unit}", value / scale);
        }
    }
    format!("{bytes} B")
}
