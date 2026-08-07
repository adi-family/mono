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
    AppItem, AppState, Badge, BadgeTone, Chat, Button, ButtonSize, ButtonVariant, CodeEditor, CodeFrame,
    CodeHeight, Composer, Crumb, Crumbs, Empty, Faq, Field, Flash, FlashKind,
    Form, Hint, Input, InputWidth, Lang, Markdown, Modal, Panel, Qna, Rail, RailCard, RailGroup, Role,
    Select, SessionItem, SessionState, Textarea, ToolCall, ToolState, TopBar, Tree, TreeNode, TreeState, Turn,
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

/// The inner markup of a 16×16 `<svg>`, which is what [`TreeNode::icon`] takes. Two are
/// enough for a file browser; a real screen keeps its own set.
const FOLDER: &str = "<path d='M2 4.5A1.5 1.5 0 0 1 3.5 3h2.8l1.2 1.6h5A1.5 1.5 0 0 1 14 6.1v5.4A1.5 1.5 0 0 1 12.5 13h-9A1.5 1.5 0 0 1 2 11.5z'/>";
const FILE: &str = "<path d='M4 2h4.5L12 5.5V14H4z'/><path d='M8.5 2v3.5H12'/>";

/// The same shape again, for [`Button`]'s `icon`.
const EYE: &str = "<path d='M1.5 8S4 3.5 8 3.5 14.5 8 14.5 8 12 12.5 8 12.5 1.5 8 1.5 8z'/><circle cx='8' cy='8' r='2'/>";
const CODE: &str = "<path d='M6 4 2.5 8 6 12'/><path d='M10 4l3.5 4-3.5 4'/>";
const SAVE: &str = "<path d='M8 2.5v6'/><path d='M5 6l3 3 3-3'/><path d='M2.5 11v2.5h11V11'/>";
const QUESTION: &str = "<circle cx='8' cy='8' r='6.25'/><path d='M6.15 6.05a1.9 1.9 0 1 1 2.6 1.75c-.5.2-.75.6-.75 1.1v.35'/><path d='M8 11.75h.01'/>";
const SPARK: &str = "<path d='M6.5 2.5 8 6l3.5 1.5L8 9l-1.5 3.5L5 9l-3.5-1.5L5 6z'/><path d='M12 2v2.5'/><path d='M13.25 3.25h-2.5'/>";

/// What a file holds, for the demo. One per format the scanner knows, so clicking down the
/// tree walks the whole palette.
fn sample(path: &str) -> &'static str {
    match Lang::from_path(path) {
        Lang::Yaml => {
            "# the front door's own service\nproxy:\n  host: app.adi\n  port: 8000\n  \
             health: \"/api/health\"\n  restart: on-failure\n\nroutes:\n  - match: /api/*\n    \
             upstream: 127.0.0.1:8000\n"
        }
        Lang::Toml => {
            "[package]\nname = \"adi-ui\"\nedition = \"2024\"\n\n[dependencies]\n\
             leptos = { version = \"0.8\", features = [\"csr\"] }\n\n\
             # excluded from default-members: this one targets wasm\n[lints]\nworkspace = true\n"
        }
        Lang::Ts => {
            "export async function routes(app: App) {\n  \
             app.get(\"/api/health\", async () => ({ ok: true }));\n\n  \
             app.post(\"/api/run\", async (req) => {\n    \
             const { agent, prompt } = await req.json();\n    \
             return start(agent, prompt, { timeout: 30_000 });\n  });\n}\n"
        }
        Lang::Sh => {
            "#!/usr/bin/env bash\nset -euo pipefail\n\nBIN=\"${1:-target/release/adi-app}\"\n\
             trunk build --release\ncargo build --release -p adi-app\n\n\
             if [ ! -x \"$BIN\" ]; then\n  echo \"no binary at $BIN\" >&2\n  exit 1\nfi\n"
        }
        Lang::Sql => {
            "CREATE TABLE session (\n  id      TEXT PRIMARY KEY,\n  \
             agent   TEXT NOT NULL,\n  started INTEGER NOT NULL,\n  \
             state   TEXT NOT NULL DEFAULT 'working'\n);\n\n\
             SELECT agent, count(*) FROM session WHERE started > 0 GROUP BY agent;\n"
        }
        Lang::Json => {
            "{\n  \"name\": \"adi-ui\",\n  \"private\": true,\n  \"version\": \"0.1.0\",\n  \
             \"scripts\": {\n    \"dev\": \"trunk serve --open\",\n    \
             \"build\": \"trunk build --release\"\n  },\n  \"sideEffects\": false\n}\n"
        }
        // The one file with two ways to read it, which is what the button in the frame's
        // top right is for.
        Lang::Md => {
            r"# adi-ui

Leptos components over the adi design tokens. Read this one **rendered** — the
toggle is up in the frame's top right, beside the file name.

## What's in it

- `Tree` — an IDE tree from one flat, depth-annotated list
- `CodeEditor` — a painted `<pre>` under a transparent `<textarea>`
- `Markdown` — *this*, which is a scanner rather than a parser

> A class name has to appear in the source as a whole string literal. Tailwind
> never runs the code, so a name assembled at runtime is never generated.

```sh
cd crates/adi-ui && trunk serve --open   # http://127.0.0.1:9081
```

---

1. Tokens live in `styles/tokens.css`
2. Utilities come from `styles/ui.css`
3. Nothing here depends on [adi-css](/crates/adi-css)
"
        }
        Lang::Rust => {
            r#"//! The tree — an IDE view over one flat, depth-annotated list.

use leptos::prelude::*;

/// One row. `depth` is the nesting level; 0 is a root.
#[derive(Clone, Debug)]
pub struct TreeNode<'a> {
    pub id: String,
    pub depth: usize,
    pub icon: Option<&'a str>,
}

impl TreeNode<'static> {
    #[must_use]
    pub fn new(id: impl Into<String>, depth: usize) -> Self {
        let label = format!("row {depth}");
        assert!(!label.is_empty(), r"a row is never nameless");
        Self { id: id.into(), depth, icon: None }
    }
}
"#
        }
        Lang::None => {
            "Nothing here knows this extension, so it paints as one plain run.\n\
             Highlighting is an enhancement, never a gate.\n"
        }
    }
}

