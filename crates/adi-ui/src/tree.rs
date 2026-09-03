//! [`Tree`] — an IDE-style tree view: a file browser, a project tree, a task tree.
//!
//! Callers hand it a **flat, depth-annotated list** and it owns the presentation: the indent
//! rails, the twisty, selection and keyboard activation. It deliberately knows nothing about
//! files or projects, so one component serves all of them.
//!
//! Collapsing is derived from depth alone — a closed row hides every following row deeper
//! than it, which is exactly its subtree. No parent pointers have to be threaded anywhere,
//! and any list already in tree order just works.

use std::collections::HashSet;

use leptos::prelude::*;

use crate::icon::{Icon, IconSize, Lucide};
use crate::merge;

/// One row of a tree.
///
/// Built with [`TreeNode::new`] plus whichever of the builder methods the row actually
/// wants, so a plain leaf costs one call and nothing is optional-shaped at the call site.
// Four bools, and they stay four bools: they are independent facts about one row, not a
// state machine. `has_children && container && separated && emphasis` is a real row — a
// project header that closes a section — so no enum can hold them without holding a
// product of the four.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeNode {
    /// Stable identity — what selection and expansion are keyed on.
    pub id: String,
    /// Nesting level; 0 is a root.
    pub depth: usize,
    pub label: String,
    /// Whether this row gets a twisty and can be collapsed.
    pub has_children: bool,
    /// A short trailing note — `"3 open"`, `"2 changed"`. Something the reader would act
    /// on: a byte size is a number every row can offer and nobody reads.
    pub badge: Option<String>,
    /// Native tooltip — the full path, when the label is only the last segment of it.
    pub title: Option<String>,
    /// A grouping row that is not itself a destination. Clicking it opens or closes it, the
    /// way clicking a folder name does, rather than selecting something unopenable.
    pub container: bool,
    /// The row's glyph. Rows without one get a dot, so the icon column keeps its width either
    /// way and every label starts on one edge.
    pub icon: Option<Lucide>,
    /// Draw a rule above this row, marking the boundary between two kinds of children.
    pub separated: bool,
    /// Give the row more presence than its siblings — the project among its own pages.
    pub emphasis: bool,
}

impl TreeNode {
    /// A plain leaf at `depth`.
    #[must_use]
    pub fn new(id: impl Into<String>, depth: usize, label: impl Into<String>) -> Self {
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

    /// Give it a twisty (see [`TreeNode::has_children`]).
    #[must_use]
    pub fn children(mut self, has_children: bool) -> Self {
        self.has_children = has_children;
        self
    }

    /// A short trailing note (see [`TreeNode::badge`]).
    #[must_use]
    pub fn badge(mut self, badge: impl Into<String>) -> Self {
        self.badge = Some(badge.into());
        self
    }

    /// The badge a row may or may not have — [`TreeNode::badge`] for a value that arrives
    /// already `Option`-shaped, which is how a count out of a store arrives.
    ///
    /// It is a second method rather than a wider bound on the first because one method
    /// cannot be both: `impl Into<Option<S>>` accepts a literal *and* an `Option`, but then
    /// `Option<String>` leaves `S` ambiguous and every data-driven caller — the ones this
    /// exists for — has to turbofish it back.
    #[must_use]
    pub fn maybe_badge(mut self, badge: Option<String>) -> Self {
        self.badge = badge;
        self
    }

    /// The native tooltip (see [`TreeNode::title`]).
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// The tooltip a row may or may not have — [`TreeNode::title`] for an already-`Option`
    /// value, paired with it the way [`TreeNode::maybe_badge`] is with the badge.
    #[must_use]
    pub fn maybe_title(mut self, title: Option<String>) -> Self {
        self.title = title;
        self
    }

    /// Mark it a pure grouping row (see [`TreeNode::container`]).
    #[must_use]
    pub fn container(mut self, container: bool) -> Self {
        self.container = container;
        self
    }

    /// The row's glyph (see [`TreeNode::icon`]).
    #[must_use]
    pub fn icon(mut self, icon: Lucide) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Draw a rule above it (see [`TreeNode::separated`]).
    #[must_use]
    pub fn separated(mut self, separated: bool) -> Self {
        self.separated = separated;
        self
    }

    /// Give it more presence than its siblings (see [`TreeNode::emphasis`]).
    #[must_use]
    pub fn emphasis(mut self, emphasis: bool) -> Self {
        self.emphasis = emphasis;
        self
    }
}

/// One tree's interaction state: what was last activated, and which branches are open.
///
/// `Copy`, so it threads into a page's view and its handlers the way the page's own state
/// does. Branches start closed — a tree listing every section of every project is unreadable
/// fully expanded — and [`Tree`] additionally opens the ancestors of the highlighted row, so
/// what is currently open is always on screen without anyone hunting for it.
///
/// `selected` is what was **clicked**; the row that is *highlighted* is passed to [`Tree`]
/// separately. Keeping the two apart lets a tree drive navigation while the highlight follows
/// the app's actual state, instead of the tree and the address bar each believing something
/// different.
#[derive(Clone, Copy, Debug)]
pub struct TreeState {
    /// The last row activated by a click or a keypress.
    pub selected: RwSignal<Option<String>>,
    /// The ids of every branch the user has opened.
    pub expanded: RwSignal<HashSet<String>>,
}

impl TreeState {
    /// Everything closed, nothing selected.
    #[must_use]
    pub fn new() -> Self {
        Self {
            selected: RwSignal::new(None),
            expanded: RwSignal::new(HashSet::new()),
        }
    }

