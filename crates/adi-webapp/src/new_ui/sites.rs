//! Websites on the home screen: any address a machine serves, added by hand — a project's own
//! pages (`app.nosh1.adi` on hetzner-nosh) that are not apps and so are never listed as one.
//!
//! Added by typing the address as the machine itself calls it and picking the machine
//! ([`AddSite`]). What the address is from *here* is worked out, not typed: a `.adi` name served by
//! a paired machine is reached as `<name>.<machine>.n.adi` ([`address`]), and that machine is
//! asked to let this one open it, as a locked app tile asks ([`crate::fetch::allow_node_service`]).
//!
//! A per-device list in `localStorage`, beside the home screen's arrangement ([`super::arrange`]).

use adi_ui::{Icon, IconSize, Lucide, Mark};
use leptos::{ev, html, prelude::*};
use serde::{Deserialize, Serialize};

use super::fleet::Fleet;
use crate::{fetch, origin, ui};

const KEY: &str = "adi-new-ui-sites";

/// One website on the home screen.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(super) struct Site {
    /// Its host as the machine serving it calls it: `app.nosh1.adi`, or a real domain.
    pub(super) host: String,
    /// The path to open, from `/`.
    pub(super) path: String,
    /// The paired machine serving it; `None` for this one.
    pub(super) machine: Option<String>,
}

impl Site {
    /// Unique on the screen, and the item id's key ([`super::arrange`]): machine first.
    pub(super) fn key(&self) -> String {
        format!("{}:{}{}", self.machine.as_deref().unwrap_or(""), self.host, self.path)
    }

    /// What its tile says: the host, without the zone every ADI name ends in.
    pub(super) fn name(&self) -> String {
        let name = self.host.strip_suffix(".adi").unwrap_or(&self.host);
        if self.path == "/" {
            name.to_string()
        } else {
            format!("{name}{}", self.path)
        }
    }

    /// Where it opens from here, if anywhere.
    pub(super) fn url(&self) -> Option<String> {
        address(&self.host, &self.path, self.machine.as_deref(), origin::viewing_node().as_deref())
    }
}

