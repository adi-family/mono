//! Widgets: live surfaces on the home screen, beside the apps' tiles.
//!
//! Two kinds:
//!
//! * the chat — ADI's own, and the one every home screen starts with: a conversation with this
//!   machine's agents across the screen's left half ([`ChatWidget`]);
//! * an app's — any app can expose one by shipping `frontend/widget.html`. Its app serves that
//!   file like any other, and the home screen frames it among the app tiles, in its machine's
//!   section ([`AppWidget`]). How big it is, the file says itself:
//!
//!   ```html
//!   <meta name="adi-widget-size" content="medium">
//!   ```
//!
//!   `small` is two tiles by two, `medium` four by two, `large` four by four; without the tag it
//!   is small. The page should paint no background and declare no `color-scheme`: the widget's
//!   frosted ground then shows through it, as it does behind the chat. Found the same way an app's picture is — read as a file from the app's directory,
//!   so a machine running an older panel still offers its apps' widgets.
//!
//! An app widget runs its app: framing it is a request to the app's address, which starts an app
//! that is stopped. That is what a widget is for — it shows the app live — and it is why only an
//! app that ships the file ever gets one.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use super::chat::Chat;

/// How much of the apps' grid an app's widget takes, in tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(super) enum Size {
    Small,
    Medium,
    Large,
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
        if tag.contains("\"large\"") || tag.contains("'large'") {
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
        }
    }
}

/// The chat, as the home screen's own widget.
#[component]
pub(super) fn ChatWidget(#[prop(into)] light: Signal<bool>) -> impl IntoView {
    view! {
        <section class="adi-new-widget is-chat" class:light=move || light.get() aria-label="Chat">
            <Chat widget=true/>
        </section>
    }
}

/// One app's widget: its `widget.html`, framed on the app's own origin — as the app window frames
/// the app, so nothing here reaches into it and nothing in it reaches out.
#[component]
pub(super) fn AppWidget(name: String, url: String, size: Size) -> impl IntoView {
    view! {
        <div class=size.class()>
            <iframe
                class="adi-new-widget__view"
                src=url
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
            Size::declared(r#"<meta name="adi-widget-size" content="huge">"#),
            Size::Small
        );
    }
}
