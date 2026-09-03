//! The control panel's icons: the nouns the app has, each mapped to the one Lucide glyph
//! `design/DESIGN.md` §9 gives it.
//!
//! The drawing is [`adi_ui::Icon`]'s — stroke 1.5, one of four sizes, `currentColor`. What is
//! here is only the *choice*: which glyph a route, a project section or an action gets, made
//! once so two screens never pick differently for the same noun.

use adi_ui::Lucide;

use crate::routing::{ProjectSection, Route};

/// A noun in the app. `lucide()` is the glyph for it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    /// The global scope in the explorer.
    Globe,
    /// Settings.
    Gear,
    Folder,
    /// Projects, as a list.
    List,
    Tasks,
    Agent,
    Trigger,
    Dashboard,
    /// Services.
    Server,
    /// The ports manager.
    Plug,
    /// The mesh.
    Mesh,
    /// A fleet node — a machine, as against `Mesh`'s links between them.
    Node,
    /// The marketplace.
    Box,
    /// Workspaces.
    Layers,
    File,
    /// A document — a project's overview, a store file.
    Doc,
    /// The meta agent.
    Spark,
    Wrench,
    /// Secrets.
    Key,
    Database,
    /// Knowledge.
    Book,
    /// The facts base, read as pairs.
    Pair,
    /// Install this as an app.
    Download,
    /// Move this machine up to the published version.
    Upgrade,
    /// Analytics.
    Chart,
    /// Narrow a list to part of itself.
    Filter,
    /// Per-run settings — dials somebody has set, as against `Gear`'s administration.
    Sliders,
}

impl Icon {
    /// The Lucide glyph for this noun (DESIGN.md §9).
    pub(crate) const fn lucide(self) -> Lucide {
        match self {
            Icon::Globe => Lucide::Globe,
            Icon::Gear => Lucide::Settings2,
            Icon::Folder => Lucide::Folder,
            Icon::List => Lucide::List,
            Icon::Tasks => Lucide::ListTree,
            Icon::Agent => Lucide::Bot,
            Icon::Trigger => Lucide::Zap,
            Icon::Dashboard => Lucide::LayoutDashboard,
            Icon::Server => Lucide::Server,
            Icon::Plug => Lucide::Plug,
            Icon::Mesh => Lucide::Network,
            Icon::Node => Lucide::Monitor,
            Icon::Box => Lucide::Store,
            Icon::Layers => Lucide::Layers,
            Icon::File => Lucide::File,
            Icon::Doc => Lucide::FileText,
            Icon::Spark => Lucide::Sparkles,
            Icon::Wrench => Lucide::Wrench,
            Icon::Key => Lucide::KeyRound,
            Icon::Database => Lucide::Database,
            Icon::Book => Lucide::BookOpen,
            Icon::Pair => Lucide::GitCompare,
            Icon::Download => Lucide::Download,
            Icon::Upgrade => Lucide::ArrowUp,
            Icon::Chart => Lucide::ChartColumn,
            Icon::Filter => Lucide::ListFilter,
            Icon::Sliders => Lucide::SlidersHorizontal,
        }
    }
}

/// The icon for a global page.
pub(crate) fn route_icon(route: Route) -> Icon {
    match route {
        Route::Meta => Icon::Spark,
        Route::Analytics => Icon::Chart,
        Route::Projects | Route::ProjectDetail => Icon::List,
        Route::Tasks => Icon::Tasks,
        Route::Agents | Route::AgentDetail => Icon::Agent,
        Route::Tools => Icon::Wrench,
        Route::Secrets => Icon::Key,
        Route::Knowledge => Icon::Book,
        Route::Facts => Icon::Pair,
        Route::Database => Icon::Database,
        Route::Triggers => Icon::Trigger,
        Route::Dashboards => Icon::Dashboard,
        Route::Marketplace => Icon::Box,
        Route::Hive => Icon::Server,
        Route::PortsManager => Icon::Plug,
        Route::Mesh => Icon::Mesh,
        Route::Fleet => Icon::Node,
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
        ProjectSection::Knowledge => Icon::Book,
        ProjectSection::Services => Icon::Server,
        ProjectSection::Workspaces => Icon::Layers,
        ProjectSection::Files => Icon::File,
    }
}
