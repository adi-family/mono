//! The menu `⌘K` opens, and the triggers that open it by pointer.
//!
//! The root screens wear no bar. A bar is a poor container for what one would hold here: it
//! costs a strip of every viewport forever to carry controls a reader touches a few times a
//! day. So everything it would have carried is a row in this menu instead — every dashboard
//! this machine can open, here or on a paired node, every page of the control panel.
//!
//! The control panel carries the same menu, and both are given their rows by [`crate::menu`] —
//! one list, so the two cannot come to disagree about what this app can do. A palette that
//! stopped working on the page you had just used it to reach would be one nobody learned to
//! trust; that is the whole reason it is on both.
//!
//! **The trigger and the menu mount separately**, because the trigger differs by screen and the
//! menu does not:
//!
//! * [`overlay`] — the `⌘K` listener and the dialog. Mounted once per screen and *outside*
//!   whatever swaps underneath it, so the menu survives the screen behind it changing.
//! * [`brand`] — the mark, the wordmark and the shortcut on one line. The chat docks it at the
//!   head of the sessions rail; it is the one screen with a column to put it in.
//! * [`floating`] — the bare mark in a corner, for the screens with nowhere to dock one: the
//!   setup wizard and the control panel.
//!
//! The mark is [`adi_ui::Mark`] and nothing else. It does not move, glow or open up on hover:
//! the mark is not a mascot (`design/DESIGN.md` §10), and its hover is the same tone change
//! every other control gets.

use leptos::{ev, prelude::*};

use adi_ui::{Kbd, Mark, Modal};

use crate::icons;

/// How far from the corner the floating mark sits.
const EDGE: f64 = 8.0;

/// The height of the workbench's status strip (`.adi-statusbar` in `adi-css`), which the control
/// panel's mark has to start clear of. Stated rather than measured: the strip is a fixed height,
/// and the corner is drawn before there is anything on screen to read.
const STATUS_STRIP: f64 = 28.0;

/// The menu, and everything it needs to remember.
///
/// Built once per mounted screen and passed to the pieces above. It is `Copy`, so the branch
/// that draws the chat and the branch that draws the wizard can hold the same one — which is the
/// point: the menu does not close or forget what was typed into it when the screen under it
/// swaps.
#[derive(Clone, Copy)]
pub(crate) struct Launcher {
    /// Whether the menu is open. Also how [`Modal`] closes itself, and how a row that navigates
    /// within this document shuts the menu on its way out.
    open: RwSignal<bool>,
    /// What has been typed into the filter.
    query: RwSignal<String>,
    /// Which row `Enter` would run, as an index into the *filtered* list.
    cursor: RwSignal<usize>,
    /// How far above the foot of the viewport [`floating`] sits. The workbench wears a status
    /// strip along that edge and the wizard does not, so the two start in different corners.
    floor: f64,
}

impl Launcher {
    pub(crate) fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            query: RwSignal::new(String::new()),
            cursor: RwSignal::new(0),
            floor: EDGE,
        }
    }

    /// The same, for the control panel: its corner clears the status strip along the bottom of
    /// the workbench, which the mark would otherwise be drawn on top of.
    pub(crate) fn workbench() -> Self {
        Self {
            floor: STATUS_STRIP + EDGE,
            ..Self::new()
        }
    }

    /// Open the menu from a clean slate: no filter, first row selected.
    fn show(self) {
        self.query.set(String::new());
        self.cursor.set(0);
        self.open.set(true);
    }
}

/// One row of the menu.
///
/// Built fresh every time the list is drawn, so a row can say something that is only true
/// right now — which version is published, which dashboards are up.
pub(crate) struct Action {
    /// What the row says. Also what the filter matches on, along with the hint.
    label: String,
    /// The dim note down the right: where the row goes, or what it is.
    hint: String,
    icon: icons::Icon,
    run: Callback<()>,
}

