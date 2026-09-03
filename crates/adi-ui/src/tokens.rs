//! [`TokenStream`] and [`PromptText`] — the two ways to read a prompt: as the tokens a model
//! is actually charged for, and as the string a person can still make sense of.
//!
//! Both take the *same* `Vec<Token>`. Nothing here tokenizes: the ranks live server-side
//! (`adi_agents::analytics`, tiktoken with its tables compiled in), and shipping a second copy
//! of them into the browser would cost more than the whole rest of this crate. So a caller
//! hands over the split it already has, exactly as [`PathPicker`](crate::PathPicker) is handed
//! a directory listing rather than reading one.
//!
//! # The newline problem
//!
//! A token is a run of bytes, and plenty of them carry a newline — `"\n"` alone, `"\n\n"`, or
//! a word with the break stuck on its end. Two obvious renderings are both wrong. Print the
//! newline literally and every chip after it starts a line, so a 6 000-token prompt becomes a
//! column one word wide. Swallow it and print `⏎` instead, and the text turns into a single
//! unreadable ribbon with no paragraphs, no code blocks, and no way to see the shape of what
//! the model was handed.
//!
//! So a newline is drawn **and** taken: a dim `⏎` marks where it is, and the break itself
//! still happens right after it. The glyph is `select-none`, so copying a stream out gives
//! back the original text and not a field of arrows.

use leptos::prelude::*;

use crate::merge;

/// One token: what the tokenizer produced, and the id it produced it as.
///
/// `special` marks the control tokens a chat template inserts — `<|im_start|>`,
/// `<|tool_call_begin|>` and their kin. They are worth their own colour because they are the
/// seams: everything between two of them is content somebody wrote, and everything on them is
/// the provider's wrapper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// The id in whatever encoding produced it. Shown on hover, never inferred here.
    pub id: u32,
    /// The token's exact text, newlines and leading spaces included.
    pub text: String,
    /// Whether this is a template control token rather than content.
    pub special: bool,
}

impl Token {
    /// A content token — the common case.
    #[must_use]
    pub fn new(id: u32, text: impl Into<String>) -> Self {
        Self {
            id,
            text: text.into(),
            special: false,
        }
    }

    /// A template control token.
    #[must_use]
    pub fn special(id: u32, text: impl Into<String>) -> Self {
        Self {
            id,
            text: text.into(),
            special: true,
        }
    }
}

/// The two surfaces a content token alternates between.
///
/// Alternated by position, not by anything about the token, because the only job the colour has
/// is to show where one token ends and the next begins — the boundary *is* the information, and
/// it is invisible in plain text. Two rather than a longer cycle: with two, *every* neighbouring
/// pair differs by the full step, which is the one thing a reader is looking for.
///
/// The ink token at two low alphas, rather than named surfaces, because the surface ladder
/// cannot do this job: its steps separate *large areas* and sit a few points apart, which as
/// adjacent chips is invisible. Ink over the raised surface gives a real step and, being one
/// token at two alphas, never implies this chip *means* something its neighbour does not.
/// Spelled as whole literals so Tailwind finds them.
const TINTS: [&str; 2] = ["bg-ink/10", "bg-ink/22"];

/// What a chip made of nothing but a break still needs: something to hold the colour open.
const BLANK: &str = "\u{00a0}";

/// The chip's children — the token's text with every newline shown, then taken.
fn body(text: &str) -> Vec<AnyView> {
    let mut out: Vec<AnyView> = Vec::new();
    for (i, piece) in text.split('\n').enumerate() {
        if i > 0 {
            // The arrow, then the real break. Both, in that order, is the whole trick.
            out.push(view! { <span class="text-ink-3 select-none">"⏎"</span>"\n" }.into_any());
        }
        if !piece.is_empty() {
            out.push(piece.to_string().into_any());
        }
    }
    // A token that was only a newline drew an arrow and nothing else; without a body the chip
    // collapses and its colour disappears along with the boundary it was there to show.
    if text.is_empty() {
        out.push(BLANK.into_any());
    }
    out
}

/// Every token in the prompt, drawn as its own chip.
///
/// Reads as text — spacing, indentation and paragraphs survive — while the alternating
/// backgrounds show where the splits fall. Hovering a chip names its id and its exact text,
/// which is the only way to see that a leading space belongs to the *next* word.
///
/// ```ignore
/// view! { <TokenStream tokens=Signal::derive(move || split.get())/> }
/// ```
#[component]
pub fn TokenStream(
    /// The tokens, in order. Reactive: a turn is added and the tail grows.
    #[prop(into)]
    tokens: Signal<Vec<Token>>,
    /// Extra classes — a height, a scroll box, a margin.
    #[prop(optional, into)]
    class: String,
) -> impl IntoView {
    let chips = move || {
        tokens
            .get()
            .into_iter()
            .enumerate()
            .map(|(i, t)| {
                // The template's own seams are the one kind of token that means something:
                // marked by weight and an underline, not by a colour the system reserves for
                // states.
                let tint = if t.special {
                    "bg-chip font-medium text-ink underline decoration-ink-3 underline-offset-2"
                } else {
                    TINTS[i % TINTS.len()]
                };
                view! {
                    <span
                        class=format!("rounded-sm {tint}")
                        title=format!("{} · {:?}", t.id, t.text)
                    >
                        {body(&t.text)}
                    </span>
                }
            })
            .collect::<Vec<_>>()
    };

    view! {
        // `pre-wrap` is what makes the break in `body` land, and what keeps the leading space
        // a token carries from being folded away by the browser.
        <div class=merge(
            "font-mono text-mono leading-[2.1] whitespace-pre-wrap break-words text-code",
            class,
        )>
            {chips}
        </div>
    }
}

/// The same tokens read as one string, with only the template's own control tokens marked.
///
/// This is the view for reading *what the agent was told* — the instructions, the tool
/// declarations, the turns — where per-token colour is noise. The seams stay coloured because
/// they are the one thing a reader cannot otherwise see: where the provider's wrapper stops
/// and the prompt somebody wrote begins.
#[component]
pub fn PromptText(
    /// The tokens, in order. Concatenated, they are the prompt exactly.
    #[prop(into)]
    tokens: Signal<Vec<Token>>,
    /// Extra classes — a height, a scroll box, a margin.
    #[prop(optional, into)]
    class: String,
) -> impl IntoView {
    // Runs of ordinary tokens are joined into one text node rather than one span each: a
    // 6 000-token prompt is 6 000 elements the browser has no reason to keep apart here.
    let runs = move || {
        let mut out: Vec<AnyView> = Vec::new();
        let mut plain = String::new();
        for t in tokens.get() {
            if t.special {
                if !plain.is_empty() {
                    out.push(std::mem::take(&mut plain).into_any());
                }
                out.push(
                    view! {
                        <span class="rounded-sm bg-chip px-0.5 font-medium text-ink underline decoration-ink-3 underline-offset-2">
                            {t.text}
                        </span>
                    }
                    .into_any(),
                );
            } else {
                plain.push_str(&t.text);
            }
        }
        if !plain.is_empty() {
            out.push(plain.into_any());
        }
        out
    };

    view! {
        <div class=merge(
            "font-mono text-mono leading-[1.6] whitespace-pre-wrap break-words text-code",
            class,
        )>
            {runs}
        </div>
    }
}
