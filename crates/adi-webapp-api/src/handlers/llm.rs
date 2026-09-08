//! `/api/llm/*` — the LLM gateway's journal: every model API call made through `llm.adi`, and
//! what was in it.
//!
//! The rows are written by the `llm-gateway` service (`crates/adi-llm-gateway`), which is the
//! only thing that may ever write them. This module reads the record of what was sent, so every
//! query runs on a **read-only** connection: a bug here cannot edit history.
//!
//! The work that isn't a `select` is the *analysis*, and it happens here rather than in the
//! browser: a request body is a quarter of a megabyte of prompt, and the page wants the shape of
//! it — the system blocks, the messages, the tools offered, the answer reassembled from a
//! thousand SSE deltas, what it cost in tokens. Sending the raw bodies over to be parsed in wasm
//! would ship megabytes to say a few hundred bytes' worth of structure.
//!
//! Token counts come out of the stored bodies rather than a column of their own, deliberately:
//! the gateway keeps the wire bytes, so a number read back out of them cannot drift from what the
//! provider actually said.

use adi_db::{Connection, Db, Error as DbStoreError};
use serde_json::Value;

use crate::types::{
    LlmBlockDto, LlmBucketDto, LlmCallDetail, LlmCallDto, LlmCallRef, LlmCalls, LlmEventCountDto,
    LlmGroupDto, LlmHeaderDto, LlmQuery, LlmSummary, LlmTokens,
};

use super::response::{FromBody, Response, ok_json};

/// The journal table. Absent until the gateway has served its first call, which is a normal
/// state and not an error — see [`table_exists`].
const TABLE: &str = "llm_gateway_requests";

/// How much of a response body is scanned for token counts in the *list* path: both wire formats
/// put their totals near the start (the opening `message_start`) and at the very end (the final
/// `message_delta` / `usage`), so the head and the tail are enough, and a 2 MB body never has to
/// be read to say what it cost. The detail path parses properly instead.
const HEAD: usize = 4000;
const TAIL: usize = 3000;

/// How many calls the summary reads to compute its numbers, and the hard ceiling on a list. A
/// window wider than this is reported honestly (`read` vs `matched`) rather than presented as a
/// total.
const SUMMARY_LIMIT: usize = 2000;
const LIST_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1000;

/// How much of one conversation block travels to the page. Long enough to read a prompt's intent,
/// bounded so a 200 KB tool result doesn't cross the wire twice — `chars` carries the real length,
/// and the raw body panel has the rest.
const BLOCK_CHARS: usize = 4000;

/// How much of each raw body the detail view carries. The structured analysis is the point of the
/// page; this is for confirming it against the bytes.
const RAW_CHARS: usize = 32_000;

/// Roughly how many bars the activity histogram draws. The bucket span is rounded to something a
/// person reads in clock terms (5m, 1h, 6h, a day), so the count only approximates this.
const BUCKETS: i64 = 30;

/// `POST /api/llm/summary` — what the traffic in a window looks like, by provider, by model and
/// by client, with an hourly-ish histogram and the totals.
#[must_use]
pub fn llm_summary(store: &Db, body: &[u8]) -> Response {
    let query = require!(body, LlmQuery);
    let (window, since) = resolve_window(query.window.as_deref());
    let conn = match journal(store) {
        Ok(Some(conn)) => conn,
        Ok(None) => return ok_json(&empty_summary(window, since)),
        Err(e) => return Response::from(&e),
    };

    let filter = Filter::new(&query, since);
    let limit = query.limit.unwrap_or(SUMMARY_LIMIT).clamp(1, SUMMARY_LIMIT);
    let calls = match read_calls(&conn, &filter, limit) {
        Ok(calls) => calls,
        Err(e) => return Response::from(&e),
    };
    let (matched, total_rows) = match (count(&conn, &filter), count(&conn, &Filter::everything())) {
        (Ok(matched), Ok(total)) => (matched, total),
        (Err(e), _) | (_, Err(e)) => return Response::from(&e),
    };

    let refs: Vec<&LlmCallDto> = calls.iter().collect();
    ok_json(&LlmSummary {
        window,
        since,
        totals: group("all", &refs),
        providers: rollup(&calls, |c| Some(c.provider.clone())),
        models: rollup(&calls, |c| c.model.clone()),
        clients: rollup(&calls, |c| c.agent.clone()),
        buckets: histogram(&calls, since),
        known_providers: distinct(&conn, "provider").unwrap_or_default(),
        known_models: distinct(&conn, "model").unwrap_or_default(),
        total_rows,
        read: as_i64(calls.len()),
        matched,
    })
}

/// `POST /api/llm/calls` — the filtered call list, newest first. Bodies stay behind; they are
/// what [`llm_call`] is for.
#[must_use]
pub fn llm_calls(store: &Db, body: &[u8]) -> Response {
    let query = require!(body, LlmQuery);
    let conn = match journal(store) {
        Ok(Some(conn)) => conn,
        Ok(None) => {
            return ok_json(&LlmCalls {
                calls: Vec::new(),
                matched: 0,
            });
        }
        Err(e) => return Response::from(&e),
    };

    let (_, since) = resolve_window(query.window.as_deref());
    let filter = Filter::new(&query, since);
    let limit = query.limit.unwrap_or(LIST_LIMIT).clamp(1, MAX_LIMIT);
    match (read_calls(&conn, &filter, limit), count(&conn, &filter)) {
        (Ok(calls), Ok(matched)) => ok_json(&LlmCalls { calls, matched }),
        (Err(e), _) | (_, Err(e)) => Response::from(&e),
    }
}

