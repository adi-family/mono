//! The event-log consumer: catches up on the bus with its own cursor (like
//! [`adi_triggers::EventDispatcher`], which this is modelled on), applies the collector's
//! allowlist to what it finds, and queues the result. A second timer flushes the queue to the
//! collector every [`FLUSH_INTERVAL`] and once more on [`Usage::stop`].
//!
//! **Allowlist, never pass-through** (the plan's own rule): every event this module forwards is
//! built field by field from a bus payload it has already named every field of — nothing here
//! ever serializes a payload wholesale into a queued event's properties.

use std::sync::Arc;
use std::time::{Duration, Instant};

use adi_agents::Agents;
use adi_config::{Config, Module, now_unix};
use adi_events::{EventRecord, Events};
use adi_projects::Projects;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::queue::{self, QueuedEvent};
use crate::{client, settings, state::State};

/// This consumer's own slot in the event log's cursor table — distinct from
/// `adi_triggers::SUBSCRIBER`, so a backlog of bus history replays independently of event
/// triggers (and of anything else that ever reads this same log).
const SUBSCRIBER: &str = "adi-usage";

/// This crate's module directory: `~/.adi/mono/analytics`.
const MODULE: &str = "analytics";

/// How often the dispatch loop catches up on the bus and checks for a day rollover. Latency does
/// not matter here the way it does for an event trigger — this only has to keep the on-disk queue
/// from growing unbounded between flushes.
const TICK: Duration = Duration::from_secs(5);

/// The most events one tick catches up on — same cap and the same reasoning as
/// `adi_triggers::dispatch::MAX_PER_TICK`: a backlog from downtime (or from usage events having
/// been off) is caught up a slice at a time, never all at once.
const MAX_PER_TICK: usize = 200;

/// "Flushes in batches every ~10 minutes and at shutdown" (the plan, §1).
const FLUSH_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// The environment's root agent — the one `onboarding_completed` is about. Named independently
/// here rather than shared from `adi-webapp-api`'s `ROOT_AGENT` / `ADI_AGENT_NAME` (and
/// `adi-webapp`'s own copy): three crates already spell this the same way without a shared
/// constant between them, and a fourth does not change that.
const ROOT_AGENT: &str = "adi-agent";

/// `adi_marketplace::sources::OFFICIAL_NAME` — the ADI Store's local source name, and the only
/// marketplace a `slug` is ever forwarded for (every other source's slugs are private to whoever
/// configured it). Not imported from that crate to avoid a dependency only this one string needs.
const PUBLIC_STORE: &str = "store";

/// Reads the bus, applies the allowlist, and queues to `queue.jsonl`; a second timer flushes it.
/// Cheap to hold; all state is on disk or behind the shared stores it was constructed with.
#[derive(Debug)]
pub struct Usage {
    config: Config,
    events: Events,
    agents: Agents,
    projects: Projects,
    version: String,
    shutdown: watch::Sender<bool>,
    done: watch::Sender<bool>,
}

impl Usage {
    /// Start the dispatch loop: spawns it and returns immediately. `version` is the running
    /// binary's own version string (`adi-app`'s `VERSION`, not this crate's `CARGO_PKG_VERSION`
    /// — a library crate's own package version is not what every event's `version` field means).
    ///
    /// # Panics
    /// If called outside a tokio runtime.
    #[must_use]
    pub fn start_watched(
        config: Config,
        events: Events,
        agents: Agents,
        projects: Projects,
        version: impl Into<String>,
    ) -> Arc<Self> {
        let (shutdown, rx) = watch::channel(false);
        let (done, _) = watch::channel(false);
        let usage = Arc::new(Self {
            config,
            events,
            agents,
            projects,
            version: version.into(),
            shutdown,
            done,
        });
        tokio::spawn(Arc::clone(&usage).run_loop(rx));
        usage
    }

    /// Stop the dispatch loop, flushing once more first, and wait for it to exit (up to `grace`).
    pub async fn stop(&self, grace: Duration) {
        let mut done = self.done.subscribe();
        if self.shutdown.send(true).is_err() {
            return;
        }
        let _ = tokio::time::timeout(grace, done.changed()).await;
    }

