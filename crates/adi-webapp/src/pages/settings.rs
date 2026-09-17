//! The Settings landing page: an index of the seven pages Settings gathers — Hive, Ports manager,
//! Mesh, Fleet, LLM backends, Embedding backends and Shared assets — so opening Settings from the
//! ⌘K menu lands on a page of its own rather than dropping onto whichever of the seven happens to
//! be first.

use adi_ui::Icon;
use leptos::prelude::*;

use crate::icons;
use crate::routing::{self, Route};
use crate::state::State;

/// The Settings page: one row per page it gathers, each naming what it holds so a reader can pick
/// the right one without opening it first.
pub(crate) fn settings_view(state: State, route: RwSignal<Route>) -> AnyView {
    view! {
        <section class="adi-panel">
            <div class="adi-panel__body">
                {Route::SETTINGS
                    .into_iter()
                    .map(|target| settings_row(state, route, target))
                    .collect::<Vec<_>>()}
            </div>
        </section>
    }
    .into_any()
}

/// One row: the page's icon, title and blurb, navigating in-panel rather than through a raw
/// `href` — Settings' pages are workbench routes, not documents of their own.
fn settings_row(state: State, route: RwSignal<Route>, target: Route) -> AnyView {
    view! {
        <button type="button" class="adi-linkrow"
            on:click=move |_| routing::go_global(state, route, target)>
            <Icon icon=icons::route_icon(target).lucide() class="adi-linkrow__icon"/>
            <span class="adi-linkrow__text">
                <span class="adi-linkrow__title">{target.title()}</span>
                <span class="adi-linkrow__blurb">{target.blurb()}</span>
            </span>
        </button>
    }
    .into_any()
}