/// `POST /api/llm/call` — one call, read: its prompt as blocks, its answer reassembled, the
/// parameters it set, both header sets, and a bounded slice of each raw body.
#[must_use]
pub fn llm_call(store: &Db, body: &[u8]) -> Response {
    let req = require!(body, LlmCallRef);
    let conn = match journal(store) {
        Ok(Some(conn)) => conn,
        Ok(None) => return super::response::error(404, "no gateway journal on this machine yet"),
        Err(e) => return Response::from(&e),
    };

    let sql = format!(
        "select id, started_at, duration_ms, provider, method, target, status, model, streamed,
                client, json_extract(request_headers, '$.\"user-agent\"'),
                coalesce(length(request_body), 0), response_bytes, error,
                upstream, request_headers, response_headers, request_body, response_body
           from {TABLE} where id = ?1"
    );
    let result = match adi_db::query_on_json(&conn, &sql, &[Value::from(req.id)]) {
        Ok(result) => result,
        Err(e) => return Response::from(&e),
    };
    let Some(row) = result.rows.first() else {
        return super::response::error(404, &format!("no call {} in the journal", req.id));
    };

    let request_body = text(row, 17).unwrap_or_default();
    let response_body = text(row, 18).unwrap_or_default();
    let mut call = call_row(row);
    let request = analyze_request(&request_body);
    let response = analyze_response(&response_body, call.streamed);
    // The exact counts the provider stated, in place of the list's head-and-tail estimate.
    if response.tokens != LlmTokens::default() {
        call.tokens = response.tokens;
    }

    ok_json(&LlmCallDetail {
        call,
        upstream: text(row, 14).unwrap_or_default(),
        request_headers: headers(text(row, 15).as_deref()),
        response_headers: headers(text(row, 16).as_deref()),
        params: request.params,
        system: request.system,
        messages: request.messages,
        tools: request.tools,
        answer: response.answer,
        thinking: response.thinking,
        tool_calls: response.tool_calls,
        stop_reason: response.stop_reason,
        events: response.events,
        request_chars: request_body.chars().count(),
        request_body: head(&request_body, RAW_CHARS),
        response_chars: response_body.chars().count(),
        response_body: head(&response_body, RAW_CHARS),
    })
}

// ------------------------------------------------------------------ filter

/// A `where` clause and its bound parameters, shared by the list, the summary and the counts.
struct Filter {
    sql: String,
    params: Vec<Value>,
}

impl Filter {
    /// The clause for one query. Every field narrows; a filter with nothing set is every row in
    /// the window.
    fn new(query: &LlmQuery, since: Option<i64>) -> Self {
        let mut clauses: Vec<String> = Vec::new();
        let mut params: Vec<Value> = Vec::new();

        if let Some(from) = since {
            clauses.push("started_at >= ?".to_string());
            params.push(Value::from(from));
        }
        if let Some(provider) = non_empty(query.provider.as_deref()) {
            clauses.push("provider = ?".to_string());
            params.push(Value::from(provider));
        }
        if let Some(model) = non_empty(query.model.as_deref()) {
            clauses.push("model = ?".to_string());
            params.push(Value::from(model));
        }
        if query.errors {
            clauses.push("(status is null or status >= 400 or error is not null)".to_string());
        }
        if let Some(q) = non_empty(query.q.as_deref()) {
            // The prompt itself is searchable: the request body is where "what did I ask it"
            // lives, and it is the reason to come to this page at all.
            clauses.push("(target like ? or model like ? or request_body like ?)".to_string());
            let like = format!("%{q}%");
            params.push(Value::from(like.clone()));
            params.push(Value::from(like.clone()));
            params.push(Value::from(like));
        }

        Self {
            sql: if clauses.is_empty() {
                String::new()
            } else {
                format!("where {}", clauses.join(" and "))
            },
            params,
        }
    }

    /// The unfiltered whole journal — what `total_rows` counts.
    fn everything() -> Self {
        Self {
            sql: String::new(),
            params: Vec::new(),
        }
    }
}

/// A trimmed value, or `None` when it was blank — a filter nobody set.
fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToString::to_string)
}

/// A window name and the timestamp it starts at. An unknown name reads as the default rather
/// than failing: this is a view, and a stale bookmark should still show something.
fn resolve_window(name: Option<&str>) -> (String, Option<i64>) {
    let name = non_empty(name).unwrap_or_else(|| "24h".to_string());
    let back_ms: Option<i64> = match name.as_str() {
        "1h" => Some(3_600_000),
        "7d" => Some(604_800_000),
        "30d" => Some(2_592_000_000),
        "all" => None,
        _ => Some(86_400_000),
    };
    let name = match name.as_str() {
        "1h" | "7d" | "30d" | "all" => name,
        _ => "24h".to_string(),
    };
    (name.clone(), back_ms.map(|back| now_ms() - back))
}

/// Now, in milliseconds since the epoch — the same clock the gateway stamps rows with.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// A read-only connection to the store, or `None` when there is no journal to read.
///
/// Two ways to have none, both ordinary on a machine the gateway has never served a call on: no
/// global database at all, and a database without the table. Neither is an error — it is an empty
/// page — so only a real failure to open comes back as one.
fn journal(store: &Db) -> Result<Option<Connection>, DbStoreError> {
    let conn = match store.connect_readonly(None) {
        Ok(conn) => conn,
        Err(DbStoreError::NotFound(_)) => return Ok(None),
        Err(e) => return Err(e),
    };
    let exists = adi_db::query_on_json(
        &conn,
        "select 1 from sqlite_master where type = 'table' and name = ?1",
        &[Value::from(TABLE)],
    )
    .is_ok_and(|r| !r.rows.is_empty());
    Ok(exists.then_some(conn))
}