    fn module(&self) -> Module {
        self.config.module(MODULE)
    }

    async fn run_loop(self: Arc<Self>, mut rx: watch::Receiver<bool>) {
        // Negative headroom so a queue left over from a prior run (the process was killed
        // between queueing and its next scheduled flush) goes out promptly rather than waiting
        // a full FLUSH_INTERVAL from a cold start.
        let mut last_flush = Instant::now()
            .checked_sub(FLUSH_INTERVAL)
            .unwrap_or_else(Instant::now);
        loop {
            let usage = Arc::clone(&self);
            if let Err(e) = tokio::task::spawn_blocking(move || usage.tick_blocking()).await {
                tracing::warn!(error = %e, "usage-events tick failed");
            }
            if last_flush.elapsed() >= FLUSH_INTERVAL {
                self.maybe_flush().await;
                last_flush = Instant::now();
            }
            tokio::select! {
                () = tokio::time::sleep(TICK) => {}
                _ = rx.changed() => break,
            }
            if *rx.borrow_and_update() {
                break;
            }
        }
        self.maybe_flush().await;
        let _ = self.done.send(true);
    }

    async fn maybe_flush(&self) {
        let module = self.module();
        if !settings::effective(&module) {
            return;
        }
        let Some(install_id) = State::load(&module).install_id else {
            return;
        };
        client::flush(&module, &install_id, now_ms()).await;
    }

    /// Catch up on the bus and act on it. Blocking: disk I/O throughout, like
    /// `adi_triggers::EventDispatcher::dispatch_pending`.
    fn tick_blocking(&self) {
        let module = self.module();
        // Caught up and acked unconditionally, on by on off — so a switch flipped on after a
        // long time off resumes from "now", not from a backlog of months of history suddenly
        // queued (and gated by the 7-day window) all at once.
        let caught_up = match self.events.catch_up(SUBSCRIBER, MAX_PER_TICK) {
            Ok(caught_up) => caught_up,
            Err(e) => {
                tracing::warn!(error = %e, "couldn't catch up on the event log");
                return;
            }
        };
        if caught_up.is_empty() {
            return;
        }

        if settings::effective(&module) {
            let mut state = State::load(&module);
            let install_id = self.ensure_install_id(&module, &mut state);
            self.roll_day_if_needed(&module, &mut state);
            for ev in &caught_up {
                self.handle_event(&module, &mut state, &ev.record);
            }
            let _ = install_id; // identity rides the batch, not each item — see client::build_body
            if let Err(e) = state.save(&module) {
                tracing::warn!(error = %e, "couldn't save usage-events state");
            }
        }

        for ev in caught_up {
            if let Err(e) = self.events.ack(SUBSCRIBER, ev.seq) {
                tracing::warn!(error = %e, "couldn't advance the usage-events cursor");
            }
        }
    }

    /// Mint `install_id` and queue `app_first_launch` the first time this runs with usage events
    /// on; every later call just hands back what is already there.
    fn ensure_install_id(&self, module: &Module, state: &mut State) -> String {
        if let Some(id) = &state.install_id {
            return id.clone();
        }
        let id = uuid::Uuid::new_v4().to_string();
        // A read failure is treated as "not fresh": overclaiming new installs matters more than
        // missing one, since 103 (Install) is counted off this field.
        let fresh = self.agents.list().is_ok_and(|v| v.is_empty())
            && self.projects.list().is_ok_and(|v| v.is_empty());
        state.install_id = Some(id.clone());
        state.first_launch_at = now_unix();
        self.enqueue(module, "app_first_launch", json!({ "fresh": fresh }));
        id
    }