    /// Open a closed branch, or close an open one.
    pub fn toggle(&self, id: &str) {
        self.expanded.update(|set| {
            if !set.remove(id) {
                set.insert(id.to_string());
            }
        });
    }
}

impl Default for TreeState {
    fn default() -> Self {
        Self::new()
    }
}

/// The tree.
///
/// Rows inside a closed branch are dropped here rather than by the caller, so the caller
/// always passes the whole tree and never has to track what is currently visible.
///
/// ```ignore
/// let tree = TreeState::new();
/// let nodes = vec![
///     TreeNode::new("src", 0, "src").children(true).icon(FOLDER),
///     TreeNode::new("src/lib.rs", 1, "lib.rs").icon(FILE),
/// ];
/// view! { <Tree nodes=nodes state=tree selected=Signal::derive(move || open.get())/> }
/// ```
#[component]
pub fn Tree(
    /// Every row, in depth-annotated tree order — including the ones inside closed
    /// branches. Takes a plain `Vec` as readily as a signal, so a static tree costs no
    /// ceremony and a live one is a one-word change.
    #[prop(into)]
    nodes: Signal<Vec<TreeNode>>,
    /// Selection and expansion. Left off, the tree keeps its own, which is all a browser
    /// nobody is navigating with needs.
    #[prop(default = TreeState::new())]
    state: TreeState,
    /// The row to highlight — the app's actual current thing, which is not always the last
    /// thing clicked. Its ancestors are opened for it.
    #[prop(optional, into)]
    selected: Signal<Option<String>>,
    /// What to say when there is nothing to show.
    #[prop(default = String::from("Nothing here."), into)]
    empty: String,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let rows = move || {
        let nodes = nodes.get();
        let expanded = state.expanded.get();
        let current = selected.get();
        let revealed = ancestors_of(&nodes, current.as_deref());

        // The depth of the closed branch we are currently inside, if any: every row deeper
        // than it belongs to that subtree and is skipped.
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
            rows.push(row(node, open, current.as_deref(), state));
        }
        rows
    };

    view! {
        <div class=merge("select-none overflow-x-auto py-2 text-row", class) role="tree">
            {move || {
                let rows = rows();
                if rows.is_empty() {
                    view! {
                        <div class="px-3 py-6 text-small text-ink-3">
                            {empty.clone()}
                        </div>
                    }
                    .into_any()
                } else {
                    rows.into_any()
                }
            }}
        </div>
    }
}

/// The ids of every branch enclosing `selected` — the path that has to be open for the
/// highlighted row to be on screen at all.
///
/// Depth ordering is enough to find them: walking the rows while keeping the last id seen at
/// each shallower depth yields the ancestor chain, with no parent pointers anywhere.
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

