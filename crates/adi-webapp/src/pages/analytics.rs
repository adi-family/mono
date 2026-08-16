//! The Global Analytics page (`/extended/analytics`): one screen answering "what has actually run
//! on this machine, and what hasn't?".
//!
//! Every number here is derived on the client from two listings the panel already carries —
//! `/api/agents` (what is *defined*) and `/api/agents/runs/all` (what was *done*) — so the page
//! costs no new endpoint and stays truthful by construction: an agent with no runs is one the run
//! history never mentions, not one a counter forgot to increment.
//!
//! The join is over agent *names*, and it is deliberately outer on both sides. An agent defined but
//! never launched is the question this page exists to answer, and a history whose agent has since
//! been deleted still spent money — dropping either side would report a tidier machine than the one
//! in front of you.

use adi_ui::{EmptyRow, Row as TableRow, Table};
use adi_webapp_api::types::AgentRunInfo;
use leptos::prelude::*;

use crate::state::State;
use crate::ui::{Key, Sort, fmt_date, sort_rows, updated_text};

/// The per-agent table's columns. No trailing blank: this page is a reading surface — every
/// control that acts on an agent lives on the Agents page, and offering half of them here would
/// be a second place to look for them.
///
/// Nine is already at the width a workbench column has with both rails out, which is why the
/// backend and the turn count ride in tooltips rather than taking one each.
pub(crate) const AGENT_COLS: &[&str] = &[
    "Agent", "Project", "State", "Runs", "Running", "Failed", "Waiting", "Cost", "Last run",
];

/// The order the agents table opens in: most runs first. The page's own question is "which of
/// these is carrying the work", and alphabetical answers it only by accident.
pub(crate) const BUSIEST_FIRST: Sort = Sort {
    col: "Runs",
    desc: true,
};

/// How many days the activity chart covers, today included.
const ACTIVITY_DAYS: i64 = 14;

/// The Global Analytics page: the totals across every agent, the last fortnight of activity, a
/// per-agent breakdown, the agents nothing has ever run, and how the runs that finished ended.
pub(crate) fn analytics_view(state: State) -> AnyView {
    view! {
        {move || overview_view(state)}
        {move || activity_view(state)}
        {agents_panel(state)}
        {move || idle_view(state)}
        {move || outcomes_view(state)}
    }
    .into_any()
}

// ---- the numbers -------------------------------------------------------------------------------

/// Everything one agent's history adds up to, beside what its definition says it is.
///
/// Built for both halves of the join, so `defined` and `backend` carry the answer for an agent that
/// exists only in the run history — see the module header.
#[derive(Clone)]
struct AgentStats {
    name: String,
    backend: String,
    project: String,
    /// Whether `/api/agents` still lists this agent. False for a history left behind by a deletion.
    defined: bool,
    /// A pty backend: its live session *is* the run, so it keeps no history and a run count of
    /// zero says nothing about whether it has ever been used.
    interactive: bool,
    /// Whether it has a live pty session or detached process right now (the agent's own flag,
    /// which is the only thing that reports an interactive agent's activity).
    live_session: bool,
    runs: usize,
    running: usize,
    failed: usize,
    /// Conversations stopped on a question nobody has answered yet.
    waiting: usize,
    turns: u64,
    cost_micro: u64,
    /// The most recent moment any of its runs said something; 0 when it has never run.
    last: u64,
}

impl AgentStats {
    /// Whether anything is happening on this agent right now — a live run, or a live pty session.
    fn busy(&self) -> bool {
        self.running > 0 || self.live_session
    }

    /// Whether this agent has never been used, as far as anything on this machine can tell. An
    /// interactive agent is excluded whatever its run count: it keeps no history to be empty.
    fn never_ran(&self) -> bool {
        !self.interactive && self.runs == 0
    }

    /// The one word for what this agent is doing, in the order a reader cares about it: working,
    /// blocked on a person, alive, never used, otherwise quiet.
    fn state(&self) -> &'static str {
        if self.running > 0 {
            "\u{25CF} running"
        } else if self.waiting > 0 {
            "waiting on you"
        } else if self.live_session {
            "\u{25CF} live session"
        } else if !self.defined {
            "removed"
        } else if self.never_ran() {
            "never run"
        } else if self.interactive {
            "no history"
        } else {
            "idle"
        }
    }
}

