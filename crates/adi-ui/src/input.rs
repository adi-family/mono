//! Text entry: [`Input`], [`Textarea`] and [`Select`], which all wear the same frame.

use leptos::prelude::*;

use crate::merge;

/// The shared frame for anything you type into — the box and its states, but not the type
/// it sets, so a control that wants different words in it (the sessions filter, which takes
/// prose rather than a value) can wear the same frame without fighting a `font-mono` it
/// cannot outrank.
///
/// Two details here are load-bearing rather than taste:
///
/// - **Focus is a border plus a soft ring, not an outline.** An outline is drawn outside the
///   box and shifts nothing, but at these sizes it reads as a second border; the accent
///   border with a 3px tinted halo says "here" without the visual jump. Buttons keep the
///   plain outline ring — they are not text entry and never sit shoulder to shoulder.
/// - **16px below 620px.** iOS zooms the page in when it focuses a field with text under
///   16px, and it does not zoom back out: one tap and the layout is wider than the screen
///   with no obvious gesture to undo it. adi's type scale tops out at 13.5px, so every
///   field in the system needs this, not just the big ones.
pub(crate) const FRAME: &str = "min-w-0 rounded-sm border border-edge bg-card px-2 py-[5px] \
                     text-body placeholder:text-placeholder \
                     transition-[border-color,box-shadow] duration-100 \
                     focus-visible:border-accent focus-visible:outline-none \
                     focus-visible:shadow-[0_0_0_3px_color-mix(in_srgb,var(--accent)_16%,transparent)] \
                     disabled:cursor-not-allowed disabled:opacity-50 \
                     max-[620px]:text-[16px]";

/// [`FRAME`] plus the type the three controls in this file share: mono, because what goes
/// in them is a value — a port, a path, a flag — and mono is what the design system sets
/// those in.
const FIELD: &str = "font-mono text-mini";

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
            Self::Default => "w-35 max-w-full",
            Self::Wide => "w-full",
            Self::Num => "w-18",
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
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let own = format!("{FRAME} {FIELD} {}", width.classes());
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
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let wrap = if prose {
        "whitespace-pre-wrap [overflow-wrap:anywhere]"
    } else {
        "whitespace-pre overflow-auto"
    };
    let own = format!(
        "{FRAME} {FIELD} block w-full resize-y align-top leading-relaxed [tab-size:2] {wrap}"
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
/// clips the longest option behind the platform's own arrow. The arrow stays native — the
/// `color-scheme` the design tokens set is what keeps it on-theme, so there is nothing to
/// draw and nothing to keep in sync.
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
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let own = format!("{FRAME} {FIELD} w-auto min-w-[150px] max-w-full cursor-pointer");
    view! {
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
    }
}
