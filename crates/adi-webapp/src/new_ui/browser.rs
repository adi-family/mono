//! An app window, at `/apps/<app>`: one app framed in a small browser — a bar with reload, the
//! address, and the way out to a whole tab — opened from its tile on the home screen
//! ([`super::apps`]). The window itself is [`super::windows::Frame`]'s.
//!
//! An iframe, so the app runs exactly as it does in a tab of its own, on its own origin: nothing
//! here reaches into it, and nothing in it reaches out.

use adi_ui::{Icon, IconSize, Lucide};
use leptos::{ev, html, prelude::*};

use super::windows::{Desk, Win};

#[component]
pub(super) fn Browser(desk: Desk, id: u32) -> impl IntoView {
    let frame: NodeRef<html::Iframe> = NodeRef::new();

    // A press inside the page is the page's own and never reaches this document, so it cannot
    // bring the window forward the way a press anywhere else in a window does. What does reach it
    // is this document losing focus to the frame — checked a turn later, once the browser has
    // said where focus went.
    let blur = window_event_listener(ev::blur, move |_| {
        set_timeout(
            move || {
                let into_frame = frame.get_untracked().is_some_and(|f| {
                    document()
                        .active_element()
                        .is_some_and(|a| f.is_same_node(Some(&a)))
                });
                if into_frame {
                    desk.focus(Win::App(id));
                }
            },
            std::time::Duration::ZERO,
        );
    });
    on_cleanup(move || blur.remove());

    // Read once: a window's app never changes, and a view that tracked the list of open apps
    // would be rebuilt — and its page reloaded — each time another app opened or closed.
    let Some(app) = desk.app_untracked(id) else {
        return ().into_any();
    };
    let reload_url = app.url.clone();
    // What the bar shows: the address without its scheme, which is always http here.
    let shown = app
        .url
        .split_once("://")
        .map_or(app.url.as_str(), |(_, rest)| rest)
        .trim_end_matches('/')
        .to_string();
    view! {
        <div class="adi-new-browser">
            <div class="adi-new-browser__bar">
                // Back to the address it opened on: a page on another origin will not say
                // where inside it has gone since.
                <button
                    class="adi-new-browser__btn"
                    type="button"
                    title="Reload"
                    aria-label="Reload"
                    on:click=move |_| {
                        if let Some(f) = frame.get_untracked() {
                            f.set_src(&reload_url);
                        }
                    }
                >
                    <Icon icon=Lucide::RotateCw size=IconSize::Md/>
                </button>
                <span class="adi-new-browser__address" title=app.url.clone()>{shown}</span>
                <a
                    class="adi-new-browser__btn adi-new-browser__out"
                    href=app.url.clone()
                    target="_blank"
                    rel="noopener"
                    title="Open this app in a tab of its own"
                >
                    <Icon icon=Lucide::ArrowUpRight size=IconSize::Md/>
                    "Open in new tab"
                </a>
            </div>
            <iframe
                node_ref=frame
                class="adi-new-browser__view"
                src=app.url.clone()
                title=app.name.clone()
                allow="clipboard-read; clipboard-write; fullscreen"
            ></iframe>
        </div>
    }
    .into_any()
}
