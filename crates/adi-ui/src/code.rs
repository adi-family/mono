//! [`CodeEditor`] — a syntax-highlighted editor built out of two stacked layers —
//! [`CodeLog`], the read-only half of it that follows a growing file, and [`CodeFrame`], the
//! card either one sits in.

use leptos::prelude::*;

use crate::{
    highlight::{Lang, highlight},
    merge,
};

/// How much room the editor asks for.
///
/// A real height, not a hint: the editor is two absolutely-positioned layers, so it has
/// nothing of its own to be tall *from* — with no height it collapses to nothing and paints
/// no code at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodeHeight {
    /// Fill the pane. For an editor that *is* the screen — the file page.
    #[default]
    Fill,
    /// A generous fixed box, for an editor that is one field among several in a form.
    Form,
}

impl CodeHeight {
    /// The box this step asks for.
    #[must_use]
    pub fn classes(self) -> &'static str {
        match self {
            Self::Fill => "h-full min-h-0",
            Self::Form => "h-85",
        }
    }

    /// The same step for [`CodeLog`], which is one scrolling element rather than two stacked
    /// absolute ones — so it can be told a *ceiling* and take less when the output is short,
    /// where the editor has to be given a height outright or collapse to nothing.
    #[must_use]
    fn max_classes(self) -> &'static str {
        match self {
            Self::Fill => "h-full min-h-0",
            Self::Form => "max-h-85",
        }
    }
}

/// The card a file is read in: a name, room for controls beside it, and whatever is showing
/// the file underneath.
///
/// It is deliberately *not* part of [`CodeEditor`]. The same chrome has to sit over the
/// rendered half of a Markdown file, a diff, or a log — and a frame that belongs to the
/// editor can only ever hold the editor, which is how a screen ends up with two headers
/// that drift apart.
///
/// The name is monospace and in the case it was written in, because it is a path.
///
/// ```ignore
/// <CodeFrame
///     title=path
///     actions=move || view! { <Button size=ButtonSize::Small>"Preview"</Button> }.into_any()
/// >
///     <CodeEditor value=buffer lang=lang/>
/// </CodeFrame>
/// ```
#[component]
pub fn CodeFrame(
    /// The file's name or path. Reactive, because the file under it changes while the frame
    /// stays.
    #[prop(optional, into)]
    title: Signal<String>,
    /// Controls pinned to the right of the name: a preview toggle, a save, a language
    /// picker. Small [`crate::Button`]s — the strip is 36px and the name is 12px.
    #[prop(optional, into)]
    actions: Option<ViewFn>,
    #[prop(optional)] height: CodeHeight,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let has_head = actions.is_some() || !title.get_untracked().is_empty();

    view! {
        <div class=merge(
            // `overflow-hidden` because this island clips two children to its corners: the
            // header strip's fill, and whatever scrolls underneath it.
            &format!(
                "island flex flex-col overflow-hidden bg-card {}",
                height.classes(),
            ),
            class,
        )>
            {has_head.then(|| view! {
                <header class="flex min-h-9 shrink-0 items-center justify-between gap-2 \
                               border-b border-divider bg-bar px-3 py-1.5">
                    <span class="truncate font-mono text-mini text-meta">{move || title.get()}</span>
                    <div class="flex shrink-0 items-center gap-1">{actions.map(|a| a.run())}</div>
                </header>
            })}
            <div class="min-h-0 flex-1">{children()}</div>
        </div>
    }
}

/// Everything that decides where a glyph lands, applied to **both** layers exactly once.
///
/// This is the load-bearing constant in the file. The painted `<pre>` and the `<textarea>`
/// over it have to agree on font, size, line height, padding, tab size, wrapping and word
/// breaking down to the pixel; any divergence shows up as the caret drifting away from the
/// text it is supposed to be inside — a bug that looks like a rendering glitch and is
/// actually a stylesheet.
const LAYER: &str = "m-0 border-0 p-3 font-mono text-mini leading-[1.55] [tab-size:2] \
                     whitespace-pre-wrap [word-break:break-word] \
                     [overflow-wrap:break-word]";

