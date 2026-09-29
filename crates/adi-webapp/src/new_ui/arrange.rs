//! How the person has arranged the home screen: the order of its apps and widgets, the size each
//! widget is drawn at, and what they took off it.
//!
//! A per-device preference, in `localStorage` beside the wallpaper and the layout — the same rule
//! as every new-UI setting. It holds only what the person changed: an app nobody moved keeps its
//! place from the listing, and one installed later lands at the end of its machine's section, as
//! a new app does on iOS.
//!
//! Everything is keyed by an item id ([`app_id`], [`widget_id`]) that carries the machine, so an
//! item can only move among its own machine's — a section is a machine, and an app from one
//! filed under another's name would say something false.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use super::widgets::Size;
use crate::ui;

const KEY: &str = "adi-new-ui-home";

/// The id of the app tile keyed `key` (`<machine>:<app>`, the machine empty for this one).
pub(super) fn app_id(key: &str) -> String {
    format!("app|{key}")
}

/// The id of the widget app `key` declares at `path`.
pub(super) fn widget_id(key: &str, path: &str) -> String {
    format!("widget|{key}|{path}")
}

/// The id of the website keyed `key` (`<machine>:<host><path>`, see `sites::Site::key`).
pub(super) fn site_id(key: &str) -> String {
    format!("site|{key}")
}

/// The machine an item belongs to: empty for this one.
fn section(id: &str) -> &str {
    let key = id.split_once('|').map_or(id, |(_, rest)| rest);
    key.split_once(':').map_or("", |(machine, _)| machine)
}

/// Whether two items are the same machine's, and so may trade places.
pub(super) fn same_machine(a: &str, b: &str) -> bool {
    section(a) == section(b)
}

/// What was changed, and nothing else.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Arrangement {
    /// Item ids in the order they were last put in. Items missing from it follow, in the
    /// listing's order.
    order: Vec<String>,
    /// A widget's size where it is not the one its app declares. At most one is `Half`.
    sizes: BTreeMap<String, Size>,
    /// Items taken off the home screen.
    hidden: BTreeSet<String>,
}

impl Arrangement {
    /// `items` — one section's, in the listing's order — in the arranged order.
    pub(super) fn sorted<T>(&self, items: Vec<(String, T)>) -> Vec<(String, T)> {
        let at: HashMap<&str, usize> = self
            .order
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        let mut items: Vec<(usize, (String, T))> = items
            .into_iter()
            .enumerate()
            .map(|(natural, item)| {
                let pos = at.get(item.0.as_str()).copied().unwrap_or(self.order.len() + natural);
                (pos, item)
            })
            .collect();
        items.sort_by_key(|(pos, _)| *pos);
        items.into_iter().map(|(_, item)| item).collect()
    }

    pub(super) fn is_hidden(&self, id: &str) -> bool {
        self.hidden.contains(id)
    }

    pub(super) fn hidden_count(&self) -> usize {
        self.hidden.len()
    }

    /// Widget `id`'s size: the one chosen here, else `declared`.
    pub(super) fn size(&self, id: &str, declared: Size) -> Size {
        self.sizes.get(id).copied().unwrap_or(declared)
    }

    /// The widget chosen here to take the right half, if one was.
    pub(super) fn chosen_half(&self) -> Option<&str> {
        self.sizes
            .iter()
            .find(|(_, s)| **s == Size::Half)
            .map(|(id, _)| id.as_str())
    }

    pub(super) fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// `ids` — every item on the screen, in the order shown — with `from` put where `to` is: after it
/// when moving forward, before it when moving back, as a dragged tile lands. `None` when that is
/// not a move: the same item, one not shown, or another machine's.
pub(super) fn moved(ids: &[String], from: &str, to: &str) -> Option<Vec<String>> {
    if from == to || section(from) != section(to) {
        return None;
    }
    let f = ids.iter().position(|i| i == from)?;
    let t = ids.iter().position(|i| i == to)?;
    let mut out = ids.to_vec();
    let item = out.remove(f);
    out.insert(t, item);
    Some(out)
}

/// The home screen's arrangement, and whether it is being edited.
#[derive(Clone, Copy)]
pub(super) struct Arrange {
    pub(super) now: RwSignal<Arrangement>,
    /// Editing: tiles no longer open, and every item can be dragged, resized or taken off.
    pub(super) editing: RwSignal<bool>,
    /// The item being dragged.
    pub(super) dragging: RwSignal<Option<String>>,
    /// The item it is over, where it would land.
    pub(super) target: RwSignal<Option<String>>,
}

impl Arrange {
    pub(super) fn load() -> Self {
        let now = ui::storage()
            .and_then(|s| s.get_item(KEY).ok().flatten())
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        Self {
            now: RwSignal::new(now),
            editing: RwSignal::new(false),
            dragging: RwSignal::new(None),
            target: RwSignal::new(None),
        }
    }

