//! A reusable IDE-style tree view.
//!
//! Callers hand it a flat, depth-annotated list — the shape `project_tree_rows` and
//! `task_tree_rows` already produce — and it owns the presentation: indent rails, the
//! expand/collapse twisty, selection, and keyboard activation. It deliberately knows
//! nothing about projects or tasks, so the same component serves the project tree, the
//! task tree, a project's sub-projects, and the file browser.
//!
//! Collapsing is derived from depth alone: a collapsed row hides every following row
//! deeper than it, which is exactly the subtree. That means no parent pointers have to be
//! threaded through, and any list already in tree order just works.

use std::collections::HashSet;

use leptos::prelude::*;

/// One row of a tree. `depth` is the nesting level (0 = root); `has_children` decides
/// whether the row gets a twisty and the branch glyph.
#[derive(Clone)]
pub(crate) struct TreeNode {
    pub(crate) id: String,
    pub(crate) depth: usize,
    pub(crate) label: String,
    pub(crate) has_children: bool,
    /// A short trailing count or status, e.g. `"3 open"`.
    pub(crate) badge: Option<String>,
    /// Native tooltip for the row.
    pub(crate) title: Option<String>,
    /// A grouping row that is not itself a destination (a scope header). Clicking it opens
    /// or closes it, the way clicking a folder name does, rather than selecting nothing.
    pub(crate) container: bool,
    /// The row's glyph — the inner markup of an `<svg>` (see the `icons` module). Rows
    /// without one fall back to a dot, so the icon column always aligns.
    pub(crate) icon: Option<&'static str>,
    /// Draw a rule above this row. Marks the boundary between kinds of children — a
    /// project's section pages and the sub-projects nested under it.
    pub(crate) separated: bool,
    /// Give the row more presence than its siblings (a project among its own pages).
    pub(crate) emphasis: bool,
}

impl TreeNode {
    /// A plain leaf at `depth`; the builder methods below add the optional parts.
    pub(crate) fn new(id: impl Into<String>, depth: usize, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            depth,
            label: label.into(),
            has_children: false,
            badge: None,
            title: None,
            container: false,
            icon: None,
            separated: false,
            emphasis: false,
        }
    }

    pub(crate) fn children(mut self, has_children: bool) -> Self {
        self.has_children = has_children;
        self
    }

    pub(crate) fn badge(mut self, badge: Option<String>) -> Self {
        self.badge = badge;
        self
    }

    pub(crate) fn title(mut self, title: Option<String>) -> Self {
        self.title = title;
        self
    }

    /// Mark this row as a pure grouping row (see [`TreeNode::container`]).
    pub(crate) fn container(mut self, container: bool) -> Self {
        self.container = container;
        self
    }

    /// The row's glyph, as the inner markup of an `<svg>` on a 16x16 grid.
    pub(crate) fn icon(mut self, icon: &'static str) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Draw a rule above this row (see [`TreeNode::separated`]).
    pub(crate) fn separated(mut self, separated: bool) -> Self {
        self.separated = separated;
        self
    }

    /// Give this row more presence than its siblings (see [`TreeNode::emphasis`]).
    pub(crate) fn emphasis(mut self, emphasis: bool) -> Self {
        self.emphasis = emphasis;
        self
    }
}

/// The interaction state of one tree: the row the user last activated, and which branches
/// the user has opened. `Copy`, so it threads into the page view and its event handlers like
/// the page state does.
///
/// Branches are closed by default — a tree that lists every section of every project is
/// unreadable fully expanded — and [`tree_view`] additionally opens the ancestors of the
/// selected row, so whatever is currently open is always revealed without the user hunting
/// for it.
///
/// Note that `selected` is what was *clicked*, not what is highlighted — the caller passes
/// the highlighted id into [`tree_view`] separately. Keeping the two apart lets a tree drive
/// navigation (click opens a project) while the highlight follows the app's actual state,
/// instead of the tree and the address bar each believing something different.
#[derive(Clone, Copy)]
pub(crate) struct TreeState {
    pub(crate) selected: RwSignal<Option<String>>,
    pub(crate) expanded: RwSignal<HashSet<String>>,
}

impl TreeState {
    /// Everything closed, nothing selected.
    pub(crate) fn new() -> Self {
        Self {
            selected: RwSignal::new(None),
            expanded: RwSignal::new(HashSet::new()),
        }
    }

    /// Open a closed branch, or close an open one.
    pub(crate) fn toggle(&self, id: &str) {
        self.expanded.update(|set| {
            if !set.remove(id) {
                set.insert(id.to_string());
            }
        });
    }
}

