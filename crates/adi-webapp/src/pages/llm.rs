//! The LLM traffic page (`/extended/llm`): every model API call this machine made through the
//! gateway, and what was actually in it.
//!
//! The gateway (`crates/adi-llm-gateway`, behind `llm.adi`) sits between an agent and
//! Anthropic/OpenAI and journals the wire bytes of both halves of every call. This page is the
//! reading surface for that journal: the shape of the traffic over a window, three rollups of it,
//! the calls themselves, and — the reason the page exists — one call opened and read: its
//! parameters, its system prompt, the conversation as it was sent up, the tools it offered, and
//! the answer reassembled from the stream it came back as.
//!
//! **Nothing is analyzed in the browser.** A prompt is routinely a quarter of a megabyte and a
//! streamed answer arrives as thousands of SSE frames; parsing that in wasm would mean shipping
//! the whole body to the client to throw most of it away. So the `llm` handlers do the reading
//! server-side, where it is unit-tested, and this module is layout.
//!
//! The journal is read over a read-only connection and there is no endpoint here that writes to
//! it: the record of what was sent to a model is the gateway's to keep, and a panel that could
//! edit it would be a panel that could launder it.

use adi_ui::{CodeEditor, CodeHeight, Icon, Lang, Lucide, Row as TableRow, Table};
use adi_webapp_api::types::{
    LlmBlockDto, LlmBucketDto, LlmCallDetail, LlmCallDto, LlmGroupDto, LlmHeaderDto, LlmQuery,
    LlmSummary, LlmTokens,
};
use leptos::prelude::*;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::State;
use crate::ui::{Key, Sort, TableState, fmt_bytes, rows_or_placeholder, segmented, sort_rows};

/// The three rollup tables. One shape, three dimensions — and the first column has to *name* its
/// dimension, because a cell builder and a comparator both find their column by header text, and
/// a layout saved under a bare "Name" would be three tables sharing one set of hidden columns.
pub(crate) const PROVIDER_COLS: &[&str] = &[
    "Provider", "Calls", "Failed", "Prompt", "Output", "Cache", "Median", "p95", "Last",
];
pub(crate) const MODEL_COLS: &[&str] = &[
    "Model", "Calls", "Failed", "Prompt", "Output", "Cache", "Median", "p95", "Last",
];
pub(crate) const CLIENT_COLS: &[&str] = &[
    "Client", "Calls", "Failed", "Prompt", "Output", "Cache", "Median", "p95", "Last",
];

/// The call list. No action column: a row's one control is its When cell, which opens the call.
pub(crate) const CALL_COLS: &[&str] = &[
    "When", "Model", "Status", "Took", "Prompt", "Output", "Cache", "Client",
];

/// The order a rollup opens in: most calls first. The question it is read for is "what is
/// spending the tokens", and alphabetical answers that only by accident.
pub(crate) const MOST_CALLS_FIRST: Sort = Sort {
    col: "Calls",
    desc: true,
};

/// The order the call list opens in. A page watched while an agent works is watched for the call
/// that just happened.
pub(crate) const LATEST_FIRST: Sort = Sort {
    col: "When",
    desc: true,
};

/// The windows the filter offers: how the API spells each, and how the page says it.
const WINDOWS: [(&str, &str); 5] = [
    ("1h", "Last hour"),
    ("24h", "Last 24 hours"),
    ("7d", "Last 7 days"),
    ("30d", "Last 30 days"),
    ("all", "Everything"),
];

/// The page's own state: what is being asked for, what came back, and which call is open.
///
/// Owned by the shell like every other console here, so a navigation away and back does not reset
/// the filters or re-fetch a call that is still on screen. `Copy` — a bundle of arena handles — so
/// it threads into the view and into async handlers without ceremony.
#[derive(Clone, Copy)]
pub(crate) struct LlmConsole {
    /// The window, as the API spells it (`1h` … `all`).
    pub(crate) window: RwSignal<String>,
    /// A provider to narrow to, or empty for all of them.
    pub(crate) provider: RwSignal<String>,
    pub(crate) model: RwSignal<String>,
    /// Only the calls that failed.
    pub(crate) errors: RwSignal<bool>,
    /// What is typed in the search box …
    pub(crate) typed: RwSignal<String>,
    /// … and what was last submitted from it. Two signals because the search runs over every
    /// stored body: it goes when Enter is pressed, not on each keystroke.
    pub(crate) search: RwSignal<String>,
    /// Bumped by Refresh, to re-run the load without changing what is asked for.
    pub(crate) reload: RwSignal<u32>,
    pub(crate) summary: RwSignal<Option<LlmSummary>>,
    pub(crate) calls: RwSignal<Option<Vec<LlmCallDto>>>,
    /// How many calls the filter matched, which is not how many were returned.
    pub(crate) matched: RwSignal<i64>,
    /// The call whose detail is open, or `None`.
    pub(crate) open: RwSignal<Option<i64>>,
    /// That call, once it has landed.
    pub(crate) detail: RwSignal<Option<LlmCallDetail>>,
    /// Whether the open call is showing its raw bodies rather than the reading.
    pub(crate) raw: RwSignal<bool>,
    /// What the histogram's bars measure: calls, or tokens.
    pub(crate) by_tokens: RwSignal<bool>,
    pub(crate) busy: RwSignal<bool>,
    /// Why the last load failed, kept beside the page rather than in the shared flash.
    pub(crate) error: RwSignal<Option<String>>,
}

