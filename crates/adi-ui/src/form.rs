//! [`Form`] — the strip of controls under a section's content.

use leptos::prelude::*;

use crate::merge;

/// A horizontal strip of controls, ruled off from what is above it by a hairline.
///
/// Fields align on their **inputs**, not their labels, so a row mixing labelled and bare
/// controls still lines up along the one edge the eye follows.
///
/// Below 620px every child takes a row of its own. Wrapping alone is not enough, and the
/// run bar is where you see why: a `flex-1` composer gives up its own width instead of
/// pushing the next control onto a new line, and a three-control strip becomes three
/// unusable slivers. A full basis makes them wrap for real.
///
/// ```ignore
/// <Form>
///     <Field label="Name" grow=true><Input value=name width=InputWidth::Wide/></Field>
///     <Button variant=ButtonVariant::Primary submit=true>"Create"</Button>
/// </Form>
/// ```
#[component]
pub fn Form(
    /// The same strip used as a toolbar: bare controls with no labels above them, so they
    /// centre on each other instead of hanging from a baseline that is not there. Toolbars
    /// are also exempt from the stack-on-mobile rule — stacking a row of small buttons
    /// turns one strip into a column the height of the screen.
    #[prop(optional)]
    toolbar: bool,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let own = if toolbar {
        "flex flex-wrap items-center justify-start gap-2 border-t border-line pt-3"
    } else {
        "flex flex-wrap items-end gap-2 border-t border-line pt-3 \
         max-[620px]:*:w-full max-[620px]:*:min-w-0 max-[620px]:*:flex-[1_1_100%]"
    };
    view! { <div class=merge(own, class)>{children()}</div> }
}

/// Help text under a form or a section — the written-out version of what a
/// [`crate::Field`]'s hint says in one line. 13px `--ink-3`, 64ch (§4).
#[component]
pub fn Hint(#[prop(optional, into)] class: String, children: Children) -> impl IntoView {
    view! {
        <p class=merge("m-0 max-w-[64ch] pt-2 text-small leading-normal text-ink-3 [&_code]:font-mono", class)>
            {children()}
        </p>
    }
}
