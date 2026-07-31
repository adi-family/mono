//! The Hive settings page: every service declared across all projects' and dashboards'
//! `.adi/hive.yaml` plus the global front-door hive, each with a live running/stopped indicator.
//! This view is meant to be the one place every hive service is visible, whichever adi-hive
//! instance actually supervises it.

use adi_webapp_api::types::{Dashboard, HiveService, Project};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::routing::{Route, open_project, project_href, push_state};
use crate::state::{Flash, State};
use crate::ui::{
    Sort, configurable_table, cpu_cell, dash, fmt_bytes, fmt_cpu, fmt_ports, memory_cell,
    placeholder_row,
};

/// Every column the Hive table can show, in its declared order — the set the settings menu
/// offers, and the order a user who has never rearranged it sees. Sort keys and cell builders
/// both match on the header *text*, so a column can move or hide without either noticing.
pub(crate) const COLS: &[&str] = &[
    "Source", "Service", "Host", "Ports", "Command", "Restart", "CPU", "Memory", "Status",
];

/// Re-fetch `/api/hive` — which re-reads every project's `.adi/hive.yaml` and the global hive
/// from disk (re-running any `bash`…`` port commands) — and refresh the Services view.
fn reload_hive(state: State) {
    spawn_local(async move {
        match fetch::hive().await {
            Ok(h) => {
                state.hive.set(Some(h));
                state
                    .flash
                    .set(Some(Flash::ok("Reloaded hive config.".to_string())));
            }
            Err(e) => state.flash.set(Some(Flash::err(format!(
                "Couldn't reload hive config: {e}"
            )))),
        }
    });
}

pub(crate) fn hive_view(state: State, route: RwSignal<Route>) -> AnyView {
    let State { hive, .. } = state;
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <span class="adi-chip adi-mono" title="Declared services">
                    {move || hive.get().map_or_else(|| "\u{2014}".to_string(),
                        |h| h.services.len().to_string())}
                </span>
                <span class="adi-updated">
                    {move || hive.get().map_or(String::new(),
                        |h| format!("{} running", h.services.iter().filter(|s| s.running).count()))}
                </span>
                <span class="adi-spacer"></span>
                <span class="adi-chip adi-mono" title="Total CPU and memory of every running service">
                    {move || hive.get().map_or_else(|| "\u{2014}".to_string(), |h| {
                        let (cpu, mem) = h.services.iter().filter_map(|s| s.usage.as_ref())
                            .fold((0.0_f32, 0_u64), |(c, m), u|
                                (c + u.cpu_percent, m.saturating_add(u.memory_bytes)));
                        format!("{} CPU · {}", fmt_cpu(cpu), fmt_bytes(mem))
                    })}
                </span>
                <button class="adi-btn adi-btn--ghost" type="button"
                    title="Re-read every project's .adi/hive.yaml and the global hive from disk"
                    on:click=move |_| reload_hive(state)>"Reload config"</button>
            </div>
            {configurable_table(state.tables.hive, COLS, move || hive_rows(state, route))}
        </section>
    }
    .into_any()
}

/// The separator between the steps of a source's name path.
const PATH_SEP: &str = " > ";

/// Where a service comes from, as the Source cell reads it. A service carries only ids, so the
/// human `label` — a project's full name path, a dashboard's name — is resolved per render
/// against the loaded listings.
///
/// `group` keeps the page's long-standing grouping (front-door, then dashboards, then projects)
/// and, with `label`, gives the Source column its order for free: derived `Ord` compares the
/// group first, then the very text the cell shows.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Source {
    group: u8,
    label: String,
}

/// A project's full name path — `proj1 > subproj > subproj2` — walked up its `parent` links.
///
/// An id the listing doesn't hold falls back to the id itself, so a row never renders blank
/// while the projects list is still loading. The walk is bounded by the number of projects, so a
/// `parent` cycle (a hand-edited manifest) can't hang the render.
fn project_path(projects: &[Project], id: &str) -> String {
    let mut steps: Vec<String> = Vec::new();
    let mut cursor = Some(id.to_string());
    while let Some(current) = cursor {
        let Some(project) = projects.iter().find(|p| p.id == current) else {
            steps.push(current);
            break;
        };
        steps.push(project.name.clone());
        cursor = project.parent.clone();
        if steps.len() > projects.len() {
            break;
        }
    }
    steps.reverse();
    steps.join(PATH_SEP)
}