impl LlmConsole {
    pub(crate) fn new() -> Self {
        Self {
            window: RwSignal::new("24h".to_string()),
            provider: RwSignal::new(String::new()),
            model: RwSignal::new(String::new()),
            errors: RwSignal::new(false),
            typed: RwSignal::new(String::new()),
            search: RwSignal::new(String::new()),
            reload: RwSignal::new(0),
            summary: RwSignal::new(None),
            calls: RwSignal::new(None),
            matched: RwSignal::new(0),
            open: RwSignal::new(None),
            detail: RwSignal::new(None),
            raw: RwSignal::new(false),
            by_tokens: RwSignal::new(false),
            busy: RwSignal::new(false),
            error: RwSignal::new(None),
        }
    }

    /// The filters as the API wants them. Reads every filter signal, so an effect built on this
    /// re-runs whenever one of them changes.
    fn query(self) -> LlmQuery {
        let some = |s: String| (!s.is_empty()).then_some(s);
        LlmQuery {
            window: some(self.window.get()),
            provider: some(self.provider.get()),
            model: some(self.model.get()),
            errors: self.errors.get(),
            q: some(self.search.get().trim().to_string()),
            limit: None,
        }
    }

    /// Close the open call. Leaving the page does this — a prompt is the most sensitive thing on
    /// this screen, and there is no reason for one to sit in memory behind another page.
    pub(crate) fn close(self) {
        self.open.set(None);
        self.detail.set(None);
        self.raw.set(false);
    }
}

/// The LLM traffic page: the filters and what they add up to, the shape of the traffic, three
/// rollups, the calls, and whichever one is open.
pub(crate) fn llm_view(state: State, console: LlmConsole) -> AnyView {
    // Reload whenever a filter changes, and once on first render. Two requests, because the
    // summary is computed over more calls than the list returns — see `LlmSummary::read`.
    Effect::new(move |_| {
        let query = console.query();
        let _ = console.reload.get();
        console.busy.set(true);
        spawn_local(async move {
            match fetch::llm_summary(&query).await {
                Ok(summary) => {
                    console.summary.set(Some(summary));
                    console.error.set(None);
                }
                Err(e) => console.error.set(Some(e)),
            }
            match fetch::llm_calls(&query).await {
                Ok(calls) => {
                    console.matched.set(calls.matched);
                    console.calls.set(Some(calls.calls));
                }
                Err(e) => console.error.set(Some(e)),
            }
            console.busy.set(false);
        });
    });

    view! {
        {traffic_panel(console)}
        {move || activity_panel(console)}
        {rollup_panel(console, "By model", state.tables.llm_models, |s| s.models)}
        {rollup_panel(console, "By provider", state.tables.llm_providers, |s| s.providers)}
        {rollup_panel(console, "By client", state.tables.llm_clients, |s| s.clients)}
        {calls_panel(state, console)}
        {move || detail_panel(console)}
    }
    .into_any()
}

// ---- the filters, and what they add up to ------------------------------------------------------

/// The head of the page: what is being asked for, and the totals for it.
///
/// Filters and totals share a panel on purpose — the numbers are *of* the filter, and a total in
/// a panel of its own would read as a total of the machine.
fn traffic_panel(console: LlmConsole) -> AnyView {
    let submit = move || {
        console
            .search
            .set(console.typed.get_untracked().trim().to_string())
    };
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Traffic"</h2>
                <span class="adi-updated" title="every call that went through llm.adi">
                    {move || match console.summary.get() {
                        None => "\u{2014}".to_string(),
                        Some(s) => format!("{} calls on record", s.total_rows),
                    }}
                </span>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--ghost" type="button"
                    prop:disabled=move || console.busy.get()
                    on:click=move |_| console.reload.update(|n| *n += 1)>
                    <Icon icon=Lucide::RefreshCw/>
                    {move || if console.busy.get() { "Loading\u{2026}" } else { "Refresh" }}
                </button>
            </div>

            <div class="adi-form adi-form--first">
                // Searched against the path, the model and the request body — so this box finds a
                // call by something said *in* the prompt, which is the only way to find one call
                // in a day of them.
                <div class="adi-field adi-field--grow">
                    <label class="adi-field__label" for="llm-q">"Search"</label>
                    <input id="llm-q" class="adi-input adi-input--wide" type="search"
                        placeholder="a phrase from the prompt, a path, a model"
                        prop:value=move || console.typed.get()
                        on:input=move |ev| console.typed.set(event_target_value(&ev))
                        on:keydown=move |ev| if ev.key() == "Enter" { submit(); } />
                </div>
                <div class="adi-field">
                    <label class="adi-field__label" for="llm-window">"Window"</label>
                    <select id="llm-window" class="adi-input"
                        on:change=move |ev| console.window.set(event_target_value(&ev))>
                        {WINDOWS.into_iter().map(|(value, shown)| view! {
                            <option value=value selected=move || console.window.get() == value>
                                {shown}
                            </option>
                        }).collect_view()}
                    </select>
                </div>
                {picker(console, "llm-model", "Model", console.model, "every model",
                    |s| s.known_models.clone())}
                {picker(console, "llm-provider", "Provider", console.provider, "every provider",
                    |s| s.known_providers.clone())}
                <button class="adi-btn adi-btn--primary" type="button"
                    prop:disabled=move || console.busy.get()
                    on:click=move |_| submit()>
                    "Search"
                </button>
            </div>

            <div class="adi-llm-mode">
                {segmented("Which calls to count", console.errors, "All calls", "Only failures")}
                {move || (!console.search.get().is_empty()).then(|| view! {
                    <span class="adi-llm-note">
                        {format!("matching \u{201c}{}\u{201d}", console.search.get())}
                    </span>
                })}
            </div>

            {move || console.error.get().map(|e| view! {
                <div class="adi-flash" data-kind="err">{e}</div>
            })}
            {move || totals_view(console)}
        </section>
    }
    .into_any()
}

