//! [`Menu`] — the small panel a control drops under itself, and the four things inside it.

use leptos::{ev, prelude::*};

use crate::help::HelpLink;
use crate::icon::{Icon, IconSize, Lucide};
use crate::merge;

/// Where a [`Menu`]'s top corner goes, in viewport coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAt {
    /// Top-**left** at this point: where a right-click landed, or the bottom-left of the button
    /// that opened it.
    Point(i32, i32),
    /// Top-**right**, measured in from the viewport's right edge, for a control that sits on one:
    /// a row's `⋯` is at the right of its table, so a left-anchored menu would open off screen.
    RightOf(i32, i32),
}

impl MenuAt {
    /// The panel is `position: fixed`, so both variants are one `style` away from each other.
    fn style(self) -> String {
        match self {
            Self::Point(x, y) => format!("left:{x}px; top:{y}px"),
            Self::RightOf(right, top) => format!("right:{right}px; top:{top}px"),
        }
    }
}

/// A menu anchored at a point: a row's `⋯`, a right-click, a checklist under a header button.
///
/// Fixed to the viewport rather than positioned inside its opener, so a menu opened from a row
/// deep inside a scroll container is never clipped by it. A scrim behind it turns the next click
/// anywhere into a dismiss — its own element rather than a window listener, so it dies with the
/// menu and can swallow a right-click too (a second right-click while a menu is open should
/// close it, not stack the browser's own menu on top). `Escape` is the third way out, the same
/// three [`Modal`](crate::Modal) offers.
///
/// A raised surface with a strong hairline; no blur, no shadow (§8).
///
/// `at` carries **both** the position and whether it is open at all, because a menu that is not
/// open has no position to be at. `None` keeps it mounted and hidden rather than dropping it, so
/// a caller that built its items once — a table row's overflow, whose handlers close over that
/// row — keeps them alive across opens instead of rebuilding them per click. The `Escape`
/// listener is per instance and therefore per row on such a table; what it does on a key that is
/// not `Escape` is one untracked signal read, which is cheaper than the effect per row it would
/// take to attach and detach the listener as each menu opens.
///
/// ```ignore
/// let at = RwSignal::new(None::<MenuAt>);
/// view! {
///     <Menu at=at on_dismiss=Callback::new(move |()| at.set(None))>
///         <MenuHead>"Sessions from"</MenuHead>
///         <MenuItem checked=true on_select=Callback::new(move |()| toggle())>"This machine"</MenuItem>
///     </Menu>
/// }
/// ```
#[component]
pub fn Menu(
    /// Where it is, and whether it is open.
    #[prop(into)]
    at: Signal<Option<MenuAt>>,
    /// The scrim, a right-click on the scrim, and `Escape` all call this.
    on_dismiss: Callback<()>,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let handle = window_event_listener(ev::keydown, move |ev| {
        if ev.key() == "Escape" && at.get_untracked().is_some() {
            on_dismiss.run(());
        }
    });
    on_cleanup(move || handle.remove());

    // One step under the panel, over everything else: the scrim has to cover what the menu is
    // acting on without covering the menu itself.
    let scrim_style = move || {
        if at.get().is_some() {
            ""
        } else {
            "display:none"
        }
    };
    let style = move || {
        at.get()
            .map(MenuAt::style)
            .unwrap_or_else(|| "display:none".to_string())
    };
    view! {
        <div
            class="fixed inset-0 z-[59]"
            style=scrim_style
            on:click=move |_| on_dismiss.run(())
            on:contextmenu=move |ev: web_sys::MouseEvent| {
                ev.prevent_default();
                on_dismiss.run(());
            }
        ></div>
        <div
            class=merge(
                "fixed z-[60] min-w-42 max-w-70 rounded-lg border border-line-strong bg-raise p-1",
                class,
            )
            role="menu"
            style=style
        >
            {children()}
        </div>
    }
}

/// What the menu is about, over a hairline: "Sessions from", "Show", the path a right-click
/// landed on.
///
/// A label rather than a title (§2.6, sentence case, 12px `--ink-3`) — the menu is the answer
/// and this is only the question, so it never competes with the items under it. One line: a head
/// long enough to wrap is a path or an id, and those are what `title` and `mono` are for.
///
/// `help` puts a [`HelpLink`] on the right of the same line. It belongs on the head rather than
/// on any one item because what it explains is the menu's *subject* — what a "source" is, what
/// starring does — and a `?` per item would be four links to the same page.
#[component]
pub fn MenuHead(
    /// The full string when the head is one the 280px will clip — a store path, a long name.
    #[prop(optional, into)]
    title: String,
    /// The head *is* a machine string: a path, an id, a command (§2.3).
    #[prop(optional)]
    mono: bool,
    /// The documentation for what this menu is about. Opens in a new tab (see [`HelpLink`]).
    #[prop(optional, into)]
    help: String,
    /// What the `?` is about, for its tooltip — the head's own words are the default.
    #[prop(optional, into)]
    help_label: String,
    children: Children,
) -> impl IntoView {
    let class = if mono {
        "mb-1 flex items-center gap-2 border-b border-line px-2 pt-1.5 pb-2 text-label \
         text-ink-3 mono"
    } else {
        "mb-1 flex items-center gap-2 border-b border-line px-2 pt-1.5 pb-2 text-label text-ink-3"
    };
    view! {
        <div class=class title=title>
            // The head takes the width and the `?` the right edge, so a long head clips against
            // the link rather than pushing it out of the panel.
            <span class="min-w-0 flex-1 truncate">{children()}</span>
            {(!help.is_empty())
                .then(|| view! { <HelpLink href=help label=help_label class="-my-1"/> })}
        </div>
    }
}

