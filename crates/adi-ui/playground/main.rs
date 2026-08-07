//! The adi-ui playground — every component on one page, in both themes, with Trunk
//! hot-reloading it as you type.
//!
//! It is a dev surface: nothing embeds it and nothing depends on it. Its job is to make a
//! component's states visible all at once — a variant you never render is a variant you
//! never notice is broken — so when you add a component here, add a row that shows *every*
//! arm of every enum it takes, not just the default one.
//!
//! The first two panels are the design system itself: the type scale and the palette,
//! rendered from the live tokens. If a value in `styles/tokens.css` is wrong, it is wrong
//! there on screen rather than three components deep.
//!
//! ```sh
//! cd crates/adi-ui && trunk serve --open      # http://127.0.0.1:9081
//! ```

#![allow(non_snake_case)]

use adi_ui::{
    Badge, BadgeTone, Button, ButtonSize, ButtonVariant, Empty, Field, Flash, FlashKind, Form,
    Hint, Input, InputWidth, Panel, Select, SessionGroup, SessionItem, SessionList, SessionRollup,
    SessionState, Textarea,
};
use leptos::prelude::*;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(Playground);
}

/// Every variant of one component, under a caps label. The gallery is a list of these.
#[component]
fn Row(label: &'static str, children: Children) -> impl IntoView {
    view! {
        <div class="flex flex-wrap items-center gap-3 border-b border-divider py-3 last:border-b-0">
            <span class="caps w-28 shrink-0 text-faint">{label}</span>
            {children()}
        </div>
    }
}

/// The type scale, one row per step: what it is called, what it is for, and what it looks
/// like at that size.
#[component]
fn TypeSpecimen() -> impl IntoView {
    // Literal class names — Tailwind reads these out of the source, so they cannot be built.
    let steps = [
        ("text-caps", "caps labels", "text-caps caps"),
        ("text-mini", "meta, secondary", "text-mini"),
        ("text-row", "list rows, buttons", "text-row"),
        ("text-msg", "chat body", "text-msg"),
        ("text-sub", "answer subheading", "text-sub"),
        ("text-title", "screen titles", "text-title"),
    ];
    view! {
        <div class="flex flex-col">
            {steps
                .map(|(name, role, class)| view! {
                    <div class="flex items-baseline gap-4 border-b border-divider py-2.5 last:border-b-0">
                        <span class="w-24 shrink-0 font-mono text-mini text-accent">{name}</span>
                        <span class="w-40 shrink-0 text-mini text-meta">{role}</span>
                        <span class=format!("{class} text-ink")>"Agent finished the run"</span>
                    </div>
                })
                .into_iter()
                .collect::<Vec<_>>()}
            <div class="flex items-baseline gap-4 py-2.5">
                <span class="w-24 shrink-0 font-mono text-mini text-accent">"metric"</span>
                <span class="w-40 shrink-0 text-mini text-meta">"metric numbers"</span>
                <span class="metric text-ink">"1,284"</span>
            </div>
        </div>
    }
}

/// A swatch grid for one family of tokens.
#[component]
fn Swatches(label: &'static str, items: Vec<(&'static str, &'static str)>) -> impl IntoView {
    view! {
        <div class="border-b border-divider py-3 last:border-b-0">
            <div class="caps mb-2 text-faint">{label}</div>
            <div class="flex flex-wrap gap-2">
                {items
                    .into_iter()
                    .map(|(name, class)| view! {
                        <div class="flex items-center gap-2 rounded-sm border border-edge \
                                    bg-card px-2 py-1">
                            <span class=format!("size-4 rounded-sm border border-dim {class}")></span>
                            <span class="font-mono text-mini text-secondary">{name}</span>
                        </div>
                    })
                    .collect::<Vec<_>>()}
            </div>
        </div>
    }
}

