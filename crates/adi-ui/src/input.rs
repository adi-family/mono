//! Text entry: [`Input`], [`Textarea`] and [`Select`], which all wear the same frame.

use leptos::prelude::*;

use crate::icon::{Icon, IconSize, Lucide};
use crate::merge;

/// The shared frame for anything you type into (§6): the raised surface, a strong hairline,
/// radius 6, `9px 12px`, 14px sans, the placeholder in `--ink-3`.
///
/// Two details here are load-bearing rather than taste:
///
/// - **Focus is the border stepping up one tone, not a ring.** A ring at these sizes reads as
///   a second border, and an orange one is a selected state nobody asked for (§3).
/// - **16px below 620px.** iOS zooms the page in when it focuses a field with text under
///   16px, and it does not zoom back out: one tap and the layout is wider than the screen
///   with no obvious gesture to undo it.
pub(crate) const FRAME: &str = "min-w-0 rounded-md border border-line-strong bg-raise px-3 \
                     py-2 text-ui text-ink placeholder:text-ink-3 \
                     transition-colors duration-100 \
                     focus-visible:border-ink-3 focus-visible:outline-none \
                     disabled:cursor-not-allowed disabled:opacity-50 \
                     max-[620px]:text-[16px]";

/// The type a control switches to when its value is a machine value — a path, a port, a
/// flag, a model id. 13px mono in the same ink (the reference's `.input.mono`); the default
/// is sans, because most of what is typed is words (§2.3).
const MONO: &str = "font-mono text-[13px]";

/// How much horizontal room a control asks for.
///
/// These are real widths, not flex hints: an input inside a [`crate::Field`] is a grid
/// child, where `flex-basis` means nothing and `w-full` would resolve against the label
/// above it — so a field labelled "Backend" would be wider than one labelled "Port" for no
/// reason anyone chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputWidth {
    /// The default — room for a short value without dominating a form row.
    #[default]
    Default,
    /// Fills whatever it is in. For the one field a form is really about.
    Wide,
    /// A couple of digits: a count, a limit, a port.
    Num,
}

impl InputWidth {
    /// The box this width asks for.
    #[must_use]
    pub fn classes(self) -> &'static str {
        match self {
            Self::Default => "w-50 max-w-full",
            Self::Wide => "w-full",
            Self::Num => "w-20",
        }
    }
}

/// A single-line text field.
///
/// Pass `value` to bind it to a signal in both directions; leave it off for an uncontrolled
/// field and attach your own `on:input`.
///
/// ```ignore
/// let name = RwSignal::new(String::new());
/// view! { <Input value=name placeholder="service name" width=InputWidth::Wide/> }
/// ```
#[component]
pub fn Input(
    /// Two-way binding. Omit for an uncontrolled field.
    #[prop(optional)]
    value: Option<RwSignal<String>>,
    /// The `type` attribute — `"text"`, `"number"`, `"password"`, `"search"`.
    #[prop(default = "text")]
    input_type: &'static str,
    #[prop(optional, into)] placeholder: String,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional)] width: InputWidth,
    /// The value is a machine value — a path, an id, a config value. Sets it in mono.
    #[prop(optional)]
    mono: bool,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let own = format!(
        "{FRAME} {} {}",
        if mono { MONO } else { "" },
        width.classes()
    );
    view! {
        <input
            class=merge(&own, class)
            type=input_type
            placeholder=placeholder
            disabled=move || disabled.get()
            prop:value=move || value.map(|v| v.get()).unwrap_or_default()
            on:input=move |ev| {
                if let Some(v) = value {
                    v.set(event_target_value(&ev));
                }
            }
        />
    }
}

/// A multi-line text field. Grows downward only — a sideways drag would fight the layout it
/// sits in, and every caller so far wanted more lines rather than more columns.
#[component]
pub fn Textarea(
    #[prop(optional)] value: Option<RwSignal<String>>,
    #[prop(default = 3)] rows: u32,
    #[prop(optional, into)] placeholder: String,
    #[prop(optional, into)] disabled: Signal<bool>,
    /// Wrap prose instead of keeping long lines intact. Off by default, which suits the
    /// config-ish content these usually hold (env vars, volumes, a command per line); on,
    /// it is the shape a message composer wants.
    #[prop(optional)]
    prose: bool,
    /// The content is machine text — a config, a command per line. Sets it in mono.
    #[prop(optional)]
    mono: bool,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let wrap = if prose {
        "whitespace-pre-wrap [overflow-wrap:anywhere]"
    } else {
        "whitespace-pre overflow-auto"
    };
    let own = format!(
        "{FRAME} {} block w-full resize-y align-top leading-normal [tab-size:2] {wrap}",
        if mono { MONO } else { "" }
    );
    view! {
        <textarea
            class=merge(&own, class)
            rows=rows
            placeholder=placeholder
            disabled=move || disabled.get()
            prop:value=move || value.map(|v| v.get()).unwrap_or_default()
            on:input=move |ev| {
                if let Some(v) = value {
                    v.set(event_target_value(&ev));
                }
            }
        ></textarea>
    }
}

/// A dropdown. Children are its `<option>`s.
///
/// It sizes to its content rather than to [`InputWidth::Default`], because a fixed width
/// clips the longest option behind the arrow. The arrow is drawn — a small chevron at the
/// right (§6) — so it is the same glyph on every platform.
///
/// ```ignore
/// view! {
///     <Select value=backend>
///         <option value="claude">"claude"</option>
///         <option value="codex">"codex"</option>
///     </Select>
/// }
/// ```
#[component]
pub fn Select(
    #[prop(optional)] value: Option<RwSignal<String>>,
    #[prop(optional, into)] disabled: Signal<bool>,
    /// The options are machine values — model ids, backends. Sets them in mono.
    #[prop(optional)]
    mono: bool,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let own = format!(
        "{FRAME} {} w-auto min-w-40 max-w-full cursor-pointer appearance-none pr-8",
        if mono { MONO } else { "" }
    );
    view! {
        <span class="relative block max-w-full">
            <select
                class=merge(&own, class)
                disabled=move || disabled.get()
                prop:value=move || value.map(|v| v.get()).unwrap_or_default()
                on:change=move |ev| {
                    if let Some(v) = value {
                        v.set(event_target_value(&ev));
                    }
                }
            >
                {children()}
            </select>
            <Icon
                icon=Lucide::ChevronDown
                size=IconSize::Sm
                class="pointer-events-none absolute top-1/2 right-2.5 -translate-y-1/2 text-ink-3"
            />
        </span>
    }
}