/// One line of a menu: an action, or one box in a checklist.
///
/// `checked` is what makes it the second of those. `None` is a plain action (`menuitem`) and
/// draws no tick column at all; `Some` is a `menuitemcheckbox` with the column always drawn,
/// ticked or empty, so the labels of a checklist line up whatever is on.
///
/// A checked item is also set in the ink and 500 — a tick alone reads as decoration on a row of
/// dim text, and this is a state the operator is about to act on.
///
/// **Disabled means listed but not takeable, and it is deliberate**: an item dropped from a menu
/// says the thing behind it is gone, when what is usually true is that it is unavailable *here*
/// and the `title` says where to fix that. Nothing about `disabled` closes the menu.
#[component]
pub fn MenuItem(
    on_select: Callback<()>,
    /// `Some` makes this a checklist box; `None` is an action.
    #[prop(optional, into)]
    checked: Option<bool>,
    /// One of a set where picking one un-picks the rest, rather than a box of its own: it is the
    /// same tick on screen and a different promise to a screen reader.
    #[prop(optional)]
    radio: bool,
    /// Destructive: Delete, Remove, Revoke. Red text and nothing else (§3).
    #[prop(optional)]
    danger: bool,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional, into)] title: String,
    children: Children,
) -> impl IntoView {
    // Whole literals per branch: Tailwind reads this file as text and never runs it.
    let class = match (danger, checked) {
        (true, _) => {
            "block w-full cursor-pointer rounded-md px-2 py-1.5 text-left text-row text-err \
             hover:bg-hover disabled:cursor-default disabled:text-ink-3 \
             disabled:hover:bg-transparent"
        }
        (_, Some(true)) => {
            "block w-full cursor-pointer rounded-md px-2 py-1.5 text-left text-row font-medium \
             text-ink hover:bg-hover disabled:cursor-default disabled:text-ink-3 \
             disabled:hover:bg-transparent"
        }
        _ => {
            "block w-full cursor-pointer rounded-md px-2 py-1.5 text-left text-row text-ink \
             hover:bg-hover disabled:cursor-default disabled:text-ink-3 \
             disabled:hover:bg-transparent"
        }
    };
    let role = match (checked.is_some(), radio) {
        (true, true) => "menuitemradio",
        (true, false) => "menuitemcheckbox",
        (false, _) => "menuitem",
    };
    view! {
        <button
            class=class
            type="button"
            role=role
            aria-checked=checked.map(|c| c.to_string())
            title=title
            prop:disabled=disabled
            on:click=move |_| on_select.run(())
        >
            {checked
                .map(|on| {
                    view! {
                        <MenuTick>
                            {on
                                .then(|| view! { <Icon icon=Lucide::Check size=IconSize::Sm/> })}
                        </MenuTick>
                    }
                })}
            {children()}
        </button>
    }
}

/// The tick column, and anything else that reads as a mark rather than a word — the lock on a
/// node this machine holds no password for.
///
/// A fixed width whether or not it carries a mark, so a checklist's labels line up. `trailing`
/// puts the gap on the other side, for a mark that follows the label rather than opening it.
#[component]
pub fn MenuTick(#[prop(optional)] trailing: bool, children: Children) -> impl IntoView {
    let class = if trailing {
        "ml-1 inline-block w-3.5 align-text-bottom text-ink-2"
    } else {
        "mr-1 inline-block w-3.5 align-text-bottom text-ink-2"
    };
    view! {
        <span class=class aria-hidden="true">
            {children()}
        </span>
    }
}

/// Why the list is as short as it is: an empty state inside the menu, not another item.
///
/// It is read, never picked, so it wraps, takes no hover and is not in the tab order. Use it to
/// answer the question a short menu raises — "no paired nodes yet" — and put the way out of that
/// in it as a [`MenuLink`].
#[component]
pub fn MenuNote(children: Children) -> impl IntoView {
    view! {
        <p class="m-0 px-2 pt-0.5 pb-1.5 text-label leading-[1.45] text-ink-3">{children()}</p>
    }
}

/// A link inside a [`MenuNote`]: where to go to make the menu longer.
///
/// Never the accent (§3) — a step brighter than the note around it and underlined, which is what
/// tells it from the prose it sits in.
#[component]
pub fn MenuLink(#[prop(into)] href: String, children: Children) -> impl IntoView {
    view! {
        <a
            class="cursor-pointer text-ink-2 underline decoration-ink-3 underline-offset-[3px] \
                   hover:text-ink"
            href=href
        >
            {children()}
        </a>
    }
}
