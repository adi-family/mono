//! Widgets: live surfaces on the home screen, each one an app's.
//!
//! The home screen draws none of its own. What it shows is whatever the apps offer — the chat
//! included, which is the Agent board app's widget — so the default screen is apps too, and
//! changes the way any app does.
//!
//! An app offers a widget by shipping `frontend/widget.html`. Its app serves that file like any
//! other, and the home screen frames it ([`AppWidget`]). How big it is, the file says itself:
//!
//! ```html
//! <meta name="adi-widget-size" content="medium">
//! ```
//!
//! `small` is two tiles by two, `medium` four by two, `large` four by four — all in the apps'
//! grid, in the app's machine's section — and `half` is the right half of the screen, beside the
//! grid, for the first app that asks (this machine's before a paired one's). Without the tag it is
//! small. The page should paint no background and declare no `color-scheme`, so the widget's
//! frosted ground shows through it, and should go light when framed with `?light`.
//!
//! Found the same way an app's picture is — read as a file from the app's directory — so a machine
//! running an older panel still offers its apps' widgets.
//!
//! An app widget runs its app: framing it is a request to the app's address, which starts an app
//! that is stopped. That is what a widget is for — it shows the app live — and it is why only an
//! app that ships the file ever gets one.

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
    /// The size `widget.html` asks for in its `adi-widget-size` meta tag; small when it names
    /// none, or one this build does not know.
    pub(super) fn declared(html: &str) -> Self {
        let Some(at) = html.find("adi-widget-size") else {
            return Self::Small;
        };
        // The whole tag the name sits in, and nothing past it — attributes come in any order.
        let start = html[..at].rfind('<').unwrap_or(0);
        let end = html[at..].find('>').map_or(html.len(), |end| at + end);
        let tag = &html[start..end];
        if tag.contains("\"half\"") || tag.contains("'half'") {
            Self::Half
        } else if tag.contains("\"large\"") || tag.contains("'large'") {
            Self::Large
        } else if tag.contains("\"medium\"") || tag.contains("'medium'") {
            Self::Medium
        } else {
            Self::Small
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

/// One app's widget: its `widget.html`, framed on the app's own origin — as the app window frames
/// the app, so nothing here reaches into it and nothing in it reaches out.
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
    view! {
        <div class=size.class() class:light=move || light.get()>
            <iframe
                class="adi-new-widget__view"
                src=move || if light.get() { format!("{url}?light") } else { url.clone() }
                title=format!("{name} widget")
                allow="clipboard-read; clipboard-write"
            ></iframe>
        </div>
    }
}

/// Where app `href`'s widget is served: `/widget.html` on the app's own origin.
pub(super) fn url(href: &str) -> Option<String> {
    let (scheme, rest) = href.split_once("://")?;
    let host = rest.split('/').next().filter(|h| !h.is_empty())?;
    Some(format!("{scheme}://{host}/widget.html"))
}

#[cfg(test)]
mod tests {
    use super::Size;

    #[test]
    fn the_file_names_its_size() {
        assert_eq!(Size::declared("<html><body>hi"), Size::Small);
        assert_eq!(
            Size::declared(r#"<meta name="adi-widget-size" content="medium">"#),
            Size::Medium
        );
        assert_eq!(
            Size::declared("<meta content='large' name='adi-widget-size'/>"),
            Size::Large
        );
        assert_eq!(
            Size::declared(r#"<meta name="adi-widget-size" content="large"><p>"medium"</p>"#),
            Size::Large
        );
        assert_eq!(
            Size::declared(r#"<meta name="adi-widget-size" content="half">"#),
            Size::Half
        );
        assert_eq!(
            Size::declared(r#"<meta name="adi-widget-size" content="huge">"#),
            Size::Small
        );
    }
}
