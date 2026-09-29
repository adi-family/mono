//! The new UI's `⌘K` command palette, drawn after Raycast rather than after the design system:
//! a translucent panel floating high over the wallpaper, a borderless search field across its
//! top, commands grouped under section labels, and a footer naming what Enter does.
//!
//! Its own component, not [`crate::launcher`] restyled — that menu is the old screens', and they
//! keep it exactly as it is. What carries over is the behaviour people already have in their
//! hands: `⌘K` / `Ctrl+K` toggles, typing filters, the arrows and the pointer move one cursor,
//! Enter runs, Escape or a click outside closes.

use adi_ui::{Icon, IconSize, Lucide, Mark};
use leptos::{ev, html, prelude::*};

/// One command.
pub(super) struct Item {
    /// The label it is listed under. Items sharing one should sit next to each other.
    section: &'static str,
    title: String,
    /// A dim second phrase after the title — what it touches.
    subtitle: String,
    icon: Lucide,
    run: Callback<()>,
}

impl Item {
    pub(super) fn new(
        section: &'static str,
        title: impl Into<String>,
        subtitle: impl Into<String>,
        icon: Lucide,
        run: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            section,
            title: title.into(),
            subtitle: subtitle.into(),
            icon,
            run: Callback::new(move |()| run()),
        }
    }

    /// Whether the typed filter, already lowercased, matches.
    fn matches(&self, needle: &str) -> bool {
        needle.is_empty()
            || self.title.to_lowercase().contains(needle)
            || self.subtitle.to_lowercase().contains(needle)
            || self.section.to_lowercase().contains(needle)
    }
}