    /// Flush `app_daily_active` for the day that just ended, the first tick that notices the
    /// local date has moved on, then start counting the new one from zero.
    fn roll_day_if_needed(&self, module: &Module, state: &mut State) {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        if state.day.is_empty() {
            state.day = today;
            return;
        }
        if state.day == today {
            return;
        }
        self.enqueue(
            module,
            "app_daily_active",
            json!({
                "runs_human": state.runs_human,
                "runs_agent": state.runs_agent,
                "runs_automation": state.runs_automation,
            }),
        );
        state.day = today;
        state.runs_human = 0;
        state.runs_agent = 0;
        state.runs_automation = 0;
    }

    fn handle_event(&self, module: &Module, state: &mut State, record: &EventRecord) {
        let Ok(payload) = serde_json::from_str::<Value>(&record.payload) else {
            return;
        };
        match record.name.as_str() {
            "adi.agents.saved" => self.on_agent_saved(module, state, &payload),
            "adi.agents.run.started" => self.on_run_started(module, state, &payload),
            "adi.agents.run.finished" => self.on_run_finished(module, state, &payload),
            "adi.dashboards.created" => self.on_dashboard_created(module, &payload),
            "adi.marketplace.installed" => self.on_marketplace_installed(module, &payload),
            "adi.fleet.invite.created" => self.enqueue(module, "fleet_invite_created", json!({})),
            _ => {}
        }
    }

    /// `onboarding_completed` — the root agent's definition saved for the first time ever on this
    /// install. Gated on [`State::onboarding_sent`], not on whether the save was a create: the
    /// bus carries only the agent's name (`adi_agents::events::AgentSaved`), never "was this new".
    fn on_agent_saved(&self, module: &Module, state: &mut State, payload: &Value) {
        if state.onboarding_sent {
            return;
        }
        if payload.get("agent").and_then(Value::as_str) != Some(ROOT_AGENT) {
            return;
        }
        let Some(backend) = self.backend_of(ROOT_AGENT) else {
            return;
        };
        state.onboarding_sent = true;
        self.enqueue(module, "onboarding_completed", json!({ "backend": backend }));
    }

    fn on_run_started(&self, module: &Module, state: &mut State, payload: &Value) {
        let Some(agent) = payload.get("agent").and_then(Value::as_str) else {
            return;
        };
        let run_id = payload.get("run_id").and_then(Value::as_str);
        let launched_by = launched_by_kind(self.launched_by_for(agent, run_id).as_deref());
        match launched_by {
            "agent" => state.runs_agent += 1,
            "automation" => state.runs_automation += 1,
            _ => state.runs_human += 1,
        }
        if !within_first_week(state) {
            return;
        }
        let Some(backend) = self.backend_of(agent) else {
            return;
        };
        self.enqueue(
            module,
            "agent_run_started",
            json!({ "launched_by": launched_by, "backend": backend }),
        );
    }

    fn on_run_finished(&self, module: &Module, state: &State, payload: &Value) {
        if !within_first_week(state) {
            return;
        }
        let Some(agent) = payload.get("agent").and_then(Value::as_str) else {
            return;
        };
        let Some(is_error) = payload.get("is_error").and_then(Value::as_bool) else {
            return;
        };
        let run_id = payload.get("run_id").and_then(Value::as_str);
        let launched_by = launched_by_kind(self.launched_by_for(agent, run_id).as_deref());
        let mut props = json!({ "is_error": is_error, "launched_by": launched_by });
        if let (Some(reason), Value::Object(map)) = (
            payload.get("terminal_reason").and_then(Value::as_str),
            &mut props,
        ) {
            map.insert("terminal_reason".into(), json!(reason));
        }
        self.enqueue(module, "agent_run_finished", props);
    }

    fn on_dashboard_created(&self, module: &Module, payload: &Value) {
        let Some(from) = payload.get("from").and_then(Value::as_str) else {
            return;
        };
        if from != "blank" && from != "import" {
            return;
        }
        self.enqueue(module, "dashboard_created", json!({ "from": from }));
    }