/// What was typed, as a host, a path, and — when it was a whole fleet address — the machine in it.
/// `None` when there is no host to take.
///
/// Forgiving on purpose, because the address is copied from wherever it was seen: a name typed
/// without its zone (`crm`, `app.nosh1`) gets the `.adi` every machine's names live under; and
/// `app.nosh1.hetzner-nosh.n.adi`, pasted from a browser, is read back as `app.nosh1.adi` on
/// hetzner-nosh. Only an address with its scheme (`https://example.com`) is taken as it is — a
/// real domain — as are `localhost`, an IP address and anything with a port: none of those is an
/// ADI name, and there is no telling `app.nosh1` from a domain by its shape.
pub(super) fn parse(typed: &str) -> Option<(String, String, Option<String>)> {
    let typed = typed.trim();
    let t = typed
        .strip_prefix("http://")
        .or_else(|| typed.strip_prefix("https://"))
        .unwrap_or(typed);
    let exact = t.len() != typed.len();
    let (host, path) = match t.find('/') {
        Some(i) => (&t[..i], &t[i..]),
        None => (t, "/"),
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || host.contains(char::is_whitespace) {
        return None;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if let [service @ .., node, "n", "adi"] = labels.as_slice()
        && !service.is_empty()
    {
        return Some((format!("{}.adi", service.join(".")), path.to_string(), Some((*node).to_string())));
    }
    // A name without a dot is no domain, however it was written.
    let literal = (exact && host.contains('.'))
        || host.ends_with(".adi")
        || host == "localhost"
        || host.contains(':')
        || host.parse::<std::net::Ipv4Addr>().is_ok();
    let host = if literal { host } else { format!("{host}.adi") };
    Some((host, path.to_string(), None))
}

/// Where `host` + `path`, served by `machine` (`None`: this one), opens for a page viewed through
/// `viewing` (`None`: viewed on this machine).
///
/// A `.adi` name is only its machine's: from here it is `<name>.<machine>.n.adi`. A real domain
/// means the same thing everywhere. Viewed through a node, only that node's own names reach — any
/// other machine's would be a hop more than the gateway routes.
pub(super) fn address(
    host: &str,
    path: &str,
    machine: Option<&str>,
    viewing: Option<&str>,
) -> Option<String> {
    let Some(service) = host.strip_suffix(".adi") else {
        return Some(format!("http://{host}{path}"));
    };
    if service.is_empty() || service == "n" || service.ends_with(".n") {
        return None;
    }
    let on = match (machine, viewing) {
        (None, None) => return Some(format!("http://{host}{path}")),
        (Some(m), None) => m,
        (None, Some(v)) => v,
        (Some(m), Some(v)) if m == v => m,
        (Some(_), Some(_)) => return None,
    };
    Some(format!("http://{service}.{on}.n.adi{path}"))
}

/// The websites added on this device, and whether the add panel is open.
#[derive(Clone, Copy)]
pub(super) struct Sites {
    pub(super) list: RwSignal<Vec<Site>>,
    pub(super) adding: RwSignal<bool>,
}

impl Sites {
    pub(super) fn load() -> Self {
        let list = ui::storage()
            .and_then(|s| s.get_item(KEY).ok().flatten())
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        Self {
            list: RwSignal::new(list),
            adding: RwSignal::new(false),
        }
    }

    fn set(self, change: impl FnOnce(&mut Vec<Site>)) {
        self.list.update(change);
        if let (Some(s), Ok(json)) = (
            ui::storage(),
            serde_json::to_string(&self.list.get_untracked()),
        ) {
            let _ = s.set_item(KEY, &json);
        }
    }

    /// Add `site`, unless it is on the screen already.
    fn add(self, site: Site) {
        self.set(|l| {
            if !l.contains(&site) {
                l.push(site);
            }
        });
    }

    /// Take the site keyed `key` off the screen — and out of the list: it was only ever added here.
    pub(super) fn remove(self, key: &str) {
        self.set(|l| l.retain(|s| s.key() != key));
    }
}

/// The add panel: the address, then the machine that serves it — one row per machine, each saying
/// where the site will open from here, so picking the row is adding it.
///
/// The palette's floating surface, and its keys: arrows move between machines, Enter adds.
#[component]
pub(super) fn AddSite(sites: Sites, fleet: Fleet, #[prop(into)] light: Signal<bool>) -> impl IntoView {
    let open = sites.adding;
    let typed = RwSignal::new(String::new());
    let cursor = RwSignal::new(0usize);
    let busy = RwSignal::new(false);
    let failed: RwSignal<Option<String>> = RwSignal::new(None);
    let field: NodeRef<html::Input> = NodeRef::new();

    Effect::new(move |was: Option<bool>| {
        let now = open.get();
        if now && was != Some(true) {
            typed.set(String::new());
            cursor.set(0);
            failed.set(None);
            if let Some(el) = field.get_untracked() {
                let _ = el.focus();
            }
        }
        now
    });
    Effect::new(move |_| {
        if open.get()
            && let Some(el) = field.get()
        {
            let _ = el.focus();
        }
    });

    // The machines to pick from: this one, then every paired one by the name this machine calls
    // it — the name in its fleet addresses.
    let machines = move || {
        let mut m: Vec<Option<String>> = vec![None];
        fleet.nodes.with(|n| {
            m.extend(n.iter().flatten().map(|n| Some(n.petname.clone())));
        });
        m
    };
    let parsed = move || typed.with(|t| parse(t));
    // A pasted fleet address names its machine: that row comes first under the cursor.
    Effect::new(move |_| {
        if let Some((_, _, Some(node))) = parsed()
            && let Some(i) = machines().iter().position(|m| m.as_deref() == Some(node.as_str()))
        {
            cursor.set(i);
        }
    });

    let add = move |machine: Option<String>| {
        let Some((host, path, _)) = parsed() else {
            return;
        };
        let site = Site { host, path, machine };
        if site.url().is_none() || busy.get_untracked() {
            return;
        }
        // Another machine's `.adi` name opens only once that machine lets this one: ask first,
        // and add only what will open. A real domain needs no one's leave.
        let ask = site
            .machine
            .clone()
            .zip(site.host.strip_suffix(".adi").map(str::to_string));
        let Some((node, service)) = ask else {
            sites.add(site);
            open.set(false);
            return;
        };
        busy.set(true);
        failed.set(None);
        leptos::task::spawn_local(async move {
            match fetch::allow_node_service(node, service).await {
                Ok(_) => {
                    sites.add(site);
                    open.set(false);
                }
                Err(e) => failed.set(Some(e)),
            }
            busy.set(false);
        });
    };

    let on_key = move |ev: ev::KeyboardEvent| {
        let n = machines().len();
        match ev.key().as_str() {
            "ArrowDown" => {
                ev.prevent_default();
                cursor.set((cursor.get_untracked() + 1) % n);
            }
            "ArrowUp" => {
                ev.prevent_default();
                cursor.set((cursor.get_untracked() + n - 1) % n);
            }
            "Enter" => {
                ev.prevent_default();
                if let Some(m) = machines().get(cursor.get_untracked().min(n - 1)) {
                    add(m.clone());
                }
            }
            // Stopped here, as the palette's is, so the window under it stays open.
            "Escape" => {
                ev.prevent_default();
                ev.stop_propagation();
                open.set(false);
            }
            _ => {}
        }
    };

    view! {
        <Show when=move || open.get()>
            <div class="adi-pal-catch" on:pointerdown=move |_| open.set(false)></div>
            <div
                class="adi-pal adi-new-add"
                class:light=move || light.get()
                role="dialog"
                aria-modal="true"
                aria-label="Add website"
            >
                <input
                    node_ref=field
                    class="adi-pal__search"
                    type="text"
                    placeholder="Website address, as its machine calls it — app.nosh1.adi"
                    autocomplete="off"
                    spellcheck="false"
                    prop:value=move || typed.get()
                    on:input=move |ev| {
                        typed.set(event_target_value(&ev));
                        failed.set(None);
                    }
                    on:keydown=on_key
                />
                <div class="adi-pal__list" role="listbox">
                    <div class="adi-pal__section">"Served by"</div>
                    {move || {
                        let at = cursor.get();
                        let p = parsed();
                        machines()
                            .into_iter()
                            .enumerate()
                            .map(|(i, m)| {
                                let url = p.as_ref().and_then(|(host, path, _)| {
                                    address(host, path, m.as_deref(), origin::viewing_node().as_deref())
                                });
                                let label = m.clone().unwrap_or_else(|| "This machine".to_string());
                                let icon = if m.is_some() { Lucide::Network } else { Lucide::Laptop };
                                let pick = m.clone();
                                let unreachable = url.is_none();
                                view! {
                                    <button
                                        type="button"
                                        class="adi-pal__row"
                                        role="option"
                                        aria-selected=(i == at).to_string()
                                        prop:disabled=unreachable
                                        on:pointermove=move |_| {
                                            if cursor.get_untracked() != i {
                                                cursor.set(i);
                                            }
                                        }
                                        on:click=move |_| add(pick.clone())
                                    >
                                        <span class="adi-pal__icon">
                                            <Icon icon=icon size=IconSize::Md/>
                                        </span>
                                        <span class="adi-pal__title">{label}</span>
                                        <span class="adi-pal__subtitle adi-new-add__url">
                                            {url.unwrap_or_default()}
                                        </span>
                                    </button>
                                }
                            })
                            .collect_view()
                    }}
                </div>
                {move || failed.get().map(|e| view! { <p class="adi-new-add__error">{e}</p> })}
                <footer class="adi-pal__foot">
                    <Mark class="adi-pal__mark"/>
                    <span class="adi-pal__spacer"></span>
                    <span class="adi-pal__hint">
                        {move || if busy.get() { "Asking for access…" } else { "Add to home screen" }}
                    </span>
                    <kbd class="adi-pal__key" aria-label="Enter">
                        <Icon icon=Lucide::CornerDownLeft size=IconSize::Sm/>
                    </kbd>
                </footer>
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::{Site, address, parse};

    fn p(s: &str) -> (String, String, Option<String>) {
        parse(s).unwrap()
    }

    #[test]
    fn what_was_typed_becomes_a_host_and_a_path() {
        assert_eq!(p("app.nosh1.adi"), ("app.nosh1.adi".into(), "/".into(), None));
        assert_eq!(p(" http://crm/deals "), ("crm.adi".into(), "/deals".into(), None));
        assert_eq!(p("app.nosh1"), ("app.nosh1.adi".into(), "/".into(), None));
        assert_eq!(p("https://example.com"), ("example.com".into(), "/".into(), None));
        assert_eq!(p("localhost:3000/x"), ("localhost:3000".into(), "/x".into(), None));
        assert_eq!(p("10.0.0.2"), ("10.0.0.2".into(), "/".into(), None));
        assert_eq!(
            p("http://app.nosh1.hetzner-nosh.n.adi/login"),
            ("app.nosh1.adi".into(), "/login".into(), Some("hetzner-nosh".into()))
        );
        assert!(parse("  ").is_none());
    }

    #[test]
    fn a_paired_machines_name_goes_through_the_fleet() {
        assert_eq!(
            address("app.nosh1.adi", "/", Some("hetzner-nosh"), None).as_deref(),
            Some("http://app.nosh1.hetzner-nosh.n.adi/")
        );
        assert_eq!(address("app.nosh1.adi", "/", None, None).as_deref(), Some("http://app.nosh1.adi/"));
        assert_eq!(
            address("example.com", "/x", Some("hetzner-nosh"), None).as_deref(),
            Some("http://example.com/x")
        );
        // Viewed through a node: only that node's names reach.
        assert_eq!(
            address("crm.adi", "/", None, Some("laptop")).as_deref(),
            Some("http://crm.laptop.n.adi/")
        );
        assert!(address("crm.adi", "/", Some("other"), Some("laptop")).is_none());
        assert!(address("x.n.adi", "/", Some("m"), None).is_none());
    }

    #[test]
    fn a_site_is_keyed_by_its_machine_first() {
        let s = Site { host: "app.nosh1.adi".into(), path: "/".into(), machine: Some("hetzner-nosh".into()) };
        assert_eq!(s.key(), "hetzner-nosh:app.nosh1.adi/");
        assert_eq!(s.name(), "app.nosh1");
    }
}