/// One of the two "narrow to a name" selects. Its options come from every name the journal has
/// ever held rather than from what is in the window, so a model that has been quiet all day can
/// still be asked about.
fn picker(
    console: LlmConsole,
    id: &'static str,
    label: &'static str,
    signal: RwSignal<String>,
    any: &'static str,
    names: fn(&LlmSummary) -> Vec<String>,
) -> AnyView {
    view! {
        <div class="adi-field">
            <label class="adi-field__label" for=id>{label}</label>
            <select id=id class="adi-input adi-mono"
                on:change=move |ev| signal.set(event_target_value(&ev))>
                <option value="" selected=move || signal.get().is_empty()>{any}</option>
                {move || console.summary.get().map(|s| names(&s).into_iter().map(|name| {
                    let selected = signal.get() == name;
                    let value = name.clone();
                    view! { <option value=value selected=selected>{name}</option> }
                }).collect_view())}
            </select>
        </div>
    }
    .into_any()
}

/// The totals: three numbers, a line of detail under them, then the figures that are only worth
/// reading when they are not zero (DESIGN.md §6 — a stats row, never boxes).
fn totals_view(console: LlmConsole) -> AnyView {
    let Some(summary) = console.summary.get() else {
        return view! { <div class="adi-empty">"Loading\u{2026}"</div> }.into_any();
    };
    let t = &summary.totals;
    if t.calls == 0 {
        return view! {
            <div class="adi-empty">
                "No calls in this window. The gateway journals what goes through "
                <span class="adi-mono">"llm.adi"</span>
                " \u{2014} an agent pointed straight at the provider is invisible here."
            </div>
        }
        .into_any();
    }
    // The summary is computed over a bounded read, and says so rather than presenting a sample as
    // a total.
    let sampled = (summary.read < summary.matched).then(|| {
        format!(
            "These figures are over the {} most recent of {} calls in this window. The rest are on record \u{2014} just not in this arithmetic.",
            summary.read, summary.matched,
        )
    });
    let detail = format!(
        "{} up, {} down \u{b7} {} streamed \u{b7} median {} \u{b7} p95 {} \u{b7} slowest {}",
        fmt_bytes(as_u64(t.request_bytes)),
        fmt_bytes(as_u64(t.response_bytes)),
        t.streamed,
        took(t.median_ms),
        took(t.p95_ms),
        took(t.slowest_ms),
    );

    view! {
        <div class="adi-llm-stats">
            {stat(t.calls.to_string(), "calls")}
            {stat(tokens_text(t.tokens.prompt()), "tokens in")}
            {stat(tokens_text(t.tokens.output), "tokens out")}
        </div>
        <div class="adi-llm-fine">{detail}</div>
        <dl class="adi-llm-kv">
            <dt>"Failed"</dt>
            <dd>{match t.failed {
                0 => view! { <span class="adi-muted">"0"</span> }.into_any(),
                n => view! { <span>{dot("err")}{n.to_string()}</span> }.into_any(),
            }}</dd>
            <dt>"Read from cache"</dt>
            <dd>{match t.tokens.cache_hit() {
                None => view! { <span class="adi-muted">"\u{2014}"</span> }.into_any(),
                Some(pct) => view! {
                    <span title="cached input tokens as a share of everything sent up">
                        {format!("{pct}% of the prompt \u{b7} {} tokens",
                            tokens_text(t.tokens.cached))}
                    </span>
                }.into_any(),
            }}</dd>
            <dt>"Written to cache"</dt>
            <dd>{match t.tokens.cache_write {
                0 => view! { <span class="adi-muted">"\u{2014}"</span> }.into_any(),
                n => view! { <span>{format!("{} tokens", tokens_text(n))}</span> }.into_any(),
            }}</dd>
        </dl>
        {sampled.map(|line| view! { <div class="adi-hint">{line}</div> })}
    }
    .into_any()
}

/// One number in the stats row: 20px, with its label under it.
fn stat(value: String, label: &'static str) -> AnyView {
    view! {
        <div class="adi-llm-stat">
            <b>{value}</b>
            <span>{label}</span>
        </div>
    }
    .into_any()
}

/// A 6px dot in a state's colour, before the word or number it qualifies. The one form a semantic
/// colour takes on this page (DESIGN.md §3).
fn dot(tone: &'static str) -> AnyView {
    view! { <span class=format!("adi-llm-dot adi-llm-dot--{tone}") aria-hidden="true"></span> }
        .into_any()
}

// ---- the shape of the traffic ------------------------------------------------------------------

