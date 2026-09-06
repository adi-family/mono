//! The face pile — who is here, as a row of overlapping circles, and one more circle counting
//! the rest.
//!
//! A pile is a **glance**, not a list: it answers "who is around?" in the width of a few
//! characters, and the names themselves are a hover away. That is why the tail folds into a
//! `+N` rather than wrapping onto a second line — a strip that grows with the fleet costs the
//! column height every day to say something that is read once.
//!
//! **Colour is the state, and colour is the identity.** A face that is on is one of the six
//! `--face-*` fills, picked from its own name so it is the same colour everywhere; a face that is
//! not on is `--bg-active` grey. Two solid colours, not one fill at two strengths and never a
//! translucent one: on a dark rail, dimming is what makes a pile look switched off rather than
//! quiet, and at 24px the eye reads hue long before it reads brightness.
//!
//! The pile says who; it does not say how many of them are on. Put that in a line beside it — the
//! caller knows whether "2 active now" or "hetzner active now" is the sentence it wants.
//!
//! The order is the caller's. Whoever should survive the fold goes first, because the fold takes
//! the tail.

use leptos::prelude::*;

use crate::merge;

/// The surface a pile sits on.
///
/// Circles overlap, and the sliver that separates one from the next is drawn in the surface
/// underneath — so a pile with the wrong one set shows a ring of the wrong grey rather than a
/// clean edge. There is no "transparent" option on purpose: the separation *is* the component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FaceRing {
    /// A rail, a bar, the right panel — where a pile usually lives.
    #[default]
    Side,
    /// The page or the transcript.
    Bg,
    /// An input, a code block, a composer.
    Raise,
}

impl FaceRing {
    /// The ring colour for this surface.
    #[must_use]
    pub fn classes(self) -> &'static str {
        match self {
            Self::Side => "ring-side",
            Self::Bg => "ring-bg",
            Self::Raise => "ring-raise",
        }
    }
}

/// One face: a name, whether it is on right now, and what a hover should say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Face {
    /// Who or what the circle stands for. Its first letter is what the circle draws, and its
    /// bytes are what picks the colour — so one name is always the same colour, on every machine
    /// that draws it.
    pub name: String,
    /// On right now, which is the whole of what the colour says: on is one of the six face
    /// fills, off is `--bg-active` grey. Never a dimmer copy of the same fill — a pile read at
    /// 24px cannot carry a difference that fine, and translucency over a rail is muddier still.
    pub online: bool,
    /// What a hover says, when the name alone is not the whole of it ("hetzner — active now").
    /// Empty falls back to the name.
    pub note: String,
}

impl Face {
    /// A face with no hover sentence of its own.
    #[must_use]
    pub fn new(name: impl Into<String>, online: bool) -> Self {
        Self {
            name: name.into(),
            online,
            note: String::new(),
        }
    }

    /// What a hover says instead of the bare name.
    #[must_use]
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = note.into();
        self
    }

    /// The letter drawn in the circle. `·` for a name that starts with nothing drawable, which
    /// is what [`crate::AppItem`]'s mark does with the same problem.
    fn initial(&self) -> String {
        self.name
            .chars()
            .next()
            .unwrap_or('\u{00b7}')
            .to_uppercase()
            .to_string()
    }

    /// The hover sentence: the note when there is one, the name otherwise.
    fn title(&self) -> String {
        if self.note.is_empty() {
            self.name.clone()
        } else {
            self.note.clone()
        }
    }

    /// Fill and letter for this face.
    ///
    /// One of the six `--face-*` colours while it is on, `--bg-active` while it is not — colour
    /// against grey, rather than one fill at two strengths, because the second is the thing a
    /// 24px circle on a dark rail cannot say (and the whole pile then reads as washed out).
    ///
    /// Which colour is a hash of the name, so it is stable for a given name on every machine that
    /// draws it, and spread across the six. Whole class strings per arm, never assembled:
    /// Tailwind scans this file as text and generates only what it can see (see the crate's own
    /// docs).
    fn tone(&self) -> &'static str {
        if !self.online {
            return "bg-active text-ink-2";
        }
        // FNV-1a, not a sum of the bytes: `hetzner` and `laptop` add up to the same number, and
        // two machines wearing one colour is the whole point of the palette lost.
        let hash = self.name.bytes().fold(0x811c_9dc5_u32, |h, b| {
            (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
        });
        match hash % 6 {
            0 => "bg-face-1 text-on-face",
            1 => "bg-face-2 text-on-face",
            2 => "bg-face-3 text-on-face",
            3 => "bg-face-4 text-on-face",
            4 => "bg-face-5 text-on-face",
            _ => "bg-face-6 text-on-face",
        }
    }
}