/// Fold the two listings into one row per agent, newest activity carried along. Returns an empty
/// vector until `/api/agents` has landed — the page shows its loading state rather than a machine
/// with nothing on it.
fn agent_stats(state: State) -> Vec<AgentStats> {
    let defined = state.agents.get().map(|a| a.agents).unwrap_or_default();
    let history = state.all_chats.get().map(|c| c.agents).unwrap_or_default();
    // An agent is filed under a project *id*, and several of those here are UUIDs. Resolving
    // them to names is not only friendlier: a 36-character cell pushes half the table off the
    // right edge, so the id would cost the reader the columns it was meant to sit beside.
    let project_name = |id: &str| -> String {
        state
            .projects
            .get()
            .and_then(|p| {
                p.projects
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| p.name.clone())
            })
            .unwrap_or_else(|| id.to_string())
    };

    let mut rows: Vec<AgentStats> = defined
        .iter()
        .map(|d| {
            let runs = history
                .iter()
                .find(|h| h.name == d.name)
                .map(|h| h.runs.as_slice())
                .unwrap_or_default();
            let mut s = AgentStats {
                name: d.name.clone(),
                backend: d.backend.clone(),
                project: d.project.as_deref().map(&project_name).unwrap_or_default(),
                defined: true,
                interactive: d.executor == "pty",
                live_session: d.running,
                runs: 0,
                running: 0,
                failed: 0,
                waiting: 0,
                turns: 0,
                cost_micro: 0,
                last: 0,
            };
            tally(&mut s, runs);
            s
        })
        .collect();

    // The other half of the join: runs whose agent is gone. They still ran, and their cost was
    // still spent, so the totals above them have to include them.
    for h in &history {
        if defined.iter().any(|d| d.name == h.name) {
            continue;
        }
        let mut s = AgentStats {
            name: h.name.clone(),
            backend: String::new(),
            project: String::new(),
            defined: false,
            interactive: h.interactive,
            live_session: false,
            runs: 0,
            running: 0,
            failed: 0,
            waiting: 0,
            turns: 0,
            cost_micro: 0,
            last: 0,
        };
        tally(&mut s, &h.runs);
        rows.push(s);
    }
    rows
}

/// Add one agent's run history into its row.
fn tally(s: &mut AgentStats, runs: &[AgentRunInfo]) {
    s.runs = runs.len();
    for r in runs {
        if r.running {
            s.running += 1;
        }
        if r.pending_question.is_some() {
            s.waiting += 1;
        }
        if let Some(o) = &r.outcome {
            if o.is_error {
                s.failed += 1;
            }
            s.turns += o.num_turns.unwrap_or(0);
            s.cost_micro += o.cost_micro_usd.unwrap_or(0);
        }
        // A run that has said nothing since it started reports `last_activity: 0` on an older
        // server; its start is then the last thing known to have happened on it.
        s.last = s.last.max(r.last_activity.max(r.started_at));
    }
}

/// Every run on the machine, flattened out of the per-agent history — what the totals and the
/// activity chart count over.
fn all_runs(state: State) -> Vec<AgentRunInfo> {
    state
        .all_chats
        .get()
        .map(|c| c.agents.into_iter().flat_map(|a| a.runs).collect())
        .unwrap_or_default()
}

// ---- the overview tiles ------------------------------------------------------------------------

