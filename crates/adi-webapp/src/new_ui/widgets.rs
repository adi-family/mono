//! Widgets: live surfaces on the home screen, each one an app's.
//!
//! The home screen draws none of its own. What it shows is whatever the apps offer — the chat
//! included, which is the Agent board app's widget — so the default screen is apps too, and
//! changes the way any app does.
//!
//! An app offers a widget in its `config.toml`, one `[widget.<id>]` table each:
//!
//! ```toml
//! [widget.chat]
//! name = "Chat"
//! url = "/widget/chat"
//! size = "half"
//! ```
//!
//! `url` is a path on the app's own origin — normally its `widget` service, the entry point an
//! app gets beside `frontend/` and `backend/` when it has a `widget/` directory (see the dashboard
//! scaffold's README). A widget whose `url` is not a path is left out: the app answers under a
//! different hostname for every viewer, so an address written into the file is right for at most
//! one of them.
//!
//! `size` is `small` (two tiles by two), `medium` (four by two) or `large` (four by four) — all in
//! the apps' grid, in the app's machine's section — or `half`, the right half of the screen beside
//! the grid, for the first widget that asks (this machine's before a paired one's). Anything else
//! is small. The page should paint no background and declare no `color-scheme`, so the widget's
//! frosted ground shows through it, and should go light when framed with `?light`.
//!
//! Read as a file from the app's directory, as its picture is — so a machine running an older
//! panel still offers its apps' widgets, and a stopped app's are known without starting it.
//!
//! An app widget runs its app: framing it is a request to the app's address, which starts an app
//! that is stopped. That is what a widget is for — it shows the app live — and it is why only an
//! app that declares one ever gets one.

use std::collections::BTreeMap;

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

/// How much of the home screen an app's widget takes: tiles of the grid, or the right half.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(super) enum Size {
    Small,
    Medium,
    Large,
    Half,
}

impl Size {
    /// Every size, smallest first, as the size menu lists them.
    pub(super) const ALL: [Self; 4] = [Self::Small, Self::Medium, Self::Large, Self::Half];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Small => "Small",
            Self::Medium => "Medium",
            Self::Large => "Large",
            Self::Half => "Half the screen",
        }
    }

    /// The size a `size = "…"` names; small when it names none, or one this build does not know.
    fn named(word: Option<&str>) -> Self {
        match word.map(str::trim) {
            Some("half") => Self::Half,
            Some("large") => Self::Large,
            Some("medium") => Self::Medium,
            _ => Self::Small,
        }
    }

    fn class(self) -> &'static str {
        match self {
            Self::Small => "adi-new-widget is-small",
            Self::Medium => "adi-new-widget is-medium",
            Self::Large => "adi-new-widget is-large",
            Self::Half => "adi-new-widget is-half",
        }
    }
}

/// One widget as an app's `config.toml` declares it, its path not yet put on an origin.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(super) struct Declared {
    /// Its `name`, when it gives one; the app's name stands in otherwise.
    pub(super) name: Option<String>,
    /// Its `url`: a path on the app's origin.
    pub(super) path: String,
    pub(super) size: Size,
}

/// The part of `config.toml` read here. The rest is the panel's (`adi_dashboards::Manifest`),
/// which the listings already carry.
#[derive(Deserialize)]
struct Config {
    #[serde(default)]
    widget: BTreeMap<String, Entry>,
}

#[derive(Deserialize)]
struct Entry {
    name: Option<String>,
    url: Option<String>,
    size: Option<String>,
}

/// The widgets `config` declares, in the order of their ids. None when it does not parse: an app
/// that broke its own config loses its widgets, not the home screen.
pub(super) fn declared(config: &str) -> Vec<Declared> {
    let Ok(config) = toml::from_str::<Config>(config) else {
        return Vec::new();
    };
    config
        .widget
        .into_values()
        .filter_map(|w| {
            let path = w.url?.trim().to_string();
            // `//host/…` is an address, not a path, however it starts.
            (path.starts_with('/') && !path.starts_with("//")).then(|| Declared {
                name: w.name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
                path,
                size: Size::named(w.size.as_deref()),
            })
        })
        .collect()
}

/// One app's widget: its page, framed on the app's own origin — as the app window frames the app,
/// so nothing here reaches into it and nothing in it reaches out.
///
/// Framed with `?light` while the home screen is light, so the widget can match it — which reloads
/// it when the appearance changes, as nothing else does.
#[component]
pub(super) fn AppWidget(
    name: String,
    url: String,
    size: Size,
    #[prop(into)] light: Signal<bool>,
) -> impl IntoView {
    let src = move || {
        if !light.get() {
            return url.clone();
        }
        let join = if url.contains('?') { '&' } else { '?' };
        format!("{url}{join}light")
    };
    view! {
        <div class=size.class() class:light=move || light.get()>
            <iframe
                class="adi-new-widget__view"
                src=src
                title=format!("{name} widget")
                allow="clipboard-read; clipboard-write"
            ></iframe>
        </div>
    }
}

/// Where a widget declared at `path` is served for app `href`: that path on the app's own origin.
pub(super) fn url(href: &str, path: &str) -> Option<String> {
    let (scheme, rest) = href.split_once("://")?;
    let host = rest.split('/').next().filter(|h| !h.is_empty())?;
    Some(format!("{scheme}://{host}{path}"))
}

#[cfg(test)]
mod tests {
    use super::{Size, declared, url};

    #[test]
    fn the_config_names_each_widget() {
        let config = r#"
name = "Agent board"
icon = "message-square"

[widget.chat]
name = "Chat"
url = "/widget/chat"
size = "half"

[widget.a-status]
url = "/widget/status"
size = "huge"
"#;
        let widgets = declared(config);
        assert_eq!(widgets.len(), 2);
        // In id order: `a-status` before `chat`.
        assert_eq!(widgets[0].name, None);
        assert_eq!(widgets[0].size, Size::Small);
        assert_eq!(widgets[1].name.as_deref(), Some("Chat"));
        assert_eq!(widgets[1].path, "/widget/chat");
        assert_eq!(widgets[1].size, Size::Half);
    }

    #[test]
    fn only_a_path_is_framed() {
        let config = r#"
[widget.away]
url = "http://elsewhere.adi/w"
[widget.sneaky]
url = "//elsewhere.adi/w"
[widget.none]
size = "large"
"#;
        assert!(declared(config).is_empty());
        assert!(declared("name = [broken").is_empty());
        assert!(declared("name = \"No widgets\"").is_empty());
    }

    #[test]
    fn a_path_lands_on_the_apps_origin() {
        assert_eq!(
            url("http://agent-board.adi/", "/widget/chat").as_deref(),
            Some("http://agent-board.adi/widget/chat")
        );
        assert_eq!(
            url("http://board.laptop.n.adi/some/page", "/widget/chat").as_deref(),
            Some("http://board.laptop.n.adi/widget/chat")
        );
    }
}