/// The palette and its `⌘K` listener.
///
/// `items` is called every time the list is drawn, so rows can say what is true right now —
/// which appearance is not the current one. `light` puts the panel in the light token set
/// (`.light` in `design/tokens.css`), to sit on a light wallpaper. `open` is the caller's, so
/// something besides `⌘K` — the island's search button — can open it too.
#[component]
pub(super) fn Palette(
    items: impl Fn() -> Vec<Item> + Copy + Send + Sync + 'static,
    #[prop(into)] light: Signal<bool>,
    open: RwSignal<bool>,
) -> impl IntoView {
    let query = RwSignal::new(String::new());
    let cursor = RwSignal::new(0usize);
    let field: NodeRef<html::Input> = NodeRef::new();
    let list: NodeRef<html::Div> = NodeRef::new();

    // `code`, not `key`: with a modifier held, a layout that puts something else on that key
    // reports *that*, and the palette must not depend on the keyboard layout.
    let keys = window_event_listener(ev::keydown, move |ev| {
        if ev.code() != "KeyK"
            || !(ev.meta_key() || ev.ctrl_key())
            || ev.alt_key()
            || ev.shift_key()
        {
            return;
        }
        ev.prevent_default();
        open.update(|o| *o = !*o);
    });
    on_cleanup(move || keys.remove());

    // Every opening starts from a clean slate — no filter, first row — however it was opened.
    Effect::new(move |was: Option<bool>| {
        let now = open.get();
        if now && was != Some(true) {
            query.set(String::new());
            cursor.set(0);
        }
        now
    });

    Effect::new(move |_| {
        if open.get()
            && let Some(el) = field.get()
        {
            let _ = el.focus();
        }
    });

    let filtered = move || {
        let needle = query.get().to_lowercase();
        items().into_iter().filter(|i| i.matches(&needle)).collect::<Vec<Item>>()
    };

    // The filtered rows and the cursor pulled back inside them — one place, so what is drawn
    // selected and what Enter runs can never disagree.
    let shown = move || {
        let rows = filtered();
        let at = cursor.get().min(rows.len().saturating_sub(1));
        (rows, at)
    };

    // Only the arrow keys scroll, and only as far as it takes to bring the row fully into
    // view. Doing this from an effect on `cursor` made a hover scroll the list too — the row
    // under the pointer was pinned to the bottom edge while a wheel was moving it up, which
    // read as the list fighting the scroll.
    let reveal = move |i: usize| {
        let Some(list) = list.get_untracked() else {
            return;
        };
        let Some(row) = list
            .query_selector(&format!("[data-row=\"{i}\"]"))
            .ok()
            .flatten()
        else {
            return;
        };
        let (b, r) = (list.get_bounding_client_rect(), row.get_bounding_client_rect());
        // The first row's section label sits above it; wrapping back to the top shows it too.
        if i == 0 {
            list.set_scroll_top(0);
        } else if r.top() < b.top() {
            list.set_scroll_top(list.scroll_top() - (b.top() - r.top()).ceil() as i32);
        } else if r.bottom() > b.bottom() {
            list.set_scroll_top(list.scroll_top() + (r.bottom() - b.bottom()).ceil() as i32);
        }
    };

    let run = move |item: &Item| {
        open.set(false);
        item.run.run(());
    };

    let on_key = move |ev: ev::KeyboardEvent| {
        let (rows, at) = shown();
        match ev.key().as_str() {
            "ArrowDown" => {
                ev.prevent_default();
                if !rows.is_empty() {
                    let next = (at + 1) % rows.len();
                    cursor.set(next);
                    reveal(next);
                }
            }
            "ArrowUp" => {
                ev.prevent_default();
                if !rows.is_empty() {
                    let next = (at + rows.len() - 1) % rows.len();
                    cursor.set(next);
                    reveal(next);
                }
            }
            "Enter" => {
                ev.prevent_default();
                if let Some(item) = rows.get(at) {
                    run(item);
                }
            }
            // Stopped here so a window-level Escape — the settings window's — does not close
            // what is under the palette along with it.
            "Escape" => {
                ev.prevent_default();
                ev.stop_propagation();
                open.set(false);
            }
            _ => {}
        }
    };

    view! {
        <Show when=move || open.get()>
            // Raycast has no scrim: the palette floats over a screen left as it was, and a
            // click anywhere off it dismisses it.
            <div class="adi-pal-catch" on:pointerdown=move |_| open.set(false)></div>
            <div
                class="adi-pal"
                class:light=move || light.get()
                role="dialog"
                aria-modal="true"
                aria-label="Command menu"
            >
                <input
                    node_ref=field
                    class="adi-pal__search"
                    type="text"
                    placeholder="Search commands…"
                    autocomplete="off"
                    spellcheck="false"
                    prop:value=move || query.get()
                    on:input=move |ev| {
                        query.set(event_target_value(&ev));
                        cursor.set(0);
                    }
                    on:keydown=on_key
                />
                <div class="adi-pal__list" role="listbox" node_ref=list>
                    // Rebuilt when the filter changes, never when the cursor moves: a hover
                    // re-renders two `aria-selected`s, not every row.
                    {move || {
                        let rows = filtered();
                        if rows.is_empty() {
                            return view! { <p class="adi-pal__empty">"No results"</p> }
                                .into_any();
                        }
                        let last_row = rows.len() - 1;
                        let mut last = "";
                        rows.into_iter()
                            .enumerate()
                            .map(|(i, item)| {
                                let head = (item.section != last).then_some(item.section);
                                last = item.section;
                                let selected = move || cursor.get().min(last_row) == i;
                                row(item, head, i, selected, cursor, open)
                            })
                            .collect::<Vec<_>>()
                            .into_any()
                    }}
                </div>
                <footer class="adi-pal__foot">
                    <Mark class="adi-pal__mark"/>
                    <span class="adi-pal__spacer"></span>
                    <span class="adi-pal__hint">"Run command"</span>
                    <kbd class="adi-pal__key" aria-label="Enter">
                        <Icon icon=Lucide::CornerDownLeft size=IconSize::Sm/>
                    </kbd>
                </footer>
            </div>
        </Show>
    }
}

/// One row, under its section label when it is the first of its section.
fn row(
    item: Item,
    head: Option<&'static str>,
    i: usize,
    selected: impl Fn() -> bool + Send + Sync + 'static,
    cursor: RwSignal<usize>,
    open: RwSignal<bool>,
) -> AnyView {
    let run = item.run;
    view! {
        {head.map(|h| view! { <div class="adi-pal__section">{h}</div> })}
        <button
            type="button"
            class="adi-pal__row"
            role="option"
            data-row=i
            aria-selected=move || selected().to_string()
            // `pointermove`, not `pointerenter`: a list that scrolls under a resting pointer
            // must not steal the cursor from the arrow keys.
            on:pointermove=move |_| {
                if cursor.get_untracked() != i {
                    cursor.set(i);
                }
            }
            on:click=move |_| {
                open.set(false);
                run.run(());
            }
        >
            <span class="adi-pal__icon">
                <Icon icon=item.icon size=IconSize::Md/>
            </span>
            <span class="adi-pal__title">{item.title}</span>
            <span class="adi-pal__subtitle">{item.subtitle}</span>
        </button>
    }
    .into_any()
}
