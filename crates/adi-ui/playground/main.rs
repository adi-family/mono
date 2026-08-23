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

use std::time::Duration;

use adi_ui::{
    AppItem, AppState, Ask, AskOption, AskQuestion, AttachState, Attached, Attaching, Badge,
    BadgeTone, Chat, Button, ButtonSize,
    ButtonVariant, CodeEditor, CodeFrame,
    CodeHeight, CodeLog, Composer, Crumb, Crumbs, DirEntry, Empty, Faq, Field, Flash, FlashKind,
    Form, Hint, Input, InputWidth, Kbd, Lang, Markdown, Modal, Panel, PathPicker, PathRoot, Qna,
    Block, Flag, FlagList, FlagMark, Param, ParamKind, PromptText, Queued, Rail, RailCard,
    RailGroup, Role, Select, SessionItem, SessionState, Simulator, SortKey, Stop, StopLine, Table,
    TableState, Textarea, Token, TokenStream, ToolCall, ToolDecl, ToolForm, ToolState, TopBar,
    Tree, TreeNode, TreeState, Turn, TurnBlocks, dir_of, sort_rows,
};
use adi_ui::{
    Change, Decided, Fact, FactCard, FactHistory, FactRow, Moved, NodeKind, Pair, PairCard,
    PairQueue, PairSide, Relation, Ruling, Stale, StaleList, Truncated, TxPanel, Verdict,
};
// The gallery's own `Row` is the label-plus-specimen line every panel is built from; the
// crate's is a table row. Both earn the name in their own context, so the import is aliased
// rather than either being renamed.
use adi_ui::{EmptyRow, Row as TableRow};
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

/// A slice of a real agent prompt, already split — the shape the server sends, so the
/// component is exercised on the thing it will actually be handed.
///
/// It has to carry the two cases that break a naive renderer: control tokens (the template's
/// own seams) and tokens with newlines in them, including one that is *only* a break.
fn prompt_tokens() -> Vec<Token> {
    let content = [
        (27, "You"), (3477, " review"), (2082, " code"), (304, " in"), (420, " this"),
        (12827, " repository"), (13, "."), (4557, " Read"), (1603, " before"), (499, " you"),
        (11913, " judge"), (26, ";"), (1475, " every"), (9455, " finding"), (5144, " names"),
        (264, " a"), (1052, " file"), (627, ".\n"),
        (2, "#"), (11208, " Where"), (499, " you"), (527, " are"), (271, "\n\n"),
        (2028, "This"), (1629, " run"), (8638, " starts"), (304, " in"), (38401, " `/"),
        (7220, "Users"), (14, "/"), (76, "m"), (39847, "gor"), (359, "un"), (14588, "uch"),
        (14, "/"), (16607, "adi"), (24815, "-family"), (63, "`"), (13, "."),
    ];
    let mut out = vec![
        Token::special(100_264, "<|im_start|>"),
        Token::new(9125, "system"),
        Token::special(100_266, "<|im_sep|>"),
    ];
    out.extend(content.into_iter().map(|(id, text)| Token::new(id, text)));
    out.push(Token::special(100_265, "<|im_end|>"));
    out.push(Token::special(100_264, "<|im_start|>"));
    out.push(Token::new(882, "user"));
    out.push(Token::special(100_266, "<|im_sep|>"));
    for (id, text) in [
        (19461, "Review"), (279, " the"), (23055, " runner"), (2098, " ref"), (5739, "actor"),
        (389, " on"), (1925, " main"), (13, "."),
    ] {
        out.push(Token::new(id, text));
    }
    out.push(Token::special(100_265, "<|im_end|>"));
    out
}

/// `Bash` as its schema declares it — the tool with one of every control, which is why it is
/// the one worth showing: a wide text body, a number, and a flag that explains itself.
#[component]
fn ToolFormDemo() -> impl IntoView {
    let params = vec![
        Param::new("command", ParamKind::Text)
            .required()
            .hint("The command line to run.")
            .placeholder("git log --oneline -3"),
        Param::new("timeout_ms", ParamKind::Number)
            .hint("Give up after this long. Defaults to 120000.")
            .placeholder("120000"),
        Param::new("background", ParamKind::Flag)
            .hint("Start it and return a job id instead of waiting for it."),
    ];
    // The caller owns the values, so the preview is written here rather than in the component:
    // what a call looks like on the wire belongs to whoever is about to send it.
    let command = params[0].text;
    let timeout = params[1].text;
    let background = params[2].flag;
    let wire = move || {
        let mut args = format!("{{\"command\":{:?}", command.get());
        if let Ok(ms) = timeout.get().trim().parse::<u64>() {
            args.push_str(&format!(",\"timeout_ms\":{ms}"));
        }
        if background.get() {
            args.push_str(",\"background\":true");
        }
        args.push('}');
        vec![
            Token::special(100_264, "<|im_start|>"),
            Token::new(0, "assistant to=functions.Bash"),
            Token::special(100_266, "<|im_sep|>"),
            Token::new(0, args),
        ]
    };

    view! {
        <div class="flex flex-col gap-4">
            <ToolForm params=params/>
            <div>
                <div class="caps mb-2 text-faint">"sent as"</div>
                <PromptText
                    tokens=Signal::derive(wire)
                    class="rounded-sm border border-edge bg-panel-alt p-3"
                />
            </div>
        </div>
    }
}

/// Bash and Read as their schemas declare them, which is what the simulator is handed.
fn agent_tools() -> Vec<ToolDecl> {
    vec![
        ToolDecl::new("Bash", "Run a command in the agent's own shell, in its own cwd.").params(
            vec![
                Param::new("command", ParamKind::Text)
                    .required()
                    .hint("The command line to run.")
                    .placeholder("git log --oneline -3"),
                Param::new("timeout_ms", ParamKind::Number)
                    .hint("Give up after this long. Defaults to 120000.")
                    .placeholder("120000"),
                Param::new("background", ParamKind::Flag)
                    .hint("Start it and return a job id instead of waiting for it."),
            ],
        ),
        ToolDecl::new("Read", "Read a file from the local filesystem.").params(vec![
            Param::new("path", ParamKind::Line)
                .required()
                .hint("Absolute path to the file.")
                .placeholder("/Users/you/adi-family/README.md"),
            Param::new("limit", ParamKind::Number).hint("How many lines to read."),
        ]),
    ]
}

/// What a tool's fields currently hold, as the arguments of a call.
///
/// This is the caller's half of the [`ToolForm`] bargain — the component renders the fields
/// and never guesses at the wire form, so composing one is the job of whoever is about to
/// send it. Empty optional fields are dropped rather than sent blank, which is what a model
/// does with a parameter it has nothing to say about.
fn args_of(tool: &ToolDecl) -> Vec<(String, String)> {
    tool.params
        .iter()
        .filter_map(|p| {
            let value = if p.kind == ParamKind::Flag {
                if !p.flag.get_untracked() {
                    return None;
                }
                "true".to_string()
            } else {
                let text = p.text.get_untracked();
                if text.trim().is_empty() && !p.required {
                    return None;
                }
                text
            };
            Some((p.name.clone(), value))
        })
        .collect()
}