/// The sessions rail, assembled: a live selection across three bands, and the filter box
/// wired to the one band long enough to need it.
#[component]
fn SessionsDemo() -> impl IntoView {
    // The finished sessions: title, the agent that ran it, how long ago.
    const DONE: [(&str, &str, &str); 5] = [
        ("What is on the linear board", "nakityok-lead", "21h"),
        ("You are coordinating ONE feature", "nakityok-lead", "17h"),
        ("Walk me through the agents", "adi-agent", "2d"),
        ("Stop the trigger, please", "bb-target-ops", "4d"),
        ("Target is Mollie", "bb-target-ops", "4d"),
    ];

    let query = RwSignal::new(String::new());
    let open = RwSignal::new("linear");
    // Selection is the caller's state, so it arrives as a signal per row rather than as an
    // index the component keeps.
    let is_open = move |id: &'static str| Signal::derive(move || open.get() == id);

    let matching = move || {
        let q = query.get().trim().to_lowercase();
        DONE.into_iter()
            .filter(|(title, agent, _)| {
                q.is_empty() || title.to_lowercase().contains(&q) || agent.contains(&q)
            })
            .collect::<Vec<_>>()
    };
    let done = move || {
        matching()
            .into_iter()
            .map(|(title, agent, age)| {
                view! {
                    <SessionItem
                        title=title
                        agent=agent
                        age=age
                        selected=is_open(title)
                        on:click=move |_| open.set(title)
                    />
                }
            })
            .collect::<Vec<_>>()
    };

    view! {
        <SessionList
            title="Sessions"
            search=query
            actions=|| {
                view! {
                    <Button variant=ButtonVariant::Link size=ButtonSize::Small>"+ New"</Button>
                }
                .into_any()
            }
        >
            <SessionGroup label="Running now" count=2>
                <SessionItem
                    title="Walk the linear board"
                    state=SessionState::Running
                    selected=is_open("linear")
                    alert="3 errors"
                    age="14m"
                    on:click=move |_| open.set("linear")
                />
                <SessionItem
                    title="Deep-analysis pass"
                    state=SessionState::Running
                    selected=is_open("deep")
                    age="2m"
                    on:click=move |_| open.set("deep")
                />
            </SessionGroup>

            <SessionGroup label="Waiting for you" count=1>
                <SessionItem
                    title="Viacheslav Teremets, 5 Aug"
                    state=SessionState::Waiting
                    selected=is_open("teremets")
                    alert="agent question"
                    age="2h"
                    on:click=move |_| open.set("teremets")
                />
            </SessionGroup>

            <SessionGroup label="Done">
                {done}
                <Show when=move || query.get().trim().is_empty()>
                    <SessionRollup
                        class="mt-1.5"
                        title="Deep-analysis pass on target"
                        note="9 repeats"
                        age="4d"
                    />
                </Show>
                <Show when=move || matching().is_empty()>
                    <Empty>"Nothing matches."</Empty>
                </Show>
            </SessionGroup>
        </SessionList>
    }
}

