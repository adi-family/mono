//! The Database page: the shared SQLite store — which databases exist, what's in the open one,
//! and a console to run SQL against it.
//!
//! Reading and writing are two buttons, not one, because they are two different connections on the
//! server: **Run** holds a read-only handle (so browsing can't mutate, whatever is typed), while
//! **Execute** is the deliberate write. The distinction is the safety property, so the UI states it
//! rather than hiding it behind a single "go".

use adi_ui::Lang;
use adi_webapp_api::types::{DbInfoDto, DbQueryResult, DbTableDto};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use adi_ui::{EmptyRow, Row as TableRow, Table};

use crate::fetch;
use crate::state::{DbConsole, Flash, State, load};
use crate::ui::{Key, placeholder_row, sort_rows, updated_text};

/// The databases table: one row per scope in the store. No action column — a row's control is the
/// Scope cell itself, which opens that database.
pub(crate) const SCOPE_COLS: &[&str] = &["Scope", "Tables", "Size", "Path"];

/// The open scope's tables; the trailing blank column holds Preview / Schema.
pub(crate) const TABLE_COLS: &[&str] = &["Name", "Rows", "Columns", ""];

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
            <Table state=state.tables.db_scopes>{move || scope_rows(state, console)}</Table>
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
            <Table state=state.tables.db_tables>{move || table_rows(state, console)}</Table>
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
                <adi_ui::CodeLog value=console.schema lang=Lang::Sql id="db-schema"
                    class="adi-ui-type island max-h-105"/>
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

            <adi_ui::CodeEditor value=console.sql lang=Lang::Sql
                height=adi_ui::CodeHeight::Form id="db-sql" class="adi-ui-type island"/>

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
    let table = state.tables.db_scopes;
    let Some(listing) = state.db.get() else {
        return view! { <EmptyRow state=table>"Loading…"</EmptyRow> }.into_any();
    };
    if listing.databases.is_empty() {
        return view! {
            <EmptyRow state=table>
                "No databases yet — the first write creates one. Try a `create table` below."
            </EmptyRow>
        }
        .into_any();
    }

    let mut databases = listing.databases;
    // By the byte count, not the `1.4 MB` the cell renders — otherwise `900 B` sorts after `2 GB`.
    sort_rows(
        &mut databases,
        table.sort.get(),
        |d, col| match col {
            "Tables" => Key::count(d.tables),
            "Size" => Key::num(d.bytes),
            "Path" => Key::text(&d.path),
            _ => Key::text(scope_name(d.project.as_deref())),
        },
        |d| Key::text(scope_name(d.project.as_deref())),
    );
    databases
        .into_iter()
        .map(|d| {
            view! { <TableRow state=table cell=move |col| scope_cell(col, &d, console)/> }
                .into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// A database's display name: its project id, or `global` for the unscoped store.
fn scope_name(project: Option<&str>) -> String {
    project.unwrap_or("global").to_string()
}

/// One database's cell under `col`. Matching the header text — the same key the sort uses — is
/// what lets the user hide and reorder columns without the row builder knowing about it.
fn scope_cell(col: &str, d: &DbInfoDto, console: DbConsole) -> AnyView {
    match col {
        "Tables" => view! { <span class="font-mono">{d.tables.to_string()}</span> }.into_any(),
        "Size" => {
            view! { <span class="font-mono text-meta">{human_bytes(d.bytes)}</span> }.into_any()
        }
        "Path" => view! { <span class="font-mono text-meta">{d.path.clone()}</span> }.into_any(),
        // "Scope", and anything the layout offers that this match doesn't name.
        _ => {
            let id = d.project.clone().unwrap_or_default();
            let label = scope_name(d.project.as_deref());
            let open = console.project.get() == id;
            view! {
                <>
                    <button class="adi-btn adi-btn--link adi-mono" on:click=move |_| {
                        if console.project.get_untracked() != id {
                            console.project.set(id.clone());
                            console.tables.set(None);
                            console.schema.set(String::new());
                            console.clear_result();
                        }
                    }>{label}</button>
                    {open.then(|| view! { <span class="adi-chip">"open"</span> })}
                </>
            }
            .into_any()
        }
    }
}

/// The open scope's tables: shape, live row count, and a preview button per table.
fn table_rows(state: State, console: DbConsole) -> AnyView {
    let table = state.tables.db_tables;
    let Some(loaded) = console.tables.get() else {
        return view! { <EmptyRow state=table>"Loading…"</EmptyRow> }.into_any();
    };
    if loaded.tables.is_empty() {
        return view! {
            <EmptyRow state=table>"This database has no tables yet."</EmptyRow>
        }
        .into_any();
    }

    let mut tables = loaded.tables;
    sort_rows(
        &mut tables,
        table.sort.get(),
        |t, col| match col {
            "Rows" => Key::Int(t.rows),
            "Columns" => Key::count(t.columns.len()),
            _ => Key::text(&t.name),
        },
        |t| Key::text(&t.name),
    );
    tables
        .into_iter()
        .map(|t| {
            let name = t.name.clone();
            view! {
                <TableRow
                    state=table
                    cell=move |col| table_cell(col, &t)
                    actions={
                        let (preview_of, schema_of) = (name.clone(), name.clone());
                        view! {
                            <button class="adi-btn adi-btn--link" on:click=move |_| {
                                // Quote the identifier the way SQLite does, so a name with a
                                // space or a keyword still parses.
                                let quoted = preview_of.replace('"', "\"\"");
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
                        }
                        .into_any()
                    }
                />
            }
            .into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// One table's cell under `col`. See [`scope_cell`] on why this matches header text.
fn table_cell(col: &str, t: &DbTableDto) -> AnyView {
    match col {
        "Rows" => view! { <span class="font-mono">{t.rows.to_string()}</span> }.into_any(),
        "Columns" => view! { <span class="text-meta">{column_summary(t)}</span> }.into_any(),
        // "Name", and anything the layout offers that this match doesn't name.
        _ => {
            let kind = (t.kind == "view").then(|| view! { <span class="adi-chip">"view"</span> });
            view! { <span class="font-mono">{t.name.clone()} {kind}</span> }.into_any()
        }
    }
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
                                    <span class="font-mono">{cell_text(cell)}</span>
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