/// The open/close control, and on a leaf the empty slot that keeps the column aligned.
///
/// A chevron that turns is the one tree control everybody already reads; the turn is the
/// reader's own click, so it may take the 100ms.
fn twisty(id: &str, has_children: bool, expanded: bool, state: TreeState) -> AnyView {
    if !has_children {
        return view! { <span class="ml-1 size-4 shrink-0"></span> }.into_any();
    }
    let id = id.to_string();
    let turn = if expanded {
        "rotate-90 transition-transform duration-100"
    } else {
        "transition-transform duration-100"
    };
    view! {
        <button
            class="ml-1 grid size-4 shrink-0 cursor-pointer place-items-center p-0 \
                   text-ink-3 hover:text-ink"
            type="button"
            tabindex="-1"
            aria-label=if expanded { "Collapse" } else { "Expand" }
            on:click=move |ev| {
                // The twisty only opens and closes — it must not also move the selection.
                ev.stop_propagation();
                state.toggle(&id);
            }
        >
            <Icon icon=Lucide::ChevronRight size=IconSize::Sm class=turn/>
        </button>
    }
    .into_any()
}

/// One row: `depth` indent rails, a twisty on branches, the glyph, the label, and whatever
/// the badge says.
///
/// The open row is a tone change plus a 3px accent marker down its left edge — the one place
/// navigation may carry the accent (§3), and a marker rather than a fill of it.
fn row(node: TreeNode, expanded: bool, selected: Option<&str>, state: TreeState) -> AnyView {
    let is_selected = selected == Some(node.id.as_str());

    // Written out per case, because Tailwind only finds a class it can read as a literal.
    let tone = if is_selected {
        "bg-active text-ink"
    } else if node.emphasis {
        "font-medium text-ink hover:bg-hover"
    } else {
        "text-ink-2 hover:bg-hover hover:text-ink"
    };
    let rule = if node.separated {
        "mt-1 border-t border-line pt-1"
    } else {
        ""
    };
    // The glyph takes the ink one step below the label, and steps up with it when the row is
    // open — never a colour of its own.
    let glyph = if is_selected || node.emphasis {
        "text-ink-2"
    } else {
        "text-ink-3"
    };

    let rails = (0..node.depth)
        .map(|_| view! { <span class="ml-2 w-3.5 shrink-0 self-stretch border-l border-line"></span> })
        .collect::<Vec<_>>();

    let icon = match node.icon {
        Some(icon) => view! { <Icon icon=icon size=IconSize::Sm/> }.into_any(),
        None => view! { <span class="text-[11px] leading-none">"·"</span> }.into_any(),
    };

    let container = node.container;
    let click_id = node.id.clone();
    let key_id = node.id.clone();
    let activate = move |id: String| {
        if container {
            state.toggle(&id);
        } else {
            state.selected.set(Some(id));
        }
    };

    view! {
        <div
            class=format!("relative flex cursor-default items-center rounded-md py-[5px] \
                           pr-2.5 whitespace-nowrap {tone} {rule}")
            role="treeitem"
            tabindex="0"
            title=node.title
            aria-selected=is_selected.to_string()
            aria-expanded=node.has_children.then(|| expanded.to_string())
            on:click=move |_| activate(click_id.clone())
            on:keydown=move |ev| {
                if ev.key() == "Enter" || ev.key() == " " {
                    ev.prevent_default();
                    activate(key_id.clone());
                }
            }
        >
            {is_selected.then(|| view! {
                <span
                    class="absolute top-1.5 bottom-1.5 left-0 w-[3px] rounded-sm bg-accent"
                    aria-hidden="true"
                ></span>
            })}
            {rails}
            {twisty(&node.id, node.has_children, expanded, state)}
            <span
                class=format!("grid h-4 w-4.5 shrink-0 place-items-center {glyph}")
                aria-hidden="true"
            >
                {icon}
            </span>
            <span class="truncate pl-1.5">{node.label}</span>
            {node.badge.map(|b| view! {
                <span class="ml-auto pl-2 text-mini text-ink-3">{b}</span>
            })}
        </div>
    }
    .into_any()
}