/// Resolve a service's Source cell: a project's name path, a dashboard's name (itself prefixed
/// by the path of the project it's filed under, so every row locates itself the same way), or
/// the front door.
fn source_of(s: &HiveService, projects: &[Project], dashboards: &[Dashboard]) -> Source {
    match (&s.project, &s.dashboard) {
        (Some(id), _) => Source {
            group: 2,
            label: project_path(projects, id),
        },
        (None, Some(id)) => Source {
            group: 1,
            label: dashboards
                .iter()
                .find(|d| &d.id == id)
                .map_or_else(|| id.clone(), |d| dashboard_path(d, projects)),
        },
        (None, None) => Source {
            group: 0,
            label: "front-door".to_string(),
        },
    }
}

/// A dashboard's name, prefixed by the path of the project it is filed under (if any).
fn dashboard_path(d: &Dashboard, projects: &[Project]) -> String {
    let Some(id) = &d.project else {
        return d.name.clone();
    };
    let path = project_path(projects, id);
    // A dashboard named after the project it sits in would read `Bugbounty > Bugbounty`; the
    // repeat says nothing, so the path already is the label.
    if path.rsplit(PATH_SEP).next() == Some(d.name.as_str()) {
        return path;
    }
    format!("{path}{PATH_SEP}{}", d.name)
}