/// The pile: a circle per face, overlapping, and a `+N` for everyone past `max`.
///
/// **The first face is drawn on top**, not the last, which is the one thing to know before
/// changing this. The caller's order is significance — on now, then whoever else — so the face
/// that matters most is the one circle drawn whole, and every one after it is a crescent of its
/// own colour. Stack it the other way and the pile's most important member is the one under
/// everybody.
///
/// The counter stands slightly apart rather than overlapping, because it is a different kind of
/// thing from the circles beside it — a number, not somebody — and a `+3` half under a neighbour
/// is a number nobody can read. Its hover names everyone it stands for.
///
/// ```ignore
/// <Faces faces=vec![Face::new("hetzner", true), Face::new("studio", false)] max=5/>
/// ```
#[component]
pub fn Faces(
    /// The pile, in the order it should read. The caller sorts it — who comes first (on now,
    /// most senior, most recently seen) is never this component's to decide — and the fold
    /// takes whatever ends up past `max`.
    faces: Vec<Face>,
    /// How many circles may be drawn, the `+N` counting as one of them. Five is what a 272px
    /// rail holds without the pile becoming the widest thing in it.
    #[prop(default = 5)]
    max: usize,
    /// The surface underneath, which is what the gap between two circles is drawn in.
    #[prop(optional)]
    ring: FaceRing,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    // One circle is the floor: a pile of zero has nothing to say, and `max - 1` below must not
    // wrap around.
    let max = max.max(1);
    let mut faces = faces;
    let folded = if faces.len() > max {
        // The counter is one of the `max`, so the last circle before it is `max - 1`.
        faces.split_off(max - 1)
    } else {
        Vec::new()
    };
    let ring = ring.classes();
    // Descending, so the first circle paints over the second rather than under it.
    let depth = faces.len();

    let circles = faces
        .into_iter()
        .enumerate()
        .map(|(i, face)| {
            let tone = face.tone();
            // Everything but the first is pulled 8px into the one before it.
            let overlap = if i == 0 { "" } else { "-ml-2" };
            let title = face.title();
            view! {
                <span
                    class=format!(
                        "inline-grid size-6 shrink-0 place-items-center rounded-full \
                         ring-2 text-mini font-medium select-none {ring} {tone} {overlap}",
                    )
                    style=format!("z-index:{}", depth - i)
                    role="img"
                    title=title.clone()
                    aria-label=title
                >
                    {face.initial()}
                </span>
            }
        })
        .collect::<Vec<_>>();

    let counter = (!folded.is_empty()).then(|| {
        let names = folded
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let title = format!("{} more: {names}", folded.len());
        view! {
            <span
                class="ml-1 inline-grid size-6 shrink-0 place-items-center rounded-full \
                       bg-active text-mini tabular-nums text-ink-3 select-none"
                role="img"
                title=title.clone()
                aria-label=title
            >
                {format!("+{}", folded.len())}
            </span>
        }
    });

    view! {
        <div class=merge("flex items-center", class)>
            {circles}
            {counter}
        </div>
    }
}