/// A file tree over one project, and the editor the selected file opens in.
#[component]
fn FilesDemo() -> impl IntoView {
    // A flat list in tree order, which is the whole input format: depth says the shape.
    let nodes = vec![
        TreeNode::new("adi-ui", 0, "adi-ui")
            .children(true)
            .container(true)
            .icon(FOLDER)
            .emphasis(true),
        TreeNode::new("adi-ui/Cargo.toml", 1, "Cargo.toml")
            .icon(FILE)
            .title("adi-ui/Cargo.toml"),
        TreeNode::new("adi-ui/hive.yaml", 1, "hive.yaml").icon(FILE),
        TreeNode::new("adi-ui/package.json", 1, "package.json").icon(FILE),
        TreeNode::new("adi-ui/README.md", 1, "README.md").icon(FILE),
        TreeNode::new("adi-ui/src", 1, "src")
            .children(true)
            .container(true)
            .icon(FOLDER)
            // The one badge left: a count you might act on, rather than a byte size nobody
            // reads.
            .badge("2 changed"),
        TreeNode::new("adi-ui/src/lib.rs", 2, "lib.rs").icon(FILE),
        TreeNode::new("adi-ui/api", 1, "api")
            .children(true)
            .container(true)
            .icon(FOLDER),
        TreeNode::new("adi-ui/api/routes.ts", 2, "routes.ts").icon(FILE),
        TreeNode::new("adi-ui/db", 1, "db")
            .children(true)
            .container(true)
            .icon(FOLDER),
        TreeNode::new("adi-ui/db/schema.sql", 2, "schema.sql").icon(FILE),
        // The rule above this row is what `separated` is for: a boundary between two kinds
        // of children, drawn without a heading.
        TreeNode::new("adi-ui/scripts", 1, "scripts")
            .children(true)
            .container(true)
            .icon(FOLDER)
            .separated(true),
        TreeNode::new("adi-ui/scripts/deploy.sh", 2, "deploy.sh").icon(FILE),
    ];

    let tree = TreeState::new();
    // The tree reports what was clicked; the screen decides what that means. Here it means
    // "open this file", which is also what the highlight follows.
    let path = Signal::derive(move || tree.selected.get());
    let buffer = RwSignal::new(String::from(sample("hive.yaml")));
    // A real screen fetches here. The demo swaps in the sample, which is enough to show the
    // language following the path.
    Effect::new(move |_| {
        if let Some(p) = path.get() {
            buffer.set(sample(&p).to_string());
        }
    });
    let lang = Signal::derive(move || match path.get() {
        Some(p) => Lang::from_path(&p),
        None => Lang::Yaml,
    });
    // Reading rather than editing. Opening another file drops back to the source, because
    // "rendered" is a way of looking at *this* file, not a mode the pane stays in.
    let preview = RwSignal::new(false);
    Effect::new(move |_| {
        let _ = path.get();
        preview.set(false);
    });

    view! {
        <div class="grid gap-4 min-[900px]:grid-cols-[240px_minmax(0,1fr)]">
            <div class="island h-100 overflow-auto bg-panel">
                <Tree nodes=nodes state=tree selected=path empty="No files."/>
            </div>
            <CodeFrame
                title=Signal::derive(move || {
                    path.get().unwrap_or_else(|| "adi-ui/hive.yaml".to_string())
                })
                height=CodeHeight::Form
                actions=move || {
                    view! {
                        // The controls a file earns. A Markdown file can be read as well as
                        // edited, so it gets the toggle and nothing else does.
                        <Show when=move || lang.get() == Lang::Md>
                            // The icon changes with the label: an eye to read it, angle
                            // brackets to go back to the text.
                            {move || view! {
                                <Button
                                    size=ButtonSize::Small
                                    variant=ButtonVariant::Ghost
                                    icon=if preview.get() { CODE } else { EYE }
                                    on:click=move |_| preview.update(|p| *p = !*p)
                                >
                                    {if preview.get() { "Source" } else { "Preview" }}
                                </Button>
                            }}
                        </Show>
                        <Button size=ButtonSize::Small variant=ButtonVariant::Ghost icon=SAVE>
                            "Save"
                        </Button>
                    }
                    .into_any()
                }
            >
                <Show
                    when=move || preview.get() && lang.get() == Lang::Md
                    fallback=move || view! {
                        <CodeEditor value=buffer lang=lang height=CodeHeight::Fill/>
                    }
                >
                    <Markdown
                        source=Signal::derive(move || buffer.get())
                        class="h-full overflow-auto p-4"
                    />
                </Show>
            </CodeFrame>
        </div>
    }
}