/// The summary a machine with no journal answers: the shape the page expects, all zeroes.
fn empty_summary(window: String, since: Option<i64>) -> LlmSummary {
    LlmSummary {
        window,
        since,
        totals: group("all", &[]),
        providers: Vec::new(),
        models: Vec::new(),
        clients: Vec::new(),
        buckets: Vec::new(),
        known_providers: Vec::new(),
        known_models: Vec::new(),
        total_rows: 0,
        read: 0,
        matched: 0,
    }
}

// -------------------------------------------------------------------- rows

/// The list projection, in the order [`call_row`] reads it.
fn list_columns() -> String {
    format!(
        "select id, started_at, duration_ms, provider, method, target, status, model, streamed,
                client, json_extract(request_headers, '$.\"user-agent\"'),
                coalesce(length(request_body), 0), response_bytes, error,
                substr(response_body, 1, {HEAD}) || substr(response_body, -{TAIL})
           from {TABLE}"
    )
}

/// Read the newest `limit` calls the filter matches.
fn read_calls(
    conn: &Connection,
    filter: &Filter,
    limit: usize,
) -> Result<Vec<LlmCallDto>, DbStoreError> {
    let sql = format!(
        "{} {} order by id desc limit {limit}",
        list_columns(),
        filter.sql
    );
    let result = adi_db::query_on_json(conn, &sql, &filter.params)?;
    Ok(result.rows.iter().map(|row| call_row(row)).collect())
}

/// How many rows the filter matches, whatever the read limit was.
fn count(conn: &Connection, filter: &Filter) -> Result<i64, DbStoreError> {
    let sql = format!("select count(*) from {TABLE} {}", filter.sql);
    let result = adi_db::query_on_json(conn, &sql, &filter.params)?;
    Ok(result.rows.first().and_then(|r| int(r, 0)).unwrap_or(0))
}

/// Every distinct value of one column, for the filter menus — including providers and models
/// that have been quiet all day, which is exactly when you go looking for them.
fn distinct(conn: &Connection, column: &str) -> Result<Vec<String>, DbStoreError> {
    let sql =
        format!("select distinct {column} from {TABLE} where {column} is not null order by 1");
    let result = adi_db::query_on_json(conn, &sql, &[])?;
    Ok(result
        .rows
        .iter()
        .filter_map(|row| text(row, 0))
        .filter(|v| !v.is_empty())
        .collect())
}

/// One journal row as a list entry. The 15th column, when the projection carries it, is the
/// head-and-tail sample the token estimate is read out of.
fn call_row(row: &[Value]) -> LlmCallDto {
    LlmCallDto {
        id: int(row, 0).unwrap_or_default(),
        started_at: int(row, 1).unwrap_or_default(),
        duration_ms: int(row, 2),
        provider: text(row, 3).unwrap_or_default(),
        method: text(row, 4).unwrap_or_default(),
        target: text(row, 5).unwrap_or_default(),
        status: int(row, 6),
        model: text(row, 7).filter(|m| !m.is_empty()),
        streamed: int(row, 8).unwrap_or_default() != 0,
        client: text(row, 9),
        agent: text(row, 10),
        request_bytes: int(row, 11).unwrap_or_default(),
        response_bytes: int(row, 12),
        error: text(row, 13).filter(|e| !e.is_empty()),
        tokens: text(row, 14).map(|s| scan_tokens(&s)).unwrap_or_default(),
    }
}

/// One cell as an integer — SQLite hands back a JSON number, or null.
fn int(row: &[Value], index: usize) -> Option<i64> {
    row.get(index).and_then(Value::as_i64)
}

/// One cell as text. A number in a text column (a port, an id) still reads.
fn text(row: &[Value], index: usize) -> Option<String> {
    match row.get(index) {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Null) | None => None,
        Some(other) => Some(other.to_string()),
    }
}

// ------------------------------------------------------------------ rollup

/// Roll the read calls up by one dimension, busiest first. Calls the key doesn't apply to (a
/// request with no model, a client that sent no user-agent) are left out of that breakdown
/// rather than bucketed under a made-up name.
///
/// A linear scan rather than a map: a machine talks to a handful of providers and models, and
/// keeping the buckets in a `Vec` keeps the whole function readable.
fn rollup(calls: &[LlmCallDto], key: impl Fn(&LlmCallDto) -> Option<String>) -> Vec<LlmGroupDto> {
    let mut buckets: Vec<(String, Vec<&LlmCallDto>)> = Vec::new();
    for call in calls {
        let Some(name) = key(call).filter(|n| !n.is_empty()) else {
            continue;
        };
        match buckets.iter_mut().find(|(n, _)| *n == name) {
            Some((_, bucket)) => bucket.push(call),
            None => buckets.push((name, vec![call])),
        }
    }
    let mut groups: Vec<LlmGroupDto> = buckets
        .iter()
        .map(|(name, calls)| group(name, calls))
        .collect();
    groups.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.name.cmp(&b.name)));
    groups
}