    fn set(self, change: impl FnOnce(&mut Arrangement)) {
        self.now.update(change);
        if let Some(s) = ui::storage() {
            let now = self.now.get_untracked();
            let _ = if now.is_empty() {
                s.remove_item(KEY)
            } else {
                serde_json::to_string(&now).map_or(Ok(()), |json| s.set_item(KEY, &json))
            };
        }
    }

    pub(super) fn edit(self, on: bool) {
        self.editing.set(on);
        self.dragging.set(None);
        self.target.set(None);
    }

    /// Put `from` where `to` is, among `ids` as shown.
    pub(super) fn put(self, ids: &[String], from: &str, to: &str) {
        if let Some(order) = moved(ids, from, to) {
            self.set(|a| a.order = order);
        }
    }

    /// Draw widget `id` at `size`. Only one widget takes the half: choosing it for this one sends
    /// the one that had it into the grid.
    pub(super) fn resize(self, id: &str, size: Size, half_now: Option<&str>) {
        let id = id.to_string();
        let half_now = half_now.filter(|h| *h != id).map(str::to_string);
        self.set(|a| {
            if size == Size::Half {
                for s in a.sizes.values_mut() {
                    if *s == Size::Half {
                        *s = Size::Large;
                    }
                }
                if let Some(h) = half_now {
                    a.sizes.insert(h, Size::Large);
                }
            }
            a.sizes.insert(id, size);
        });
    }

    pub(super) fn hide(self, id: &str) {
        let id = id.to_string();
        self.set(|a| {
            a.hidden.insert(id);
        });
    }

    pub(super) fn show_hidden(self) {
        self.set(|a| a.hidden.clear());
    }

    /// Back to the listing's order and the apps' own sizes, everything shown.
    pub(super) fn reset(self) {
        self.set(|a| *a = Arrangement::default());
    }
}

#[cfg(test)]
mod tests {
    use super::{Arrangement, app_id, moved, section, widget_id};

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_item_knows_its_machine() {
        assert_eq!(section(&app_id(":crm")), "");
        assert_eq!(section(&app_id("laptop:crm")), "laptop");
        assert_eq!(section(&widget_id(":board", "/widget/chat")), "");
        assert_eq!(section(&widget_id("laptop:board", "/w:x")), "laptop");
    }

    #[test]
    fn a_drop_lands_after_forward_and_before_back() {
        let all = ids(&["app|:a", "app|:b", "app|:c", "app|:d"]);
        assert_eq!(
            moved(&all, "app|:a", "app|:c").unwrap(),
            ids(&["app|:b", "app|:c", "app|:a", "app|:d"])
        );
        assert_eq!(
            moved(&all, "app|:d", "app|:b").unwrap(),
            ids(&["app|:a", "app|:d", "app|:b", "app|:c"])
        );
        assert!(moved(&all, "app|:a", "app|:a").is_none());
    }

    #[test]
    fn nothing_moves_to_another_machine() {
        let all = ids(&["app|:a", "app|m:b"]);
        assert!(moved(&all, "app|:a", "app|m:b").is_none());
    }

    #[test]
    fn unarranged_items_follow_in_listing_order() {
        let a = Arrangement {
            order: ids(&["app|:c", "app|:a"]),
            ..Default::default()
        };
        let items = vec![
            ("app|:a".to_string(), 1),
            ("app|:b".to_string(), 2),
            ("app|:c".to_string(), 3),
            ("app|:new".to_string(), 4),
        ];
        let got: Vec<i32> = a.sorted(items).into_iter().map(|(_, v)| v).collect();
        assert_eq!(got, vec![3, 1, 2, 4]);
    }
}
