//! The paired devices, read once for the whole screen: the top bar's count and list
//! ([`super::sources`]) and the device window ([`super::device`]) draw the same answer, so a
//! device unpaired in one is gone from the other without either asking again.
//!
//! Both halves start from the last load's answers ([`super::cache`]), so a reload shows the
//! devices at once and the reads below only update them.

use std::collections::HashMap;

use adi_webapp_api::types::{FleetNode, FleetState, Reach};
use leptos::prelude::*;

use super::cache;
use crate::fetch;

const NODES_KEY: &str = "fleet-nodes";
const REACH_KEY: &str = "fleet-reach";

/// How often the paired devices are read and dialled again. A machine coming up is not urgent
/// news, and every round is a mesh dial per node. Opening the list asks at once.
const TICK_MS: u32 = 30_000;

#[derive(Clone, Copy)]
pub(super) struct Fleet {
    /// `None` until the fleet first answers (or a previous load's answer is found), so nothing
    /// claims "0 devices" it has not been told.
    pub(super) nodes: RwSignal<Option<Vec<FleetNode>>>,
    /// `None` while the first dials are out: a node not yet dialled is not yet unreachable.
    pub(super) reach: RwSignal<Option<HashMap<String, Reach>>>,
}

impl Fleet {
    /// Read now, and again every [`TICK_MS`] for as long as the calling scope lives.
    pub(super) fn load() -> Self {
        let fleet = Self {
            nodes: RwSignal::new(cache::load(NODES_KEY)),
            reach: RwSignal::new(cache::load(REACH_KEY)),
        };
        fleet.refresh();
        let tick = set_interval_with_handle(
            move || fleet.refresh(),
            std::time::Duration::from_millis(TICK_MS.into()),
        );
        on_cleanup(move || {
            if let Ok(t) = tick {
                t.clear();
            }
        });
        fleet
    }

    pub(super) fn refresh(self) {
        self.read();
        // Apart from the fleet, which is local and quick: this one waits on every node's dial.
        // The fleet is read again once it is back, because an answered dial is what moves a
        // device's "last connected".
        leptos::task::spawn_local(async move {
            if let Ok(r) = fetch::fleet_reach().await {
                let reach: HashMap<String, Reach> =
                    r.nodes.into_iter().map(|n| (n.node, n.reach)).collect();
                cache::save(REACH_KEY, &reach);
                self.reach.set(Some(reach));
                self.read();
            }
        });
    }

    fn read(self) {
        leptos::task::spawn_local(async move {
            // A failed read keeps the last answer: stale by a tick beats blank.
            if let Ok(fleet) = fetch::fleet().await {
                self.set_nodes(fleet.nodes);
            }
        });
    }

    /// Take the registry an edit answered with, so every view shows it without a second read.
    pub(super) fn apply(self, state: FleetState) {
        self.set_nodes(state.nodes);
    }

    fn set_nodes(self, nodes: Vec<FleetNode>) {
        cache::save(NODES_KEY, &nodes);
        self.nodes.set(Some(nodes));
    }

    /// One device by the name this machine files it under — tracked.
    pub(super) fn node(self, petname: &str) -> Option<FleetNode> {
        self.nodes
            .with(|n| n.as_ref()?.iter().find(|n| n.petname == petname).cloned())
    }

    /// What the last dial to a device found — tracked; `None` until it is back.
    pub(super) fn reach_of(self, petname: &str) -> Option<Reach> {
        self.reach.with(|r| r.as_ref()?.get(petname).copied())
    }
}

/// A reachability as the dot's tone and the words beside it.
pub(super) fn reach_label(reach: Option<Reach>) -> (&'static str, &'static str) {
    match reach {
        Some(Reach::Reachable) => ("on", "Reachable"),
        Some(Reach::Refused) => ("warn", "Refuses this machine"),
        Some(Reach::Unreachable) => ("err", "Unreachable"),
        Some(Reach::MeshOff) => ("err", "Mesh is off"),
        None => ("off", "Checking…"),
    }
}

/// How long ago a Unix-seconds moment was: `just now`, `3m ago`, `2d ago` — or `never`.
pub(super) fn ago(at: Option<u64>, now: u64) -> String {
    match at {
        None => "never".into(),
        Some(at) => match now.saturating_sub(at) {
            0..=59 => "just now".into(),
            s if s < 3_600 => format!("{}m ago", s / 60),
            s if s < 86_400 => format!("{}h ago", s / 3_600),
            s => format!("{}d ago", s / 86_400),
        },
    }
}

/// Now, in Unix seconds, by the browser's clock.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(super) fn now_unix() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64
}

#[cfg(test)]
mod tests {
    use super::ago;

    #[test]
    fn ago_reads_the_gap() {
        let now = 1_000_000;
        assert_eq!(ago(None, now), "never");
        assert_eq!(ago(Some(now - 10), now), "just now");
        assert_eq!(ago(Some(now - 90), now), "1m ago");
        assert_eq!(ago(Some(now - 3 * 86_400), now), "3d ago");
    }
}