/// A count as the wire type. Every count here is a row tally, so the saturating conversion is
/// unreachable in practice and never worth a panic.
fn as_i64(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// One group's row: counts, bytes, the duration spread, and the tokens it burned.
fn group(name: &str, calls: &[&LlmCallDto]) -> LlmGroupDto {
    let mut durations: Vec<i64> = calls.iter().filter_map(|c| c.duration_ms).collect();
    durations.sort_unstable();
    let mut tokens = LlmTokens::default();
    for call in calls {
        tokens.add(call.tokens);
    }
    LlmGroupDto {
        name: name.to_string(),
        calls: as_i64(calls.len()),
        failed: as_i64(calls.iter().filter(|c| c.failed()).count()),
        streamed: as_i64(calls.iter().filter(|c| c.streamed).count()),
        request_bytes: calls.iter().map(|c| c.request_bytes).sum(),
        response_bytes: calls.iter().filter_map(|c| c.response_bytes).sum(),
        median_ms: percentile(&durations, 50),
        p95_ms: percentile(&durations, 95),
        slowest_ms: durations.last().copied(),
        last_seen: calls.iter().map(|c| c.started_at).max().unwrap_or_default(),
        tokens,
    }
}

/// The `p`th percentile of an already-sorted slice — nearest rank, so the answer is always a
/// duration that really happened rather than an interpolation between two that didn't.
fn percentile(sorted: &[i64], p: usize) -> Option<i64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (sorted.len() * p).div_ceil(100).max(1) - 1;
    sorted.get(rank.min(sorted.len() - 1)).copied()
}

/// Calls per bucket over the window, oldest first — the shape of the traffic.
///
/// The window's own bounds set the axis, so an idle hour is a gap in the bars rather than a
/// missing one. An unbounded window spans the calls that were actually read.
fn histogram(calls: &[LlmCallDto], since: Option<i64>) -> Vec<LlmBucketDto> {
    let Some(newest) = calls.iter().map(|c| c.started_at).max() else {
        return Vec::new();
    };
    let oldest = calls
        .iter()
        .map(|c| c.started_at)
        .min()
        .unwrap_or(newest)
        .min(newest);
    let start = since.unwrap_or(oldest).min(oldest);
    let end = now_ms().max(newest);
    let span = bucket_span((end - start).max(1));
    let first = start - start.rem_euclid(span);

    let count = usize::try_from((end - first) / span + 1)
        .unwrap_or(1)
        .min(240);
    let mut buckets: Vec<LlmBucketDto> = (0..count)
        .map(|i| LlmBucketDto {
            at: first + span * as_i64(i),
            span_ms: span,
            calls: 0,
            failed: 0,
            tokens: 0,
        })
        .collect();
    for call in calls {
        let index = usize::try_from((call.started_at - first) / span).unwrap_or(0);
        if let Some(bucket) = buckets.get_mut(index) {
            bucket.calls += 1;
            bucket.failed += i64::from(call.failed());
            bucket.tokens += call.tokens.prompt() + call.tokens.output;
        }
    }
    buckets
}

/// A bucket width a person reads in clock terms, for a window of `span` milliseconds.
fn bucket_span(span: i64) -> i64 {
    const STEPS: [i64; 9] = [
        60_000,      // a minute
        300_000,     // 5 minutes
        900_000,     // a quarter hour
        3_600_000,   // an hour
        10_800_000,  // 3 hours
        21_600_000,  // 6 hours
        43_200_000,  // half a day
        86_400_000,  // a day
        604_800_000, // a week
    ];
    let want = (span / BUCKETS).max(1);
    STEPS
        .into_iter()
        .find(|step| *step >= want)
        .unwrap_or(604_800_000)
}

// ------------------------------------------------------------------ tokens

/// Token counts scraped out of a body sample, for the list.
///
/// Both wire formats state their totals in JSON — Anthropic in `message_start` and the final
/// `message_delta`, OpenAI in the trailing `usage` object — so the largest value each name takes
/// is its total (a streamed `output_tokens` is cumulative). The leading quote in each needle is
/// what stops `cache_creation_input_tokens` being read as `input_tokens`, and the trailing colon
/// what stops `output_tokens_details` being read as `output_tokens`.
fn scan_tokens(text: &str) -> LlmTokens {
    let cached =
        max_after(text, "cache_read_input_tokens").or_else(|| max_after(text, "cached_tokens"));
    let prompt = max_after(text, "prompt_tokens");
    LlmTokens {
        // OpenAI's `prompt_tokens` is the whole prompt, cache included; Anthropic's
        // `input_tokens` is only the fresh part. Report the fresh part either way.
        input: max_after(text, "input_tokens")
            .or_else(|| prompt.map(|p| p - cached.unwrap_or(0)))
            .unwrap_or(0),
        cached: cached.unwrap_or(0),
        cache_write: max_after(text, "cache_creation_input_tokens").unwrap_or(0),
        output: max_after(text, "output_tokens")
            .or_else(|| max_after(text, "completion_tokens"))
            .unwrap_or(0),
    }
}

/// The largest number written as `"<name>":<digits>` anywhere in `text`.
fn max_after(text: &str, name: &str) -> Option<i64> {
    let needle = format!("\"{name}\":");
    let mut best: Option<i64> = None;
    let mut rest = text;
    while let Some(at) = rest.find(&needle) {
        let after = &rest[at + needle.len()..];
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(n) = digits.parse::<i64>() {
            best = Some(best.map_or(n, |b: i64| b.max(n)));
        }
        rest = after;
    }
    best
}

/// The counts a provider's own `usage` object states — the exact figure, for the detail view.
fn usage_tokens(usage: &Value) -> LlmTokens {
    let n = |key: &str| usage.get(key).and_then(Value::as_i64);
    let cached = n("cache_read_input_tokens").or_else(|| {
        usage
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(Value::as_i64)
    });
    LlmTokens {
        input: n("input_tokens")
            .or_else(|| n("prompt_tokens").map(|p| p - cached.unwrap_or(0)))
            .unwrap_or(0),
        cached: cached.unwrap_or(0),
        cache_write: n("cache_creation_input_tokens").unwrap_or(0),
        output: n("output_tokens")
            .or_else(|| n("completion_tokens"))
            .unwrap_or(0),
    }
}