#[component]
fn Playground() -> impl IntoView {
    // The theme override the tokens read off <html data-theme>. `None` follows the OS — the
    // third state a two-way toggle would hide, and the one most people are actually in.
    let theme = RwSignal::new(None::<&'static str>);
    Effect::new(move |_| {
        let Some(root) = document().document_element() else {
            return;
        };
        match theme.get() {
            Some(t) => {
                let _ = root.set_attribute("data-theme", t);
            }
            None => {
                let _ = root.remove_attribute("data-theme");
            }
        }
    });

    let disabled = RwSignal::new(false);
    let name = RwSignal::new(String::from("ports"));
    let port = RwSignal::new(String::from("8000"));
    let backend = RwSignal::new(String::from("claude"));
    let notes = RwSignal::new(String::from("--rm\n--network=host"));

    view! {
        <main class="mx-auto flex max-w-4xl flex-col gap-4 p-6">
            <header class="flex flex-wrap items-center justify-between gap-3">
                // The logo is mono, per the type spec.
                <h1 class="m-0 font-mono text-title font-medium text-ink">
                    "adi" <span class="text-accent">"·"</span> "ui"
                </h1>
                // Re-rendered as a block on every theme change so the active button can
                // switch `variant` — cheaper than a reactive variant prop for a dev toggle.
                <div class="flex items-center gap-1">
                    {move || [("OS", None), ("Light", Some("light")), ("Dark", Some("dark"))]
                        .map(|(label, value)| {
                            let variant = if theme.get() == value {
                                ButtonVariant::Primary
                            } else {
                                ButtonVariant::Default
                            };
                            view! {
                                <Button
                                    size=ButtonSize::Small
                                    variant=variant
                                    on:click=move |_| theme.set(value)
                                >
                                    {label}
                                </Button>
                            }
                        })
                        .into_iter()
                        .collect::<Vec<_>>()}
                </div>
            </header>

            <Panel title="Type" flush=true>
                <div class="px-4">
                    <TypeSpecimen/>
                </div>
            </Panel>

            <Panel title="Palette" flush=true>
                <div class="px-4">
                    <Swatches
                        label="surfaces"
                        items=vec![
                            ("canvas", "bg-canvas"),
                            ("stage", "bg-stage"),
                            ("panel", "bg-panel"),
                            ("panel-alt", "bg-panel-alt"),
                            ("bar", "bg-bar"),
                            ("card", "bg-card"),
                            ("bubble", "bg-bubble"),
                            ("selected", "bg-selected"),
                        ]
                    />
                    <Swatches
                        label="lines"
                        items=vec![
                            ("divider", "bg-divider"),
                            ("frame", "bg-frame"),
                            ("edge", "bg-edge"),
                            ("edge-2", "bg-edge-2"),
                            ("dim", "bg-dim"),
                        ]
                    />
                    <Swatches
                        label="text"
                        items=vec![
                            ("ink", "bg-ink"),
                            ("body", "bg-body"),
                            ("secondary", "bg-secondary"),
                            ("meta", "bg-meta"),
                            ("placeholder", "bg-placeholder"),
                            ("faint", "bg-faint"),
                            ("fainter", "bg-fainter"),
                        ]
                    />
                    <Swatches
                        label="accent"
                        items=vec![
                            ("accent", "bg-accent"),
                            ("accent-fill", "bg-accent-fill"),
                            ("on-accent", "bg-on-accent"),
                            ("accent-soft", "bg-accent-soft"),
                            ("accent-soft-edge", "bg-accent-soft-edge"),
                            ("tip", "bg-tip"),
                            ("tip-edge", "bg-tip-edge"),
                        ]
                    />
                    <Swatches
                        label="states"
                        items=vec![
                            ("err", "bg-err"),
                            ("err-btn", "bg-err-btn"),
                            ("err-bg", "bg-err-bg"),
                            ("err-bg-2", "bg-err-bg-2"),
                            ("err-edge", "bg-err-edge"),
                            ("queue", "bg-queue"),
                            ("queue-ink", "bg-queue-ink"),
                            ("queue-bg", "bg-queue-bg"),
                            ("attention", "bg-attention"),
                        ]
                    />
                </div>
            </Panel>

            <Panel title="Button" flush=true>
                <div class="px-4">
                    <Row label="variant">
                        <Button>"Default"</Button>
                        <Button variant=ButtonVariant::Primary>"Primary"</Button>
                        <Button variant=ButtonVariant::Ghost>"Ghost"</Button>
                        <Button variant=ButtonVariant::Danger>"Danger"</Button>
                        <Button variant=ButtonVariant::Link>"Link"</Button>
                    </Row>
                    <Row label="size">
                        <Button size=ButtonSize::Small>"Small"</Button>
                        <Button size=ButtonSize::Medium>"Medium"</Button>
                        <Button size=ButtonSize::Small variant=ButtonVariant::Primary>
                            "Small primary"
                        </Button>
                    </Row>
                    <Row label="disabled">
                        <Button disabled=disabled>"Toggle me"</Button>
                        <Button disabled=disabled variant=ButtonVariant::Primary>"Primary"</Button>
                        <Button
                            variant=ButtonVariant::Ghost
                            on:click=move |_| disabled.update(|d| *d = !*d)
                        >
                            {move || if disabled.get() { "enable" } else { "disable" }}
                        </Button>
                    </Row>
                </div>
            </Panel>

            <Panel title="Badge" flush=true>
                <div class="px-4">
                    <Row label="tone">
                        <Badge>"neutral"</Badge>
                        <Badge tone=BadgeTone::Online>"running"</Badge>
                        <Badge tone=BadgeTone::Warn>"queued"</Badge>
                        <Badge tone=BadgeTone::Down>"failed"</Badge>
                        <Badge tone=BadgeTone::Accent>"selected"</Badge>
                    </Row>
                    <Row label="mono">
                        <Badge mono=true>":8000"</Badge>
                        <Badge mono=true tone=BadgeTone::Accent>"a1b2c3d"</Badge>
                    </Row>
                </div>
            </Panel>

            <Panel
                title="Panel"
                actions=|| {
                    view! {
                        <Button size=ButtonSize::Small variant=ButtonVariant::Ghost>"Refresh"</Button>
                        <Button size=ButtonSize::Small variant=ButtonVariant::Primary>"Add"</Button>
                    }
                    .into_any()
                }
            >
                <p class="m-0 text-mini text-meta">
                    "A panel with a title and header actions. The one below has neither, so it \
                     drops its header and is just a surface."
                </p>
                <Panel class="mt-3">
                    <span class="text-row">"Bare panel."</span>
                </Panel>
            </Panel>

            // Controls shown where they actually live — closing a panel, not floating in a
            // row of their own. A form strip only looks right against the body above it.
            <Panel title="Form · Field · Input" flush=true>
                <div class="px-4 pt-3 pb-1 text-mini text-meta">
                    "Fields align on their inputs, not their labels. Hover a "
                    <span class="font-mono">"?"</span>
                    " for the hint — it costs the row no height."
                </div>
                <Form>
                    <Field label="Name" grow=true hint="Unique within the project.">
                        <Input value=name width=InputWidth::Wide placeholder="service name"/>
                    </Field>
                    <Field label="Port" hint="Left blank, the registry picks a free one.">
                        <Input value=port input_type="number" width=InputWidth::Num/>
                    </Field>
                    <Field label="Backend">
                        <Select value=backend>
                            <option value="claude">"claude"</option>
                            <option value="codex">"codex"</option>
                            <option value="kimi">"kimi"</option>
                        </Select>
                    </Field>
                    <Button variant=ButtonVariant::Primary submit=true>"Create"</Button>
                </Form>
                <Flash kind=FlashKind::Ok>
                    {move || format!("reserved {} on :{}", name.get(), port.get())}
                </Flash>
                <Hint>"A hint block is the written-out version of a field's ?."</Hint>
            </Panel>

            <Panel title="Textarea · Select · widths" flush=true>
                <div class="grid gap-3 p-4">
                    <Field label="Docker args" hint="One flag per line.">
                        <Textarea value=notes rows=3/>
                    </Field>
                    <div class="flex flex-wrap items-end gap-2">
                        <Field label="Default"><Input placeholder="140px"/></Field>
                        <Field label="Num"><Input width=InputWidth::Num placeholder="72"/></Field>
                        <Field label="Wide" grow=true>
                            <Input width=InputWidth::Wide placeholder="fills the row"/>
                        </Field>
                    </div>
                    <Field label="Disabled">
                        <Input placeholder="not now" disabled=true/>
                    </Field>
                </div>
            </Panel>

            <Panel title="Flash · Empty" flush=true>
                <div class="flex flex-col gap-2 p-4">
                    <Flash kind=FlashKind::Ok card=true>"Reserved :8000 for ports."</Flash>
                    <Flash kind=FlashKind::Err card=true>"Port 8000 is already held by app."</Flash>
                    <Flash card=true>"Reserving…"</Flash>
                </div>
                <div class="border-t border-divider">
                    <Empty>"No ports reserved yet."</Empty>
                </div>
            </Panel>

            <Panel title="Sessions" flush=true>
                <div class="px-4 pt-3 text-mini text-meta">
                    "The rail on the left is live: click a row. The filter box binds a signal \
                     and does nothing else — what a query matches is the caller's to decide, \
                     and here it is wired to the done band only."
                </div>
                <div class="grid gap-4 p-4 min-[900px]:grid-cols-[320px_minmax(0,1fr)]">
                    // A rail fills the height it is given, so the demo has to give it one.
                    <div class="h-140 overflow-hidden rounded-md border border-edge">
                        <SessionsDemo/>
                    </div>

                    <div class="rounded-md border border-edge bg-panel px-1.5 pb-3">
                        <SessionGroup label="state × selected">
                            <SessionItem
                                title="Running"
                                state=SessionState::Running
                                agent="adi-agent"
                                age="14m"
                            />
                            <SessionItem
                                title="Running · selected"
                                state=SessionState::Running
                                selected=true
                                alert="3 errors"
                                age="14m"
                            />
                            <SessionItem
                                title="Waiting"
                                state=SessionState::Waiting
                                alert="agent question"
                                age="2h"
                            />
                            <SessionItem
                                title="Waiting · selected"
                                state=SessionState::Waiting
                                selected=true
                                alert="agent question"
                                age="2h"
                            />
                            <SessionItem title="Done" agent="nakityok-lead" age="21h"/>
                            <SessionItem
                                title="Done · selected"
                                selected=true
                                agent="bb-target-ops"
                                age="4d"
                            />
                        </SessionGroup>

                        <SessionGroup label="the rest" count=3>
                            // A title too long for the rail truncates; the line under it
                            // clips rather than wrapping the row to two heights.
                            <SessionItem
                                title="A title far longer than any rail is ever going to be wide"
                                state=SessionState::Running
                                agent="nakityok-lead"
                                alert="2 errors"
                                age="3d"
                            />
                            // Children land after the title, for what a prop cannot say.
                            <SessionItem title="With a child" agent="adi-agent" age="6h">
                                <Badge tone=BadgeTone::Warn>"draft"</Badge>
                            </SessionItem>
                            <SessionRollup
                                class="mt-1.5"
                                title="Deep-analysis pass on target"
                                note="9 repeats"
                                age="4d"
                            />
                        </SessionGroup>

                        <SessionGroup label="empty">
                            <Empty>"No sessions yet."</Empty>
                        </SessionGroup>
                    </div>
                </div>
            </Panel>
        </main>
    }
}