/// The window as a row of bars — *when* the calls happened, which no total says. Divs rather than
/// an SVG: thirty rectangles are not worth a chart library in a wasm bundle.
fn activity_panel(console: LlmConsole) -> AnyView {
    let Some(summary) = console.summary.get() else {
        return ().into_any();
    };
    if summary.buckets.is_empty() {
        return ().into_any();
    }
    let by_tokens = console.by_tokens.get();
    let value = move |b: &LlmBucketDto| if by_tokens { b.tokens } else { b.calls };
    // Every bar is a percentage of the tallest, so both series get the full height rather than the
    // call count being invisible next to a token count.
    let peak = summary.buckets.iter().map(value).max().unwrap_or(0);
    let total: i64 = summary.buckets.iter().map(value).sum();
    let last = summary.buckets.len() - 1;
    // A label under every bar would be thirty timestamps in a row; one every sixth reads the axis,
    // and every bar carries its exact moment in its hover text anyway.
    let tick_every = (summary.buckets.len() / 6).max(1);

    let bars = summary
        .buckets
        .iter()
        .enumerate()
        .map(|(i, b)| {
            // Floored at a sliver, so a bucket with one call is visibly a bucket with one call
            // rather than an empty column.
            let height = match (peak, value(b)) {
                (0, _) | (_, 0) => "0".to_string(),
                (peak, n) => format!("{}%", (n * 100 / peak).max(6)),
            };
            let hover = format!(
                "{} \u{2014} {} call(s), {} tokens{}",
                stamp(b.at),
                b.calls,
                tokens_text(b.tokens),
                if b.failed > 0 {
                    format!(", {} failed", b.failed)
                } else {
                    String::new()
                },
            );
            let tick = (i % tick_every == 0 || i == last).then(|| clock(b.at));
            view! {
                <div class="adi-llm-spark__col" title=hover>
                    <span class="adi-llm-spark__bar" class:adi-llm-spark__bar--last=(i == last)
                        style=format!("height:{height}")></span>
                    <span class="adi-llm-spark__tick">{tick.unwrap_or_default()}</span>
                </div>
            }
        })
        .collect::<Vec<_>>();

    let span = span_text(summary.buckets[0].span_ms);
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Activity"</h2>
                <span class="adi-updated" title="the window, bucketed">
                    {if by_tokens {
                        format!("{} tokens, per {span}", tokens_text(total))
                    } else {
                        format!("{total} calls, per {span}")
                    }}
                </span>
                <span class="adi-spacer"></span>
                {segmented("What the bars measure", console.by_tokens, "Calls", "Tokens")}
            </div>
            <div class="adi-llm-spark">{bars}</div>
        </section>
    }
    .into_any()
}

// ---- the rollups -------------------------------------------------------------------------------

/// One of the three breakdowns. All three are the same table over the same shape, differing only
/// in which dimension the server grouped by — so they share a row builder and a comparator, and
/// the caller supplies the table state and the accessor.
fn rollup_panel(
    console: LlmConsole,
    title: &'static str,
    table: TableState,
    rows: fn(LlmSummary) -> Vec<LlmGroupDto>,
) -> AnyView {
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">{title}</h2>
                <span class="adi-updated">
                    {move || match console.summary.get() {
                        None => "\u{2014}".to_string(),
                        Some(s) => format!("{} in this window", rows(s).len()),
                    }}
                </span>
            </div>
            <Table state=table>{move || group_rows(table, console, rows)}</Table>
        </section>
    }
    .into_any()
}

/// A rollup table's body.
fn group_rows(
    table: TableState,
    console: LlmConsole,
    pick: fn(LlmSummary) -> Vec<LlmGroupDto>,
) -> AnyView {
    let mut rows = match rows_or_placeholder(
        table,
        console.summary.get().map(pick),
        "Nothing in this window.",
    ) {
        Ok(rows) => rows,
        Err(placeholder) => return placeholder,
    };
    // Ties fall back to the name, so two models with the same call count keep a stable order
    // between reloads instead of swapping places under the reader.
    sort_rows(&mut rows, table.sort.get(), group_key, |g| {
        Key::text(&g.name)
    });
    rows.into_iter()
        .map(|g| view! { <TableRow state=table cell=move |col| group_cell(col, &g)/> }.into_any())
        .collect::<Vec<_>>()
        .into_any()
}

/// A rollup row's sort key under `col`.
fn group_key(g: &LlmGroupDto, col: &str) -> Key {
    match col {
        "Calls" => Key::Int(g.calls),
        "Failed" => Key::Int(g.failed),
        "Prompt" => Key::Int(g.tokens.prompt()),
        "Output" => Key::Int(g.tokens.output),
        "Cache" => Key::Int(i64::from(g.tokens.cache_hit().unwrap_or(0))),
        "Median" => Key::Int(g.median_ms.unwrap_or(0)),
        "p95" => Key::Int(g.p95_ms.unwrap_or(0)),
        "Last" => Key::Int(g.last_seen),
        // The name column, whichever of the three it is, and any header without a key of its own.
        _ => Key::text(&g.name),
    }
}