// ----------------------------------------------------------------- request

/// What a request body says, once read: its settings, its system prompt, its conversation and
/// the tools it offered.
struct RequestAnalysis {
    params: Vec<LlmHeaderDto>,
    system: Vec<LlmBlockDto>,
    messages: Vec<LlmBlockDto>,
    tools: Vec<String>,
}

/// Read a request body. A body that isn't JSON (or isn't a chat request at all — a `GET
/// /v1/models` has none) analyses to nothing, and the raw panel is what shows it.
fn analyze_request(body: &str) -> RequestAnalysis {
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(body) else {
        return RequestAnalysis {
            params: Vec::new(),
            system: Vec::new(),
            messages: Vec::new(),
            tools: Vec::new(),
        };
    };

    // Everything that isn't the conversation is a parameter: model, max_tokens, stream,
    // temperature, thinking, and whatever else the SDK of the day has started sending.
    let params = root
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "messages" | "system" | "tools" | "input"))
        .map(|(key, value)| LlmHeaderDto {
            name: key.clone(),
            value: head(&compact(value), 300),
        })
        .collect();

    let system = root
        .get("system")
        .map(|v| blocks("system", v))
        .unwrap_or_default();

    let mut messages = Vec::new();
    if let Some(Value::Array(list)) = root.get("messages") {
        for message in list {
            let role = message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("user")
                .to_string();
            match message.get("content") {
                Some(content) => messages.extend(blocks(&role, content)),
                None => messages.extend(blocks(&role, message)),
            }
        }
    }

    let tools = match root.get("tools") {
        Some(Value::Array(list)) => list.iter().filter_map(tool_name).collect(),
        _ => Vec::new(),
    };

    RequestAnalysis {
        params,
        system,
        messages,
        tools,
    }
}

/// A tool's name, in either dialect: Anthropic states it at the top level, OpenAI nests it under
/// `function`.
fn tool_name(tool: &Value) -> Option<String> {
    tool.get("name")
        .and_then(Value::as_str)
        .or_else(|| {
            tool.get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
        })
        .map(ToString::to_string)
}

/// One message's content as blocks. A string content is one text block; an array is its blocks,
/// each read for what it is.
fn blocks(role: &str, content: &Value) -> Vec<LlmBlockDto> {
    match content {
        Value::String(text) => vec![block(role, "text", None, text)],
        Value::Array(list) => list.iter().map(|item| content_block(role, item)).collect(),
        other => vec![block(role, "other", None, &compact(other))],
    }
}

/// One content block, by its `type`.
fn content_block(role: &str, item: &Value) -> LlmBlockDto {
    let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "text" => block(
            role,
            "text",
            None,
            item.get("text").and_then(Value::as_str).unwrap_or(""),
        ),
        "thinking" => block(
            role,
            "thinking",
            None,
            item.get("thinking").and_then(Value::as_str).unwrap_or(""),
        ),
        "tool_use" | "server_tool_use" => block(
            role,
            "tool_use",
            item.get("name").and_then(Value::as_str),
            &compact(item.get("input").unwrap_or(&Value::Null)),
        ),
        "tool_result" => block(
            "tool",
            "tool_result",
            item.get("tool_use_id").and_then(Value::as_str),
            &flatten(item.get("content").unwrap_or(&Value::Null)),
        ),
        "image" => block(
            role,
            "image",
            None,
            item.get("source")
                .and_then(|s| s.get("media_type"))
                .and_then(Value::as_str)
                .unwrap_or("image"),
        ),
        "" => block(role, "text", None, &flatten(item)),
        other => block(role, "other", Some(other), &compact(item)),
    }
}

/// A block, truncated for transport with its real length kept.
fn block(role: &str, kind: &str, name: Option<&str>, text: &str) -> LlmBlockDto {
    LlmBlockDto {
        role: role.to_string(),
        kind: kind.to_string(),
        name: name.map(ToString::to_string),
        text: head(text, BLOCK_CHARS),
        chars: text.chars().count(),
    }
}

