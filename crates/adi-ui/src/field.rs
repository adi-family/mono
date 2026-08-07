//! [`Field`] — a control with a label over it, and optionally an explanation that costs the
//! form no room until it is asked for.

use leptos::prelude::*;

use crate::merge;

/// A labelled control.
///
/// Laid out as a grid rather than a stack so the hint can sit *beside* the label while the
/// control below still spans the full width. The payoff is that a hint is written last, in
/// the call where it reads naturally, and still lands next to the title.
///
/// ```ignore
/// <Field label="Port" hint="Left blank, the registry picks a free one.">
///     <Input value=port width=InputWidth::Num input_type="number"/>
/// </Field>
/// ```
#[component]
pub fn Field(
    #[prop(optional, into)] label: String,
    /// One line explaining the control, collapsed behind a `?` next to the label. Keeping
    /// the words out of the form until they are wanted is what lets a dense row of fields
    /// stay readable — and it stops a long hint from pushing the layout around.
    #[prop(optional, into)]
    hint: String,
    /// Absorb the free space on a wrapping [`crate::Form`] row. For the one field the form
    /// is really about; the rest keep their natural width.
    #[prop(optional)]
    grow: bool,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let head = !label.is_empty() || !hint.is_empty();
    let own = if grow {
        "grid min-w-0 flex-1 basis-60 grid-cols-[auto_minmax(0,1fr)] items-center gap-1"
    } else {
        "grid min-w-0 grid-cols-[auto_minmax(0,1fr)] items-center gap-1"
    };

    view! {
        <div class=merge(own, class)>
            {head.then(|| view! {
                <span class="caps col-start-1 row-start-1 text-faint">
                    {label}
                </span>
                {(!hint.is_empty()).then(|| view! {
                    <HintBubble text=hint/>
                })}
            })}
            <div class="col-span-2 min-w-0">{children()}</div>
        </div>
    }
}

/// The `?` bubble and the words under it. Opens on hover *and* on keyboard focus — hence
/// `tabindex`, without which the explanation would be mouse-only.
///
/// The panel is anchored below and slightly left so it opens into the form's own body
/// rather than off its top or right edge, and it is hidden with `invisible` rather than
/// `hidden` so the fade has something to animate.
#[component]
fn HintBubble(text: String) -> impl IntoView {
    view! {
        <span
            class="group relative col-start-2 row-start-1 inline-grid size-3.5 cursor-help \
                   place-items-center justify-self-start rounded-full border border-dim \
                   text-[9px] font-bold leading-none text-faint transition-colors \
                   duration-100 hover:border-accent hover:text-accent \
                   focus-visible:border-accent focus-visible:text-accent \
                   focus-visible:outline-none"
            tabindex="0"
            role="note"
        >
            "?"
            <span class="invisible absolute top-[calc(100%+6px)] -left-1.5 z-30 w-max \
                         max-w-65 rounded-sm border border-edge bg-bar px-2.5 py-1.5 \
                         text-left font-sans text-mini leading-normal font-normal \
                         tracking-normal normal-case text-body opacity-0 \
                         shadow-[var(--shadow)] \
                         transition-[opacity,visibility] duration-100 \
                         group-hover:visible group-hover:opacity-100 \
                         group-focus-visible:visible group-focus-visible:opacity-100">
                {text}
            </span>
        </span>
    }
}
