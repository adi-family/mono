//! The simulator: a run of an agent with **you** in the model's seat.
//!
//! The agent's own environment is materialized — its directory, its `.bin`, its PATH, its secrets —
//! the prompt is shown as the model receives it, and the tools you call really run. What this file
//! does is fetch and hold; everything on screen is [`adi_ui::Simulator`], and everything it shows
//! comes off the wire already composed, already split, already declared.
//!
//! **Nothing here builds a prompt or a tool list.** That is the rule the feature rests on: the
//! moment this page renders its own version of either, it is showing somebody a run that does not
//! happen. The one thing it *does* assemble is a call's arguments, and only because that is
//! deliberately the caller's half of the [`adi_ui::ToolForm`] bargain — the component owns the
//! fields, whoever sends the call owns its JSON.

use adi_webapp_api::types::{
    AgentSimBlock, AgentSimField, AgentSimFieldKind, AgentSimState, AgentSimTool, AgentToken,
};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;
use adi_ui::{
    Block, Flag, Param, ParamKind, Simulator, Stop, Token, ToolDecl,
};

use crate::fetch;
use crate::state::{Flash, Simulate, State};

/// Open a fresh simulated run of `name` on `message`, and show it.
pub(crate) fn start_simulation(state: State, sim: Simulate, name: String, message: String) {
    sim.name.set(Some(name.clone()));
    sim.busy.set(true);
    // A fresh run is a fresh reading: flags are about *this* prompt, and blocks staged against the
    // last one would be emitted into a conversation that never saw them.
    sim.blocks.set(Vec::new());
    sim.flags.set(Vec::new());
    sim.run.set(None);
    spawn_local(async move {
        match fetch::simulate_agent(name, message).await {
            Ok(next) => land(sim, next),
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        sim.busy.set(false);
    });
}

/// Re-read the open run from the server.
///
/// Not a poll, and deliberately a button: nothing about a simulated run changes on its own — the
/// seat is a person's, and it moves when they move it. One thing *can* change from outside, though,
/// and it is the reason this exists: `Ask` posts its question to the real user, and when they answer
/// it in the chat the run's transcript grows without this page doing anything.
pub(crate) fn reload_simulation(state: State, sim: Simulate) {
    let (Some(name), Some(run_id)) = (
        sim.name.get_untracked(),
        sim.run.get_untracked().map(|r| r.run_id),
    ) else {
        return;
    };
    sim.busy.set(true);
    spawn_local(async move {
        match fetch::simulate_prompt(name, run_id).await {
            Ok(next) => land(sim, next),
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        sim.busy.set(false);
    });
}

/// Close the simulator and forget what was open in it.
pub(crate) fn close_simulation(sim: Simulate) {
    sim.name.set(None);
    sim.run.set(None);
    sim.blocks.set(Vec::new());
    sim.flags.set(Vec::new());
}

/// Take a landed state, and rebuild the tool forms it implies.
///
/// The forms are rebuilt on every landing rather than diffed, and that is deliberate: a form holds
/// the arguments of a call that has now been *made*, and leaving them in place would let a second
/// click send the same call again without anyone retyping it.
fn land(sim: Simulate, next: AgentSimState) {
    sim.tools.set(next.tools.iter().map(tool_decl).collect());
    sim.run.set(Some(next));
}

/// One declared tool as the component's own, with a signal-backed field per parameter.
fn tool_decl(tool: &AgentSimTool) -> ToolDecl {
    ToolDecl::new(&tool.name, &tool.description)
        .params(tool.fields.iter().map(param).collect())
}

fn param(field: &AgentSimField) -> Param {
    let kind = match field.kind {
        AgentSimFieldKind::Line => ParamKind::Line,
        AgentSimFieldKind::Text => ParamKind::Text,
        AgentSimFieldKind::Number => ParamKind::Number,
        AgentSimFieldKind::List => ParamKind::List,
        AgentSimFieldKind::Flag => ParamKind::Flag,
    };
    let mut p = Param::new(&field.name, kind).hint(&field.hint);
    if field.required {
        p = p.required();
    }
    p
}

/// What a tool's fields currently hold, as the JSON of a call.
///
/// The caller's half of the bargain: [`adi_ui::ToolForm`] renders the fields and never guesses at
/// the wire form, because a component that did would be a second, drifting copy of the runner's own
/// serialization. An empty optional field is dropped rather than sent blank — a model with nothing
/// to say about a parameter omits it. An empty *required* one is sent as it stands, because a call
/// that omits something required is a real thing a model does, and watching the tool refuse is the
/// lesson.
fn call_input(tool: &ToolDecl) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for p in &tool.params {
        let value = match p.kind {
            ParamKind::Flag => {
                if !p.flag.get_untracked() {
                    continue;
                }
                serde_json::Value::Bool(true)
            }
            ParamKind::Number => {
                let text = p.text.get_untracked();
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                match text.parse::<i64>() {
                    Ok(n) => serde_json::Value::from(n),
                    // Not a number: sent as it was typed, so the tool says so rather than this
                    // page quietly deciding what was meant.
                    Err(_) => serde_json::Value::String(text.to_string()),
                }
            }
            ParamKind::List => {
                let text = p.text.get_untracked();
                let items: Vec<serde_json::Value> = text
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(|l| serde_json::Value::String(l.to_string()))
                    .collect();
                if items.is_empty() && !p.required {
                    continue;
                }
                serde_json::Value::Array(items)
            }
            ParamKind::Line | ParamKind::Text => {
                let text = p.text.get_untracked();
                if text.trim().is_empty() && !p.required {
                    continue;
                }
                serde_json::Value::String(text)
            }
        };
        out.insert(p.name.clone(), value);
    }
    serde_json::Value::Object(out)
}

/// One staged block as the wire's own.
fn wire_block(block: &Block) -> AgentSimBlock {
    match block {
        Block::Text(text) => AgentSimBlock::Text { text: text.clone() },
        Block::Call { name, params } => AgentSimBlock::Call {
            name: name.clone(),
            // Re-parsed from what was staged, so what is sent is what the row on screen says. The
            // staged form is the display form; this is the wire one, and the block is the only
            // thing between them that a person has actually looked at.
            input: staged_input(params),
        },
    }
}

/// A staged call's displayed arguments, back as JSON.
///
/// The staged row keeps values as text because that is what it draws. Numbers and booleans are
/// recovered here rather than being kept in a second parallel copy of the call — one truth on
/// screen, and it is the one that gets sent.
fn staged_input(params: &[(String, String)]) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for (key, value) in params {
        let typed = if value == "true" {
            serde_json::Value::Bool(true)
        } else if let Ok(n) = value.parse::<i64>() {
            serde_json::Value::from(n)
        } else {
            serde_json::Value::String(value.clone())
        };
        out.insert(key.clone(), typed);
    }
    serde_json::Value::Object(out)
}

