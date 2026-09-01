//! [`ToolForm`] — a tool's parameters as a form, built from the tool's own declaration.
//!
//! An agent's tools describe themselves in JSON Schema, and that description is the only
//! honest source for what a call may contain: hand-writing a form per tool means the day a
//! parameter is added the form is quietly wrong. So the caller decodes the schema into
//! [`Param`]s and this renders them — the same division [`PathPicker`](crate::PathPicker)
//! makes, where the component names what it wants and never goes and gets it.
//!
//! It deliberately does **not** build the call's JSON. A call's wire shape belongs to whoever
//! is going to send it, and a component that guessed at it would be a second, drifting copy of
//! the runner's own serialization. [`Param::text`] and [`Param::flag`] are the values; the
//! caller reads them and composes.

use leptos::prelude::*;

use crate::{Field, Input, InputWidth, Textarea, merge};

/// The control a parameter gets, chosen from its declared type.
///
/// Five, not one per JSON-Schema type, because the distinction that matters to somebody
/// filling a form is how much room the value needs and whether it is typed at all — a
/// `string` holding a shell command and a `string` holding a filename are not the same
/// control, and no schema type tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParamKind {
    /// A single line — a path, a pattern, a name.
    #[default]
    Line,
    /// Something with body to it: a command, a file's contents, a question.
    Text,
    /// `integer` or `number`.
    Number,
    /// An array of scalars, written one per line.
    List,
    /// `boolean`. The only kind that reads [`Param::flag`] instead of [`Param::text`].
    Flag,
}

impl ParamKind {
    /// Whether this control wants the whole row rather than a column of the grid.
    #[must_use]
    fn wide(self) -> bool {
        matches!(self, Self::Text | Self::List)
    }
}

/// One parameter of a tool, and the value being typed into it.
///
/// The value lives in signals the caller owns, so a form can be pre-filled, cleared, or read
/// without this component having an opinion about any of it.
#[derive(Debug, Clone)]
pub struct Param {
    /// The parameter's name, exactly as the tool declares it — it is what goes on the wire.
    pub name: String,
    /// The control it gets.
    pub kind: ParamKind,
    /// Whether the tool requires it. Shown, never enforced: a call that omits a required
    /// parameter is a real thing a model does, and seeing the tool refuse is the lesson.
    pub required: bool,
    /// The schema's own description, shown as the field's hint.
    pub hint: String,
    /// What an empty field suggests. A placeholder, so it is never mistaken for a value.
    pub placeholder: String,
    /// The typed value, for every kind but [`ParamKind::Flag`].
    pub text: RwSignal<String>,
    /// The checked state, for [`ParamKind::Flag`].
    pub flag: RwSignal<bool>,
}

impl Param {
    /// A parameter with empty signals of its own.
    #[must_use]
    pub fn new(name: impl Into<String>, kind: ParamKind) -> Self {
        Self {
            name: name.into(),
            kind,
            required: false,
            hint: String::new(),
            placeholder: String::new(),
            text: RwSignal::new(String::new()),
            flag: RwSignal::new(false),
        }
    }

    /// Mark it required.
    #[must_use]
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// The schema's description.
    #[must_use]
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    /// What to suggest in the empty field.
    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// The label: the name, a mark if it is required, and the shape it wants.
    fn label(&self) -> String {
        let kind = match self.kind {
            ParamKind::Line | ParamKind::Text => "string",
            ParamKind::Number => "number",
            ParamKind::List => "string[]",
            ParamKind::Flag => "boolean",
        };
        format!(
            "{}{}  {kind}",
            self.name,
            if self.required { " *" } else { "" }
        )
    }
}

/// A tool's parameters, laid out as a form.
///
/// Narrow controls share a row and wide ones take their own, so a tool whose only parameter is
/// a shell command does not get a one-line box in a three-column grid, and one with four paths
/// does not become a tall column of nearly-empty fields.
///
/// ```ignore
/// let params = vec![
///     Param::new("command", ParamKind::Text).required().hint("The command line to run."),
///     Param::new("background", ParamKind::Flag).hint("Return a job id instead of waiting."),
/// ];
/// view! { <ToolForm params=params/> }
/// ```
#[component]
pub fn ToolForm(
    /// The parameters, in the order the tool declares them — which is the order it expects
    /// them to be considered in.
    params: Vec<Param>,
    /// Extra classes — a margin, a width.
    #[prop(optional, into)]
    class: String,
) -> impl IntoView {
    let fields = params
        .into_iter()
        .map(|p| {
            let span = if p.kind.wide() { "sm:col-span-2" } else { "" };
            let control = match p.kind {
                ParamKind::Flag => {
                    let flag = p.flag;
                    view! {
                        <label class="flex cursor-pointer items-center gap-2 text-row \
                                      text-secondary">
                            <input
                                type="checkbox"
                                class="size-3.5 accent-accent"
                                prop:checked=move || flag.get()
                                on:change=move |ev| flag.set(event_target_checked(&ev))
                            />
                            {p.hint.clone()}
                        </label>
                    }
                    .into_any()
                }
                ParamKind::Text | ParamKind::List => view! {
                    <Textarea
                        value=p.text
                        rows=3
                        placeholder=p.placeholder.clone()
                    />
                }
                .into_any(),
                ParamKind::Number => view! {
                    <Input
                        value=p.text
                        input_type="number"
                        width=InputWidth::Num
                        placeholder=p.placeholder.clone()
                    />
                }
                .into_any(),
                ParamKind::Line => view! {
                    <Input value=p.text placeholder=p.placeholder.clone()/>
                }
                .into_any(),
            };

            // A flag says what it is next to the box, so repeating the hint above it would
            // print the same sentence twice.
            let hint = if p.kind == ParamKind::Flag {
                String::new()
            } else {
                p.hint.clone()
            };
            view! {
                <div class=span>
                    <Field label=p.label() hint=hint>
                        {control}
                    </Field>
                </div>
            }
        })
        .collect::<Vec<_>>();

    view! {
        <div class=merge("grid grid-cols-1 gap-3 sm:grid-cols-2", class)>{fields}</div>
    }
}
