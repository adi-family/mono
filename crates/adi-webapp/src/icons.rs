//! The icon set: small stroked glyphs drawn on a 16×16 grid.
//!
//! Each icon is the *inner* markup of an `<svg>`; the element around it supplies the
//! viewBox, `currentColor` stroke, and joins, so an icon inherits the colour and weight of
//! whatever row it sits in. Kept as hand-written paths rather than a font or sprite sheet so
//! the UI stays a single self-contained wasm bundle with no external requests.

use crate::routing::{ProjectSection, Route};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    Globe,
    Gear,
    Folder,
    List,
    Tasks,
    Agent,
    Trigger,
    Dashboard,
    Server,
    Plug,
    Mesh,
    Box,
    Layers,
    File,
    Doc,
    Spark,
    Wrench,
    Key,
}

impl Icon {
    /// The glyph's paths. Coordinates assume a 16×16 viewBox and a 1.5 stroke.
    pub(crate) fn path(self) -> &'static str {
        match self {
            Icon::Globe => {
                r#"<circle cx="8" cy="8" r="6.25"/><path d="M1.75 8h12.5"/>
                   <path d="M8 1.75c1.6 1.7 2.5 3.9 2.5 6.25S9.6 12.55 8 14.25c-1.6-1.7-2.5-3.9-2.5-6.25S6.4 3.45 8 1.75z"/>"#
            }
            Icon::Gear => {
                r#"<circle cx="8" cy="8" r="2.25"/>
                   <path d="M8 1.5v1.75M8 12.75v1.75M1.5 8h1.75M12.75 8h1.75M3.4 3.4l1.25 1.25M11.35 11.35l1.25 1.25M12.6 3.4l-1.25 1.25M4.65 11.35L3.4 12.6"/>"#
            }
            Icon::Folder => {
                r#"<path d="M1.75 4.5a1 1 0 0 1 1-1h3.1l1.5 1.5h6.9a1 1 0 0 1 1 1v6.5a1 1 0 0 1-1 1H2.75a1 1 0 0 1-1-1z"/>"#
            }
            Icon::List => {
                r#"<path d="M5.75 4h8M5.75 8h8M5.75 12h8"/><path d="M2.75 4h.01M2.75 8h.01M2.75 12h.01"/>"#
            }
            // A ticked box rather than a two-row checklist: at 13px the extra strokes of a
            // checklist collapse into noise.
            Icon::Tasks => {
                r#"<rect x="2.5" y="2.5" width="11" height="11" rx="2"/>
                   <path d="M5.5 8.25l1.75 1.75 3.25-3.5"/>"#
            }
            // A chip, not a robot head — a small rounded box with an antenna reads as a
            // padlock at this size.
            Icon::Agent => {
                r#"<rect x="4.5" y="4.5" width="7" height="7" rx="1.5"/>
                   <path d="M6.5 1.75v2.75M9.5 1.75v2.75M6.5 11.5v2.75M9.5 11.5v2.75"/>
                   <path d="M1.75 6.5h2.75M1.75 9.5h2.75M11.5 6.5h2.75M11.5 9.5h2.75"/>"#
            }
            Icon::Trigger => r#"<path d="M8.75 1.75L3.75 9h3.5l-.5 5.25L12.25 7h-3.5z"/>"#,
            Icon::Dashboard => {
                r#"<rect x="2.25" y="2.25" width="5" height="5" rx="1"/>
                   <rect x="8.75" y="2.25" width="5" height="5" rx="1"/>
                   <rect x="2.25" y="8.75" width="5" height="5" rx="1"/>
                   <rect x="8.75" y="8.75" width="5" height="5" rx="1"/>"#
            }
            Icon::Server => {
                r#"<rect x="2.25" y="2.75" width="11.5" height="4.5" rx="1"/>
                   <rect x="2.25" y="8.75" width="11.5" height="4.5" rx="1"/>
                   <path d="M4.75 5h.01M4.75 11h.01"/>"#
            }
            Icon::Plug => {
                r#"<path d="M6 1.75v3.5M10 1.75v3.5"/>
                   <path d="M3.75 5.25h8.5V8a4.25 4.25 0 0 1-8.5 0z"/><path d="M8 12.25v2"/>"#
            }
            Icon::Mesh => {
                r#"<circle cx="8" cy="3.25" r="1.75"/><circle cx="3.25" cy="12.25" r="1.75"/>
                   <circle cx="12.75" cy="12.25" r="1.75"/>
                   <path d="M6.9 4.8L4.35 10.7M9.1 4.8l2.55 5.9M5 12.25h6"/>"#
            }
            Icon::Box => {
                r#"<path d="M8 1.75l5.75 3.1v6.3L8 14.25l-5.75-3.1v-6.3z"/>
                   <path d="M2.25 4.85L8 7.95l5.75-3.1M8 7.95v6.3"/>"#
            }
            Icon::Layers => {
                r#"<path d="M8 1.75l6.25 3.1L8 7.95 1.75 4.85z"/>
                   <path d="M1.75 8.4L8 11.5l6.25-3.1M1.75 11.6L8 14.7l6.25-3.1"/>"#
            }
            Icon::File => {
                r#"<path d="M9.25 1.75H4.5a1 1 0 0 0-1 1v10.5a1 1 0 0 0 1 1h7a1 1 0 0 0 1-1V5z"/>
                   <path d="M9.25 1.75V5h3.25"/>"#
            }
            Icon::Doc => {
                r#"<rect x="2.75" y="2.25" width="10.5" height="11.5" rx="1"/>
                   <path d="M5.5 5.75h5M5.5 8.25h5M5.5 10.75h3"/>"#
            }
            // Two four-point sparkles — the "assistant / meta-agent" mark. Concave points keep it
            // reading as a spark rather than a plus at 13px.
            Icon::Spark => {
                r#"<path d="M6.5 1.75c.35 2.55 1.2 3.4 3.75 3.75-2.55.35-3.4 1.2-3.75 3.75-.35-2.55-1.2-3.4-3.75-3.75 2.55-.35 3.4-1.2 3.75-3.75z"/>
                   <path d="M11.75 9.25c.2 1.5.7 2 2.2 2.2-1.5.2-2 .7-2.2 2.2-.2-1.5-.7-2-2.2-2.2 1.5-.2 2-.7 2.2-2.2z"/>"#
            }
            // A wrench — the "tool" mark: an open-end head over a diagonal handle.
            Icon::Wrench => {
                r#"<path d="M11.4 2.4a3 3 0 0 0-3.85 3.75l-5 5a1.35 1.35 0 0 0 1.9 1.9l5-5A3 3 0 0 0 13.1 4.1l-1.85 1.85-1.5-.35-.35-1.5z"/>"#
            }
            // A key — the "secret" mark: a ringed bow with a notched bit on a diagonal shaft.
            Icon::Key => {
                r#"<circle cx="5.25" cy="10.75" r="3"/><path d="M7.4 8.6l5.35-5.35"/>
                   <path d="M10.5 5.5l1.5 1.5M12.75 3.25l1.5 1.5"/>"#
            }
        }
    }
}