/// Render a tree from rows already in depth-annotated tree order. Rows inside a collapsed
/// branch are dropped here rather than by the caller, so the caller always passes the whole
/// tree and never has to track what is currently visible.
pub(crate) fn tree_view(
    nodes: Vec<TreeNode>,
    tree: TreeState,
    selected: Option<String>,
    empty: &str,
) -> AnyView {
    if nodes.is_empty() {
        return view! { <div class="adi-empty">{empty.to_string()}</div> }.into_any();
    }

    let expanded = tree.expanded.get();
    let revealed = ancestors_of(&nodes, selected.as_deref());

    // `hidden_below` holds the depth of the closed branch we are currently inside; every row
    // deeper than it belongs to that subtree and is skipped.
    let mut hidden_below: Option<usize> = None;
    let mut rows = Vec::new();
    for node in nodes {
        if let Some(depth) = hidden_below {
            if node.depth > depth {
                continue;
            }
            hidden_below = None;
        }
        let open = expanded.contains(&node.id) || revealed.contains(&node.id);
        if node.has_children && !open {
            hidden_below = Some(node.depth);
        }
        rows.push(row_view(node, open, selected.as_deref(), tree));
    }

    view! { <div class="adi-tree" role="tree">{rows}</div> }.into_any()
}

/// The ids of every branch enclosing `selected` — the path that has to be open for the
/// selection to be on screen at all. Depth ordering is enough to find them: walking the rows
/// while keeping the last id seen at each shallower depth yields the ancestor chain.
fn ancestors_of(nodes: &[TreeNode], selected: Option<&str>) -> HashSet<String> {
    let mut chain: Vec<&str> = Vec::new();
    for node in nodes {
        chain.truncate(node.depth);
        if Some(node.id.as_str()) == selected {
            return chain.into_iter().map(str::to_string).collect();
        }
        chain.push(node.id.as_str());
    }
    HashSet::new()
}

/// One tree row: `depth` indent rails, a twisty (branches only), the kind glyph, the label,
/// and an optional trailing badge.
fn row_view(node: TreeNode, expanded: bool, selected: Option<&str>, tree: TreeState) -> AnyView {
    let is_selected = selected == Some(node.id.as_str());
    let rails = (0..node.depth)
        .map(|_| view! { <span class="adi-tree__rail"></span> })
        .collect::<Vec<_>>();

    let twisty = if node.has_children {
        let id = node.id.clone();
        view! {
            <button class="adi-tree__twisty" type="button" tabindex="-1"
                aria-label=if expanded { "Collapse" } else { "Expand" }
                on:click=move |ev: web_sys::MouseEvent| {
                    // The twisty only opens and closes — it must not also move the selection.
                    ev.stop_propagation();
                    tree.toggle(&id);
                }>
                {if expanded { "▾" } else { "▸" }}
            </button>
        }
        .into_any()
    } else {
        view! { <span class="adi-tree__twisty"></span> }.into_any()
    };

    // An icon when the row has one, else a dot — either way the column keeps its width, so
    // labels stay on a single left edge down the whole tree.
    let icon = match node.icon {
        Some(path) => view! {
            <svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor"
                stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"
                inner_html=path></svg>
        }
        .into_any(),
        None => view! { <span class="adi-tree__dot">"\u{b7}"</span> }.into_any(),
    };
    let container = node.container;
    let click_id = node.id.clone();
    let key_id = node.id.clone();
    let activate = move |id: String, tree: TreeState| {
        if container {
            tree.toggle(&id);
        } else {
            tree.selected.set(Some(id));
        }
    };
    view! {
        <div class="adi-tree__row" role="treeitem" tabindex="0"
            title=node.title
            data-selected=is_selected.to_string()
            data-separated=node.separated.to_string()
            data-emphasis=node.emphasis.to_string()
            aria-selected=is_selected.to_string()
            aria-expanded=node.has_children.then(|| expanded.to_string())
            on:click=move |_| activate(click_id.clone(), tree)
            on:keydown=move |ev: web_sys::KeyboardEvent| {
                if ev.key() == "Enter" || ev.key() == " " {
                    ev.prevent_default();
                    activate(key_id.clone(), tree);
                }
            }>
            {rails}
            {twisty}
            <span class="adi-tree__icon" aria-hidden="true">{icon}</span>
            <span class="adi-tree__label">{node.label}</span>
            {node.badge.map(|b| view! { <span class="adi-tree__badge">{b}</span> })}
        </div>
    }
    .into_any()
}