    /// `store_item_installed` — dropped outright for the four marketplace kinds
    /// (`llm`/`embedding`/`trigger`/`project`) the collector's dictionary has no `kind` value
    /// for; `slug` is forwarded only for [`PUBLIC_STORE`], per the dictionary's own note ("public
    /// withadi.dev store slug only; omitted for any other marketplace").
    fn on_marketplace_installed(&self, module: &Module, payload: &Value) {
        let Some(kind) = payload.get("kind").and_then(Value::as_str) else {
            return;
        };
        if !matches!(kind, "agent" | "tool" | "dashboard" | "service") {
            return;
        }
        let mut props = json!({ "kind": kind });
        if payload.get("marketplace").and_then(Value::as_str) == Some(PUBLIC_STORE)
            && let Some(slug) = payload.get("slug").and_then(Value::as_str)
            && let Value::Object(map) = &mut props
        {
            map.insert("slug".into(), json!(slug));
        }
        self.enqueue(module, "store_item_installed", props);
    }

    /// The agent's current backend, as the manifest names it (`harness:claude-sdk`, …) — `None`
    /// for an agent that no longer exists or names no backend, which is reason enough to send
    /// nothing rather than an empty token.
    fn backend_of(&self, agent: &str) -> Option<String> {
        let backend = self.agents.get(agent).ok().flatten()?.manifest.runtime().to_string();
        (!backend.is_empty()).then_some(backend)
    }

    /// Who started a run, in `adi_agents::store::SessionRecord::launched_by`'s own words —
    /// looked up because neither shape of `adi.agents.run.started`/`.finished` carries it
    /// (`record.rs:80`'s own doc comment is why: it is deliberately not duplicated onto every
    /// bus event, so a reader always learns it from the one column that holds it).
    ///
    /// A `Process` launch's event carries `run_id` directly. A `Pty` launch's does not — a pty
    /// session is one per agent, not one per run — so a started event with no `run_id` resolves
    /// to whichever of that agent's runs is currently live and started most recently, which is
    /// exactly one run under the pty executor's own one-session-per-agent rule.
    fn launched_by_for(&self, agent: &str, run_id: Option<&str>) -> Option<String> {
        let stored = self.agents.get(agent).ok().flatten()?;
        let runs = self.agents.runs(&stored);
        match run_id {
            Some(run_id) => runs.into_iter().find(|r| r.run_id == run_id),
            None => runs.into_iter().filter(|r| r.running).max_by_key(|r| r.started_at),
        }
        .map(|r| r.launched_by)
    }

    /// Build a [`QueuedEvent`] with the three defaults every app event carries, and push it.
    fn enqueue(&self, module: &Module, event: &str, extra: Value) {
        let mut properties = extra;
        if let Value::Object(map) = &mut properties {
            map.insert("version".into(), json!(self.version));
            map.insert("os".into(), json!(std::env::consts::OS));
            map.insert("arch".into(), json!(std::env::consts::ARCH));
        }
        queue::enqueue(
            module,
            QueuedEvent {
                id: uuid::Uuid::new_v4().to_string(),
                event: event.to_string(),
                ts_ms: now_ms(),
                properties,
            },
        );
    }
}

/// `adi_agents::launcher`'s three words, read down to the collector's own three-value enum:
/// `agent:<name>` → `agent`, [`adi_agents::launcher::AUTOMATION`] → `automation`, everything else
/// (a person, or the empty string nobody wrote down) → `human`. The empty case reads as `human`
/// rather than being dropped, matching `launcher::is_mine`'s own reasoning for why unattributed
/// counts as a person's rather than as neither.
fn launched_by_kind(raw: Option<&str>) -> &'static str {
    let raw = raw.unwrap_or_default();
    if raw.starts_with(adi_agents::launcher::AGENT_PREFIX) {
        "agent"
    } else if raw == adi_agents::launcher::AUTOMATION {
        "automation"
    } else {
        "human"
    }
}

/// Whether `state`'s install is still within its first 7 days — the window `agent_run_started`/
/// `agent_run_finished` are gated on. `false` for a `State` that has no `install_id` yet (and
/// therefore no [`State::first_launch_at`] either), which cannot happen on the path that calls
/// this (it always runs after [`Usage::ensure_install_id`] in the same tick) but is the safe
/// answer regardless.
fn within_first_week(state: &State) -> bool {
    state.first_launch_at > 0 && now_unix().saturating_sub(state.first_launch_at) <= 7 * 24 * 3600
}

fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis()),
    )
    .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Harness {
        usage: Usage,
        // Held for the harness's whole lifetime: every test here reads `settings::effective`,
        // which reads `ADI_ANALYTICS`/`DO_NOT_TRACK` — see `settings::ENV_TEST_LOCK`'s own doc.
        _env_guard: std::sync::MutexGuard<'static, ()>,
    }

    fn harness(tag: &str) -> Harness {
        let env_guard = settings::ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let root = std::env::temp_dir().join(format!(
            "adi-usage-dispatch-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        let config = Config::with_root(root);
        let (shutdown, _rx) = watch::channel(false);
        let (done, _) = watch::channel(false);
        let usage = Usage {
            config: config.clone(),
            events: Events::with_config(config.clone()),
            agents: Agents::with_config(config.clone()),
            projects: Projects::with_config(config.clone()),
            version: "1.30.0-test".to_string(),
            shutdown,
            done,
        };
        Harness {
            usage,
            _env_guard: env_guard,
        }
    }

    fn queued(h: &Harness) -> Vec<QueuedEvent> {
        queue::read(&h.usage.module())
    }

    fn enable(h: &Harness) {
        settings::set_enabled(&h.usage.module(), true).expect("enable");
    }

    fn disable(h: &Harness) {
        settings::set_enabled(&h.usage.module(), false).expect("disable");
    }

    #[test]
    fn while_off_nothing_is_queued_but_the_cursor_still_advances() {
        let h = harness("off");
        // Explicit: DEFAULT_ENABLED is now `true`, so this exercises "switched off" rather than
        // "never touched the setting".
        disable(&h);
        h.usage.events.emit("adi.fleet.invite.created", "{}").expect("emit");
        h.usage.tick_blocking();
        assert!(queued(&h).is_empty(), "off means nothing is queued");
        assert_eq!(
            h.usage.events.cursor(SUBSCRIBER).expect("cursor"),
            1,
            "the event is still caught up on, so it never floods in once switched on"
        );
    }

    #[test]
    fn the_first_tick_with_it_on_mints_an_install_id_and_sends_app_first_launch() {
        let h = harness("first-launch");
        enable(&h);
        h.usage.events.emit("adi.fleet.invite.created", "{}").expect("emit");
        h.usage.tick_blocking();

        let state = State::load(&h.usage.module());
        assert!(state.install_id.is_some());
        assert!(state.first_launch_at > 0);

        let items = queued(&h);
        let launch = items.iter().find(|i| i.event == "app_first_launch").expect("queued");
        assert_eq!(launch.properties["fresh"], true, "a store with no agents or projects");
        assert_eq!(launch.properties["os"], std::env::consts::OS);
        assert_eq!(launch.properties["version"], "1.30.0-test");

        // Minted exactly once — a later tick with it still on reuses the same id.
        h.usage.tick_blocking();
        assert_eq!(State::load(&h.usage.module()).install_id, state.install_id);
    }

    #[test]
    fn fresh_is_false_once_the_store_already_holds_an_agent() {
        let h = harness("not-fresh");
        // `Agents::save` itself publishes `adi.agents.saved`, which is also what gives the first
        // `tick_blocking` something to catch up on.
        h.usage
            .agents
            .save("some-agent", adi_agents::StoredAgentManifest::default())
            .expect("save");
        enable(&h);
        h.usage.tick_blocking();
        let launch = queued(&h)
            .into_iter()
            .find(|i| i.event == "app_first_launch")
            .expect("queued");
        assert_eq!(launch.properties["fresh"], false);
    }

    #[test]
    fn onboarding_completed_fires_once_for_the_root_agent_only() {
        let h = harness("onboarding");
        enable(&h);
        // `Agents::save` itself publishes `adi.agents.saved` (lib.rs's own `with_event` call) —
        // no need to emit it by hand, and doing so as well would double-count.
        h.usage
            .agents
            .save(
                ROOT_AGENT,
                adi_agents::StoredAgentManifest {
                    backend: Some(adi_agents::Backend::from("harness:claude-sdk")),
                    ..Default::default()
                },
            )
            .expect("save root agent");
        h.usage
            .agents
            .save("someone-else", adi_agents::StoredAgentManifest::default())
            .expect("save another agent");
        // Saved again later — must not fire a second time.
        h.usage
            .agents
            .save(
                ROOT_AGENT,
                adi_agents::StoredAgentManifest {
                    backend: Some(adi_agents::Backend::from("harness:claude-sdk")),
                    ..Default::default()
                },
            )
            .expect("resave root agent");

        h.usage.tick_blocking();

        let items = queued(&h);
        let onboarding: Vec<&QueuedEvent> =
            items.iter().filter(|i| i.event == "onboarding_completed").collect();
        assert_eq!(onboarding.len(), 1, "exactly one, however many times it is saved");
        assert_eq!(onboarding[0].properties["backend"], "harness:claude-sdk");
        assert!(State::load(&h.usage.module()).onboarding_sent);
    }

    #[test]
    fn run_events_outside_the_first_week_are_dropped_but_still_counted_daily() {
        let h = harness("week-window");
        enable(&h);
        {
            let module = h.usage.module();
            let mut state = State::load(&module);
            state.install_id = Some("11111111-1111-1111-1111-111111111111".to_string());
            state.first_launch_at = now_unix() - 8 * 24 * 3600; // nine days ago: past the window
            state.save(&module).expect("seed state");
        }
        h.usage
            .events
            .emit(
                "adi.agents.run.started",
                r#"{"backend":"process","agent":"ghost","message":"hi","pid":1,"run_id":"r-1"}"#,
            )
            .expect("emit");
        h.usage.tick_blocking();

        assert!(
            queued(&h).iter().all(|i| i.event != "agent_run_started"),
            "past the 7-day window, no per-run event goes out"
        );
        let state = State::load(&h.usage.module());
        assert_eq!(
            state.runs_human, 1,
            "the daily aggregate counts every run regardless of the window"
        );
    }

    #[test]
    fn run_events_inside_the_first_week_carry_backend_and_launched_by() {
        let h = harness("week-inside");
        enable(&h);
        h.usage
            .agents
            .save(
                "my-agent",
                adi_agents::StoredAgentManifest {
                    backend: Some(adi_agents::Backend::from("harness:claude-sdk")),
                    ..Default::default()
                },
            )
            .expect("save agent");
        h.usage
            .events
            .emit(
                "adi.agents.run.started",
                r#"{"backend":"process","agent":"my-agent","message":"hi","pid":1,"run_id":"r-1"}"#,
            )
            .expect("emit started");
        h.usage
            .events
            .emit(
                "adi.agents.run.finished",
                r#"{"agent":"my-agent","run_id":"r-1","is_error":false,"terminal_reason":"completed","result_head":"should never leave"}"#,
            )
            .expect("emit finished");
        h.usage.tick_blocking();

        let items = queued(&h);
        let started = items.iter().find(|i| i.event == "agent_run_started").expect("queued");
        assert_eq!(started.properties["backend"], "harness:claude-sdk");
        assert_eq!(started.properties["launched_by"], "human");
        assert!(started.properties.get("message").is_none(), "the task text never crosses");

        let finished = items.iter().find(|i| i.event == "agent_run_finished").expect("queued");
        assert_eq!(finished.properties["is_error"], false);
        assert_eq!(finished.properties["terminal_reason"], "completed");
        assert!(finished.properties.get("result_head").is_none(), "never forwarded");
        assert!(finished.properties.get("cost_micro_usd").is_none());
    }

    #[test]
    fn app_daily_active_rolls_over_once_the_local_day_changes() {
        let h = harness("daily");
        enable(&h);
        {
            let module = h.usage.module();
            let mut state = State::load(&module);
            state.install_id = Some("22222222-2222-2222-2222-222222222222".to_string());
            state.first_launch_at = now_unix();
            state.day = "2020-01-01".to_string(); // deliberately not today
            state.runs_human = 3;
            state.runs_agent = 1;
            state.save(&module).expect("seed state");
        }
        h.usage.events.emit("adi.fleet.invite.created", "{}").expect("emit");
        h.usage.tick_blocking();

        let items = queued(&h);
        let daily = items.iter().find(|i| i.event == "app_daily_active").expect("queued");
        assert_eq!(daily.properties["runs_human"], 3);
        assert_eq!(daily.properties["runs_agent"], 1);
        assert_eq!(daily.properties["runs_automation"], 0);

        let state = State::load(&h.usage.module());
        assert_eq!(state.day, chrono::Local::now().format("%Y-%m-%d").to_string());
        assert_eq!(state.runs_human, 0, "the new day starts at zero");
    }

    #[test]
    fn dashboard_created_forwards_only_a_known_from() {
        let h = harness("dashboard");
        enable(&h);
        h.usage.events.emit("adi.dashboards.created", r#"{"from":"import"}"#).expect("emit");
        h.usage.events.emit("adi.dashboards.created", r#"{"from":"nonsense"}"#).expect("emit");
        h.usage.tick_blocking();
        let items = queued(&h);
        let dashboards: Vec<&QueuedEvent> =
            items.iter().filter(|i| i.event == "dashboard_created").collect();
        assert_eq!(dashboards.len(), 1);
        assert_eq!(dashboards[0].properties["from"], "import");
    }

    #[test]
    fn store_item_installed_keeps_slug_for_the_public_store_only() {
        let h = harness("marketplace");
        enable(&h);
        h.usage
            .events
            .emit(
                "adi.marketplace.installed",
                r#"{"marketplace":"store","slug":"crm-suite","kind":"dashboard"}"#,
            )
            .expect("emit public");
        h.usage
            .events
            .emit(
                "adi.marketplace.installed",
                r#"{"marketplace":"my-private-source","slug":"secret-tool","kind":"tool"}"#,
            )
            .expect("emit private");
        // Dropped outright: not in the dictionary's four-value kind enum.
        h.usage
            .events
            .emit(
                "adi.marketplace.installed",
                r#"{"marketplace":"store","slug":"local-llm","kind":"llm"}"#,
            )
            .expect("emit llm");
        h.usage.tick_blocking();

        let items = queued(&h);
        let installs: Vec<&QueuedEvent> =
            items.iter().filter(|i| i.event == "store_item_installed").collect();
        assert_eq!(installs.len(), 2, "the llm install is dropped");

        let public = installs.iter().find(|i| i.properties["kind"] == "dashboard").unwrap();
        assert_eq!(public.properties["slug"], "crm-suite");

        let private = installs.iter().find(|i| i.properties["kind"] == "tool").unwrap();
        assert!(
            private.properties.get("slug").is_none(),
            "a non-store marketplace's slug is never forwarded"
        );
    }

    #[test]
    fn fleet_invite_created_carries_no_properties_beyond_the_defaults() {
        let h = harness("fleet");
        enable(&h);
        h.usage.events.emit("adi.fleet.invite.created", "{}").expect("emit");
        h.usage.tick_blocking();
        let items = queued(&h);
        let invite = items.iter().find(|i| i.event == "fleet_invite_created").expect("queued");
        let mut keys: Vec<&String> = invite.properties.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, vec!["arch", "os", "version"]);
    }

    #[test]
    fn launched_by_kind_maps_the_three_launcher_words() {
        assert_eq!(launched_by_kind(Some(adi_agents::launcher::HUMAN)), "human");
        assert_eq!(launched_by_kind(Some("")), "human");
        assert_eq!(launched_by_kind(None), "human");
        assert_eq!(launched_by_kind(Some(adi_agents::launcher::AUTOMATION)), "automation");
        assert_eq!(launched_by_kind(Some("agent:research-worker")), "agent");
    }

}