/// A tool result's content as plain text — it may be a string, or blocks of its own.
fn flatten(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(list) => list
            .iter()
            .map(|item| match item.get("text").and_then(Value::as_str) {
                Some(text) => text.to_string(),
                None => compact(item),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => compact(other),
    }
}

/// A JSON value as one line — strings unquoted, everything else as JSON.
fn compact(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The first `limit` characters, with a note when there were more. Character-wise, so a prompt
/// that ends mid-emoji still renders.
fn head(text: &str, limit: usize) -> String {
    let total = text.chars().count();
    if total <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit).collect();
    out.push_str(&format!("\n… +{} more characters", total - limit));
    out
}

/// A stored header map as sorted pairs. Credentials were already replaced with `<redacted>` on
/// the way in, so nothing here is a secret — see `adi-llm-gateway`.
fn headers(json: Option<&str>) -> Vec<LlmHeaderDto> {
    let Some(Ok(Value::Object(map))) = json.map(serde_json::from_str::<Value>) else {
        return Vec::new();
    };
    map.into_iter()
        .map(|(name, value)| LlmHeaderDto {
            name,
            value: head(&compact(&value), 400),
        })
        .collect()
}

// ---------------------------------------------------------------- response

/// What came back, once the stream is put back together.
struct ResponseAnalysis {
    answer: String,
    thinking: String,
    tool_calls: Vec<LlmBlockDto>,
    stop_reason: Option<String>,
    events: Vec<LlmEventCountDto>,
    tokens: LlmTokens,
}

impl ResponseAnalysis {
    fn empty() -> Self {
        Self {
            answer: String::new(),
            thinking: String::new(),
            tool_calls: Vec::new(),
            stop_reason: None,
            events: Vec::new(),
            tokens: LlmTokens::default(),
        }
    }
}

/// Read a response body: reassemble a stream delta by delta, or read a whole JSON answer.
fn analyze_response(body: &str, streamed: bool) -> ResponseAnalysis {
    if body.trim().is_empty() {
        return ResponseAnalysis::empty();
    }
    if streamed || body.starts_with("event:") || body.starts_with("data:") {
        return analyze_stream(body);
    }
    match serde_json::from_str::<Value>(body) {
        Ok(value) => analyze_whole(&value),
        // Not JSON and not a stream — an upstream error page, say. The raw panel shows it.
        Err(_) => ResponseAnalysis::empty(),
    }
}

/// A server-sent-event stream, put back together: the text deltas are the answer, the thinking
/// deltas the reasoning that preceded it, and the `input_json` deltas each tool call's arguments.
fn analyze_stream(body: &str) -> ResponseAnalysis {
    let mut out = ResponseAnalysis::empty();
    let mut counts: Vec<(String, i64)> = Vec::new();
    // OpenAI sends bare data frames with no `event:` line at all; those are counted as one kind.
    let mut frames: i64 = 0;
    // Tool calls arrive as a `content_block_start` naming the tool, then arguments in fragments
    // under that block's index.
    let mut tools: Vec<(usize, String, String)> = Vec::new();

    for line in body.lines() {
        if let Some(name) = line.strip_prefix("event:") {
            let name = name.trim().to_string();
            match counts.iter_mut().find(|(n, _)| *n == name) {
                Some(entry) => entry.1 += 1,
                None => counts.push((name, 1)),
            }
            continue;
        }
        let Some(payload) = line.strip_prefix("data:").map(str::trim) else {
            continue;
        };
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        frames += 1;
        let Ok(event) = serde_json::from_str::<Value>(payload) else {
            continue;
        };

        // Anthropic: typed events, with usage stated twice — at the start and at the end.
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "message_start" => {
                if let Some(usage) = event.get("message").and_then(|m| m.get("usage")) {
                    out.tokens = merge_tokens(out.tokens, usage_tokens(usage));
                }
            }
            "content_block_start" => {
                let block = event.get("content_block");
                if block.and_then(|b| b.get("type")).and_then(Value::as_str) == Some("tool_use") {
                    let index = event
                        .get("index")
                        .and_then(Value::as_u64)
                        .and_then(|i| usize::try_from(i).ok())
                        .unwrap_or(0);
                    let name = block
                        .and_then(|b| b.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_string();
                    tools.push((index, name, String::new()));
                }
            }
            "content_block_delta" => {
                let delta = event.get("delta");
                let kind = delta
                    .and_then(|d| d.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let field = match kind {
                    "text_delta" => Some("text"),
                    "thinking_delta" => Some("thinking"),
                    "input_json_delta" => Some("partial_json"),
                    _ => None,
                };
                let Some(piece) = field
                    .and_then(|f| delta.and_then(|d| d.get(f)))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                match kind {
                    "text_delta" => out.answer.push_str(piece),
                    "thinking_delta" => out.thinking.push_str(piece),
                    _ => {
                        let index = event
                            .get("index")
                            .and_then(Value::as_u64)
                            .and_then(|i| usize::try_from(i).ok())
                            .unwrap_or(0);
                        if let Some(tool) = tools.iter_mut().find(|(i, _, _)| *i == index) {
                            tool.2.push_str(piece);
                        }
                    }
                }
            }
            "message_delta" => {
                if let Some(reason) = event
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    out.stop_reason = Some(reason.to_string());
                }
                if let Some(usage) = event.get("usage") {
                    out.tokens = merge_tokens(out.tokens, usage_tokens(usage));
                }
            }
            "error" => {
                if let Some(message) = event
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                {
                    out.answer.push_str(message);
                }
            }
            // OpenAI: untyped chunks, the answer in `choices[].delta.content`.
            _ => {
                if let Some(choice) = event
                    .get("choices")
                    .and_then(Value::as_array)
                    .and_then(|c| c.first())
                {
                    if let Some(piece) = choice
                        .get("delta")
                        .and_then(|d| d.get("content"))
                        .and_then(Value::as_str)
                    {
                        out.answer.push_str(piece);
                    }
                    if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                        out.stop_reason = Some(reason.to_string());
                    }
                }
                if let Some(usage) = event.get("usage").filter(|u| u.is_object()) {
                    out.tokens = merge_tokens(out.tokens, usage_tokens(usage));
                }
            }
        }
    }

    for (_, name, arguments) in tools {
        out.tool_calls
            .push(block("assistant", "tool_use", Some(&name), &arguments));
    }
    if counts.is_empty() && frames > 0 {
        counts.push(("data".to_string(), frames));
    }
    out.events = counts
        .into_iter()
        .map(|(name, count)| LlmEventCountDto { name, count })
        .collect();
    out
}