/// A stand-in for the tokenizer, which in the real thing lives server-side and has tiktoken's
/// tables compiled in ([`adi_ui::TokenStream`] explains why it is not in the browser).
///
/// It splits on spaces keeping the space with the word that follows, and gives every newline
/// its own token — close enough to a BPE's shape that the playground reads honestly, and ids
/// that are made up and say so.
fn fake_tokens(text: &str) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<Token>| {
        if !cur.is_empty() {
            out.push(Token::new(0, std::mem::take(cur)));
        }
    };
    for ch in text.chars() {
        if ch == '\n' {
            flush(&mut cur, &mut out);
            out.push(Token::new(0, "\n"));
        } else {
            if ch == ' ' {
                flush(&mut cur, &mut out);
            }
            cur.push(ch);
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// One turn wrapped in the template's control tokens, the way a chat template does it.
fn wrapped(role: &str, body: &str) -> Vec<Token> {
    let mut out = vec![
        Token::special(100_264, "<|im_start|>"),
        Token::new(0, role.to_string()),
        Token::special(100_266, "<|im_sep|>"),
    ];
    out.extend(fake_tokens(body));
    out.push(Token::special(100_265, "<|im_end|>"));
    out
}

/// The instructions and tool declarations the simulated agent opens with — everything above
/// the first turn, which is the part that never changes once a run has launched.
fn simulator_head() -> Vec<Token> {
    let mut out = wrapped(
        "system",
        "You review code in this repository. Read before you judge; every finding names a \
         file.\n\n# Where you are\n\nThis run starts in `/Users/mgorunuch/adi-family`.\n\n\
         # Your tools\n\nBash — run a command in your own shell, in your own cwd.\nRead — read \
         a file from the local filesystem.",
    );
    out.extend(wrapped("user", "Review the runner refactor on main."));
    out
}

/// The staging area on its own, with a block of each kind already in it.
#[component]
fn StagingDemo() -> impl IntoView {
    let staged = RwSignal::new(vec![
        Block::text(
            "Looking at the runner split now. `detached.rs` still composes the prompt itself, \
             so I'll read it before saying anything about the refactor.",
        ),
        Block::call(
            "Bash",
            vec![
                ("command".into(), "rg -n 'fn own_prompt' crates/adi-agents/src".into()),
                ("timeout_ms".into(), "30000".into()),
            ],
        ),
        Block::call("Read", vec![("path".into(), "crates/adi-agents/src/runner/prompt.rs".into())]),
    ]);
    view! {
        <TurnBlocks
            blocks=Signal::derive(move || staged.get())
            on_drop=Callback::new(move |i: usize| staged.update(|b| { b.remove(i); }))
        />
    }
}

/// A passage worth flagging, and the list it collects into.
#[component]
fn FlagDemo() -> impl IntoView {
    let flags = RwSignal::new(Vec::<Flag>::new());
    let tokens = Signal::stored(wrapped(
        "system",
        "# Your tools\n\nBash — run a command. Be careful with it.\n\nYou should probably \
         prefer the smallest change that works, unless a bigger one is better. Always be \
         helpful.",
    ));
    view! {
        <div class="flex flex-col gap-4">
            <FlagMark
                on_flag=Callback::new(move |quote: String| {
                    flags.update(|f| f.push(Flag::new(quote)));
                })
            >
                <PromptText
                    tokens=tokens
                    class="rounded-sm border border-edge bg-panel-alt p-3"
                />
            </FlagMark>
            <FlagList
                flags=Signal::derive(move || flags.get())
                on_drop=Callback::new(move |i: usize| flags.update(|f| { f.remove(i); }))
            />
        </div>
    }
}

/// The whole flow, wired to itself: stage blocks, end the turn, watch the prompt grow.
///
/// It fakes exactly one thing — executing a call, which here returns a canned line instead of
/// really running. Everything else is the shape the real screen has, including that ending a
/// turn is what appends anything to the prompt at all.
#[component]
fn SimulatorDemo() -> impl IntoView {
    let tools = agent_tools();
    let convo = RwSignal::new(Vec::<Token>::new());
    let staged = RwSignal::new(Vec::<Block>::new());
    let stop = RwSignal::new(None::<Stop>);
    let flags = RwSignal::new(Vec::<Flag>::new());

    let prompt = Signal::derive(move || {
        let mut all = simulator_head();
        all.extend(convo.get());
        all
    });

    let for_call = tools.clone();
    let on_call = Callback::new(move |name: String| {
        let Some(tool) = for_call.iter().find(|t| t.name == name) else {
            return;
        };
        staged.update(|b| b.push(Block::call(&tool.name, args_of(tool))));
    });

    let on_end_turn = Callback::new(move |()| {
        let blocks = staged.get_untracked();
        if blocks.is_empty() {
            return;
        }
        // The turn goes into the prompt as one assistant message, then every call's result
        // comes back as its own — which is the order the runner appends them in, and the
        // reason a turn with two calls produces two results before anyone is asked again.
        let mut body = String::new();
        for block in &blocks {
            if !body.is_empty() {
                body.push_str("\n\n");
            }
            match block {
                Block::Text(text) => body.push_str(text),
                Block::Call { name, params } => {
                    body.push_str(&format!("<invoke name=\"{name}\">\n"));
                    for (k, v) in params {
                        body.push_str(&format!("  <parameter name=\"{k}\">{v}</parameter>\n"));
                    }
                    body.push_str("</invoke>");
                }
            }
        }
        let mut turn = wrapped("assistant", &body);
        for block in &blocks {
            if let Block::Call { name, .. } = block {
                turn.extend(wrapped(
                    "tool",
                    &format!("<result>\n{name} ran here. The playground does not.\n</result>"),
                ));
            }
        }
        convo.update(|c| c.extend(turn));
        stop.set(Some(Stop::of(&blocks)));
        staged.set(Vec::new());
    });

    view! {
        <SimulatorShell
            prompt=prompt
            staged=staged
            tools=tools
            stop=stop
            flags=flags
            on_call=on_call
            on_end_turn=on_end_turn
            convo=convo
        />
    }
}

/// The callbacks that are one line each, kept out of [`SimulatorDemo`] so its own logic reads.
#[component]
fn SimulatorShell(
    prompt: Signal<Vec<Token>>,
    staged: RwSignal<Vec<Block>>,
    tools: Vec<ToolDecl>,
    stop: RwSignal<Option<Stop>>,
    flags: RwSignal<Vec<Flag>>,
    on_call: Callback<String>,
    on_end_turn: Callback<()>,
    convo: RwSignal<Vec<Token>>,
) -> impl IntoView {
    view! {
        <Simulator
            prompt=prompt
            blocks=Signal::derive(move || staged.get())
            tools=Signal::stored(tools)
            stop=Signal::derive(move || stop.get())
            flags=Signal::derive(move || flags.get())
            encoding="o200k_base (faked here)"
            on_text=Callback::new(move |text: String| staged.update(|b| b.push(Block::Text(text))))
            on_call=on_call
            on_drop_block=Callback::new(move |i: usize| staged.update(|b| { b.remove(i); }))
            on_end_turn=on_end_turn
            on_flag=Callback::new(move |quote: String| flags.update(|f| f.push(Flag::new(quote))))
            on_unflag=Callback::new(move |i: usize| flags.update(|f| { f.remove(i); }))
            on_user=Callback::new(move |text: String| {
                convo.update(|c| c.extend(wrapped("user", &text)));
                // The person has answered, so the seat is the model's again and there is no
                // last turn to report a reason for.
                stop.set(None);
            })
        />
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

| Block   | Written as         | Inline spans |
| :------ | :----------------: | -----------: |
| Heading | `## Title`         |          yes |
| Fence   | three backticks    |           no |
| Table   | `\| a \| b \|`     |          yes |

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


/// A machine, for [`PathPicker`] to browse. Every path the fake `read_dir` below knows
/// about: a trailing slash makes it a directory, and the directories in between are implied
/// rather than listed, exactly as they would be on a real disk.
const DISK: &[&str] = &[
    "/Users/you/adi-family/crates/adi-ui/src/",
    "/Users/you/adi-family/crates/adi-ui/styles/",
    "/Users/you/adi-family/crates/adi-ui/fonts/",
    "/Users/you/adi-family/crates/adi-ui/Cargo.toml",
    "/Users/you/adi-family/crates/adi-ui/README.md",
    "/Users/you/adi-family/crates/adi-app/src/",
    "/Users/you/adi-family/crates/adi-agents/src/",
    "/Users/you/adi-family/crates/adi-indexer/src/",
    "/Users/you/adi-family/crates/adi-css/styles/",
    "/Users/you/adi-family/crates/adi-webapp/src/",
    "/Users/you/adi-family/apps/windows/",
    "/Users/you/adi-family/docs/indexer.md",
    "/Users/you/adi-family/scripts/build-app.sh",
    "/Users/you/adi-family/Cargo.toml",
    "/Users/you/adi-family/CLAUDE.md",
    "/Users/you/adi-family/README.md",
    "/Users/you/Documents/",
    "/Users/you/Downloads/",
    "/Users/you/.ssh/",
    "/Users/you/.zshrc",
    "/Users/guest/",
    "/etc/hosts",
    "/tmp/",
];

/// One directory's children, from [`DISK`]: the next segment of every path under it, once
/// each. Directories sort first and then alphabetically — an order the picker keeps, since
/// what a listing should be sorted by is the lister's business.
fn read_dir(dir: &str) -> Vec<DirEntry> {
    let prefix = if dir.ends_with('/') {
        dir.to_string()
    } else {
        format!("{dir}/")
    };
    let mut found: Vec<(String, bool)> = Vec::new();
    for path in DISK {
        let Some(rest) = path.strip_prefix(&prefix) else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        let (head, is_dir) = match rest.find('/') {
            Some(i) => (&rest[..i], true),
            None => (rest, false),
        };
        if !found.iter().any(|(name, _)| name == head) {
            found.push((head.to_string(), is_dir));
        }
    }
    found.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    found
        .into_iter()
        .map(|(name, is_dir)| {
            if is_dir {
                DirEntry::dir(name)
            } else {
                DirEntry::file(name)
            }
        })
        .collect()
}

/// The picker, wired to [`read_dir`] the way a real screen wires it to an API: it says which
/// directory it wants ([`dir_of`] of what is typed), and this answers — after a beat, so the
/// reading state is a state you can actually see, and with a refusal for `~/.ssh`, because a
/// path you are not allowed to read is the normal way to meet the error line.
#[component]
fn PathDemo(#[prop(optional)] inline: bool) -> impl IntoView {
    let path = RwSignal::new(String::from("/Users/you/adi-family/crates/"));
    let entries = RwSignal::new(read_dir("/Users/you/adi-family/crates"));
    let reading = RwSignal::new(false);
    let refused = RwSignal::new(None::<String>);
    let picked = RwSignal::new(String::new());

    // The one directory the picker ever needs read. Crossing a separator is what changes it,
    // so typing inside a folder filters without costing a read.
    let dir = Signal::derive(move || dir_of(&path.get()).to_string());
    Effect::new(move |_| {
        let want = dir.get();
        reading.set(true);
        refused.set(None);
        set_timeout(
            move || {
                if want.ends_with("/.ssh") {
                    entries.set(Vec::new());
                    refused.set(Some(format!("{want}: permission denied")));
                } else {
                    entries.set(read_dir(&want));
                }
                reading.set(false);
            },
            Duration::from_millis(260),
        );
    });

    let picker = move || {
        view! {
            <PathPicker
                value=path
                entries=entries
                loading=reading
                error=refused
                inline=inline
                roots=vec![
                    PathRoot::new("Home", "/Users/you"),
                    PathRoot::new("Repo", "/Users/you/adi-family"),
                    PathRoot::new("Root", "/"),
                ]
                on_pick=Callback::new(move |dir: String| picked.set(dir))
            />
        }
    };

    view! {
        <div class="flex flex-col gap-2">
            {if inline {
                view! { <div class="max-w-100">{picker()}</div> }.into_any()
            } else {
                view! {
                    <Field label="Working directory" hint="Where the agent runs its commands.">
                        <div class="max-w-100">{picker()}</div>
                    </Field>
                }
                    .into_any()
            }}
            <Flash kind=FlashKind::Ok class="bg-transparent px-0">
                {move || {
                    let chosen = picked.get();
                    if chosen.is_empty() {
                        String::from("Nothing picked yet.")
                    } else {
                        format!("on_pick → {chosen}")
                    }
                }}
            </Flash>
        </div>
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

/// A log that grows on demand, because the follow is only interesting against output that
/// is still arriving. Append with the box scrolled to the bottom and it comes with you;
/// scroll up first, append again, and it stays where you left it.
#[component]
fn CodeLogDemo() -> impl IntoView {
    const LINES: &[&str] = &[
        "   Compiling adi-css v0.1.0",
        "   Compiling adi-ui v0.1.0",
        "warning: unused import: `std::fmt`",
        "  --> crates/adi-app/src/serve.rs:11:5",
        "   Compiling adi-webapp v0.1.0",
        "   Compiling adi-app v0.1.0",
        "    Finished `release` profile [optimized] target(s) in 51.02s",
        "$ ./target/release/adi-app 8000",
        "listening on 127.0.0.1:8000",
        "GET /api/health 200 0.4ms",
    ];
    let buffer = RwSignal::new(String::from("$ cargo build --release -p adi-app"));
    let at = RwSignal::new(0usize);

    view! {
        <div class="flex flex-col items-start gap-2">
            <CodeLog value=buffer lang=Lang::Sh height=CodeHeight::Form class="island w-full"/>
            <Button
                size=ButtonSize::Small
                variant=ButtonVariant::Ghost
                on:click=move |_| {
                    // Wrap rather than run dry, so the demo never needs resetting.
                    let i = at.get_untracked();
                    buffer.update(|b| {
                        b.push('\n');
                        b.push_str(LINES[i % LINES.len()]);
                    });
                    at.set(i + 1);
                }
            >
                "Append a line"
            </Button>
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
            images: Vec::new(),
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
            images: Vec::new(),
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
            // Stopped mid-call: the run ended before this returned, so it reads as unanswered
            // rather than keeping a green "running" flag over a conversation that is over.
            ToolCall::new("Bash")
                .param("command", "cargo test -p adi-mesh --all-features")
                .param("description", "Run the whole mesh suite")
                .state(ToolState::Unanswered),
        ]),
        Turn::Said {
            role: Role::User,
            body: "Stopped that — just the pairing tests.".into(),
            images: Vec::new(),
        },
        Turn::Did(vec![
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
    // The transcript above ends on a call that is still running, so this box opens in the state
    // the Stop exists for. Pressing it settles the turn and the button goes with it.
    let answering = RwSignal::new(true);
    // One message already said but not yet asked, so the hollow bubble has something to be.
    let queued = RwSignal::new(true);
    // The attachment tray, with no server behind it: a file picked here goes straight to Ready on
    // its own object URL. That is enough to develop every part of this that is not the upload —
    // the paste, the drop, the picker, the ✕, and a send button that lights up for a message with
    // a picture and no words.
    let files = RwSignal::new(Vec::<Attached>::new());
    let attach = Attaching {
        files,
        on_files: Callback::new(move |picked: Vec<web_sys::File>| {
            for (n, file) in picked.into_iter().enumerate() {
                let url = web_sys::Url::create_object_url_with_blob(file.as_ref())
                    .unwrap_or_default();
                files.update(|list| {
                    list.push(Attached {
                        key: format!("demo-{}-{n}", list.len()),
                        name: file.name(),
                        preview: url.clone(),
                        state: AttachState::Ready(url),
                    });
                });
            }
        }),
        can_attach: Signal::derive(|| true),
        refusal: Signal::derive(String::new),
    };

    view! {
        <div class="flex flex-col gap-3">
            // The composer sits above the transcript, not below it: newest is at the top, so
            // the box you type into belongs next to where your message will appear.
            <Composer
                value=draft
                busy=false
                attach=attach
                on_send=Callback::new(move |text: String| {
                    sent.set(text);
                    draft.set(String::new());
                    files.set(Vec::new());
                })
                stoppable=answering
                on_stop=Callback::new(move |()| answering.set(false))
            />
            <Show when=move || !sent.get().is_empty()>
                <Flash kind=FlashKind::Ok>
                    {move || format!("sent: {}", sent.get())}
                </Flash>
            </Show>
            <Show when=move || !answering.get()>
                <Flash kind=FlashKind::Ok>"stopped — the turn was cut short"</Flash>
            </Show>
            // A queued message sits between the composer and the transcript, which is where
            // the newest-first order puts a thing that has not happened yet. Its × takes it
            // back; press it and the bubble goes, which is the whole of the interaction.
            <Show when=move || queued.get()>
                <div class="px-1">
                    <Queued
                        body="And once that lands, run the migration against staging.".to_string()
                        on_unqueue=Callback::new(move |()| queued.set(false))
                    />
                </div>
            </Show>
            // Keyed by position, which is only ever right for a transcript like this one:
            // fixed, and never added to while it is on screen.
            <Chat turns=adi_ui::by_position(turns.clone()) class="max-h-140 p-1"/>
        </div>
    }
}

/// The question card, in both of its shapes: the one-question ask a click settles outright, and
/// the batched one that has to be read before it can be sent.
#[component]
fn AskDemo() -> impl IntoView {
    let answered = RwSignal::new(String::new());
    let batch_answered = RwSignal::new(String::new());
    view! {
        <div class="flex flex-col gap-4">
            <Ask
                note="The migration touches `orders`, which is 40M rows. Both ways work; they \
                      fail differently."
                questions=vec![AskQuestion {
                    header: "Migration".into(),
                    question: "Alter in place, or write to a new table and swap?".into(),
                    options: vec![
                        AskOption {
                            label: "In place".into(),
                            description: "one statement, but locks writes for ~4 minutes".into(),
                        },
                        AskOption {
                            label: "New table".into(),
                            description: "no lock, but doubles disk until the swap".into(),
                        },
                    ],
                    multi_select: false,
                }]
                deadline_note="takes its own default in 12m"
                on_answer=Callback::new(move |replies: Vec<String>| {
                    answered.set(replies.join(" | "));
                })
            />
            <Show when=move || !answered.get().is_empty()>
                <Flash kind=FlashKind::Ok>
                    {move || format!("answered: {}", answered.get())}
                </Flash>
            </Show>

            <Ask
                questions=vec![
                    AskQuestion {
                        header: "Rollout".into(),
                        question: "Which environments should this go to first?".into(),
                        options: vec![
                            AskOption { label: "staging".into(), description: String::new() },
                            AskOption { label: "eu-west".into(), description: String::new() },
                            AskOption { label: "us-east".into(), description: String::new() },
                        ],
                        multi_select: true,
                    },
                    AskQuestion {
                        header: "Rollback".into(),
                        question: "How long do I keep the old table before dropping it?".into(),
                        options: Vec::new(),
                        multi_select: false,
                    },
                ]
                on_answer=Callback::new(move |replies: Vec<String>| {
                    batch_answered.set(replies.join(" | "));
                })
            />
            <Show when=move || !batch_answered.get().is_empty()>
                <Flash kind=FlashKind::Ok>
                    {move || format!("answered: {}", batch_answered.get())}
                </Flash>
            </Show>
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

/// One reserved port, for the table demo. Deliberately mixed types: a number that must not
/// sort as text, a name that must, and a duration whose cell says `2h 14m` while its key is
/// seconds.
struct Reservation {
    port: u16,
    service: &'static str,
    pid: u32,
    uptime: u64,
    state: &'static str,
}

/// The columns this table can show. The trailing blank is the action column — not data, so
/// the settings menu never offers it.
const PORT_HEADERS: &[&str] = &["Port", "Service", "PID", "Uptime", "State", ""];

const PORTS: &[Reservation] = &[
    // The uptimes are chosen to make the sort key visible: as text, `9h 5m` sorts *after*
    // `53h 11m` — as a number it does not. A demo where both orders agree proves nothing.
    Reservation { port: 80, service: "adi-hive", pid: 4417, uptime: 191_460, state: "up" },
    Reservation { port: 8000, service: "adi-app", pid: 88_231, uptime: 8_040, state: "up" },
    Reservation { port: 9081, service: "adi-ui-playground", pid: 91_002, uptime: 92, state: "up" },
    Reservation { port: 15353, service: "adi.hive · dns", pid: 512, uptime: 356_460, state: "up" },
    Reservation { port: 45353, service: "adi-dns · scratch", pid: 0, uptime: 0, state: "down" },
    Reservation { port: 5432, service: "postgres", pid: 3301, uptime: 32_700, state: "idle" },
];

/// An uptime as `Ns` / `Nm Ss` / `Nh Mm` — the rendered cell, which is exactly what must not
/// be what the column sorts on.
fn fmt_uptime(s: u64) -> String {
    if s == 0 {
        "—".to_string()
    } else if s < 60 {
        format!("{s}s")
    } else if s < 3_600 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}h {}m", s / 3_600, (s % 3_600) / 60)
    }
}

/// A live table: click a header to sort, open the gear to show, hide and reorder columns.
/// Both survive a reload — `TableState` persists them under the key it was built with.
#[component]
fn PortsDemo() -> impl IntoView {
    let table = TableState::new("playground-ports", PORT_HEADERS);

    let rows = move || {
        let sort = table.sort.get();
        let mut order: Vec<&Reservation> = PORTS.iter().collect();
        sort_rows(
            &mut order,
            sort,
            |p, col| match col {
                // The number, never the string that was rendered from it: `9081` sorts after
                // `80`, and `2h 14m` after `92s`.
                "Port" => SortKey::num(u64::from(p.port)),
                "PID" => SortKey::num(u64::from(p.pid)),
                "Uptime" => SortKey::num(p.uptime),
                "State" => SortKey::text(p.state),
                _ => SortKey::text(p.service),
            },
            // Ties break on the port, ascending in both directions, so a re-sort never
            // reshuffles rows that compared equal.
            |p| SortKey::num(u64::from(p.port)),
        );
        order
            .into_iter()
            .map(|p| {
                view! {
                    <TableRow
                        state=table
                        cell=move |col| match col {
                            "Port" => {
                                view! { <span class="font-medium text-accent">{p.port}</span> }
                                    .into_any()
                            }
                            "Service" => view! { <span class="text-ink">{p.service}</span> }.into_any(),
                            "PID" => {
                                view! {
                                    <span class="font-mono text-mini text-meta">
                                        {if p.pid == 0 { "—".to_string() } else { p.pid.to_string() }}
                                    </span>
                                }
                                    .into_any()
                            }
                            "Uptime" => {
                                view! { <span class="text-meta">{fmt_uptime(p.uptime)}</span> }
                                    .into_any()
                            }
                            "State" => {
                                view! {
                                    <Badge tone=match p.state {
                                        "up" => BadgeTone::Online,
                                        "idle" => BadgeTone::Warn,
                                        _ => BadgeTone::Down,
                                    }>{p.state}</Badge>
                                }
                                    .into_any()
                            }
                            // A header the row builder does not know is its own business.
                            _ => ().into_any(),
                        }
                        actions=view! {
                            <Button size=ButtonSize::Small variant=ButtonVariant::Link>
                                "Free"
                            </Button>
                        }
                            .into_any()
                    />
                }
            })
            .collect_view()
    };

    view! { <Table state=table>{rows}</Table> }
}

/// The same table with nothing in it — the placeholder spans whatever the user is currently
/// showing, so hiding a column keeps it centred.
#[component]
fn EmptyTableDemo() -> impl IntoView {
    let table = TableState::new("playground-leases", &["Lease", "Owner", "Expires"]);
    view! {
        <Table state=table>
            <EmptyRow state=table>"No leases held."</EmptyRow>
        </Table>
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
    // The TopBar panel's miniature window: whether its row is open — what its mark resets.
    let opened = RwSignal::new(false);
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
                <div class="px-4 pt-2 text-mini text-meta">
                    "The mark has three shapes. With "<code>"home"</code>" it is a link out of \
                     here; with neither prop it is plain text, because a link to the page you \
                     are on does nothing. This one has "<code>"on_home"</code>": open the row \
                     below and click "<code>"adi."</code>" — a screen that keeps \"where you \
                     are\" in state rather than in the URL still owes you the way back, and \
                     the way back is putting it as it opened."
                </div>
                <div class="p-4">
                    // A window, in miniature: the bar's corners are clipped by the island
                    // around it, which is why that island owns the `overflow-hidden`.
                    <div class="island overflow-hidden bg-canvas">
                        <TopBar
                            logo="adi"
                            on_home=Callback::new(move |()| opened.set(false))
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
                            // Something to be got out of: with a row open the window is no
                            // longer as it opened, and the mark is what closes it again.
                            {move || if opened.get() {
                                view! {
                                    <div class="island flex h-20 flex-1 items-center \
                                                justify-center bg-card text-mini text-meta">
                                        "a row is open — click the mark"
                                    </div>
                                }
                                .into_any()
                            } else {
                                view! {
                                    <button
                                        type="button"
                                        class="island h-20 flex-1 cursor-pointer bg-card \
                                               text-mini text-meta hover:border-accent/60"
                                        on:click=move |_| opened.set(true)
                                    >
                                        "open a row"
                                    </button>
                                }
                                .into_any()
                            }}
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

            <Panel title="Kbd" flush=true>
                <div class="px-4 pt-3 text-mini text-meta">
                    "A shortcut, printed as a key cap. Quieter than a badge on purpose: it \
                     rides a row that already does the thing, and a list wearing forty of \
                     them is a list nobody scans. It renders the text it is handed — which \
                     platform's spelling that is belongs to the call site."
                </div>
                <div class="px-4">
                    <Row label="a key">
                        <Kbd>"\u{2318}1"</Kbd>
                        <Kbd>"\u{2318}K"</Kbd>
                        <Kbd>"Esc"</Kbd>
                        <Kbd>"Ctrl+9"</Kbd>
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
            <Panel title="Table" flush=true>
                <div class="px-4 pt-3 pb-3 text-mini text-meta">
                    "Live: click a header to sort it, click again to reverse, and open the \
                     gear to show, hide and reorder columns. Both are persisted, so the \
                     table is still arranged that way after a reload. What is compared is \
                     the "
                    <span class="font-mono">"SortKey"</span>
                    ", never the rendered cell — which is why Uptime puts "
                    <span class="font-mono">"9h 5m"</span>
                    " before "
                    <span class="font-mono">"53h 11m"</span>
                    " and Port puts "
                    <span class="font-mono">"9081"</span>
                    " before "
                    <span class="font-mono">"15353"</span>
                    ". Sort either column as text and both of those come out backwards. The \
                     last column is the action column: blank header, never offered by the \
                     gear, and shrink-wrapped so the data keeps the width."
                </div>
                <PortsDemo/>
            </Panel>

            <Panel title="Table · empty" flush=true>
                <div class="px-4 pt-3 pb-3 text-mini text-meta">
                    "The placeholder spans the columns the table is "
                    <em>"currently"</em>
                    " showing, so it stays centred after a column is hidden. This one has no \
                     action column, which is the other half of the gear's rule: three \
                     columns to arrange, so the gear is there — a one-column table has \
                     nothing to offer and shows none."
                </div>
                <EmptyTableDemo/>
            </Panel>

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

            <Panel title="CodeLog" flush=true>
                <div class="px-4 pt-3 text-mini text-meta">
                    "The read-only half: a log that grows under a poll. It follows the tail \
                     while you are at the bottom and stops the moment you scroll up, so \
                     history holds still while output keeps arriving — scroll back down and \
                     it picks the follow up again. A "
                    <span class="font-mono">"<pre>"</span>
                    ", not a read-only editor: writing a textarea's value resets its scroll, \
                     which under a poll means being yanked to the top every second."
                </div>
                <div class="p-4">
                    <CodeLogDemo/>
                </div>
            </Panel>

            <Panel title="PathPicker" flush=true>
                <div class="px-4 pt-3 text-mini text-meta">
                    "A directory, typed or browsed to \u{2014} and both at once, because the two \
                     halves are one value. Paste a path and the list is already inside it; \
                     click a folder and the text grows a segment. Typing filters where you \
                     are without moving you, and typing a "
                    <span class="font-mono">"/"</span>
                    " steps in. The keyboard does the whole set: "
                    <span class="font-mono">"\u{2193}\u{2191}"</span>
                    " walk the folders and skip the files, "
                    <span class="font-mono">"Enter"</span>
                    " steps into the highlighted one or picks where you are, "
                    <span class="font-mono">"Tab"</span>
                    " completes as far as the names agree, "
                    <span class="font-mono">"Esc"</span>
                    " puts the list away. Try "
                    <span class="font-mono">"~/.ssh"</span>
                    " for the refusal, and "
                    <span class="font-mono">"Documents"</span>
                    " for a folder with nothing in it."
                </div>
                <div class="grid gap-6 p-4 min-[880px]:grid-cols-2">
                    <div>
                        <div class="caps mb-2 text-faint">"in a form row"</div>
                        <PathDemo/>
                    </div>
                    <div>
                        <div class="caps mb-2 text-faint">"inline"</div>
                        <PathDemo inline=true/>
                    </div>
                </div>
            </Panel>

            <Panel title="TokenStream · PromptText" flush=true>
                <div class="px-4 pt-3 text-mini text-meta">
                    "The same tokens, twice. Above, every split shown — the boundary is the \
                     information, so the colour cycles by position and means nothing else. \
                     Below, the string as a person reads it, with only the template's control \
                     tokens marked. A newline is drawn "
                    <span class="font-mono">"⏎"</span>
                    " and then still taken, which is the only way a long prompt stays both \
                     legible and honest about where it breaks. Hover a chip for its id and its \
                     exact text — that is how you catch a leading space belonging to the next \
                     word."
                </div>
                <div class="flex flex-col gap-4 p-4">
                    <div>
                        <div class="caps mb-2 text-faint">"tokens"</div>
                        <TokenStream
                            tokens=Signal::derive(prompt_tokens)
                            class="max-h-60 overflow-auto rounded-sm border border-edge p-3"
                        />
                    </div>
                    <div>
                        <div class="caps mb-2 text-faint">"prompt"</div>
                        <PromptText
                            tokens=Signal::derive(prompt_tokens)
                            class="max-h-60 overflow-auto rounded-sm border border-edge p-3"
                        />
                    </div>
                </div>
            </Panel>

            <Panel title="ToolForm" flush=true>
                <div class="px-4 pt-3 text-mini text-meta">
                    "A tool's parameters, built from the schema the tool itself declares — so a \
                     parameter added to the tool shows up here rather than being quietly \
                     missing. Wide controls take their own row. Nothing here builds the call: \
                     the values are signals the caller owns, and the preview under the form is \
                     written by the caller too, because what goes on the wire belongs to \
                     whoever is sending it."
                </div>
                <div class="p-4">
                    <ToolFormDemo/>
                </div>
            </Panel>

            <Panel title="TurnBlocks · StopLine" flush=true>
                <div class="px-4 pt-3 text-mini text-meta">
                    "A model does not answer and then call a tool — it emits one turn made of \
                     blocks, and the turn is over when it stops emitting. So this is a list, \
                     and it is a staging area rather than history: nothing in it has happened, \
                     and any of it can still be dropped. A call is drawn from the same \
                     component the transcript draws a real one with, because a simulated call \
                     that looked different would be teaching the wrong shape on the one screen \
                     built to teach the right one."
                </div>
                <div class="flex flex-col gap-4 p-4">
                    <StagingDemo/>
                    <div>
                        <div class="caps mb-2 text-faint">"how a turn ends"</div>
                        <StopLine stop=Stop::ToolUse/>
                        <StopLine stop=Stop::EndTurn/>
                    </div>
                </div>
            </Panel>

            <Panel title="FlagMark · FlagList" flush=true>
                <div class="px-4 pt-3 text-mini text-meta">
                    "Select any of the text below with the mouse or with Shift+arrows. The \
                     offer follows the selection, quotes it as it read at the time — a copy, \
                     not an offset into a document that is about to be edited — and drops a \
                     note field under it. Nothing else is asked for: a form standing between \
                     noticing something and recording it loses most of what gets noticed."
                </div>
                <div class="p-4">
                    <FlagDemo/>
                </div>
            </Panel>

            <Panel title="Simulator" flush=true>
                <div class="px-4 pt-3 text-mini text-meta">
                    "The whole flow, wired to itself. Left is what the model sees — one \
                     document, instructions and tools and every turn so far, because to a model \
                     there is no separate transcript. Right is what the model does. Stack a \
                     block or two and press "
                    <span class="font-mono">"end turn"</span>
                    ": with a call in it the results append and the loop comes back to you as \
                     the model; without one the run yields and the bottom composer wakes up so \
                     you can answer as yourself. Only the execution is faked here — the real \
                     screen calls the runner's own tools."
                </div>
                <div class="p-4">
                    <SimulatorDemo/>
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

            <Panel title="Ask" flush=true>
                <div class="px-4 pt-3 text-mini text-meta">
                    "What a run puts up when it needs a person to decide something. It is the \
                     visible half of a stored question: while the card is there the conversation \
                     is blocked, and when it goes the answer is a turn like any other. One \
                     question with a fixed set of answers sends on the click — an answer somebody \
                     taps is an answer you get. Anything longer has to be read first, so it gets \
                     a button, and Send lights up as soon as *any* question has an answer rather \
                     than holding the conversation hostage to the one nobody can settle. Every \
                     question keeps a free-text box, because “neither, do this instead” is \
                     regularly the right answer."
                </div>
                <div class="p-4">
                    <AskDemo/>
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

            <FactsPanel/>
        </main>
    }
}

// ---------------------------------------------------------------------------------------
// facts
// ---------------------------------------------------------------------------------------

/// The facts the pair fixtures are built from. Real sentences off the measured base, because
/// the pair that decides the whole design — 0.886, rank 6 — only makes its point in its own
/// words.
fn cis() -> Fact {
    Fact::new("f091", "The company supports all countries except the CIS.")
        .by("igor", "agent:chat@1")
        .at(2)
}

fn ukraine() -> Fact {
    Fact::new("f104", "Within the CIS, the company supports Ukraine.").by("igor", "agent:chat@1")
}

fn china_market() -> Fact {
    Fact::new("f044", "China is one of the operator's main target markets.")
        .by("igor", "agent:extractor@1")
}

fn china_great() -> Fact {
    Fact::new("n#7", "China is a great market.").by("igor", "agent:chat@1")
}

fn china_unsure() -> Fact {
    Fact::new("f038", "The company is not sure it can enter the China market.")
        .by("igor", "agent:extractor@1")
}

fn china_can() -> Fact {
    Fact::new("n#9", "The company can support China after all.").by("igor", "agent:chat@1")
}

fn plan() -> Fact {
    Fact::new("a012", "Market entry plan: skip China for now, open the EU first.")
        .by("igor", "agent:planner@2")
        .at(3)
        .kind(NodeKind::Artifact)
}

/// The pending list of an open transaction: one of each relation, and the two cases that need
/// more than a click.
fn demo_pairs() -> Vec<Pair> {
    vec![
        Pair::new(
            "p1",
            0.886,
            Relation::Narrows,
            PairSide::staged(cis()),
            PairSide::base(ukraine()),
        )
        .reason("one excludes the CIS, the other carves Ukraine out of it"),
        Pair::new(
            "p2",
            0.821,
            Relation::Duplicate,
            PairSide::staged(china_great()),
            PairSide::base(china_market()),
        )
        .reason("both name China as a market the company wants"),
        Pair::new(
            "p3",
            0.712,
            Relation::Controversy,
            PairSide::staged(china_can()),
            PairSide::base(china_unsure()),
        )
        .reason("one asserts capability, the other doubts it"),
        // Both sides staged: `drop` has no base fact to point at, so the card asks which one
        // lands instead of guessing.
        Pair::new(
            "p4",
            0.664,
            Relation::Duplicate,
            PairSide::staged(Fact::new("n#12", "The company was incorporated in Delaware.")
                .by("igor", "agent:chat@1")),
            PairSide::staged(Fact::new("n#13", "The company is a Delaware C-corp.")
                .by("igor", "agent:chat@1")),
        )
        .reason("the same incorporation, said twice"),
    ]
}

/// Every component in the facts family, in every state it has.
#[component]
fn FactsPanel() -> impl IntoView {
    // The queue is live here: a ruling marks its pair decided in place rather than removing
    // it, which is what the real screen does too — a pair that vanished when you decided it
    // would take the record of the decision with it.
    let pairs = RwSignal::new(demo_pairs());
    let rule = Callback::new(move |r: Ruling| {
        pairs.update(|list| {
            if let Some(p) = list.iter_mut().find(|p| p.id == r.pair) {
                p.decided = Some(Decided::new(r.verdict, "igor"));
            }
        });
    });
    let open = Signal::derive(move || pairs.get().iter().filter(|p| p.decided.is_none()).count());
    let reset = move |_| pairs.set(demo_pairs());

    let stale = RwSignal::new(vec![Stale::new(
        plan(),
        vec![
            Moved::new(
                "f038",
                "The company is not sure it can enter the China market.",
                "The company can support China after all.",
            )
            .versions(1, 2),
            Moved::new(
                "f091",
                "The company supports all countries.",
                "The company supports all countries except the CIS.",
            )
            .versions(1, 2),
        ],
    )]);

    let history = RwSignal::new(vec![
        Change::rewritten(
            2,
            Verdict::Supersede,
            "igor",
            "The company supports all countries.",
            "The company supports all countries except the CIS.",
        ),
        Change::created("agent:chat@1", "The company supports all countries."),
    ]);

    view! {
        <Panel title="Facts \u{2014} the pair" flush=true>
            <div class="px-4 pt-3 text-mini text-meta">
                "The decision atom. Two facts of equal weight, the classifier's guess and its \
                 strength as a plain number, its reason underneath and clearly labelled as \
                 its own, and four verdicts of equal weight \u{2014} `coexist` among them, \
                 because confirming that both are true is a decision. Click a card and the \
                 keys work: c / m / s / d rule it, \u{2193}\u{2191} walk the queue."
            </div>
            <div class="flex flex-col gap-3 p-4">
                // One card per relation, so all three tones are on screen at once.
                <PairCard
                    pair=Pair::new(
                        "x1",
                        0.886,
                        Relation::Narrows,
                        PairSide::staged(cis()),
                        PairSide::base(ukraine()),
                    )
                        .reason("one excludes the CIS, the other carves Ukraine out of it")
                    rank=6
                    on_rule=Callback::new(|_: Ruling| ())
                />
                <PairCard
                    pair=Pair::new(
                        "x2",
                        0.712,
                        Relation::Controversy,
                        PairSide::staged(china_can()),
                        PairSide::base(china_unsure()),
                    )
                        .reason("one asserts capability, the other doubts it")
                    rank=31
                    on_rule=Callback::new(|_: Ruling| ())
                />
                <PairCard
                    pair=Pair::new(
                        "x3",
                        0.821,
                        Relation::Duplicate,
                        PairSide::staged(china_great()),
                        PairSide::base(china_market()),
                    )
                    rank=12
                    on_rule=Callback::new(|_: Ruling| ())
                />
                // A reason that is about something else. The check is the caller's — whether
                // the stated reason names the facts it was handed — and it is free.
                <PairCard
                    pair=Pair::new(
                        "x4",
                        0.402,
                        Relation::Controversy,
                        PairSide::staged(
                            Fact::new("n#31", "The plan includes launching a website.")
                                .by("igor", "agent:chat@1"),
                        ),
                        PairSide::base(
                            Fact::new("f077", "The company can support China.")
                                .by("igor", "agent:extractor@1"),
                        ),
                    )
                        .reason(
                            "supporting all non-sanctioned countries conflicts with supporting \
                             China",
                        )
                    rank=4279
                    reason_suspect=true
                    on_rule=Callback::new(|_: Ruling| ())
                />
                // Settled. Every verdict carries its confirmer, so a card that is done shows
                // who did it and offers nothing further.
                <PairCard
                    pair=Pair::new(
                        "x5",
                        0.617,
                        Relation::Duplicate,
                        PairSide::staged(china_great()),
                        PairSide::base(china_market()),
                    )
                        .decided(Decided::new(Verdict::Merge, "agent:verifier@3"))
                    on_rule=Callback::new(|_: Ruling| ())
                />
                <PairCard
                    pair=Pair::new(
                        "x6",
                        0.664,
                        Relation::Narrows,
                        PairSide::staged(cis()),
                        PairSide::base(ukraine()),
                    )
                        .decided(Decided::new(Verdict::Drop, "igor"))
                    on_rule=Callback::new(|_: Ruling| ())
                />
                <PairCard
                    pair=Pair::new(
                        "x7",
                        0.886,
                        Relation::Narrows,
                        PairSide::staged(cis()),
                        PairSide::base(ukraine()),
                    )
                        .decided(Decided::new(Verdict::Coexist, "igor"))
                    on_rule=Callback::new(|_: Ruling| ())
                />
                <PairCard
                    pair=Pair::new(
                        "x8",
                        0.712,
                        Relation::Controversy,
                        PairSide::staged(china_can()),
                        PairSide::base(china_unsure()),
                    )
                        .decided(Decided::new(Verdict::Supersede, "agent:verifier@3"))
                    on_rule=Callback::new(|_: Ruling| ())
                />
            </div>
        </Panel>

        <Panel
            title="Facts \u{2014} the transaction"
            flush=true
            actions=move || view! {
                <Button size=ButtonSize::Small variant=ButtonVariant::Ghost on:click=reset>
                    "reset"
                </Button>
            }
                .into_any()
        >
            <div class="px-4 pt-3 pb-3 text-mini text-meta">
                "The queue, live: rule on a card and it keeps its place wearing its verdict, \
                 the count drops, and the commit unlocks only when nothing is open. The \
                 truncation line is drawn either way \u{2014} \"nothing more\" and \"we \
                 stopped looking\" are different facts."
            </div>
            <div class="px-4 pb-4">
                // The other half of the truncation line: nothing was left out, and the queue
                // says so rather than saying nothing.
                <TxPanel
                    id="tx_0c11e2"
                    staged=1
                    pending=1
                    on_commit=Callback::new(|()| ())
                    class="mb-4"
                >
                    <PairQueue
                        pairs=vec![Pair::new(
                            "solo",
                            0.617,
                            Relation::Duplicate,
                            PairSide::staged(china_great()),
                            PairSide::base(china_market()),
                        )]
                        acting_as="agent:verifier@3"
                        on_rule=Callback::new(|_: Ruling| ())
                    />
                </TxPanel>
                <TxPanel
                    id="tx_7f3a91"
                    staged=12
                    pending=open
                    busy=false
                    on_commit=Callback::new(|()| ())
                    on_abort=Callback::new(|()| ())
                >
                    <PairQueue
                        pairs=pairs
                        acting_as="igor"
                        truncated=Some(Truncated::new(214, 0.601))
                        on_rule=rule
                    />
                </TxPanel>
            </div>
        </Panel>

        <Panel title="Facts \u{2014} the node" flush=true>
            <div class="px-4">
                <Row label="rows">
                    <div class="flex w-full min-w-0 flex-col">
                        <FactRow fact=cis()/>
                        <FactRow fact=ukraine() selected=true/>
                        <FactRow fact=Fact::new(
                            "c003",
                            "The company supports every country outside the CIS, and Ukraine \
                             inside it.",
                        )
                            .by("igor", "agent:composer@1")
                            .at(4)
                            .kind(NodeKind::Composed)/>
                        <FactRow fact=plan()>
                            <Badge tone=BadgeTone::Warn mono=true>"stale"</Badge>
                        </FactRow>
                    </div>
                </Row>
                <Row label="card">
                    <div class="w-full min-w-0">
                        <FactCard
                            fact=cis()
                            actions=move || view! {
                                <Button size=ButtonSize::Small variant=ButtonVariant::Ghost>
                                    "history"
                                </Button>
                            }
                                .into_any()
                        />
                    </div>
                </Row>
                <Row label="card · derived">
                    <div class="w-full min-w-0">
                        <FactCard fact=plan()/>
                    </div>
                </Row>
            </div>
        </Panel>

        <Panel title="Facts \u{2014} stale, and history" flush=true>
            <div class="px-4 pt-3 text-mini text-meta">
                "Both are was/now surfaces. Which fact moved is not the question \u{2014} \
                 whether the derived text still holds is, and only the two sentences answer it."
            </div>
            <div class="px-4">
                <Row label="stale">
                    <div class="w-full min-w-0">
                        <StaleList
                            items=stale
                            on_refresh=Callback::new(move |_: String| stale.set(Vec::new()))
                        />
                    </div>
                </Row>
                <Row label="stale · ro">
                    <div class="w-full min-w-0">
                        <StaleList items=vec![Stale::new(plan(), vec![Moved::new(
                            "f038",
                            "The company is not sure it can enter the China market.",
                            "The company can support China after all.",
                        )])]/>
                    </div>
                </Row>
                <Row label="stale · empty">
                    <div class="w-full min-w-0">
                        <StaleList items=Vec::new()/>
                    </div>
                </Row>
                <Row label="history · old">
                    <div class="w-full min-w-0">
                        <FactHistory fact=cis() changes=history against=Some(1)/>
                    </div>
                </Row>
                <Row label="history · now">
                    <div class="w-full min-w-0">
                        <FactHistory fact=cis() changes=history against=Some(2)/>
                    </div>
                </Row>
                <Row label="history · merge">
                    <div class="w-full min-w-0">
                        <FactHistory
                            fact=Fact::new(
                                "f044",
                                "China is one of the operator's main target markets, and a \
                                 great one.",
                            )
                                .by("igor", "agent:extractor@1")
                                .at(2)
                            changes=vec![
                                Change::rewritten(
                                    2,
                                    Verdict::Merge,
                                    "agent:verifier@3",
                                    "China is one of the operator's main target markets.",
                                    "China is one of the operator's main target markets, and a \
                                     great one.",
                                ),
                                Change::created(
                                    "agent:extractor@1",
                                    "China is one of the operator's main target markets.",
                                ),
                            ]
                        />
                    </div>
                </Row>
                <Row label="history · v1">
                    <div class="w-full min-w-0">
                        <FactHistory
                            fact=ukraine()
                            changes=vec![Change::created(
                                "agent:chat@1",
                                "Within the CIS, the company supports Ukraine.",
                            )]
                        />
                    </div>
                </Row>
            </div>
        </Panel>
    }
}