/// A rollup row's cell under `col`. Matching the header text — the same key the sort uses — is
/// what lets the reader hide and reorder columns without the row builder knowing about it.
fn group_cell(col: &str, g: &LlmGroupDto) -> AnyView {
    match col {
        "Calls" => view! { <span class="adi-tabnums">{g.calls.to_string()}</span> }.into_any(),
        "Failed" => tally(g.failed),
        "Prompt" => tokens_cell(g.tokens.prompt(), Some(g.tokens)),
        "Output" => tokens_cell(g.tokens.output, None),
        "Cache" => cache_cell(g.tokens),
        "Median" => took_cell(g.median_ms),
        "p95" => took_cell(g.p95_ms),
        "Last" => {
            view! { <span class="adi-muted whitespace-nowrap">{ago(g.last_seen)}</span> }.into_any()
        }
        // The name — a provider, a model or a user-agent, all three machine strings and so all
        // three mono. The bytes have no column of their own; they ride here, where a reader
        // asking what a model cost is already looking.
        _ => {
            let title = format!(
                "{} up, {} down \u{b7} {} streamed",
                fmt_bytes(as_u64(g.request_bytes)),
                fmt_bytes(as_u64(g.response_bytes)),
                g.streamed,
            );
            view! { <span class="adi-mono" title=title>{g.name.clone()}</span> }.into_any()
        }
    }
}

// ---- the calls ---------------------------------------------------------------------------------

/// Every call the filter matched, newest first. Its rows are the way into the detail below.
fn calls_panel(state: State, console: LlmConsole) -> AnyView {
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Calls"</h2>
                <span class="adi-updated" title="click a time to read the whole call">
                    {move || match console.calls.get() {
                        None => "\u{2014}".to_string(),
                        Some(rows) => {
                            let matched = console.matched.get();
                            if matched > rows.len() as i64 {
                                format!("the {} newest of {matched} matching", rows.len())
                            } else {
                                format!("{matched} matching")
                            }
                        }
                    }}
                </span>
            </div>
            <Table state=state.tables.llm_calls>{move || call_rows(state, console)}</Table>
        </section>
    }
    .into_any()
}