/// A stand-in favicon: a rounded square with a letter in it, as a `data:` URI so the
/// playground stays a single page with no requests. A real app serves its own.
fn favicon(letter: char, fill: &str) -> String {
    let svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'>\
         <rect width='16' height='16' rx='3' fill='{fill}'/>\
         <text x='8' y='11.5' font-family='monospace' font-size='9' font-weight='700' \
         text-anchor='middle' fill='white'>{letter}</text></svg>"
    );
    format!(
        "data:image/svg+xml,{}",
        svg.replace('#', "%23").replace('<', "%3C").replace('>', "%3E").replace('"', "%22")
    )
}

/// One app in the demo's data: what it is called, its mark, the node it runs on, and how
/// alive it is.
type Row = (&'static str, char, &'static str, &'static str, AppState);

/// The right rail: every living app on this stack, banded by the project it belongs to and
/// tagged with the fleet node it runs on.
#[component]
fn AppsDemo() -> impl IntoView {
    // (project, [(title, node, status, state)]) — the two things a dashboard belongs to are
    // the band it is in and the name under its title.
    // (project, [(title, mark, colour, node, state)]). No ages anywhere: a number that
    // only says something changed is not worth a column.
    let bands: Vec<(&str, Vec<Row>)> = vec![
        (
            "nakityok",
            vec![
                ("NakitYok Status", 'N', "#1f7a5c", "zomro-de1", AppState::Live),
                ("IVR Call Funnel", 'I', "#2f5fa8", "zomro-de1", AppState::Live),
                ("IIKO Sync Errors", 'K', "#8a6414", "teremec", AppState::Live),
            ],
        ),
        (
            "bugbounty",
            vec![
                ("Bugbounty Targets", 'B', "#6b3fa0", "teremec", AppState::Live),
                ("Payout Queue", 'P', "#a03f3f", "8626e4721660", AppState::Offline),
            ],
        ),
        (
            "infrastructure",
            vec![// Empty machine: it runs right here, and the row says so itself.
                ("Fleet Load", 'F', "#3f6b6b", "", AppState::ViewOnly)],
        ),
    ];

    let open = RwSignal::new("IVR Call Funnel");

    view! {
        <Rail
            title="Apps"
            actions=|| {
                view! {
                    <Button size=ButtonSize::Small variant=ButtonVariant::Ghost>"Refresh"</Button>
                    <Button size=ButtonSize::Small variant=ButtonVariant::Ghost>"Manage"</Button>
                }
                .into_any()
            }
        >
            {bands
                .into_iter()
                .map(|(project, rows)| {
                    let count = rows.len();
                    view! {
                        <RailGroup label=project count=count>
                            {rows
                                .into_iter()
                                .map(|(title, mark, fill, node, state)| {
                                    let is_open = Signal::derive(move || open.get() == title);
                                    // Only a broken row has anything to offer — and even
                                    // then it stays hidden until its dot is asked. A row
                                    // with no action leaves its dot a plain mark.
                                    if state == AppState::Offline {
                                        view! {
                                            <AppItem
                                                title=title
                                                state=state
                                                favicon=favicon(mark, fill)
                                                machine=node
                                                selected=is_open
                                                action=|| {
                                                    view! {
                                                        <Button size=ButtonSize::Small>
                                                            "Connect machine"
                                                        </Button>
                                                    }
                                                    .into_any()
                                                }
                                                on:click=move |_| open.set(title)
                                            />
                                        }
                                        .into_any()
                                    } else {
                                        view! {
                                            <AppItem
                                                title=title
                                                state=state
                                                favicon=favicon(mark, fill)
                                                machine=node
                                                selected=is_open
                                                on:click=move |_| open.set(title)
                                            />
                                        }
                                        .into_any()
                                    }
                                })
                                .collect::<Vec<_>>()}
                        </RailGroup>
                    }
                })
                .collect::<Vec<_>>()}
        </Rail>
    }
}

/// A transcript, newest first: what the agent said, what it did between saying things, and
/// one run still going.
#[component]
fn ChatDemo() -> impl IntoView {
    let turns: Vec<Turn> = vec![
        Turn::Said {
            role: Role::User,
            body: "Walk the linear board and tell me what is actually blocked.".into(),
        },
        Turn::Did(vec![
            ToolCall::new("Bash")
                .param("command", "adi-mono linear issues --state started --json")
                .param("description", "List started issues")
                .result("14 issues · 3 without an assignee"),
            ToolCall::new("Read")
                .param("file_path", "/Users/ihor/adi-family/docs/fleet.md")
                .result("…1.2 kB…"),
        ]),
        Turn::Said {
            role: Role::Agent,
            body: "Three of the fourteen started issues have **no assignee**, and all three                    are on the fleet:

- `ADI-214` — node pairing retries
- `ADI-221` —                    relay fallback
- `ADI-233` — `adi up` on a cold store

The first two                    are the same bug. I will read the pairing path before saying more."
                .into(),
        },
        Turn::Did(vec![
            ToolCall::new("Grep")
                .param("pattern", "fn pair(")
                .param("path", "crates/adi-mesh/src")
                .result("crates/adi-mesh/src/pair.rs:88"),
            ToolCall::new("Read")
                .param("file_path", "crates/adi-mesh/src/pair.rs")
                .param("offset", "60")
                .param("limit", "80")
                .result("…retry loop, 3 attempts, no backoff…"),
            ToolCall::new("Bash")
                .param(
                    "command",
                    "cargo test -p adi-mesh pair -- --nocapture --test-threads 1",
                )
                .param("description", "Run the pairing tests")
                .state(ToolState::Running),
        ]),
    ];

    let draft = RwSignal::new(String::new());
    let sent = RwSignal::new(String::new());

    view! {
        <div class="flex flex-col gap-3">
            // The composer sits above the transcript, not below it: newest is at the top, so
            // the box you type into belongs next to where your message will appear.
            <Composer
                value=draft
                busy=false
                on_send=Callback::new(move |text: String| {
                    sent.set(text);
                    draft.set(String::new());
                })
            />
            <Show when=move || !sent.get().is_empty()>
                <Flash kind=FlashKind::Ok>
                    {move || format!("sent: {}", sent.get())}
                </Flash>
            </Show>
            <Chat turns=turns.clone() class="max-h-140 p-1"/>
        </div>
    }
}

/// The sessions rail, assembled: a live selection across three bands, and the filter box
/// wired to the one band long enough to need it.
#[component]
fn SessionsDemo() -> impl IntoView {
    // The sessions nobody is waiting on: title, the agent that ran it, how long ago, and
    // how it ended — one of them failed, which is a state and not a note under the title.
    const DONE: [(&str, &str, &str, SessionState); 5] = [
        (
            "What is on the linear board",
            "nakityok-lead",
            "21h",
            SessionState::Done,
        ),
        (
            "You are coordinating ONE feature",
            "nakityok-lead",
            "17h",
            SessionState::Done,
        ),
        (
            "Walk me through the agents",
            "adi-agent",
            "2d",
            SessionState::Done,
        ),
        (
            "Stop the trigger, please",
            "bb-target-ops",
            "4d",
            SessionState::Error,
        ),
        (
            "Target is Mollie",
            "bb-target-ops",
            "4d",
            SessionState::Done,
        ),
    ];

    let query = RwSignal::new(String::new());
    let open = RwSignal::new("linear");
    // Selection is the caller's state, so it arrives as a signal per row rather than as an
    // index the component keeps.
    let is_open = move |id: &'static str| Signal::derive(move || open.get() == id);

    let matching = move || {
        let q = query.get().trim().to_lowercase();
        DONE.into_iter()
            .filter(|(title, agent, _, _)| {
                q.is_empty() || title.to_lowercase().contains(&q) || agent.contains(&q)
            })
            .collect::<Vec<_>>()
    };
    let done = move || {
        matching()
            .into_iter()
            .map(|(title, agent, age, state)| {
                view! {
                    <SessionItem
                        title=title
                        state=state
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
        <Rail
            title="Sessions"
            search=query
            actions=|| {
                view! {
                    <Button variant=ButtonVariant::Link size=ButtonSize::Small>"+ New"</Button>
                }
                .into_any()
            }
        >
            // One band for everything live. A session that stopped to ask you something is
            // still the same conversation you left running, and a band of its own put a
            // heading between it and the row above for one row's worth of news.
            <RailGroup label="Running now" count=3>
                <SessionItem
                    title="Walk the linear board"
                    state=SessionState::Working
                    selected=is_open("linear")
                    age="14m"
                    on:click=move |_| open.set("linear")
                />
                <SessionItem
                    title="Viacheslav Teremets, 5 Aug"
                    state=SessionState::Waiting
                    selected=is_open("teremets")
                    alert="agent question"
                    age="2h"
                    on:click=move |_| open.set("teremets")
                />
                <SessionItem
                    title="Deep-analysis pass"
                    state=SessionState::Working
                    selected=is_open("deep")
                    age="2m"
                    on:click=move |_| open.set("deep")
                />
            </RailGroup>

            <RailGroup label="Done">
                {done}
                <Show when=move || matching().is_empty()>
                    <Empty>"Nothing matches."</Empty>
                </Show>
            </RailGroup>
        </Rail>
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

    // The FAQ the bar opens. Closed by default, and it closes itself three ways.
    let faq_open = RwSignal::new(false);
    let questions = vec![
        Qna::new(
            "What is this page?",
            "Every component in `adi-ui`, in one place, in **both themes**. It is a dev \
             surface: nothing embeds it and nothing depends on it.",
        ),
        Qna::new(
            "Why is there no `dark:` anywhere?",
            "Because a token is already both themes. `bg-card` compiles to `var(--card)`, and \
             that is one `light-dark()` declaration — `color-scheme` picks the half.",
        ),
        Qna::new(
            "Why did my class do nothing?",
            "Tailwind finds classes by *reading* the source, never by running it. \
             `format!(\"bg-{tone}\")` generates no CSS at all. Write the whole literal per \
             branch:\n\n```rust\nSelf::Primary => \"bg-accent-fill text-on-accent\",\n```",
        ),
        Qna::new(
            "How do I run it?",
            "```sh\ncd crates/adi-ui && trunk serve --open\n```\n\nPort 9081, deliberately \
             not 9080 — that one is the webapp dev server.",
        ),
    ];

    let disabled = RwSignal::new(false);
    let name = RwSignal::new(String::from("ports"));
    let port = RwSignal::new(String::from("8000"));
    let backend = RwSignal::new(String::from("claude"));
    let notes = RwSignal::new(String::from("--rm\n--network=host"));

    view! {
        // The page's own lid, and the component under test: wall to wall, hairline at the
        // bottom, and it stays there while everything below it scrolls.
        //
        // What is *in* it is the point. The mark goes home, the middle says what is open,
        // and the right holds the way out to the other version of the app — the two or
        // three things a screen owes you at all times. A theme toggle is not one of them;
        // it lives with the palette it changes, further down the page.
        <TopBar
            logo="adi"
            home="/"
            actions=move || {
                view! {
                    // Left of the way out: the way to have this explained.
                    <Button
                        size=ButtonSize::Small
                        variant=ButtonVariant::Ghost
                        icon=QUESTION
                        on:click=move |_| faq_open.set(true)
                    >
                        "FAQ"
                    </Button>
                    <Button size=ButtonSize::Small variant=ButtonVariant::Ghost icon=SPARK>
                        "Extended"
                    </Button>
                }
                .into_any()
            }
        >
            <Crumbs items=vec![
                Crumb::new("adi-ui").href("/"),
                Crumb::new("playground"),
            ]/>
        </TopBar>

        <Modal open=faq_open title="Questions" width="max-w-3xl">
            <Faq items=questions.clone()/>
        </Modal>

        <main class="mx-auto flex max-w-4xl flex-col gap-4 p-6">
            <Panel title="Type" flush=true>
                <div class="px-4">
                    <TypeSpecimen/>
                </div>
            </Panel>

            <Panel
                title="Palette"
                flush=true
                actions=move || {
                    // Re-rendered as a block on every theme change so the active button can
                    // switch `variant` — cheaper than a reactive variant prop for a dev
                    // toggle. It sits here rather than in the bar: this is the panel it
                    // changes, and a screen's header owes you navigation, not preferences.
                    view! {
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
                    }
                    .into_any()
                }
            >
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

            <Panel title="TopBar" flush=true>
                <div class="px-4 pt-3 text-mini text-meta">
                    "The page's own header is this component — scroll and it stays. Here it \
                     is again inside a window, which is where it lives: wall to wall with a \
                     hairline under it, and islands below. It is the one thing in the crate \
                     that is not an island itself, because it is the screen's edge rather \
                     than an object on the screen."
                </div>
                <div class="p-4">
                    // A window, in miniature: the bar's corners are clipped by the island
                    // around it, which is why that island owns the `overflow-hidden`.
                    <div class="island overflow-hidden bg-canvas">
                        <TopBar
                            logo="adi"
                            actions=|| {
                                view! {
                                    <Button
                                        size=ButtonSize::Small
                                        variant=ButtonVariant::Ghost
                                        icon=SAVE
                                    >
                                        "Install"
                                    </Button>
                                    <Button size=ButtonSize::Small variant=ButtonVariant::Primary>
                                        "+ New"
                                    </Button>
                                }
                                .into_any()
                            }
                        >
                            <span class="font-mono text-mini text-meta">
                                "/ " <span class="text-secondary">"projects"</span>
                            </span>
                        </TopBar>
                        <div class="flex gap-3 p-3">
                            <div class="island h-20 w-32 shrink-0 bg-panel"></div>
                            <div class="island h-20 flex-1 bg-card"></div>
                        </div>
                    </div>
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

            <Panel title="Tree · CodeEditor" flush=true>
                <div class="px-4 pt-3 text-mini text-meta">
                    "A file browser and the editor a file opens in. The tree takes one flat, \
                     depth-annotated list — depth is what makes it a tree, and a closed row \
                     hides everything deeper than it. The editor is a painted "
                    <span class="font-mono">"<pre>"</span>
                    " under a transparent "
                    <span class="font-mono">"<textarea>"</span>
                    ": the browser still owns the caret, undo and IME, and the paint only \
                     has to keep up. Click a "
                    <span class="font-mono">".toml"</span> " and a "
                    <span class="font-mono">".yaml"</span>
                    " — the language follows the path."
                </div>
                <div class="p-4">
                    <FilesDemo/>
                </div>
            </Panel>

            <Panel title="Chat" flush=true>
                <div class="px-4 pt-3 text-mini text-meta">
                    "Newest first — an agent's run is long and mostly tool calls, and you come \
                     back to it to find out what just happened, not to re-read it. Nothing \
                     pins to the bottom, so streaming content pushes away from you instead of \
                     under you. Every entry carries "
                    <span class="font-mono">"content-visibility: auto"</span>
                    ", so the browser skips what is off screen without a virtual list taking \
                     find-in-page away. Tool runs are folded by default and text is the \
                     divider between them; the one still running shows itself in the closed \
                     summary."
                </div>
                <div class="p-4">
                    <ChatDemo/>
                </div>
            </Panel>

            <Panel title="Apps" flush=true>
                <div class="px-4 pt-3 text-mini text-meta">
                    "The right rail: the living apps on this stack. Same container, same bands \
                     and same card as the sessions on the other side — a different row in \
                     them. An app belongs to two things and the row says both: the project it \
                     is part of is the band, the fleet node it runs on is the name under its \
                     title. The mark is its own favicon, and the state rides its corner."
                </div>
                <div class="p-4">
                    <div class="h-140 w-80 max-w-full">
                        <AppsDemo/>
                    </div>
                </div>
            </Panel>

            <Panel title="Sessions" flush=true>
                <div class="px-4 pt-3 text-mini text-meta">
                    "The rail is live: click a row. Scroll it and the title goes with the \
                     rows while the filter box stays — that box binds a signal and does \
                     nothing else, since what a query matches is the caller's to decide, and \
                     here it is wired to the done band only."
                </div>
                <div class="p-4">
                    // A rail fills the height it is given, so the demo has to give it one.
                    <div class="h-140 w-80 max-w-full">
                        <SessionsDemo/>
                    </div>
                </div>
            </Panel>

            <Panel title="Session card" flush=true>
                <div class="px-4 pt-3 text-mini text-meta">
                    "Every state a row can be in, selected and not — the open one is a fill \
                     and a hairline all the way round, and a waiting one washes amber on a \
                     5s cycle whether it is open or not."
                </div>
                <div class="p-4">
                    <div class="island bg-panel px-1.5 pb-3">
                        // Every state, twice: as it sits in the list and as the open one.
                        // Only the fill and the hairline change between the two.
                        <RailGroup label="done">
                            <SessionItem title="Done" agent="nakityok-lead" age="21h"/>
                            <SessionItem
                                title="Done · selected"
                                selected=true
                                agent="nakityok-lead"
                                age="21h"
                            />
                        </RailGroup>

                        <RailGroup label="waiting">
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
                        </RailGroup>

                        <RailGroup label="error">
                            <SessionItem
                                title="Error"
                                state=SessionState::Error
                                agent="bb-target-ops"
                                age="4d"
                            />
                            <SessionItem
                                title="Error · selected"
                                state=SessionState::Error
                                selected=true
                                agent="bb-target-ops"
                                age="4d"
                            />
                        </RailGroup>

                        <RailGroup label="working">
                            <SessionItem
                                title="Working"
                                state=SessionState::Working
                                agent="adi-agent"
                                age="14m"
                            />
                            <SessionItem
                                title="Working · selected"
                                state=SessionState::Working
                                selected=true
                                agent="adi-agent"
                                age="14m"
                            />
                        </RailGroup>

                        <RailGroup label="the rest" count=2>
                            // A title too long for the rail truncates; the line under it
                            // clips rather than wrapping the row to two heights.
                            <SessionItem
                                title="A title far longer than any rail is ever going to be wide"
                                state=SessionState::Working
                                agent="nakityok-lead"
                                age="3d"
                            />
                            // Children land after the title, for what a prop cannot say.
                            <SessionItem title="With a child" agent="adi-agent" age="6h">
                                <Badge tone=BadgeTone::Warn>"draft"</Badge>
                            </SessionItem>
                        </RailGroup>

                        // The box on its own, for a row a session does not describe. `fill`
                        // is where the state goes; everything inside is the caller's.
                        <RailGroup label="bare card" count=2>
                            <RailCard fill="hover:bg-card">
                                <span class="text-row text-body">"Anything, in a row"</span>
                            </RailCard>
                            <RailCard fill="border-edge bg-selected" current=true>
                                <span class="text-row text-ink">"…and the same one, open"</span>
                            </RailCard>
                        </RailGroup>

                        <RailGroup label="empty">
                            <Empty>"No sessions yet."</Empty>
                        </RailGroup>
                    </div>
                </div>
            </Panel>
        </main>
    }
}
