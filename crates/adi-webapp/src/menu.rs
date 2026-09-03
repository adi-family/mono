//! What the ⌘K menu offers, as one list.
//!
//! The app has two shells — the root document (`/`: the chat and the setup wizard) and the control
//! panel (`/extended/…`) — and the ⌘K menu ([`crate::launcher`]) is on both. Almost every row
//! is worth the same on either: the panel's pages, the pairing QR, the dashboards this machine can
//! open, the version. So they are written once, here, and each shell asks for them
//! rather than keeping a copy. Two hand-maintained lists is how a menu comes to disagree with
//! itself about what the app can do — one shell grows a row and the other quietly doesn't.
//!
//! What the two do *not* share is how they get anywhere, and that is the whole of [`Shell`]. On the
//! root screen the panel is another document, so a page row is a navigation and pairing has to be
//! handed over through the URL; inside the panel a page row is a route change and the QR can be
//! raised in place. The rows read the same either way — the difference is a detail of plumbing and
//! is kept to this one type.
//!
//! The two rows that genuinely differ are each shell's way *out* of itself (the panel from the
//! chat, the chat from the panel) and the root screen's chat rows, which act on a conversation the
//! panel does not have. Those live with the shell that means them: the first pair below, the second
//! in `root_actions`.

use leptos::prelude::*;

use crate::launcher::Action;
use crate::routing::{self, Route};
use crate::state::{FleetForm, State};
use crate::{icons, origin, pages, pwa, update};

/// The marker the root screen leaves in the URL to say the panel was opened *in order to pair*.
///
/// An intent flag and nothing else. The token is minted by the panel over the API once it is
/// there — a URL is typed, pasted, logged and kept in browser history, and a pairing invite is a
/// bearer credential until it is spent, so the two never meet.
const PAIR_INTENT: &str = "pair";

/// Which shell is asking for the rows, and how it gets where a row points.
#[derive(Clone, Copy)]
pub(crate) enum Shell {
    /// The root document. Every panel page is another document from here, so its rows are real
    /// navigations and the pairing QR is asked for across the trip rather than raised in place.
    Root,
    /// The control panel, which owns the route signal its pages hang off and the Fleet page's
    /// form — so it can both navigate and pair without leaving.
    Panel {
        route: RwSignal<Route>,
        fleet: FleetForm,
    },
}

impl Shell {
    /// A row that opens one of the panel's pages.
    fn page(self, state: State, target: Route) -> Action {
        let icon = icons::route_icon(target);
        match self {
            Shell::Root => Action::link(target.title(), target.blurb(), icon, target.path()),
            Shell::Panel { route, .. } => {
                Action::new(target.title(), target.blurb(), icon, move || {
                    routing::go_global(state, route, target);
                })
            }
        }
    }

    /// Start pairing a device: land on the Fleet page **and** put a fresh QR on it.
    ///
    /// Navigate first, then mint — never the other way round. The panel clears the invite on every
    /// route that is not Fleet (a live token has no business outliving the screen it was drawn
    /// on), so a code minted before the navigation would be thrown away by the navigation itself.
    fn pair(self, state: State) {
        match self {
            // Across a document boundary the intent is all that travels; the panel picks it up in
            // [`consume_pair_intent`] and mints there.
            Shell::Root => {
                let _ = window()
                    .location()
                    .set_href(&format!("{}?{PAIR_INTENT}=1", Route::Fleet.path()));
            }
            Shell::Panel { route, fleet } => {
                routing::go_global(state, route, Route::Fleet);
                // Unconditional, so pressing this while already on Fleet mints a fresh code —
                // which is what somebody asking to pair a device on that page means by it.
                pages::fleet::mint(state, fleet);
            }
        }
    }
}

/// Every row both shells offer, in the order the menu reads them.
///
/// Rebuilt on every draw (see [`crate::launcher::overlay`]), which is what lets it be honest
/// about the moment: the dashboards it lists are the ones that are up *now*, and the version row
/// says what the last poll said. Reading signals here is therefore deliberate — it is what makes
/// the menu track them.
pub(crate) fn rows(
    shell: Shell,
    state: State,
    updates: update::UpdateWatch,
    can_install: RwSignal<bool>,
) -> Vec<Action> {
    let mut rows = Vec::new();

    // The way out of whichever shell is asking, first — it is the one row that is about where you
    // are rather than about what is on this machine.
    rows.push(match shell {
        Shell::Root => Action::link(
            "Extended",
            "The control panel",
            icons::Icon::Layers,
            routing::BASE,
        ),
        Shell::Panel { .. } => Action::link(
            "Back to chat",
            "The simple view — sessions and a composer",
            icons::Icon::Spark,
            "/",
        ),
    });

    rows.push(Action::new(
        "Pair new device",
        // Says what will happen, and carries the words somebody would type looking for it —
        // "invite", "qr" and "fleet" are all names this one press goes by.
        "Mint an invite and show its QR on Fleet",
        icons::Icon::Node,
        move || shell.pair(state),
    ));

    // Every page of the panel that can be opened by name alone. Driven off [`Route::NAV`] rather
    // than written out, so a page added there is in the menu the same day.
    rows.extend(Route::NAV.into_iter().map(|t| shell.page(state, t)));

    // One row per dashboard that is actually up, so the menu is a way *into* them and not a
    // list of names. A dashboard with no address is down, and a row that opens nothing is
    // worse than no row — the Dashboards row above is the way to those.
    if let Some(ds) = state.dashboards.get() {
        for d in ds.dashboards.iter().filter(|d| !d.is_archived()) {
            if let Some(href) = pages::dashboards::open_url(d) {
                rows.push(Action::tab(
                    d.name.clone(),
                    "Dashboard",
                    icons::Icon::Dashboard,
                    href,
                ));
            }
        }
    }
    // And the fleet's, under the same rule the rail applies to them: a node's dashboard is
    // reachable only when it is running, when this machine has actually been granted it, and
    // when the address the node gave resolves from here (see [`origin::mapped_url`]). The node
    // is the hint, because on a fleet the same dashboard name is on more than one machine.
    //
    // Only the root screen ever fills this list — the panel has no dashboards rail to ask the
    // fleet what it runs — so on the panel these rows are simply absent, which is the truth.
    if let Some(fleet) = state.fleet_dashboards.get() {
        for node in &fleet.nodes {
            for d in node.dashboards.iter().filter(|d| d.running && d.allowed) {
                if let Some(href) = d.url.as_deref().and_then(origin::mapped_url) {
                    rows.push(Action::tab(
                        d.name.clone(),
                        node.node.clone(),
                        icons::Icon::Dashboard,
                        href,
                    ));
                }
            }
        }
    }

    if can_install.get() {
        rows.push(Action::new(
            "Install app",
            "Run adi in its own window",
            icons::Icon::Download,
            pwa::install,
        ));
    }
    rows.push(update::action(updates));
    rows
}

/// Act on the `?pair=1` the root screen's **Pair new device** row left behind: raise the QR on the
/// Fleet page the browser has just landed on, and take the marker back out of the address bar.
///
/// Called once, as the panel mounts. Stripping the marker first is what stops a reload of this page
/// from being a second invite nobody asked for — after it, the URL is a plain visit to Fleet.
pub(crate) fn consume_pair_intent(state: State, fleet: FleetForm, route: RwSignal<Route>) {
    if route.get_untracked() != Route::Fleet || routing::query_param(PAIR_INTENT).is_none() {
        return;
    }
    routing::replace_state(Route::Fleet.path());
    pages::fleet::mint(state, fleet);
}