/// The icon for a global page.
pub(crate) fn route_icon(route: Route) -> Icon {
    match route {
        Route::Meta => Icon::Spark,
        Route::Projects | Route::ProjectDetail => Icon::List,
        Route::Tasks => Icon::Tasks,
        Route::Agents => Icon::Agent,
        Route::Tools => Icon::Wrench,
        Route::Secrets => Icon::Key,
        Route::Triggers => Icon::Trigger,
        Route::Dashboards => Icon::Dashboard,
        Route::Hive => Icon::Server,
        Route::PortsManager => Icon::Plug,
        Route::Mesh => Icon::Mesh,
        // Reached from the Store rail rather than the explorer, so this icon is a fallback.
        Route::StoreFile => Icon::Doc,
    }
}

/// The icon for one of a project's sections.
pub(crate) fn section_icon(section: ProjectSection) -> Icon {
    match section {
        ProjectSection::Overview => Icon::Doc,
        ProjectSection::Tasks => Icon::Tasks,
        ProjectSection::Agents => Icon::Agent,
        ProjectSection::Triggers => Icon::Trigger,
        ProjectSection::Tools => Icon::Wrench,
        ProjectSection::Secrets => Icon::Key,
        ProjectSection::Services => Icon::Box,
        ProjectSection::Workspaces => Icon::Layers,
        ProjectSection::Files => Icon::File,
    }
}
