//! The button — and the reference for how a component in this crate is shaped: an enum per
//! axis of variation, each arm owning one complete class literal, and the assembled string
//! handed to a single element.

use leptos::prelude::*;

use crate::merge;

/// What the button is *for*, which is the only thing that decides how it looks. Weight is
/// spent on the one action a screen wants; everything else recedes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    /// The default action on a surface: bordered, on the surface colour.
    #[default]
    Default,
    /// The one action a view exists for. At most one per screen.
    Primary,
    /// Present but quiet — no border until you point at it.
    Ghost,
    /// Destructive. Reads as text until hover, so it never competes for the eye.
    Danger,
    /// Reads as a link, behaves as a button. For inline actions inside prose or a table cell.
    Link,
}

impl ButtonVariant {
    /// The complete class list for this variant — written out per arm rather than composed,
    /// because Tailwind only finds classes it can read as literals in the source.
    #[must_use]
    pub fn classes(self) -> &'static str {
        match self {
            Self::Default => {
                "border border-edge bg-card text-body hover:bg-bubble hover:text-ink \
                 active:bg-selected"
            }
            Self::Primary => {
                "border border-transparent bg-accent-fill text-on-accent hover:opacity-90 \
                 active:opacity-80"
            }
            Self::Ghost => {
                "border border-transparent bg-transparent text-meta hover:bg-card \
                 hover:text-ink"
            }
            Self::Danger => {
                "border border-transparent bg-transparent text-err hover:bg-err-bg-2 \
                 active:bg-err-bg-2"
            }
            Self::Link => {
                "border border-transparent bg-transparent px-0 text-accent hover:underline"
            }
        }
    }
}

/// Button height. The dense default matches the rest of the panel furniture; `Small` is for
/// controls that sit inside a table row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonSize {
    /// Table-row scale.
    Small,
    /// The default.
    #[default]
    Medium,
}

impl ButtonSize {
    /// Padding and type size for this step.
    #[must_use]
    pub fn classes(self) -> &'static str {
        match self {
            Self::Small => "h-6 gap-1 px-2 text-mini",
            Self::Medium => "h-7 gap-2 px-3 text-row",
        }
    }

    /// The glyph's box at this step — a shade under the type size, so an icon reads as part
    /// of the label rather than as a second thing next to it.
    #[must_use]
    pub fn icon_classes(self) -> &'static str {
        match self {
            Self::Small => "size-3 shrink-0",
            Self::Medium => "size-3.5 shrink-0",
        }
    }
}

/// Shared by every variant: the box, the type, and the states. Focus uses the same ring the
/// design system puts on `:focus-visible`, so a button focused by keyboard looks like every
/// other focused control on the page.
const BASE: &str = "inline-flex items-center justify-center whitespace-nowrap rounded-sm \
                    font-medium transition-[background-color,opacity,color] duration-100 \
                    cursor-pointer select-none \
                    focus-visible:outline-2 focus-visible:outline-offset-2 \
                    focus-visible:outline-accent \
                    disabled:cursor-not-allowed disabled:opacity-50";

/// A button.
///
/// Event handlers attach the ordinary Leptos way — `<Button on:click=…>` lands on the
/// underlying `<button>`, so this component has no callback prop of its own.
///
/// ```ignore
/// <Button variant=ButtonVariant::Danger size=ButtonSize::Small on:click=drop_it>
///     "Release"
/// </Button>
/// ```
#[component]
pub fn Button(
    #[prop(optional)] variant: ButtonVariant,
    #[prop(optional)] size: ButtonSize,
    /// Rendered as `type="submit"` — the one form-related behaviour worth a prop, since a
    /// bare `<button>` inside a `<form>` submits it whether you meant it to or not.
    #[prop(optional)]
    submit: bool,
    #[prop(optional, into)] disabled: Signal<bool>,
    /// A glyph before the label, as the inner markup of a 16×16 `<svg>` — the same shape
    /// [`crate::TreeNode::icon`] takes, so one set of paths serves both.
    ///
    /// It is drawn in `currentColor` at a size the button picks, which is what keeps an
    /// icon button on-theme through every variant and both themes without the call site
    /// knowing anything. A button with an icon and no children is a square icon button;
    /// give it an `aria-label` when it has no words.
    #[prop(optional)]
    icon: Option<&'static str>,
    /// Extra utilities from the call site: layout, width, margin.
    #[prop(optional, into)]
    class: String,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let own = format!("{BASE} {} {}", variant.classes(), size.classes());
    view! {
        <button
            class=merge(&own, class)
            type=if submit { "submit" } else { "button" }
            disabled=move || disabled.get()
        >
            {icon.map(|markup| view! {
                <svg
                    class=size.icon_classes()
                    viewBox="0 0 16 16"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.5"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    aria-hidden="true"
                    inner_html=markup
                ></svg>
            })}
            {children.map(|c| c())}
        </button>
    }
}