/// The totals, as a row of tiles: how many agents there are, how many of them are working, how
/// many have never been used, and what the runs behind those numbers came to.
fn overview_view(state: State) -> AnyView {
    let rows = agent_stats(state);
    let runs = all_runs(state);
    if state.agents.get().is_none() {
        return view! {
            <section class="adi-panel">
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">"Overview"</h2>
                </div>
                <div class="adi-empty">"Loading…"</div>
            </section>
        }
        .into_any();
    }

    let agents = rows.iter().filter(|s| s.defined).count();
    let busy = rows.iter().filter(|s| s.busy()).count();
    let never = rows.iter().filter(|s| s.defined && s.never_ran()).count();
    let running = runs.iter().filter(|r| r.running).count();
    let waiting = runs.iter().filter(|r| r.pending_question.is_some()).count();
    let failed = runs
        .iter()
        .filter(|r| r.outcome.as_ref().is_some_and(|o| o.is_error))
        .count();
    let cost: u64 = runs
        .iter()
        .filter_map(|r| r.outcome.as_ref()?.cost_micro_usd)
        .sum();
    let offset = tz_offset_ms();
    let today = local_day(now_ms(), offset);
    let today_runs = runs
        .iter()
        .filter(|r| local_day(r.started_at, offset) == today)
        .count();
    let week_runs = runs
        .iter()
        .filter(|r| today - local_day(r.started_at, offset) < 7)
        .count();

    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Overview"</h2>
                <span class="adi-spacer"></span>
                // Its own island: `updated_text` reads the seconds ticker, and read out here it
                // would re-fold every agent's history once a second to redraw one label.
                <span class="adi-updated">
                    {move || updated_text(state.all_chats, state.secs_since)}
                </span>
            </div>
            <div class="adi-stats">
                {stat(agents.to_string(), "Agents", format!("{busy} working right now"), "")}
                {stat(runs.len().to_string(), "Runs on record",
                    format!("{today_runs} today · {week_runs} this week"), "")}
                {stat(running.to_string(), "Running now",
                    "across every agent".to_string(),
                    if running > 0 { "adi-stat--live" } else { "" })}
                {stat(waiting.to_string(), "Waiting on you",
                    "conversations stopped on a question".to_string(),
                    if waiting > 0 { "adi-stat--warn" } else { "" })}
                {stat(never.to_string(), "Never run",
                    "defined, never launched".to_string(),
                    if never > 0 { "adi-stat--warn" } else { "" })}
                {stat(failed.to_string(), "Ended in error",
                    "the engine's own verdict".to_string(),
                    if failed > 0 { "adi-stat--down" } else { "" })}
                {stat(fmt_cost(cost), "Spend", "what the engines reported".to_string(), "")}
            </div>
        </section>
    }
    .into_any()
}

/// One tile: the number, what it counts, and a line of context under it. `tone` is a modifier
/// class — empty for the ordinary case, so a screen where nothing is wrong has no colour on it at
/// all and the one tile that lights up is the one worth reading.
fn stat(value: String, label: &'static str, note: String, tone: &'static str) -> AnyView {
    view! {
        <div class=format!("adi-stat {tone}")>
            <span class="adi-stat__value">{value}</span>
            <span class="adi-stat__label">{label}</span>
            <span class="adi-stat__note">{note}</span>
        </div>
    }
    .into_any()
}

// ---- the activity chart ------------------------------------------------------------------------

/// Runs started per day over the last fortnight — the shape of the machine's week, which no
/// single total says. Bars are drawn from divs rather than an SVG or a chart library: fourteen
/// rectangles are not worth a dependency in a wasm bundle.
fn activity_view(state: State) -> AnyView {
    if state.all_chats.get().is_none() {
        return ().into_any();
    }
    let runs = all_runs(state);
    let offset = tz_offset_ms();
    let today = local_day(now_ms(), offset);
    // One bucket per day, oldest first, so the chart reads left-to-right into today.
    let mut days = vec![0usize; ACTIVITY_DAYS as usize];
    for r in &runs {
        let age = today - local_day(r.started_at, offset);
        if (0..ACTIVITY_DAYS).contains(&age) {
            days[(ACTIVITY_DAYS - 1 - age) as usize] += 1;
        }
    }
    let peak = days.iter().copied().max().unwrap_or(0);
    let total: usize = days.iter().sum();

    let bars = days
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let day = today - (ACTIVITY_DAYS - 1 - i as i64);
            let date = fmt_date((day * 86_400) as u64);
            // Percent of the tallest bar, floored at a sliver so a day with one run is still
            // visibly a day with one run rather than an empty column.
            let height = match (peak, *n) {
                (0, _) | (_, 0) => "0".to_string(),
                (peak, n) => format!("{}%", (n * 100 / peak).max(6)),
            };
            let is_today = day == today;
            view! {
                <div class="adi-spark__col" title=format!("{date} — {n} run(s)")>
                    <span class="adi-spark__bar" class:adi-spark__bar--today=is_today
                        style=format!("height:{height}")></span>
                    <span class="adi-spark__day">{if is_today {
                        "today".to_string()
                    } else {
                        date.chars().skip(5).collect::<String>()
                    }}</span>
                </div>
            }
        })
        .collect::<Vec<_>>();

    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Activity"</h2>
                <span class="adi-chip adi-mono" title="runs started in the last 14 days">
                    {total.to_string()}
                </span>
                <span class="adi-spacer"></span>
                <span class="adi-updated">"runs started, per day"</span>
            </div>
            <div class="adi-spark">{bars}</div>
        </section>
    }
    .into_any()
}