impl Action {
    /// A row that does something on this page.
    pub(crate) fn new(
        label: impl Into<String>,
        hint: impl Into<String>,
        icon: icons::Icon,
        run: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            hint: hint.into(),
            icon,
            run: Callback::new(move |()| run()),
        }
    }

    /// A row that leaves for another document. `/extended` and a dashboard are both their own
    /// origins-worth of page, so this is a navigation and not a route change.
    pub(crate) fn link(
        label: impl Into<String>,
        hint: impl Into<String>,
        icon: icons::Icon,
        href: impl Into<String>,
    ) -> Self {
        let href = href.into();
        Self::new(label, hint, icon, move || {
            let _ = window().location().set_href(&href);
        })
    }

    /// A row that opens somewhere in a new tab — a dashboard, which is its own origin and
    /// which the reader is stepping *to* rather than *away* from the chat.
    pub(crate) fn tab(
        label: impl Into<String>,
        hint: impl Into<String>,
        icon: icons::Icon,
        href: impl Into<String>,
    ) -> Self {
        let href = href.into();
        Self::new(label, hint, icon, move || {
            let _ = window().open_with_url_and_target(&href, "_blank");
        })
    }

    /// Whether the typed filter, already lowercased, matches this row.
    fn matches(&self, needle: &str) -> bool {
        needle.is_empty()
            || self.label.to_lowercase().contains(needle)
            || self.hint.to_lowercase().contains(needle)
    }
}

/// The `⌘K` listener and the dialog it opens — everything but the button.
///
/// `actions` is called every time the list is drawn rather than taken once, so the rows track
/// whatever signals they read: a dashboard that comes up mid-session appears in the menu without
/// anything having to invalidate it.
pub(crate) fn overlay(
    l: Launcher,
    actions: impl Fn() -> Vec<Action> + Copy + Send + Sync + 'static,
) -> impl IntoView {
    // `⌘K` from anywhere on the page. `code`, not `key`: with a modifier held, a layout that
    // puts something else on that key reports *that*, and the menu must not depend on the
    // keyboard. Toggles rather than opens, so the same press that opened it closes it.
    let keys = window_event_listener(ev::keydown, move |ev| {
        if ev.code() != "KeyK"
            || !(ev.meta_key() || ev.ctrl_key())
            || ev.alt_key()
            || ev.shift_key()
        {
            return;
        }
        ev.prevent_default();
        if l.open.get_untracked() {
            l.open.set(false);
        } else {
            l.show();
        }
    });
    on_cleanup(move || keys.remove());

    menu(l, actions)
}

/// The docked trigger: the mark, the wordmark and the shortcut on one line.
///
/// `place` is the modifier the stylesheet keys off — the sessions rail's head or the
/// narrow-viewport bar. Both are drawn, and CSS shows whichever belongs to the current width;
/// they cannot be one element, because the two live in different parents.
pub(crate) fn brand(l: Launcher, place: &'static str) -> impl IntoView {
    view! {
        <button
            type="button"
            class=format!("adi-brand {place}")
            title=format!("Menu — {}K", mod_glyph())
            aria-label="Open the menu"
            aria-haspopup="dialog"
            aria-keyshortcuts="Meta+K Control+K"
            on:click=move |_| l.show()
        >
            <Mark class="adi-brand__mark"/>
            <span class="adi-brand__word">"adi"</span>
            <Kbd class="adi-brand__kbd">{format!("{}K", mod_glyph())}</Kbd>
        </button>
    }
}

/// The trigger for a screen with no column to dock one in: the mark alone, in a corner.
pub(crate) fn floating(l: Launcher) -> impl IntoView {
    view! {
        <button
            type="button"
            class="fixed z-40 flex size-11 cursor-pointer items-center justify-center \
                   rounded-md border border-line-strong bg-side text-ink hover:bg-raise"
            style=format!("left:{EDGE}px;bottom:{}px", l.floor)
            title=format!("Menu — {}K", mod_glyph())
            aria-label="Open the menu"
            aria-haspopup="dialog"
            aria-keyshortcuts="Meta+K Control+K"
            on:click=move |_| l.show()
        >
            <Mark class="size-7"/>
        </button>
    }
}