/// A whole JSON answer — the non-streamed shape of both dialects.
fn analyze_whole(value: &Value) -> ResponseAnalysis {
    let mut out = ResponseAnalysis::empty();
    if let Some(usage) = value.get("usage") {
        out.tokens = usage_tokens(usage);
    }
    if let Some(reason) = value.get("stop_reason").and_then(Value::as_str) {
        out.stop_reason = Some(reason.to_string());
    }

    // Anthropic: one `content` array of blocks.
    if let Some(Value::Array(content)) = value.get("content") {
        for item in content {
            let block = content_block("assistant", item);
            match block.kind.as_str() {
                "text" => push_line(&mut out.answer, &block.text),
                "thinking" => push_line(&mut out.thinking, &block.text),
                _ => out.tool_calls.push(block),
            }
        }
    }

    // OpenAI: one `choices` array, the answer already assembled.
    if let Some(choice) = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
    {
        if let Some(text) = choice
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
        {
            push_line(&mut out.answer, text);
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            out.stop_reason = Some(reason.to_string());
        }
        if let Some(Value::Array(calls)) = choice.get("message").and_then(|m| m.get("tool_calls")) {
            for call in calls {
                let name = tool_name(call).unwrap_or_else(|| "tool".to_string());
                let arguments = call
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .map_or_else(String::new, compact);
                out.tool_calls
                    .push(block("assistant", "tool_use", Some(&name), &arguments));
            }
        }
    }
    out
}

/// Append a paragraph, keeping the blocks apart.
fn push_line(buffer: &mut String, text: &str) {
    if !buffer.is_empty() {
        buffer.push_str("\n\n");
    }
    buffer.push_str(text);
}

/// Take the larger of each count. A stream states its usage twice — the opening figures are
/// partial and the closing ones final — and `output_tokens` grows as it goes.
fn merge_tokens(a: LlmTokens, b: LlmTokens) -> LlmTokens {
    LlmTokens {
        input: a.input.max(b.input),
        cached: a.cached.max(b.cached),
        cache_write: a.cache_write.max(b.cache_write),
        output: a.output.max(b.output),
    }
}

impl FromBody for LlmQuery {
    const EXPECTED: &'static str = "expected JSON body { \"window\"?: \"1h|24h|7d|30d|all\", \"provider\"?: \"…\", \"model\"?: \"…\", \"errors\"?: true, \"q\"?: \"…\", \"limit\"?: 100 }";

    // An empty body is the default view — the last day of everything.
    fn on_empty() -> Option<Self> {
        Some(Self::default())
    }
}

impl FromBody for LlmCallRef {
    const EXPECTED: &'static str = "expected JSON body { \"id\": 42 }";