// ---- the per-agent table -----------------------------------------------------------------------

/// Every agent, with what it has actually done. The table is its own reactive island, so the
/// live channel refreshes the rows in place without rebuilding the panel around them.
fn agents_panel(state: State) -> AnyView {
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"By agent"</h2>
                <span class="adi-chip adi-mono" title="agents defined on this machine">
                    {move || state.agents.get().map_or_else(|| "\u{2014}".to_string(),
                        |a| a.agents.len().to_string())}
                </span>
                <span class="adi-spacer"></span>
                <span class="adi-updated">"every agent, and what it has run"</span>
            </div>
            <Table state=state.tables.analytics_agents>{move || agent_rows(state)}</Table>
        </section>
    }
    .into_any()
}

/// The table body: one row per agent, or a loading/empty placeholder.
fn agent_rows(state: State) -> AnyView {
    let table = state.tables.analytics_agents;
    if state.agents.get().is_none() {
        return view! { <EmptyRow state=table>"Loading…"</EmptyRow> }.into_any();
    }
    let mut rows = agent_stats(state);
    if rows.is_empty() {
        return view! {
            <EmptyRow state=table>"No agents yet — define one on the Agents page."</EmptyRow>
        }
        .into_any();
    }
    // Ties on the sorted column fall back to the name, so two agents with the same run count keep
    // a stable order between polls instead of swapping places under the reader.
    sort_rows(&mut rows, table.sort.get(), agent_key, |s| Key::text(&s.name));
    rows.into_iter()
        .map(|s| {
            view! { <TableRow state=table cell=move |col| agent_cell(col, &s)/> }.into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// An agent row's sort key under `col`.
fn agent_key(s: &AgentStats, col: &str) -> Key {
    match col {
        "Project" => Key::text(&s.project),
        "State" => Key::text(s.state()),
        "Runs" => Key::count(s.runs),
        "Running" => Key::count(s.running),
        "Failed" => Key::count(s.failed),
        "Waiting" => Key::count(s.waiting),
        "Cost" => Key::num(s.cost_micro),
        "Last run" => Key::num(s.last),
        // "Agent", and any header without a key of its own.
        _ => Key::text(&s.name),
    }
}

/// One agent row's cell under `col`. Matching the header text — the same key the sort uses — is
/// what lets the user hide and reorder columns without the row builder knowing about it.
fn agent_cell(col: &str, s: &AgentStats) -> AnyView {
    /// A count that is only worth reading when it isn't zero: a dash otherwise, so the eye finds
    /// the three rows that failed in a table of forty that didn't. `tone` is a design token, so
    /// the colour follows the theme rather than being spelled twice.
    fn count(n: usize, tone: &'static str) -> AnyView {
        if n == 0 {
            return view! { <span class="adi-muted">"—"</span> }.into_any();
        }
        view! {
            <span class="font-mono" style=format!("color:var(--{tone})")>{n.to_string()}</span>
        }
        .into_any()
    }
    match col {
        "Project" => match s.project.is_empty() {
            true => view! { <span class="adi-muted">"—"</span> }.into_any(),
            false => {
                view! { <span><span class="adi-chip adi-mono">{s.project.clone()}</span></span> }
                    .into_any()
            }
        },
        "State" => {
            let quiet = !s.busy() && s.waiting == 0;
            let title = if s.interactive {
                "an interactive backend keeps no run history — its live session is the run"
            } else if !s.defined {
                "this agent no longer exists; its runs are still on disk"
            } else {
                ""
            };
            view! {
                <span title=title class:adi-muted=quiet>{s.state()}</span>
            }
            .into_any()
        }
        "Runs" => match (s.interactive, s.runs) {
            (true, 0) => view! { <span class="adi-muted">"—"</span> }.into_any(),
            (_, n) => view! { <span class="font-mono">{n.to_string()}</span> }.into_any(),
        },
        "Running" => count(s.running, "online"),
        "Failed" => count(s.failed, "down"),
        "Waiting" => count(s.waiting, "warn"),
        // The turn count has no column of its own — ten of those already run past the edge of a
        // narrow main column — so it rides here, where a reader who wants to know what the money
        // bought is already looking.
        "Cost" => view! {
            <span class="font-mono text-meta" title=format!("{} turns", s.turns)>
                {fmt_cost(s.cost_micro)}
            </span>
        }
        .into_any(),
        "Last run" => view! {
            <span class="text-meta" style="white-space:nowrap">{ago(s.last)}</span>
        }
        .into_any(),
        // "Agent", and anything the layout offers that this match doesn't name. The backend has
        // no column of its own — in a real tree most rows carry the same one, and it is the
        // widest cell on the page — so it rides here as the name's hover text.
        _ => view! { <span title=s.backend.clone()>{s.name.clone()}</span> }.into_any(),
    }
}

// ---- the agents nothing has run ----------------------------------------------------------------

/// The agents that have never been launched — the question this page was asked for. Rendered only
/// when there are some, so a machine where everything gets used says so by staying quiet.
///
/// Interactive backends are left out on purpose: they keep no run history, so "never run" is not a
/// thing their zero can mean.
fn idle_view(state: State) -> AnyView {
    let rows = agent_stats(state);
    let idle: Vec<AgentStats> = rows
        .into_iter()
        .filter(|s| s.defined && s.never_ran())
        .collect();
    if idle.is_empty() {
        return ().into_any();
    }
    let chips = idle
        .iter()
        .map(|s| {
            let title = format!("{} · never launched", s.backend);
            view! { <span class="adi-chip adi-mono" title=title>{s.name.clone()}</span> }
        })
        .collect::<Vec<_>>();
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Never run"</h2>
                <span class="adi-chip adi-mono">{idle.len().to_string()}</span>
                <span class="adi-spacer"></span>
                <span class="adi-updated">"defined here, never launched"</span>
            </div>
            <div class="adi-panel__body">
                <div class="adi-chiprow">{chips}</div>
                <div class="adi-hint">
                    "Interactive (pty) agents are left out: their live session " <em>"is"</em>
                    " the run, so they keep no history for this to count."
                </div>
            </div>
        </section>
    }
    .into_any()
}

// ---- how runs ended ----------------------------------------------------------------------------

/// The endings, counted: the engine's own terminal reason for every run that has one. Runs still
/// going, and runs that ended before the store began recording this, are simply absent — the panel
/// says what it knows rather than inventing a bucket for what it doesn't.
fn outcomes_view(state: State) -> AnyView {
    let runs = all_runs(state);
    let mut counts: std::collections::BTreeMap<(bool, String), usize> =
        std::collections::BTreeMap::new();
    for r in &runs {
        let Some(o) = &r.outcome else { continue };
        let reason = o
            .terminal_reason
            .clone()
            .unwrap_or_else(|| if o.is_error { "error" } else { "completed" }.to_string());
        *counts.entry((o.is_error, reason)).or_default() += 1;
    }
    if counts.is_empty() {
        return ().into_any();
    }
    let total: usize = counts.values().sum();
    // Errors first, then by count: the endings worth acting on lead.
    let mut rows: Vec<((bool, String), usize)> = counts.into_iter().collect();
    rows.sort_by(|a, b| {
        b.0.0
            .cmp(&a.0.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.0.1.cmp(&b.0.1))
    });
    let bars = rows
        .into_iter()
        .map(|((is_error, reason), n)| {
            let width = format!("width:{}%", (n * 100 / total).max(1));
            view! {
                <div class="adi-endrow">
                    <span class="adi-endrow__name font-mono"
                        style=if is_error { "color:var(--down)" } else { "" }>{reason}</span>
                    <span class="adi-endrow__track">
                        <span class="adi-endrow__fill" class:adi-endrow__fill--err=is_error
                            style=width></span>
                    </span>
                    <span class="adi-endrow__n font-mono">{n.to_string()}</span>
                </div>
            }
        })
        .collect::<Vec<_>>();
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"How runs ended"</h2>
                <span class="adi-chip adi-mono" title="runs that reported an ending">
                    {total.to_string()}
                </span>
                <span class="adi-spacer"></span>
                <span class="adi-updated">"the engine's own verdict"</span>
            </div>
            <div class="adi-panel__body">{bars}</div>
        </section>
    }
    .into_any()
}

// ---- formatting --------------------------------------------------------------------------------

/// Now, in Unix milliseconds.
fn now_ms() -> u64 {
    js_sys::Date::now() as u64
}

/// The browser's current UTC offset in milliseconds, as an amount to *subtract* from an instant to
/// get the local wall clock. Read once per render rather than per run: it is one value for the
/// whole page, and a `Date` per timestamp would be fourteen days of allocation for one number.
///
/// Taking today's offset for every day in the window means a chart spanning a DST change buckets
/// one hour of one day on the wrong side of midnight. That is the right trade for a day-grained
/// chart — the alternative is a `Date` per run to ask which offset applied at the time.
fn tz_offset_ms() -> f64 {
    // `getTimezoneOffset` is *minutes behind UTC* — positive west of Greenwich, which is the
    // opposite sign to how the offset is usually written (UTC-5 reports +300).
    js_sys::Date::new_0().get_timezone_offset() * 60_000.0
}

/// The local day an instant falls in, as whole days since the epoch. Days rather than dates so
/// bucketing and "how many days ago" are the same subtraction.
fn local_day(ms: u64, offset_ms: f64) -> i64 {
    ((ms as f64 - offset_ms) / 86_400_000.0).floor() as i64
}

/// A coarse "how long ago" for a run's last activity; `0` (never) renders as a dash.
fn ago(ms: u64) -> String {
    if ms == 0 {
        return "—".to_string();
    }
    let secs = now_ms().saturating_sub(ms) / 1000;
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

/// Micro-dollars as money. Anything under a cent reads as `<$0.01` rather than `$0.00`, which
/// would say "free" about a run that wasn't.
fn fmt_cost(micro: u64) -> String {
    match micro {
        0 => "—".to_string(),
        n if n < 10_000 => "<$0.01".to_string(),
        n => format!("${:.2}", n as f64 / 1_000_000.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_under_a_cent_never_reads_as_free() {
        assert_eq!(fmt_cost(0), "—");
        assert_eq!(fmt_cost(1), "<$0.01");
        assert_eq!(fmt_cost(9_999), "<$0.01");
        assert_eq!(fmt_cost(10_000), "$0.01");
        assert_eq!(fmt_cost(1_500_000), "$1.50");
    }

    /// The offset is subtracted, so a timestamp just before local midnight belongs to the day that
    /// is ending and not to the one starting an hour later in UTC.
    #[test]
    fn a_day_is_bucketed_by_the_local_clock() {
        // 2024-01-02T00:30:00Z, in a zone three hours ahead of UTC (offset reported as -180).
        let ms = 1_704_155_400_000;
        let offset = -180.0 * 60_000.0;
        assert_eq!(local_day(ms, 0.0), 19_724); // UTC: the 2nd
        assert_eq!(local_day(ms, offset), 19_724); // local 03:30, still the 2nd
        // 2024-01-01T22:30:00Z is local 01:30 on the 2nd in that zone.
        let earlier = ms - 2 * 3_600_000;
        assert_eq!(local_day(earlier, 0.0), 19_723);
        assert_eq!(local_day(earlier, offset), 19_724);
    }
}