/// The menu itself: a filter, and the rows that survive it.
fn menu(
    l: Launcher,
    actions: impl Fn() -> Vec<Action> + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let field: NodeRef<leptos::html::Input> = NodeRef::new();
    // Focus the filter as it opens, so `⌘K` and then typing is one movement. On the ref
    // landing rather than on `open`, because the node does not exist until the dialog is up.
    Effect::new(move |_| {
        if l.open.get()
            && let Some(el) = field.get()
        {
            let _ = el.focus();
        }
    });

    // What Enter would run, and what the arrows move over: the filtered list, with the cursor
    // pulled back into it. Kept in one place so the row rendering and the key handling can
    // never disagree about which row is selected.
    let shown = move || {
        let needle = l.query.get().to_lowercase();
        let rows: Vec<Action> = actions()
            .into_iter()
            .filter(|a| a.matches(&needle))
            .collect();
        let at = l.cursor.get().min(rows.len().saturating_sub(1));
        (rows, at)
    };

    view! {
        <Modal open=l.open title="Menu" width="max-w-lg">
            <div class="flex flex-col gap-3">
                <input
                    node_ref=field
                    class="h-9 w-full rounded-md border border-line-strong bg-raise px-3 \
                           text-ui text-ink focus-visible:border-ink-3 focus-visible:outline-none"
                    type="text"
                    placeholder="Search actions…"
                    autocomplete="off"
                    prop:value=move || l.query.get()
                    on:input=move |ev| {
                        l.query.set(event_target_value(&ev));
                        // A new filter is a new list; leaving the cursor where it was would
                        // aim Enter at whatever happens to have shuffled under it.
                        l.cursor.set(0);
                    }
                    on:keydown=move |ev: ev::KeyboardEvent| {
                        let (rows, at) = shown();
                        match ev.key().as_str() {
                            "ArrowDown" => {
                                ev.prevent_default();
                                if !rows.is_empty() {
                                    l.cursor.set((at + 1) % rows.len());
                                }
                            }
                            "ArrowUp" => {
                                ev.prevent_default();
                                if !rows.is_empty() {
                                    l.cursor.set((at + rows.len() - 1) % rows.len());
                                }
                            }
                            "Enter" => {
                                ev.prevent_default();
                                if let Some(row) = rows.get(at) {
                                    l.open.set(false);
                                    row.run.run(());
                                }
                            }
                            _ => {}
                        }
                    }
                />
                <div class="-mx-1 flex max-h-[52vh] flex-col gap-px overflow-y-auto px-1">
                    {move || {
                        let (rows, at) = shown();
                        if rows.is_empty() {
                            return view! {
                                <p class="px-2 py-6 text-center text-small text-ink-3">
                                    "Nothing matches."
                                </p>
                            }
                            .into_any();
                        }
                        rows.into_iter()
                            .enumerate()
                            .map(|(i, row)| action_row(l, i, i == at, row))
                            .collect::<Vec<_>>()
                            .into_any()
                    }}
                </div>
            </div>
        </Modal>
    }
}

/// One row, drawn.
fn action_row(l: Launcher, i: usize, selected: bool, row: Action) -> AnyView {
    let run = row.run;
    view! {
        <button
            type="button"
            class="flex w-full shrink-0 cursor-pointer items-center gap-2.5 rounded-md px-2 \
                   py-1.5 text-left text-row hover:bg-hover"
            class:bg-active=selected
            // The pointer moving over a row is the reader saying which one they mean, so it
            // takes the cursor with it — otherwise Enter and the pointer aim at two
            // different rows at once.
            on:pointerenter=move |_| l.cursor.set(i)
            on:click=move |_| {
                l.open.set(false);
                run.run(());
            }
        >
            <adi_ui::Icon icon=row.icon.lucide() class="text-ink-3"/>
            <span class="min-w-0 flex-1 truncate text-ink">{row.label}</span>
            <span class="shrink-0 truncate text-mini text-ink-3">{row.hint}</span>
        </button>
    }
    .into_any()
}

/// The modifier this platform writes its shortcuts with.
///
/// A guess from the user agent, which is the only thing a browser still offers — and a wrong
/// one costs a hint that names the wrong key, not a shortcut that fails: [`overlay`] answers to
/// `⌘K` and `Ctrl+K` on every platform either way.
pub(crate) fn mod_glyph() -> &'static str {
    let mac = window()
        .navigator()
        .user_agent()
        .is_ok_and(|ua| ua.contains("Mac"));
    if mac { "\u{2318}" } else { "Ctrl+" }
}