/// A code editor over `value`: a highlighted view of the text with a real textarea on top
/// of it.
///
/// Two layers sharing one box — a painted `<pre>` underneath and a transparent-text
/// `<textarea>` above it that still owns the caret, the selection and the typing. The
/// browser does the editing; the paint only has to keep up, which it does by being dragged
/// along behind the textarea's scroll.
///
/// The alternative — a contenteditable — means owning the caret, undo, IME and paste by
/// hand. This owns none of them.
///
/// ```ignore
/// let buffer = RwSignal::new(String::new());
/// view! { <CodeEditor value=buffer lang=Lang::from_path("hive.yaml")/> }
/// ```
#[component]
pub fn CodeEditor(
    /// The text being edited, two-way.
    value: RwSignal<String>,
    /// What to scan it as. A signal, because the editor stays mounted while the file under
    /// it changes — and [`Lang::from_path`] is one call away from a path that already moves.
    #[prop(optional, into)]
    lang: Signal<Lang>,
    #[prop(optional)] height: CodeHeight,
    /// Read the code without editing it. The textarea stays — it is what carries selection
    /// and keyboard scrolling — but refuses input.
    #[prop(optional, into)]
    readonly: Signal<bool>,
    /// DOM id for the textarea — the editable element, so a `<label for=…>` points at the
    /// thing that actually takes focus rather than at the box around it.
    #[prop(optional, into)]
    id: String,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let area = NodeRef::<leptos::html::Textarea>::new();
    let paint = NodeRef::<leptos::html::Pre>::new();

    // Keep the painted layer on the textarea's scroll offset. The textarea is the one that
    // actually scrolls; the `<pre>` is dragged along behind it.
    let sync = move || {
        if let (Some(a), Some(p)) = (area.get_untracked(), paint.get_untracked()) {
            p.set_scroll_top(a.scroll_top());
            p.set_scroll_left(a.scroll_left());
        }
    };

    let painted = move || {
        let mut text = value.get();
        // A trailing newline paints as nothing, so the last line of the file would have no
        // ink under the caret sitting on it. One space gives that line something to be.
        if text.ends_with('\n') {
            text.push(' ');
        }
        highlight(lang.get(), &text)
            .into_iter()
            .map(|(tok, run)| view! { <span class=tok.classes()>{run}</span> })
            .collect::<Vec<_>>()
    };

    view! {
        // No border and no radius of its own: inside a `CodeFrame` those would draw a second
        // edge a pixel inside the first. A caller using the editor bare adds them through
        // `class`, where nothing of the component's own is there to fight.
        <div class=merge(
            &format!("relative overflow-hidden bg-card {}", height.classes()),
            class,
        )>
            <pre class=format!("{LAYER} absolute inset-0 overflow-auto text-syn-plain \
                                pointer-events-none")
                aria-hidden="true"
                node_ref=paint
            >
                {painted}
            </pre>
            // Above the paint, and invisible: the glyphs come from the layer underneath, but
            // the caret, the selection and every editing behaviour a browser has are still
            // this element's. `-webkit-text-fill-color` is the one Safari honours.
            <textarea
                class=format!("{LAYER} relative block size-full resize-none overflow-auto \
                               bg-transparent text-transparent caret-ink outline-none \
                               [-webkit-text-fill-color:transparent] selection:bg-meta/35")
                spellcheck="false"
                autocomplete="off"
                id=(!id.is_empty()).then_some(id)
                node_ref=area
                readonly=move || readonly.get()
                prop:value=move || value.get()
                on:scroll=move |_| sync()
                on:input=move |ev| {
                    value.set(event_target_value(&ev));
                    sync();
                }
            ></textarea>
        </div>
    }
}

/// A growing file, read: [`CodeEditor`]'s highlighting with none of its editing, following
/// the tail the way `tail -f` does.
///
/// **Not a read-only [`CodeEditor`].** That one is a textarea, and a textarea's scroll
/// position resets every time its `value` is written — which for output arriving under a
/// poll is every second, yanking the reader back to the top of a log they were reading.
/// This is a single scrolling `<pre>`: appending to it moves nothing on its own, so the
/// follow can be a decision rather than a side effect.
///
/// The decision is the reader's, and it is made by scrolling. Pinned to the bottom, new
/// lines pull the view down with them; scroll up and the follow stops, so history holds
/// still while the log keeps growing; scroll back to the end and it resumes. Nothing has to
/// be toggled, because scrolling away from the newest line *is* the statement that you are
/// reading something else.
///
/// ```ignore
/// <CodeLog value=output lang=Lang::Sh height=CodeHeight::Form/>
/// ```
#[component]
pub fn CodeLog(
    /// The text, which is expected to grow. Read-only here — the component never writes it.
    #[prop(into)]
    value: Signal<String>,
    /// What to scan it as (see [`CodeEditor`]'s `lang`).
    #[prop(optional, into)]
    lang: Signal<Lang>,
    #[prop(optional)] height: CodeHeight,
    /// DOM id, for a `<label for=…>` or an anchor.
    #[prop(optional, into)]
    id: String,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let pre = NodeRef::<leptos::html::Pre>::new();
    // Whether the view is pinned to the newest line. Starts pinned, so a log opens on what
    // just happened rather than on how it began.
    let follow = RwSignal::new(true);

    // Follow the tail. Reading `value` is what subscribes this to the growth — and it also
    // makes the effect run on mount, which is what lands a log that arrived complete on its
    // last line instead of its first.
    Effect::new(move |_| {
        let _ = value.get();
        if follow.get_untracked()
            && let Some(p) = pre.get_untracked()
        {
            p.set_scroll_top(p.scroll_height());
        }
    });

    view! {
        <pre
            class=merge(
                &format!(
                    "{LAYER} overflow-auto bg-card text-syn-plain {}",
                    height.max_classes(),
                ),
                class,
            )
            id=(!id.is_empty()).then_some(id)
            node_ref=pre
            // Focusable, so the log can be scrolled from the keyboard like any other pane.
            tabindex="0"
            on:scroll=move |_| {
                if let Some(p) = pre.get_untracked() {
                    // Within a couple of lines of the end still counts as following: a
                    // browser's own scroll maths lands a pixel or two short of the bottom
                    // often enough that exact equality would unpin a log nobody moved.
                    let dist = p.scroll_height() - p.scroll_top() - p.client_height();
                    follow.set(dist <= 24);
                }
            }
        >
            {move || {
                highlight(lang.get(), &value.get())
                    .into_iter()
                    .map(|(tok, run)| view! { <span class=tok.classes()>{run}</span> })
                    .collect::<Vec<_>>()
            }}
        </pre>
    }
}
