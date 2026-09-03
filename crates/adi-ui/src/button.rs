//! The button — and the reference for how a component in this crate is shaped: an enum per
//! axis of variation, each arm owning one complete class literal, and the assembled string
//! handed to a single element.

use leptos::prelude::*;

use crate::icon::{Icon, IconSize, Lucide};
use crate::merge;

/// What the button is *for*, which is the only thing that decides how it looks.
///
/// `design/DESIGN.md` §2.4: one filled orange per screen. [`Primary`](Self::Primary) is that
/// orange and a screen gets one; when the orange is already spent — an update button in the
/// bar, a running dot the page is about — the page's main action is [`Strong`](Self::Strong),
/// an ink fill. Everything else recedes into the translucent default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    /// Most buttons: a translucent fill on whatever surface it sits on.
    #[default]
    Default,
    /// The one action a screen exists for — Send, Save, Update. Orange, and one per screen.
    Primary,
    /// The page's main action when orange is taken: ink on the page.
    Strong,
    /// Present but quiet — no fill until you point at it. Cancel, tertiary controls.
    Ghost,
    /// Destructive. Red text on the default fill; the word says the rest.
    Danger,
    /// Reads as a link, behaves as a button. For an action inside prose or a meta line. Never
    /// orange: links inside the app take the ink around them (§3).
    Link,
}

impl ButtonVariant {
    /// The complete class list for this variant — written out per arm rather than composed,
    /// because Tailwind only finds classes it can read as literals in the source.
    #[must_use]
    pub fn classes(self) -> &'static str {
        match self {
            Self::Default => "bg-btn text-ink hover:bg-btn-hover",
            Self::Primary => "bg-accent text-on-accent hover:bg-accent-hover",
            Self::Strong => "bg-ink text-bg hover:bg-white",
            Self::Ghost => "bg-transparent text-ink-2 hover:bg-hover hover:text-ink",
            Self::Danger => "bg-btn text-err hover:bg-btn-hover",
            Self::Link => {
                "bg-transparent px-0 text-ink-2 underline decoration-ink-3 \
                 underline-offset-[3px] hover:text-ink"
            }
        }
    }
}

/// Button height. The default is the spec's `7px 14px`; `Small` is for a control that sits
/// inside a table row or a 48px bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonSize {
    /// Row scale: `5px 10px`, 13px.
    Small,
    /// The default: `7px 14px`, 13.5px.
    #[default]
    Medium,
}

impl ButtonSize {
    /// Padding and type size for this step.
    #[must_use]
    pub fn classes(self) -> &'static str {
        match self {
            Self::Small => "gap-1 px-2.5 py-[5px] text-small",
            Self::Medium => "gap-1.5 px-3.5 py-[7px] text-row",
        }
    }

    /// The glyph at this step: 14 in a row-scale button, 16 otherwise (DESIGN.md §9).
    #[must_use]
    pub fn icon_size(self) -> IconSize {
        match self {
            Self::Small => IconSize::Sm,
            Self::Medium => IconSize::Md,
        }
    }
}

/// Shared by every variant: the box, the type, and the states. Focus is the same quiet ring
/// the design system puts on every `:focus-visible` — never orange (§3).
const BASE: &str = "inline-flex items-center justify-center whitespace-nowrap rounded-md \
                    font-medium leading-[1.2] transition-[background-color,color] duration-100 \
                    cursor-pointer select-none \
                    focus-visible:outline-[1.5px] focus-visible:outline-offset-2 \
                    focus-visible:outline-focus \
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
    /// A glyph before the label. Drawn in `currentColor` at the size the button picks, which
    /// is what keeps an icon button on-theme through every variant without the call site
    /// knowing anything. A button with an icon and no children is a square icon button; give
    /// it an `aria-label` when it has no words.
    #[prop(optional)]
    icon: Option<Lucide>,
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
            {icon.map(|icon| view! { <Icon icon=icon size=size.icon_size()/> })}
            {children.map(|c| c())}
        </button>
    }
}
