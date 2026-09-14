//! The Lucide icons these pages draw, taken from the one set the product pins.
//!
//! The SVGs are `crates/adi-ui/icons/*.svg` verbatim — the files `scripts/lucide.sh add` fetches
//! and `adi-ui`'s build script turns into `adi_ui::Lucide`. This crate cannot use that enum (it is
//! a Leptos component, and these pages are strings), so it includes the same files at compile
//! time and normalizes them: the licence comment, the `class`, and the `width`/`height`/
//! `stroke-width` attributes come off, and CSS sizes and strokes them from the tokens instead
//! (`.i`, `.i--sm`, `.i--lg` in assets/site.css, at the 14/16/20 of DESIGN.md §9).
//!
//! Adding one is `scripts/lucide.sh add <name>` and a line in [`Icon`] — never an inline path,
//! and never a glyph.

/// Every icon these pages draw. The noun → icon mapping is DESIGN.md §9's table; the eight kinds
/// follow the control panel's `kind_icon`, so a bundle's contents read the same in both places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// An agent.
    Bot,
    /// A tool.
    Wrench,
    /// A dashboard.
    LayoutDashboard,
    /// An LLM backend.
    Brain,
    /// An embedding backend.
    ScanLine,
    /// A hive service.
    Server,
    /// A trigger.
    Zap,
    /// A project scaffold.
    Folder,
    /// A bundle, and the tile an entry with no published icon gets.
    Package,
    /// "Cloned to this machine".
    Laptop,
    /// "Installed is not started".
    Power,
    /// "Pinned to one commit".
    GitCommitHorizontal,
    /// Back to the shelf.
    ArrowLeft,
    /// After the label of a link that leaves the site.
    ArrowUpRight,
}

impl Icon {
    /// The source file, as `adi-ui` keeps it.
    fn source(self) -> &'static str {
        match self {
            Icon::Bot => include_str!("../../adi-ui/icons/bot.svg"),
            Icon::Wrench => include_str!("../../adi-ui/icons/wrench.svg"),
            Icon::LayoutDashboard => include_str!("../../adi-ui/icons/layout-dashboard.svg"),
            Icon::Brain => include_str!("../../adi-ui/icons/brain.svg"),
            Icon::ScanLine => include_str!("../../adi-ui/icons/scan-line.svg"),
            Icon::Server => include_str!("../../adi-ui/icons/server.svg"),
            Icon::Zap => include_str!("../../adi-ui/icons/zap.svg"),
            Icon::Folder => include_str!("../../adi-ui/icons/folder.svg"),
            Icon::Package => include_str!("../../adi-ui/icons/package.svg"),
            Icon::Laptop => include_str!("../../adi-ui/icons/laptop.svg"),
            Icon::Power => include_str!("../../adi-ui/icons/power.svg"),
            Icon::GitCommitHorizontal => {
                include_str!("../../adi-ui/icons/git-commit-horizontal.svg")
            }
            Icon::ArrowLeft => include_str!("../../adi-ui/icons/arrow-left.svg"),
            Icon::ArrowUpRight => include_str!("../../adi-ui/icons/arrow-up-right.svg"),
        }
    }

    /// The icon as markup, at the size `class` asks for (`""`, `"i--sm"`, `"i--lg"`).
    ///
    /// Always `aria-hidden`: DESIGN.md §9 has every icon in the app paired with a text label, and
    /// every icon on these pages is, so a screen reader that also announced the glyph would read
    /// each one twice.
    #[must_use]
    pub fn svg(self, class: &str) -> String {
        let class = format!("i {class}");
        let class = class.trim_end();
        rewrite(self.source(), class)
    }
}

/// Strip what the file declares about its own size and stroke, and give it the class the
/// stylesheet draws it by.
///
/// Attribute-level surgery rather than a parser: these are fourteen files from one generator with
/// one attribute order, pinned to a `lucide-static` release, and `the_set_normalizes` holds every
/// one of them to the result.
fn rewrite(svg: &str, class: &str) -> String {
    let body = svg.rsplit_once("<svg").map_or(svg, |(_, rest)| rest);
    let mut out = String::with_capacity(body.len() + 64);
    out.push_str("<svg class=\"");
    out.push_str(class);
    out.push_str("\" aria-hidden=\"true\"");
    for attribute in [
        " xmlns=\"http://www.w3.org/2000/svg\"",
        " viewBox=\"0 0 24 24\"",
        " fill=\"none\"",
        " stroke=\"currentColor\"",
        " stroke-linecap=\"round\"",
        " stroke-linejoin=\"round\"",
    ] {
        out.push_str(attribute);
    }
    // Everything after the opening tag is the drawing itself, which is copied through untouched.
    let drawing = body.split_once('>').map_or("", |(_, rest)| rest);
    out.push('>');
    out.push_str(drawing.trim());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Icon; 14] = [
        Icon::Bot,
        Icon::Wrench,
        Icon::LayoutDashboard,
        Icon::Brain,
        Icon::ScanLine,
        Icon::Server,
        Icon::Zap,
        Icon::Folder,
        Icon::Package,
        Icon::Laptop,
        Icon::Power,
        Icon::GitCommitHorizontal,
        Icon::ArrowLeft,
        Icon::ArrowUpRight,
    ];

    /// Every file in the set survives [`rewrite`] as one element with a drawing in it and nothing
    /// of its own about size — which is the only thing a page then cannot get wrong about it.
    #[test]
    fn the_set_normalizes() {
        for icon in ALL {
            let svg = icon.svg("i--sm");
            assert!(svg.starts_with("<svg class=\"i i--sm\""), "{svg}");
            assert!(svg.ends_with("</svg>"), "{svg}");
            assert!(!svg.contains("width=\"24\""), "size comes from the stylesheet: {svg}");
            assert!(!svg.contains("stroke-width"), "stroke comes from the tokens: {svg}");
            assert!(svg.contains("stroke=\"currentColor\""), "colour follows the text: {svg}");
            assert!(
                svg.contains("<path") || svg.contains("<rect") || svg.contains("<circle"),
                "there is a drawing in it: {svg}"
            );
            assert_eq!(svg.matches("<svg").count(), 1, "{svg}");
        }
    }

    #[test]
    fn the_default_size_carries_no_modifier() {
        assert!(Icon::Package.svg("").starts_with("<svg class=\"i\""));
    }
}