/// Order the rows by the clicked column. Absent values sort as empty/zero, which puts a stopped
/// service's blank usage at the bottom of a descending CPU or Memory sort — where "what is
/// costing me the most?" wants it.
///
/// Written out rather than handed to [`ui::sort_rows`](crate::ui::sort_rows) like every other
/// table's: this one tiebreaks twice, on the source grouping and then the name, and a
/// [`Key`](crate::ui::Key) describes a single value.
fn sort_rows(rows: &mut [(HiveService, Source)], sort: Sort) {
    /// An optional text cell as a sort key.
    fn text(value: Option<&String>) -> &str {
        value.map_or("", String::as_str)
    }
    rows.sort_by(|(a, a_src), (b, b_src)| {
        let ord = match sort.col {
            "Service" => a.name.cmp(&b.name),
            "Host" => text(a.host.as_ref()).cmp(text(b.host.as_ref())),
            // By the port itself, not its `key:port` rendering — `http:9` belongs before `http:80`.
            "Ports" => a
                .primary_port
                .unwrap_or(0)
                .cmp(&b.primary_port.unwrap_or(0)),
            "Command" => text(a.run.as_ref()).cmp(text(b.run.as_ref())),
            "Restart" => text(a.restart.as_ref()).cmp(text(b.restart.as_ref())),
            "CPU" => cpu(a).total_cmp(&cpu(b)),
            "Memory" => mem(a).cmp(&mem(b)),
            "Status" => a.running.cmp(&b.running),
            // "Source", and any header without a key of its own. Sorted by the displayed name
            // path, so the column orders the way it reads.
            _ => a_src.cmp(b_src),
        };
        // Only the key flips with the direction: the tiebreaks stay ascending, so a descending
        // sort is the exact mirror of its ascending self.
        sort.dir(ord)
            .then_with(|| a_src.cmp(b_src))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// A service's sampled CPU share, or zero when it isn't running.
fn cpu(s: &HiveService) -> f32 {
    s.usage.as_ref().map_or(0.0, |u| u.cpu_percent)
}

/// A service's sampled resident memory, or zero when it isn't running.
fn mem(s: &HiveService) -> u64 {
    s.usage.as_ref().map_or(0, |u| u.memory_bytes)
}

/// Rows for the aggregated hive table, in the order and arrangement the header controls select.
fn hive_rows(state: State, route: RwSignal<Route>) -> AnyView {
    let layout = state.tables.hive.layout.get();
    let Some(h) = state.hive.get() else {
        return placeholder_row(layout.span(), "Loading…");
    };
    if h.services.is_empty() {
        return placeholder_row(
            layout.span(),
            "No hive services declared in any project or the global hive.",
        );
    }
    // A service names its origin by id; the Source cell shows the name path instead, resolved
    // against the projects list (shell data, loaded on every route) and the dashboards listing
    // (fetched alongside the hive on this route).
    let projects = state.projects.get().map(|p| p.projects).unwrap_or_default();
    let dashboards = state
        .dashboards
        .get()
        .map(|d| d.dashboards)
        .unwrap_or_default();
    let mut rows: Vec<(HiveService, Source)> = h
        .services
        .into_iter()
        .map(|s| {
            let source = source_of(&s, &projects, &dashboards);
            (s, source)
        })
        .collect();
    sort_rows(&mut rows, state.tables.hive.sort.get());
    let shown = layout.shown();
    rows.into_iter()
        .map(|(s, src)| {
            let cells: Vec<AnyView> = shown
                .iter()
                .map(|col| cell(col, &s, &src, state, route))
                .collect();
            view! { <tr>{cells}</tr> }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// One service's cell under `col`. Matching the header text — the same key the sort uses — is
/// what lets the user hide and reorder columns without the row builder knowing about it.
fn cell(col: &str, s: &HiveService, src: &Source, state: State, route: RwSignal<Route>) -> AnyView {
    match col {
        "Service" => view! { <td class="adi-mono">{s.name.clone()}</td> }.into_any(),
        "Host" => host_cell(s.host.as_deref()),
        "Ports" => {
            view! { <td class="adi-mono adi-table__port">{fmt_ports(&s.ports)}</td> }.into_any()
        }
        "Command" => view! { <td class="adi-mono adi-muted">{dash(s.run.clone())}</td> }.into_any(),
        "Restart" => view! { <td class="adi-muted">{dash(s.restart.clone())}</td> }.into_any(),
        "CPU" => cpu_cell(s.usage.as_ref()),
        "Memory" => memory_cell(s.usage.as_ref()),
        "Status" => {
            let (attr, label) = if s.running {
                ("online", "Running")
            } else {
                ("down", "Stopped")
            };
            view! {
                <td>
                    <span class="adi-status" data-state=attr>
                        <span class="adi-status__led"></span><span>{label}</span>
                    </span>
                </td>
            }
            .into_any()
        }
        // "Source", and anything the layout offers that this match doesn't name.
        _ => view! { <td>{source_cell(s, src, state, route)}</td> }.into_any(),
    }
}

/// The Host cell: the front door serves every declared host over plain HTTP, so the name doubles
/// as the way in. A new tab, since leaving the panel to visit a service is rarely what was meant.
fn host_cell(host: Option<&str>) -> AnyView {
    let Some(host) = host else {
        return view! { <td class="adi-mono">{dash(None)}</td> }.into_any();
    };
    let href = format!("http://{host}/");
    view! {
        <td class="adi-mono">
            <a href=href.clone() target="_blank" rel="noreferrer" title=href>{host.to_string()}</a>
        </td>
    }
    .into_any()
}

/// The Source cell's contents: the resolved name path, linking into the project detail page or
/// the dashboards listing. The id stays in the `title`, since it is what the YAML and the store
/// paths use.
fn source_cell(s: &HiveService, src: &Source, state: State, route: RwSignal<Route>) -> AnyView {
    let label = src.label.clone();
    match (&s.project, &s.dashboard) {
        // Supervised by the per-user dashboards hive, not the front door.
        (_, Some(id)) => {
            let title = format!("dashboard {id}");
            view! {
                <a class="adi-btn adi-btn--link" href=Route::Dashboards.path() title=title
                    on:click=move |ev: web_sys::MouseEvent| {
                        if ev.meta_key() || ev.ctrl_key() || ev.shift_key() || ev.button() != 0 { return; }
                        ev.prevent_default();
                        push_state(Route::Dashboards.path());
                        route.set(Route::Dashboards);
                    }>{label}</a>
            }
            .into_any()
        }
        (None, None) => view! { <span class="adi-chip">"front-door"</span> }.into_any(),
        (Some(id), None) => {
            let open_id = id.clone();
            let href = project_href(id);
            let title = format!("project {id}");
            view! {
                <a class="adi-btn adi-btn--link" href=href title=title
                    on:click=move |ev: web_sys::MouseEvent| {
                        if ev.meta_key() || ev.ctrl_key() || ev.shift_key() || ev.button() != 0 { return; }
                        ev.prevent_default();
                        open_project(state, route, open_id.clone());
                    }>{label}</a>
            }
            .into_any()
        }
    }
}

#[cfg(test)]
mod tests {
    use adi_webapp_api::types::ProcessUsage;

    use super::*;

    /// Ascending by `header`, asserting the table actually declares it — so a renamed column
    /// breaks its tests instead of silently falling through the comparator's catch-all.
    fn asc(header: &'static str) -> Sort {
        Sort::new(known(header))
    }

    /// The sort a second click on `header` produces: that column, descending.
    fn desc(header: &'static str) -> Sort {
        Sort {
            col: known(header),
            desc: true,
        }
    }

    fn known(header: &'static str) -> &'static str {
        assert!(COLS.contains(&header), "{header} is not a column");
        header
    }

    /// A service with just the fields the sorts read; `cpu`/`mem` of `None` means "not running",
    /// which is what a stopped service carries.
    fn svc(name: &str, project: Option<&str>, usage: Option<(f32, u64)>) -> HiveService {
        HiveService {
            project: project.map(str::to_string),
            dashboard: None,
            name: name.to_string(),
            host: None,
            ports: Vec::new(),
            run: None,
            restart: None,
            primary_port: None,
            running: usage.is_some(),
            usage: usage.map(|(cpu, memory_bytes)| ProcessUsage {
                pid: 1,
                cpu_percent: cpu,
                memory_bytes,
                processes: 1,
                uptime_secs: 0,
            }),
        }
    }

    /// A project in the listing the paths are resolved against.
    fn proj(id: &str, name: &str, parent: Option<&str>) -> Project {
        Project {
            id: id.to_string(),
            name: name.to_string(),
            description: None,
            parent: parent.map(str::to_string),
            created_at: 0,
            archived_at: None,
        }
    }

    /// Pair each service with its resolved source, the way a render does, so the tests order
    /// exactly what the table orders.
    fn rows(services: Vec<HiveService>, projects: &[Project]) -> Vec<(HiveService, Source)> {
        services
            .into_iter()
            .map(|s| {
                let source = source_of(&s, projects, &[]);
                (s, source)
            })
            .collect()
    }

    fn names(rows: &[(HiveService, Source)]) -> Vec<&str> {
        rows.iter().map(|(s, _)| s.name.as_str()).collect()
    }

    /// The Source cell reads as a path of names, not the opaque ids the service carries.
    #[test]
    fn a_nested_project_resolves_to_its_full_name_path() {
        let projects = [
            proj("p1", "proj1", None),
            proj("p2", "subproj", Some("p1")),
            proj("p3", "subproj2", Some("p2")),
        ];
        assert_eq!(
            project_path(&projects, "p3"),
            "proj1 > subproj > subproj2",
            "every ancestor, root first"
        );
        assert_eq!(project_path(&projects, "p1"), "proj1", "a root is one step");
    }

    /// The listing arrives asynchronously and manifests are hand-editable, so neither an
    /// unknown id nor a `parent` cycle may blank the cell or hang the render.
    #[test]
    fn an_unresolvable_path_degrades_to_the_id() {
        assert_eq!(
            project_path(&[], "3156da92"),
            "3156da92",
            "before the projects list loads, the id still identifies the row"
        );
        // A parent chain that points back into itself.
        let looped = [proj("a", "A", Some("b")), proj("b", "B", Some("a"))];
        assert!(
            !project_path(&looped, "a").is_empty(),
            "a cycle terminates with something to show"
        );
    }

    /// A dashboard names itself the same way, under the project it is filed beneath.
    #[test]
    fn a_dashboard_reads_as_its_project_path_plus_its_own_name() {
        let projects = [proj("p1", "proj1", None)];
        let board = Dashboard {
            id: "84ddcba0".to_string(),
            dir: String::new(),
            name: "Metrics".to_string(),
            description: None,
            project: Some("p1".to_string()),
            host: None,
            frontend_port: None,
            backend_port: None,
            frontend_running: false,
            backend_running: false,
            modules: Vec::new(),
            routes: Vec::new(),
            archived_at: None,
        };
        let mut svc = svc("frontend", None, None);
        svc.dashboard = Some(board.id.clone());

        let source = source_of(&svc, &projects, std::slice::from_ref(&board));
        assert_eq!(source.label, "proj1 > Metrics");

        // Unfiled, it is just its own name.
        let unfiled = Dashboard {
            project: None,
            ..board.clone()
        };
        assert_eq!(
            source_of(&svc, &projects, &[unfiled]).label,
            "Metrics",
            "no project to prefix"
        );

        // Named after its own project, the prefix would only repeat itself.
        let eponymous = Dashboard {
            name: "proj1".to_string(),
            ..board
        };
        assert_eq!(
            source_of(&svc, &projects, &[eponymous]).label,
            "proj1",
            "no `proj1 > proj1`"
        );
    }

    /// The default: the source grouping the page has always shown — front-door, then dashboards,
    /// then projects — with each group's services by name.
    #[test]
    fn the_default_sort_preserves_the_source_grouping() {
        // A dashboard service carries no project id — only a dashboard one.
        let mut board = svc("frontend", None, None);
        board.dashboard = Some("board".to_string());
        let mut services = rows(
            vec![
                svc("web", Some("proj"), None),
                svc("app", None, None),
                board,
                svc("api", None, None),
            ],
            &[proj("proj", "Web Project", None)],
        );

        sort_rows(&mut services, asc("Source"));
        assert_eq!(names(&services), ["api", "app", "frontend", "web"]);
    }

    /// The point of the feature: "what is costing me the most?" is one click on Memory.
    #[test]
    fn memory_sorts_by_the_sampled_bytes_not_the_formatted_cell() {
        let mut services = rows(
            vec![
                svc("small", Some("a"), Some((0.0, 900_000))),
                svc("huge", Some("b"), Some((0.0, 4_000_000_000))),
                svc("mid", Some("c"), Some((0.0, 30_000_000))),
            ],
            &[],
        );
        sort_rows(&mut services, asc("Memory"));
        assert_eq!(names(&services), ["small", "mid", "huge"]);

        sort_rows(&mut services, desc("Memory"));
        assert_eq!(
            names(&services),
            ["huge", "mid", "small"],
            "descending is the exact mirror"
        );
    }

    /// A stopped service has no sample at all; it must sort as zero rather than vanish or float
    /// to the top of a "biggest first" view.
    #[test]
    fn stopped_services_sort_as_zero_usage() {
        let mut services = rows(
            vec![
                svc("stopped", Some("a"), None),
                svc("busy", Some("b"), Some((45.0, 1_000))),
                svc("idle", Some("c"), Some((0.5, 1_000))),
            ],
            &[],
        );
        sort_rows(&mut services, asc("CPU"));
        assert_eq!(names(&services), ["stopped", "idle", "busy"]);

        sort_rows(&mut services, desc("CPU"));
        assert_eq!(names(&services), ["busy", "idle", "stopped"]);
    }

    /// Rows that tie on the sort key keep the source grouping — in both directions, since only
    /// the key flips. Without that, every redraw of a mostly-idle hive would reshuffle.
    #[test]
    fn ties_keep_a_stable_order_in_both_directions() {
        let tied = || {
            rows(
                vec![
                    svc("web", Some("bbb"), None),
                    svc("api", Some("aaa"), None),
                    svc("app", None, None),
                ],
                &[],
            )
        };
        // Every row's Restart is None, so all three tie on the key.
        let mut ascending = tied();
        sort_rows(&mut ascending, asc("Restart"));
        let mut descending = tied();
        sort_rows(&mut descending, desc("Restart"));

        assert_eq!(names(&ascending), ["app", "api", "web"]);
        assert_eq!(
            names(&descending),
            names(&ascending),
            "a tie is direction-independent"
        );
    }
}