/// The call table's body.
fn call_rows(state: State, console: LlmConsole) -> AnyView {
    let table = state.tables.llm_calls;
    let mut rows = match rows_or_placeholder(table, console.calls.get(), "No call matches this.") {
        Ok(rows) => rows,
        Err(placeholder) => return placeholder,
    };
    sort_rows(&mut rows, table.sort.get(), call_key, |c| Key::Int(c.id));
    rows.into_iter()
        .map(|c| {
            view! { <TableRow state=table cell=move |col| call_cell(col, &c, console)/> }.into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// A call row's sort key under `col`.
fn call_key(c: &LlmCallDto, col: &str) -> Key {
    match col {
        "Model" => Key::maybe(c.model.as_deref()),
        "Status" => Key::Int(c.status.unwrap_or(0)),
        "Took" => Key::Int(c.duration_ms.unwrap_or(0)),
        "Prompt" => Key::Int(c.tokens.prompt()),
        "Output" => Key::Int(c.tokens.output),
        "Cache" => Key::Int(i64::from(c.tokens.cache_hit().unwrap_or(0))),
        "Client" => Key::maybe(c.agent.as_deref()),
        // "When", and any header without a key of its own. The id is the tie-break the timestamp
        // needs: several calls in the same millisecond is ordinary while an agent works.
        _ => Key::Int(c.started_at * 1_000 + c.id % 1_000),
    }
}

/// A call row's cell under `col`.
fn call_cell(col: &str, c: &LlmCallDto, console: LlmConsole) -> AnyView {
    match col {
        "Model" => match &c.model {
            None => view! { <span class="adi-muted">"\u{2014}"</span> }.into_any(),
            Some(model) => {
                let title = format!("{} \u{b7} {} {}", c.provider, c.method, c.target);
                view! { <span class="adi-mono" title=title>{model.clone()}</span> }.into_any()
            }
        },
        "Status" => status_cell(c),
        "Took" => took_cell(c.duration_ms),
        "Prompt" => tokens_cell(c.tokens.prompt(), Some(c.tokens)),
        "Output" => tokens_cell(c.tokens.output, None),
        "Cache" => cache_cell(c.tokens),
        // Which agent made the call, from its user-agent — the full string is the hover, because
        // in a real journal most rows carry the same one and it is the widest cell on the page.
        "Client" => match &c.agent {
            None => view! { <span class="adi-muted">"\u{2014}"</span> }.into_any(),
            Some(agent) => {
                let title = c.client.clone().unwrap_or_default();
                view! { <span class="adi-mono adi-muted" title=title>{agent.clone()}</span> }
                    .into_any()
            }
        },
        // "When", and anything the layout offers that this match doesn't name. The cell *is* the
        // row's control: clicking the moment opens what happened at it.
        _ => {
            let id = c.id;
            let title = format!("{} \u{b7} {} {}", stamp(c.started_at), c.method, c.target);
            let clock = clock(c.started_at);
            view! {
                <span>
                    <button class="adi-link adi-tabnums" type="button" title=title
                        on:click=move |_| open_call(console, id)>
                        {clock}
                    </button>
                    {move || (console.open.get() == Some(id))
                        .then(|| view! { " "<span class="adi-chip">"open"</span> })}
                </span>
            }
            .into_any()
        }
    }
}

/// Open one call — or close it, if it is the one already open. The reading is done server-side,
/// so this is one request whatever the body's size.
fn open_call(console: LlmConsole, id: i64) {
    if console.open.get_untracked() == Some(id) {
        console.close();
        return;
    }
    console.open.set(Some(id));
    console.detail.set(None);
    console.raw.set(false);
    spawn_local(async move {
        match fetch::llm_call(id).await {
            // A second click may have landed while this was in flight; the reader's latest choice
            // wins over an answer to their previous one.
            Ok(detail) => {
                if console.open.get_untracked() == Some(id) {
                    console.detail.set(Some(detail));
                }
            }
            Err(e) => console.error.set(Some(e)),
        }
    });
}

// ---- one call, read ----------------------------------------------------------------------------

/// The open call. Everything here was parsed on the server out of the stored bodies: the request's
/// own settings, the prompt as its blocks, the tools it offered, and the answer put back together
/// from whatever shape it came back in.
fn detail_panel(console: LlmConsole) -> AnyView {
    let Some(id) = console.open.get() else {
        return ().into_any();
    };
    let detail = console.detail.get();
    let body = match &detail {
        None => view! { <div class="adi-empty">"Loading\u{2026}"</div> }.into_any(),
        Some(d) if console.raw.get() => raw_view(d),
        Some(d) => read_view(d),
    };
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">{format!("Call {id}")}</h2>
                <span class="adi-updated">{detail.map(|d| headline(&d.call))}</span>
                <span class="adi-spacer"></span>
                {segmented("How to show the call", console.raw, "Read", "Raw")}
                <button class="adi-btn adi-btn--ghost" type="button"
                    on:click=move |_| console.close()>
                    <Icon icon=Lucide::X/>"Close"
                </button>
            </div>
            {body}
        </section>
    }
    .into_any()
}

/// The one line under the call's heading: what it was, how it went, what it spent.
fn headline(c: &LlmCallDto) -> String {
    let status = match (c.status, &c.error) {
        (_, Some(e)) => format!("failed: {e}"),
        (Some(s), None) => s.to_string(),
        (None, None) => "no status".to_string(),
    };
    format!(
        "{} \u{b7} {} \u{b7} {status} \u{b7} {} \u{b7} {} in, {} out",
        c.provider,
        c.model.clone().unwrap_or_else(|| "no model".to_string()),
        took(c.duration_ms),
        tokens_text(c.tokens.prompt()),
        tokens_text(c.tokens.output),
    )
}

/// The call as something to read: where it went, what it was told, and what it said back.
fn read_view(d: &LlmCallDetail) -> AnyView {
    let stream = (!d.events.is_empty()).then(|| {
        d.events
            .iter()
            .map(|e| format!("{} \u{d7}{}", e.name, e.count))
            .collect::<Vec<_>>()
            .join(" \u{b7} ")
    });
    let tools = (!d.tools.is_empty()).then(|| {
        d.tools
            .iter()
            .map(|t| view! { <span class="adi-chip adi-mono">{t.clone()}</span> })
            .collect::<Vec<_>>()
    });
    let stop = d.stop_reason.clone();

    view! {
        <div class="adi-llm-detail">
            {sect("Where it went", view! {
                <dl class="adi-llm-kv adi-llm-kv--wide">
                    <dt>"Upstream"</dt>
                    <dd class="adi-mono">{d.upstream.clone()}</dd>
                    <dt>"Asked for"</dt>
                    <dd class="adi-mono">{format!("{} {}", d.call.method, d.call.target)}</dd>
                </dl>
            }.into_any())}

            {(!d.params.is_empty()).then(|| sect("Settings", pairs(&d.params, false)))}
            {tools.map(|chips| sect("Tools offered",
                view! { <div class="adi-llm-chips">{chips}</div> }.into_any()))}

            {(!d.system.is_empty()).then(|| sect("System prompt", blocks(&d.system)))}
            {(!d.messages.is_empty()).then(|| sect("Conversation", blocks(&d.messages)))}

            {(!d.answer.is_empty()).then(|| sect("Answer",
                view! { <pre class="adi-llm-body">{d.answer.clone()}</pre> }.into_any()))}
            {(!d.thinking.is_empty()).then(|| sect("Thinking",
                view! {
                    <pre class="adi-llm-body adi-llm-body--quiet">{d.thinking.clone()}</pre>
                }.into_any()))}
            {(!d.tool_calls.is_empty()).then(|| sect("Tools it called", blocks(&d.tool_calls)))}

            {(stop.is_some() || stream.is_some()).then(|| sect("How it ended", view! {
                <dl class="adi-llm-kv adi-llm-kv--wide">
                    {stop.map(|reason| view! {
                        <dt>"Stop reason"</dt>
                        <dd class="adi-mono">{reason}</dd>
                    })}
                    {stream.map(|line| view! {
                        <dt>"Stream"</dt>
                        <dd class="adi-mono">{line}</dd>
                    })}
                </dl>
            }.into_any()))}

            {sect("Request headers", pairs(&d.request_headers, true))}
            {(!d.response_headers.is_empty())
                .then(|| sect("Response headers", pairs(&d.response_headers, true)))}
        </div>
    }
    .into_any()
}

/// The call as it went over the wire. Read-only editors rather than logs: a `CodeLog` follows the
/// tail, and a body is read from its top.
fn raw_view(d: &LlmCallDetail) -> AnyView {
    let request = RwSignal::new(d.request_body.clone());
    let response = RwSignal::new(d.response_body.clone());
    // A streamed answer is a run of SSE frames, not a JSON document — scanning it as JSON would
    // paint most of it as broken.
    let response_lang = if d.call.streamed {
        Lang::None
    } else {
        Lang::Json
    };
    view! {
        <div class="adi-llm-detail">
            {sect("Request body", view! {
                <div class="adi-llm-note">{size_note(d.request_body.len(), d.request_chars)}</div>
                <CodeEditor value=request lang=Lang::Json readonly=true
                    height=CodeHeight::Form id="llm-raw-request" class="island"/>
            }.into_any())}
            {sect("Response body", view! {
                <div class="adi-llm-note">{size_note(d.response_body.len(), d.response_chars)}</div>
                <CodeEditor value=response lang=response_lang readonly=true
                    height=CodeHeight::Form id="llm-raw-response" class="island"/>
            }.into_any())}
        </div>
    }
    .into_any()
}

/// What a raw pane is showing, against what the journal holds. A pane that showed 32,000
/// characters of a 250,000-character prompt without saying so would be a pane that lies about
/// what was sent.
fn size_note(shown: usize, stored: usize) -> String {
    if shown >= stored {
        return format!("{stored} characters, all of them");
    }
    format!("{shown} of {stored} characters \u{2014} the head and the tail")
}

/// One labelled part of the detail: a 12px grey label over a hairline, then the thing itself. Not
/// a card — a card inside a panel is the shape DESIGN.md §8 forbids.
fn sect(label: &'static str, body: AnyView) -> AnyView {
    view! {
        <div class="adi-llm-sect">
            <div class="adi-llm-sect__label">{label}</div>
            {body}
        </div>
    }
    .into_any()
}

/// A key-value list — the settings, and both header sets. `mono_key` is for headers, whose names
/// are machine strings; a setting's name is prose, though its value is not.
fn pairs(rows: &[LlmHeaderDto], mono_key: bool) -> AnyView {
    let rows = rows
        .iter()
        .map(|p| {
            let name = p.name.clone();
            let key = if mono_key {
                view! { <dt class="adi-mono">{name}</dt> }.into_any()
            } else {
                view! { <dt>{name}</dt> }.into_any()
            };
            view! { {key}<dd class="adi-mono">{p.value.clone()}</dd> }
        })
        .collect::<Vec<_>>();
    view! { <dl class="adi-llm-kv adi-llm-kv--wide">{rows}</dl> }.into_any()
}

/// A run of conversation blocks, in the order they were sent.
fn blocks(rows: &[LlmBlockDto]) -> AnyView {
    rows.iter().map(block_view).collect::<Vec<_>>().into_any()
}

/// One block: who said it and what kind it is, then the text itself at reading width.
fn block_view(b: &LlmBlockDto) -> AnyView {
    let cut = b.chars > b.text.chars().count();
    let meta = match &b.name {
        Some(name) => format!("{name} \u{b7} {} chars", b.chars),
        None => format!("{} chars", b.chars),
    };
    view! {
        <div class="adi-llm-block">
            <div class="adi-llm-block__head">
                <span class="adi-llm-block__role">{b.role.clone()}</span>
                <span class="adi-llm-block__kind">{b.kind.clone()}</span>
                <span class="adi-spacer"></span>
                <span class="adi-llm-block__meta">{meta}</span>
            </div>
            <pre class="adi-llm-body">{b.text.clone()}</pre>
            {cut.then(|| view! {
                <div class="adi-llm-note">"Cut for transport \u{2014} Raw has the stored body."</div>
            })}
        </div>
    }
    .into_any()
}

// ---- formatting --------------------------------------------------------------------------------

/// A status cell: quiet when it worked, `--err` when it did not, and the transport error's own
/// words in the hover when the call never reached a status at all.
fn status_cell(c: &LlmCallDto) -> AnyView {
    match (c.status, &c.error) {
        (_, Some(e)) => {
            let title = e.clone();
            view! { <span title=title>{dot("err")}"error"</span> }.into_any()
        }
        (None, None) => {
            view! { <span title="the call never reached a status">{dot("warn")}"\u{2014}"</span> }
                .into_any()
        }
        (Some(s), None) if s >= 400 => {
            view! { <span class="adi-tabnums">{dot("err")}{s.to_string()}</span> }.into_any()
        }
        (Some(s), None) => {
            let streamed = c
                .streamed
                .then(|| view! { " "<span class="adi-chip">"stream"</span> });
            view! { <span class="adi-tabnums adi-muted">{s.to_string()}{streamed}</span> }
                .into_any()
        }
    }
}

/// A count worth reading only when it is not zero: a dash otherwise, so the eye finds the three
/// rows that failed in a table of forty that didn't.
fn tally(n: i64) -> AnyView {
    if n == 0 {
        return view! { <span class="adi-muted">"\u{2014}"</span> }.into_any();
    }
    view! { <span class="adi-tabnums">{dot("err")}{n.to_string()}</span> }.into_any()
}

/// A token count, compact, with the exact figures in its hover text.
fn tokens_cell(n: i64, breakdown: Option<LlmTokens>) -> AnyView {
    if n == 0 {
        return view! { <span class="adi-muted">"\u{2014}"</span> }.into_any();
    }
    let title = breakdown.map(|t| {
        format!(
            "{} fresh \u{b7} {} from cache \u{b7} {} written to cache",
            t.input, t.cached, t.cache_write,
        )
    });
    view! { <span class="adi-tabnums" title=title>{tokens_text(n)}</span> }.into_any()
}

/// What share of the prompt came from the cache.
fn cache_cell(t: LlmTokens) -> AnyView {
    match t.cache_hit() {
        None | Some(0) => view! { <span class="adi-muted">"\u{2014}"</span> }.into_any(),
        Some(pct) => {
            let title = format!("{} of {} input tokens were cached", t.cached, t.prompt());
            view! { <span class="adi-tabnums adi-muted" title=title>{format!("{pct}%")}</span> }
                .into_any()
        }
    }
}

/// A duration cell.
fn took_cell(ms: Option<i64>) -> AnyView {
    match ms {
        None => view! { <span class="adi-muted">"\u{2014}"</span> }.into_any(),
        Some(_) => view! { <span class="adi-tabnums">{took(ms)}</span> }.into_any(),
    }
}

/// A duration in words: milliseconds while that is the honest unit, then seconds, then minutes.
fn took(ms: Option<i64>) -> String {
    let Some(ms) = ms else {
        return "\u{2014}".to_string();
    };
    match ms {
        ms if ms < 1_000 => format!("{ms}ms"),
        ms if ms < 60_000 => format!("{:.1}s", ms as f64 / 1_000.0),
        ms => format!("{}m {:02}s", ms / 60_000, (ms % 60_000) / 1_000),
    }
}

/// A token count for a cell: thousands as `93.7k`, millions as `2.40M`. The exact number is always
/// one hover away — this is a column read across rows, not added up by eye.
fn tokens_text(n: i64) -> String {
    match n {
        n if n < 10_000 => n.to_string(),
        n if n < 1_000_000 => format!("{:.1}k", n as f64 / 1_000.0),
        n => format!("{:.2}M", n as f64 / 1_000_000.0),
    }
}

/// How wide one histogram bar is, in words.
fn span_text(ms: i64) -> String {
    match ms {
        ms if ms >= 86_400_000 => format!("{}d", ms / 86_400_000),
        ms if ms >= 3_600_000 => format!("{}h", ms / 3_600_000),
        ms if ms >= 60_000 => format!("{}m", ms / 60_000),
        ms => format!("{}s", ms / 1_000),
    }
}

/// A wall-clock time in the reader's own zone — `14:23:05`.
fn clock(ms: i64) -> String {
    date(ms).to_locale_time_string("en-GB").into()
}

/// The full local date and time, for a hover.
fn stamp(ms: i64) -> String {
    date(ms)
        .to_locale_string("en-GB", &JsValue::UNDEFINED)
        .into()
}

/// A JS `Date` for a millisecond timestamp.
fn date(ms: i64) -> js_sys::Date {
    js_sys::Date::new(&JsValue::from_f64(ms as f64))
}

/// A coarse "how long ago" for a millisecond timestamp; `0` (never) renders as a dash.
fn ago(ms: i64) -> String {
    if ms <= 0 {
        return "\u{2014}".to_string();
    }
    let secs = (js_sys::Date::now() as i64 - ms).max(0) as u64 / 1_000;
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3_600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3_600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// A byte count from the journal, which stores them signed. A negative one would be a corrupt row
/// rather than a small number, so it reads as zero.
fn as_u64(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Milliseconds are the honest unit for a call that took 340 of them; seconds are for one that
    /// took a while. A number without its unit is not a duration.
    #[test]
    fn a_duration_keeps_the_unit_it_is_honest_in() {
        assert_eq!(took(None), "—");
        assert_eq!(took(Some(0)), "0ms");
        assert_eq!(took(Some(999)), "999ms");
        assert_eq!(took(Some(1_000)), "1.0s");
        assert_eq!(took(Some(59_949)), "59.9s");
        assert_eq!(took(Some(60_000)), "1m 00s");
        assert_eq!(took(Some(185_000)), "3m 05s");
    }

    /// A token count reads across rows, so it is compact — but only once compacting it stops
    /// hiding anything: 9,999 tokens is still four digits worth reading.
    #[test]
    fn a_token_count_compacts_only_when_it_has_to() {
        assert_eq!(tokens_text(0), "0");
        assert_eq!(tokens_text(9_999), "9999");
        assert_eq!(tokens_text(10_000), "10.0k");
        assert_eq!(tokens_text(93_668), "93.7k");
        assert_eq!(tokens_text(2_400_000), "2.40M");
    }

    /// A bar says how wide it is in the unit it actually is, so a 30-day window does not report
    /// "one bar per 86400s".
    #[test]
    fn a_bar_says_how_wide_it_is() {
        assert_eq!(span_text(30_000), "30s");
        assert_eq!(span_text(900_000), "15m");
        assert_eq!(span_text(3_600_000), "1h");
        assert_eq!(span_text(86_400_000), "1d");
    }

    /// A cut body says it was cut.
    #[test]
    fn a_truncated_body_says_so() {
        assert_eq!(size_note(120, 120), "120 characters, all of them");
        assert_eq!(
            size_note(32_000, 250_000),
            "32000 of 250000 characters — the head and the tail"
        );
    }

    /// A byte count the journal reports as negative is a broken row, not a small one.
    #[test]
    fn a_negative_byte_count_reads_as_nothing() {
        assert_eq!(as_u64(0), 0);
        assert_eq!(as_u64(4_096), 4_096);
        assert_eq!(as_u64(-1), 0);
    }
}