/// A wire token as the component's own.
fn token(t: &AgentToken) -> Token {
    if t.special {
        Token::special(t.id, t.text.clone())
    } else {
        Token::new(t.id, t.text.clone())
    }
}

/// The whole document the model sees, as one stream of tokens.
///
/// A straight map, and deliberately nothing more. The stream already carries the conversation —
/// every turn, every call, and every result — because the server tokenized it with the same encoder
/// it tokenized the instructions with. Appending turns here instead would mean this page deciding
/// what a turn *looks like* to a model, and inventing token ids for text nobody split.
fn prompt_tokens(run: &AgentSimState) -> Vec<Token> {
    run.tokens.iter().map(token).collect()
}

/// The simulator, when one is open.
pub(crate) fn simulate_view(state: State, sim: Simulate) -> AnyView {
    let Some(name) = sim.name.get() else {
        return ().into_any();
    };
    let Some(run) = sim.run.get() else {
        // Opening: the run is being created and its prompt composed. Not an empty state — a
        // moment — so it says which of the two it is.
        return view! {
            <section class="adi-panel">
                <div class="adi-panel__head">
                    <strong>{format!("Simulating “{name}”")}</strong>
                </div>
                <div class="adi-empty">"Materializing the run and composing its prompt…"</div>
            </section>
        }
        .into_any();
    };

    let run_id = run.run_id.clone();
    let blocks = sim.blocks;
    let flags = sim.flags;
    let tools = sim.tools;
    let busy = sim.busy;

    // What the last turn stopped for. Empty before one has, which is `None` — there is no stop
    // reason for a turn that has not ended, and inventing one would put a claim on screen.
    let stop = {
        let reason = run.stop_reason.clone();
        Signal::derive(move || match reason.as_str() {
            "tool_use" => Some(Stop::ToolUse),
            "end_turn" => Some(Stop::EndTurn),
            _ => None,
        })
    };
    let tokens = {
        let run = run.clone();
        Signal::derive(move || prompt_tokens(&run))
    };
    let encoding = run.encoding.clone();

    let on_call = {
        let tools = tools;
        Callback::new(move |name: String| {
            let Some(tool) = tools.get_untracked().into_iter().find(|t| t.name == name) else {
                return;
            };
            let input = call_input(&tool);
            // Staged as the display form the row draws — see `staged_input` for why there is only
            // the one copy.
            let params = input
                .as_object()
                .map(|o| {
                    o.iter()
                        .map(|(k, v)| {
                            let text = match v {
                                serde_json::Value::String(s) => s.clone(),
                                other => other.to_string(),
                            };
                            (k.clone(), text)
                        })
                        .collect()
                })
                .unwrap_or_default();
            blocks.update(|b| b.push(Block::call(&tool.name, params)));
            // The fields are cleared, because the call has been made: leaving them filled invites a
            // second, identical call that nobody retyped.
            for p in &tool.params {
                p.text.set(String::new());
                p.flag.set(false);
            }
        })
    };

    let on_end_turn = {
        let (name, run_id) = (name.clone(), run_id.clone());
        Callback::new(move |()| {
            let staged: Vec<AgentSimBlock> =
                blocks.get_untracked().iter().map(wire_block).collect();
            if staged.is_empty() {
                return;
            }
            let (name, run_id) = (name.clone(), run_id.clone());
            busy.set(true);
            spawn_local(async move {
                match fetch::simulate_turn(name, run_id, staged).await {
                    Ok(turn) => {
                        // Cleared only once the turn has actually landed: a failed request must
                        // leave the blocks where they are rather than throwing the turn away.
                        blocks.set(Vec::new());
                        land(sim, turn.state);
                    }
                    Err(e) => state.flash.set(Some(Flash::err(e))),
                }
                busy.set(false);
            });
        })
    };

    let on_user = {
        let (name, run_id) = (name.clone(), run_id.clone());
        Callback::new(move |text: String| {
            let (name, run_id) = (name.clone(), run_id.clone());
            busy.set(true);
            spawn_local(async move {
                match fetch::simulate_reply(name, run_id, text).await {
                    Ok(next) => land(sim, next),
                    Err(e) => state.flash.set(Some(Flash::err(e))),
                }
                busy.set(false);
            });
        })
    };

    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <strong>{format!("Simulating “{name}”")}</strong>
                <span class="adi-chip adi-mono" title="Run id">{run_id.clone()}</span>
                <span class="adi-spacer"></span>
                <button
                    class="adi-btn"
                    type="button"
                    title="Re-read the run — an Ask answered in the chat lands here"
                    on:click=move |_| reload_simulation(state, sim)
                >
                    "Refresh"
                </button>
                <button
                    class="adi-btn"
                    type="button"
                    on:click=move |_| close_simulation(sim)
                >
                    "Close"
                </button>
            </div>
            <div class="adi-panel__body adi-ui-type">
                <Simulator
                    prompt=tokens
                    blocks=Signal::derive(move || blocks.get())
                    tools=Signal::derive(move || tools.get())
                    stop=stop
                    busy=Signal::derive(move || busy.get())
                    encoding=encoding
                    flags=Signal::derive(move || flags.get())
                    on_text=Callback::new(move |text: String| {
                        blocks.update(|b| b.push(Block::Text(text)));
                    })
                    on_call=on_call
                    on_drop_block=Callback::new(move |i: usize| {
                        blocks.update(|b| { b.remove(i); });
                    })
                    on_end_turn=on_end_turn
                    on_user=on_user
                    on_flag=Callback::new(move |quote: String| {
                        flags.update(|f| f.push(Flag::new(quote)));
                    })
                    on_unflag=Callback::new(move |i: usize| {
                        flags.update(|f| { f.remove(i); });
                    })
                />
            </div>
        </section>
    }
    .into_any()
}
