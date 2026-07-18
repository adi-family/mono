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
}

/// The interaction state of one tree: the row the user last activated, and which branches
/// are closed. `Copy`, so it threads into the page view and its event handlers like the page
/// state does.
///
/// Note that `selected` is what was *clicked*, not what is highlighted — the caller passes
/// the highlighted id into [`tree_view`] separately. Keeping the two apart lets a tree drive
/// navigation (click opens a project) while the highlight follows the app's actual state,
/// instead of the tree and the address bar each believing something different.
#[derive(Clone, Copy)]
pub(crate) struct TreeState {
    pub(crate) selected: RwSignal<Option<String>>,
    pub(crate) collapsed: RwSignal<HashSet<String>>,
}

impl TreeState {
    /// Everything expanded, nothing selected.
    pub(crate) fn new() -> Self {
        Self {
            selected: RwSignal::new(None),
            collapsed: RwSignal::new(HashSet::new()),
        }
    }

    /// Open a closed branch, or close an open one.
    pub(crate) fn toggle(&self, id: &str) {
        self.collapsed.update(|set| {
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

    let collapsed = tree.collapsed.get();

    // `hidden_below` holds the depth of the collapsed branch we are currently inside; every
    // row deeper than it belongs to that subtree and is skipped.
    let mut hidden_below: Option<usize> = None;
    let mut rows = Vec::new();
    for node in nodes {
        if let Some(depth) = hidden_below {
            if node.depth > depth {
                continue;
            }
            hidden_below = None;
        }
        let expanded = !collapsed.contains(&node.id);
        if node.has_children && !expanded {
            hidden_below = Some(node.depth);
        }
        rows.push(row_view(node, expanded, selected.as_deref(), tree));
    }

    view! { <div class="adi-tree" role="tree">{rows}</div> }.into_any()
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

    // Branches are already marked by their twisty, so only leaves carry a glyph — two
    // near-identical marks per row read as noise rather than as structure.
    let icon = if node.has_children { "" } else { "·" };
    let click_id = node.id.clone();
    let key_id = node.id.clone();
    view! {
        <div class="adi-tree__row" role="treeitem" tabindex="0"
            title=node.title
            data-selected=is_selected.to_string()
            aria-selected=is_selected.to_string()
            aria-expanded=node.has_children.then(|| expanded.to_string())
            on:click=move |_| tree.selected.set(Some(click_id.clone()))
            on:keydown=move |ev: web_sys::KeyboardEvent| {
                if ev.key() == "Enter" || ev.key() == " " {
                    ev.prevent_default();
                    tree.selected.set(Some(key_id.clone()));
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