    fn is_complete(&self) -> bool {
        self.id > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_db() -> Db {
        let root = std::env::temp_dir().join(format!(
            "adi-webapp-api-llm-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Db::with_config(adi_config::Config::with_root(root))
    }

    /// The journal as the gateway creates it, plus one call.
    fn seed(store: &Db) {
        store
            .exec(
                None,
                "create table llm_gateway_requests (
                     id integer primary key,
                     started_at integer not null,
                     duration_ms integer,
                     provider text not null,
                     method text not null,
                     target text not null,
                     upstream text not null,
                     status integer,
                     model text,
                     streamed integer not null default 0,
                     client text,
                     request_headers text not null,
                     request_body text,
                     response_headers text,
                     response_body text,
                     response_bytes integer,
                     error text
                 )",
                &[],
            )
            .unwrap();

        let request = json!({
            "model": "claude-opus-5",
            "max_tokens": 64000,
            "stream": true,
            "system": [{"type": "text", "text": "You are a careful assistant."}],
            "messages": [
                {"role": "user", "content": "what is in the journal?"},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "name": "Bash", "input": {"command": "ls"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": "db/global.db"}
                ]}
            ],
            "tools": [{"name": "Bash"}, {"name": "Read"}]
        })
        .to_string();

        let response = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":7,",
            "\"cache_creation_input_tokens\":120,\"cache_read_input_tokens\":9000,\"output_tokens\":1}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hmm \"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"one \"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"call.\"}}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":7,",
            "\"cache_read_input_tokens\":9000,\"output_tokens\":42,\"output_tokens_details\":{\"thinking_tokens\":9}}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );

        let headers =
            json!({"user-agent": "claude-cli/2.1", "authorization": "<redacted>"}).to_string();
        store
            .exec_json(
                None,
                "insert into llm_gateway_requests
                   (id, started_at, duration_ms, provider, method, target, upstream, status, model,
                    streamed, client, request_headers, request_body, response_headers,
                    response_body, response_bytes, error)
                 values (1, ?1, 900, 'anthropic', 'POST', '/anthropic/v1/messages',
                         'https://api.anthropic.com/v1/messages', 200, 'claude-opus-5', 1,
                         '127.0.0.1', ?2, ?3, '{}', ?4, ?5, null)",
                &[
                    json!(now_ms() - 60_000),
                    json!(headers),
                    json!(request),
                    json!(response),
                    json!(response.len()),
                ],
            )
            .unwrap();
    }

    #[test]
    fn a_machine_with_no_journal_answers_an_empty_summary() {
        // The gateway may never have run here; that is an empty page, not a failure.
        let store = temp_db();
        let Response { status, body } = llm_summary(&store, b"{}");
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["total_rows"], 0);
        assert_eq!(v["providers"].as_array().unwrap().len(), 0);

        let Response { status, body } = llm_calls(&store, b"{}");
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap()["calls"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn the_summary_rolls_traffic_up_by_provider_and_model() {
        let store = temp_db();
        seed(&store);

        let Response { status, body } = llm_summary(&store, br#"{"window":"24h"}"#);
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["totals"]["calls"], 1);
        assert_eq!(v["totals"]["failed"], 0);
        assert_eq!(v["providers"][0]["name"], "anthropic");
        assert_eq!(v["models"][0]["name"], "claude-opus-5");
        assert_eq!(v["clients"][0]["name"], "claude-cli/2.1");
        // Read off the body, not off a column the gateway would have to keep in step.
        assert_eq!(v["totals"]["tokens"]["cached"], 9000);
        assert_eq!(v["totals"]["tokens"]["output"], 42);
        assert_eq!(v["totals"]["median_ms"], 900);
        assert_eq!(v["known_models"][0], "claude-opus-5");
        assert!(
            !v["buckets"].as_array().unwrap().is_empty(),
            "the window has bars: {body}"
        );
    }

    #[test]
    fn a_window_that_excludes_the_call_reports_nothing_in_it() {
        let store = temp_db();
        seed(&store);
        // The seeded call is a minute old, so an unbounded window sees it …
        let Response { body, .. } = llm_summary(&store, br#"{"window":"all"}"#);
        assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["matched"], 1);

        // … and a filter nothing matches answers an empty list rather than everything.
        let Response { body, .. } = llm_calls(&store, br#"{"provider":"openai"}"#);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["calls"].as_array().unwrap().len(), 0);
        assert_eq!(v["matched"], 0);
    }

    #[test]
    fn the_prompt_itself_is_searchable() {
        let store = temp_db();
        seed(&store);
        let Response { body, .. } = llm_calls(&store, br#"{"q":"what is in the journal"}"#);
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap()["calls"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let Response { body, .. } = llm_calls(&store, br#"{"q":"a phrase nobody sent"}"#);
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap()["calls"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn a_call_reads_as_its_prompt_and_its_reassembled_answer() {
        let store = temp_db();
        seed(&store);

        let Response { status, body } = llm_call(&store, br#"{"id":1}"#);
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();

        assert_eq!(v["system"][0]["text"], "You are a careful assistant.");
        assert_eq!(v["messages"][0]["role"], "user");
        assert_eq!(v["messages"][0]["text"], "what is in the journal?");
        assert_eq!(v["messages"][1]["kind"], "tool_use");
        assert_eq!(v["messages"][1]["name"], "Bash");
        // A tool result is the tool speaking, not the user who carried it.
        assert_eq!(v["messages"][2]["role"], "tool");
        assert_eq!(v["messages"][2]["text"], "db/global.db");
        assert_eq!(v["tools"][1], "Read");

        // The deltas are put back together, thinking kept apart from the answer.
        assert_eq!(v["answer"], "one call.");
        assert_eq!(v["thinking"], "hmm ");
        assert_eq!(v["stop_reason"], "end_turn");
        assert_eq!(v["call"]["tokens"]["output"], 42);
        assert_eq!(v["call"]["tokens"]["cache_write"], 120);

        // The settings are separated from the conversation …
        let params = v["params"].as_array().unwrap();
        assert!(
            params
                .iter()
                .any(|p| p["name"] == "max_tokens" && p["value"] == "64000"),
            "{params:?}"
        );
        assert!(
            !params.iter().any(|p| p["name"] == "messages"),
            "the conversation is not a parameter: {params:?}"
        );
        // … and the header the gateway redacted stays redacted.
        let headers = v["request_headers"].as_array().unwrap();
        assert!(
            headers
                .iter()
                .any(|h| h["name"] == "authorization" && h["value"] == "<redacted>"),
            "{headers:?}"
        );

        let events = v["events"].as_array().unwrap();
        assert!(
            events
                .iter()
                .any(|e| e["name"] == "content_block_delta" && e["count"] == 3),
            "{events:?}"
        );
    }

    #[test]
    fn a_call_that_is_not_there_is_a_404() {
        let store = temp_db();
        seed(&store);
        assert_eq!(llm_call(&store, br#"{"id":404}"#).status, 404);
        assert_eq!(llm_call(&store, b"{}").status, 400);
    }

    #[test]
    fn cache_tokens_are_not_read_as_fresh_input() {
        // The bug this guards: `"input_tokens"` matching inside `cache_creation_input_tokens`,
        // which would report a cached prompt as a freshly billed one.
        let sample = r#"{"usage":{"input_tokens":2,"cache_creation_input_tokens":1260,"cache_read_input_tokens":93668,"output_tokens":266,"output_tokens_details":{"thinking_tokens":81}}}"#;
        let tokens = scan_tokens(sample);
        assert_eq!(tokens.input, 2);
        assert_eq!(tokens.cache_write, 1260);
        assert_eq!(tokens.cached, 93668);
        assert_eq!(tokens.output, 266);
        assert_eq!(tokens.cache_hit(), Some(98));
    }

    #[test]
    fn an_openai_answer_reads_the_same_way() {
        let body = json!({
            "choices": [{"message": {"content": "hi there"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 5,
                      "prompt_tokens_details": {"cached_tokens": 60}}
        })
        .to_string();
        let out = analyze_response(&body, false);
        assert_eq!(out.answer, "hi there");
        assert_eq!(out.stop_reason.as_deref(), Some("stop"));
        // `prompt_tokens` counts the cache too; the fresh part is what `input` means here.
        assert_eq!(out.tokens.input, 40);
        assert_eq!(out.tokens.cached, 60);
        assert_eq!(out.tokens.output, 5);
    }

    #[test]
    fn a_streamed_tool_call_is_reassembled_from_its_fragments() {
        let body = concat!(
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"name\":\"Bash\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"ls\\\"}\"}}\n\n",
        );
        let out = analyze_response(body, true);
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].name.as_deref(), Some("Bash"));
        assert_eq!(out.tool_calls[0].text, r#"{"command":"ls"}"#);
    }

    #[test]
    fn percentiles_report_a_duration_that_happened() {
        let sorted = [10, 20, 30, 40, 100];
        assert_eq!(percentile(&sorted, 50), Some(30));
        assert_eq!(percentile(&sorted, 95), Some(100));
        assert_eq!(percentile(&[], 50), None);
    }
}
