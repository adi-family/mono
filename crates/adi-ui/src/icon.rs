//! [`Icon`] — one Lucide glyph, drawn the one way `design/DESIGN.md` §9 allows.
//!
//! Lucide is the only icon library in the product: app, landing, docs and menubar. The set is
//! `icons/*.svg`, fetched by `scripts/lucide.sh`, and the [`Lucide`] enum is generated from
//! that directory, so an icon that is not in the set is a compile error rather than a blank.
//!
//! The rendering rules live here and nowhere else: stroke **1.5** (Lucide's default 2 is too
//! heavy against Geist 400), four sizes and no others, `currentColor` so the glyph takes the
//! ink of the text beside it. An icon in the app is always next to a label; the handful that
//! stand alone (send, attach, dictate, filter, close, the ⋯ menu) carry a `label` instead.

use leptos::prelude::*;

use crate::merge;

include!(concat!(env!("OUT_DIR"), "/lucide.rs"));

/// The four sizes an icon may be (§9). Nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IconSize {
    /// 14px — inside list-item meta and tags.
    Sm,
    /// 16px — tree and explorer rows, table cells, buttons, panel headers. The default.
    #[default]
    Md,
    /// 20px — landing feature blocks.
    Lg,
    /// 24px — empty states only.
    Xl,
}

impl IconSize {
    /// The box, in CSS pixels.
    #[must_use]
    pub const fn px(self) -> u32 {
        match self {
            Self::Sm => 14,
            Self::Md => 16,
            Self::Lg => 20,
            Self::Xl => 24,
        }
    }

    /// The utilities that size it. Whole literals, so Tailwind finds them.
    #[must_use]
    pub const fn classes(self) -> &'static str {
        match self {
            Self::Sm => "size-3.5 shrink-0",
            Self::Md => "size-4 shrink-0",
            Self::Lg => "size-5 shrink-0",
            Self::Xl => "size-6 shrink-0",
        }
    }
}

/// The stroke every icon is drawn at (§9).
pub const STROKE: &str = "1.5";

/// One icon.
///
/// Hidden from assistive technology unless given a `label`, because an icon in this product is
/// beside the word it stands for and a screen reader announcing both is worse than one.
///
/// ```ignore
/// <Icon icon=Lucide::Bot/>                                   // beside a label
/// <Icon icon=Lucide::Ellipsis label="More actions"/>          // standing alone
/// <Icon icon=Lucide::Folder size=IconSize::Sm class="text-ink-3"/>
/// ```
#[component]
pub fn Icon(
    icon: Lucide,
    #[prop(optional)] size: IconSize,
    /// Extra utilities: a colour, a margin, a rotation.
    #[prop(optional, into)]
    class: String,
    /// What the icon means, for an icon with no text beside it. Leave it off when there is a
    /// label — the label already says it.
    #[prop(optional, into)]
    label: String,
) -> impl IntoView {
    let standalone = !label.is_empty();
    view! {
        <svg
            class=merge(size.classes(), class)
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width=STROKE
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden=(!standalone).then_some("true")
            role=standalone.then_some("img")
            aria-label=standalone.then_some(label)
            inner_html=icon.markup()
        ></svg>
    }
}

/// The same icon as a string of markup, for pages rendered outside Leptos — the front door's
/// error pages, the app's placeholder, a generated dashboard shell.
#[must_use]
pub fn svg(icon: Lucide, size: IconSize, class: &str) -> String {
    let px = size.px();
    format!(
        r#"<svg class="{class}" width="{px}" height="{px}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="{STROKE}" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{}</svg>"#,
        icon.markup()
    )
}

#[cfg(test)]
mod tests {
    use super::{IconSize, Lucide, svg};

    /// The nouns DESIGN.md §9 maps must all be in the set — a screen that reaches for one of
    /// them should never have to draw its own.
    #[test]
    fn the_noun_map_is_covered() {
        for name in [
            "message-square",
            "bot",
            "folder",
            "wrench",
            "book-open",
            "code",
            "list-tree",
            "zap",
            "server",
            "layout-dashboard",
            "key-round",
            "database",
            "network",
            "brain",
            "settings-2",
            "arrow-up",
            "paperclip",
            "mic",
            "ellipsis",
            "x",
            "arrow-up-right",
        ] {
            assert!(
                Lucide::from_name(name).is_some(),
                "{name} is missing from icons/ICONS"
            );
        }
    }

    #[test]
    fn names_round_trip() {
        for icon in Lucide::ALL {
            assert_eq!(Lucide::from_name(icon.name()), Some(*icon));
            assert!(!icon.markup().is_empty(), "{} has no paths", icon.name());
            assert!(
                !icon.markup().contains("<svg"),
                "{} kept its wrapper",
                icon.name()
            );
        }
    }

    #[test]
    fn standalone_markup_is_a_whole_svg() {
        let s = svg(Lucide::Bot, IconSize::Md, "ic");
        assert!(s.starts_with("<svg class=\"ic\" width=\"16\""));
        assert!(s.contains("stroke-width=\"1.5\""));
        assert!(s.ends_with("</svg>"));
    }
}
