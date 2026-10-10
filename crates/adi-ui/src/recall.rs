//! [`Recall`] — what a [`Composer`](crate::Composer) offers to send again: the prompts already said.

use leptos::{ev, html, prelude::*};
use web_sys::wasm_bindgen::JsCast;

use crate::icon::{Icon, IconSize, Lucide};
use crate::menu::{Menu, MenuAt, MenuHead, MenuItem, MenuNote};

/// One prompt the person already sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PastPrompt {
    /// The words, exactly as they go back into the box.
    pub text: String,
    /// When it was last sent, already put the way a person reads it ("3h ago"). The caller's,
    /// because whose clock and which phrasing is a question about the app.
    pub when: String,
    /// How many times the same words were sent. Above one, the row says so: a prompt that keeps
    /// being re-sent is the reason this list exists.
    pub times: usize,
}

/// What a composer needs to offer its history.
///
/// The caller fetches and keeps the list — where prompts are stored is a question about the app,
/// the same answer [`Attaching`](crate::Attaching) gives for files. `on_open` is the moment to
/// refresh it: the list is shown at once from what `prompts` already holds, and replaced when the
/// fresh answer lands, so a second open never waits on the network.
#[derive(Clone, Copy)]
pub struct Recall {
    /// Newest first.
    pub prompts: Signal<Vec<PastPrompt>>,
    /// A fetch is in flight. Only said while there is nothing to show yet.
    pub loading: Signal<bool>,
    /// Why the last fetch failed. Said instead of the empty note, which would otherwise claim
    /// nothing was ever sent.
    pub error: Signal<Option<String>>,
    pub on_open: Callback<()>,
}

/// Where the list opens for a button at `el`: under it, or over it when the button sits in the
/// lower half of the screen — a composer is as often pinned to the foot of a chat as to its head.
pub(crate) fn place(el: &web_sys::Element) -> MenuAt {
    let rect = el.get_bounding_client_rect();
    let (width, height) = window()
        .inner_width()
        .ok()
        .zip(window().inner_height().ok())
        .map_or((rect.right(), rect.bottom()), |(w, h)| {
            (
                w.as_f64().unwrap_or_default(),
                h.as_f64().unwrap_or_default(),
            )
        });
    #[allow(clippy::cast_possible_truncation)]
    let px = |v: f64| v.round() as i32;
    let right = px(width - rect.right());
    if rect.top() > height - rect.bottom() {
        MenuAt::RightAbove(right, px(height - rect.top()) + 4)
    } else {
        MenuAt::RightOf(right, px(rect.bottom()) + 4)
    }
}

/// Move focus to the first row of the list, if it has one yet.
pub(crate) fn focus_first(list: NodeRef<html::Div>) -> bool {
    list.get_untracked()
        .and_then(|el| el.query_selector("button").ok().flatten())
        .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
        .is_some_and(|el| el.focus().is_ok())
}

/// The clock button in the composer's row, and the list it opens.
///
/// The open state, the button and the list are the composer's, not this component's: Arrow-up in
/// an empty box opens the same list from the keyboard, and closing it hands focus back to the box.
#[component]
pub(crate) fn RecallButton(
    recall: Recall,
    at: RwSignal<Option<MenuAt>>,
    button: NodeRef<html::Button>,
    list: NodeRef<html::Div>,
    /// Arrow-up off the first row: back to what is being typed.
    on_leave: Callback<()>,
    on_pick: Callback<String>,
    on_dismiss: Callback<()>,
) -> impl IntoView {
    // Arrow keys walk the rows, which are siblings; up from the first leaves the list.
    let walk = move |ev: ev::KeyboardEvent| {
        let down = match ev.key().as_str() {
            "ArrowDown" => true,
            "ArrowUp" => false,
            _ => return,
        };
        ev.prevent_default();
        let Some(current) = document().active_element() else {
            return;
        };
        let next = if down {
            current.next_element_sibling()
        } else {
            current.previous_element_sibling()
        };
        match next.and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok()) {
            Some(el) => {
                let _ = el.focus();
            }
            None if !down => on_leave.run(()),
            None => {}
        }
    };
    let row = move |prompt: PastPrompt| {
        let PastPrompt { text, when, times } = prompt;
        let meta = if times > 1 {
            format!("{when} · sent {times} times")
        } else {
            when
        };
        let pick = text.clone();
        view! {
            <MenuItem
                title=text.clone()
                on_select=Callback::new(move |()| on_pick.run(pick.clone()))
            >
                <span class="line-clamp-2 break-words whitespace-pre-line">{text}</span>
                <span class="mt-0.5 block text-label text-ink-3">{meta}</span>
            </MenuItem>
        }
    };
    let note = move || {
        if !recall.prompts.get().is_empty() {
            return None;
        }
        Some(if recall.loading.get() {
            "Loading…".to_string()
        } else if let Some(error) = recall.error.get() {
            format!("Couldn't load recent prompts: {error}")
        } else {
            "Nothing sent yet — what you send shows up here.".to_string()
        })
    };
    view! {
        <button
            class="grid size-8 shrink-0 cursor-pointer place-items-center rounded-md text-ink-2 \
                   transition-colors duration-100 hover:bg-hover hover:text-ink"
            type="button"
            title="Recent prompts — or press ↑ in an empty box"
            aria-haspopup="menu"
            aria-expanded=move || at.get().is_some().to_string()
            node_ref=button
            on:click=move |_| {
                if at.get_untracked().is_some() {
                    on_dismiss.run(());
                    return;
                }
                let Some(el) = button.get_untracked() else { return };
                recall.on_open.run(());
                at.set(Some(place(&el)));
            }
        >
            <Icon icon=Lucide::History size=IconSize::Md label="Recent prompts"/>
        </button>
        <Menu
            at=at
            on_dismiss=on_dismiss
            class="w-[min(560px,calc(100vw-32px))] max-w-none max-h-[min(420px,60vh)] \
                   overflow-y-auto"
        >
            <MenuHead>"Recent prompts"</MenuHead>
            // Keyed, so the refresh `on_open` starts replaces only the rows that changed: Arrow-up
            // focuses the first row of the held list, and rebuilding it under the cursor when the
            // fresh answer lands would drop that focus on the floor.
            <div node_ref=list on:keydown=walk>
                <For
                    each=move || recall.prompts.get()
                    key=|p| (p.text.clone(), p.when.clone(), p.times)
                    children=row
                />
            </div>
            {move || note().map(|note| view! { <MenuNote>{note}</MenuNote> })}
        </Menu>
    }
}
