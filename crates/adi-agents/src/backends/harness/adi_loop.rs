//! `harness:adi` — ADI's own agentic loop over a chosen model provider.
//!
//! Unlike `harness:claude-sdk`, there is no vendor CLI: each conversation turn spawns
//! `adi-mono harness-turn --agent <name> --conv <id>`, which runs [`run_turn`]. That reads the
//! conversation's committed transcript, calls the configured provider's chat API with the whole
//! history, and writes the turn's events to stdout — which the detached machinery captures and
//! [`super::conversation`] folds into the transcript, exactly like a Claude turn. So continuation
//! here is transcript replay rather than a resumable session id.
//!
//! It speaks every provider the manifest can name: **Anthropic**'s Messages API, **`OpenAI`**,
//! **Monshoot** (Moonshot's Kimi) and **z.ai** (Zhipu's GLM) over the shared chat-completions
//! dialect, **Gemini**'s `generateContent`, and a local **Ollama**. The only thing [`validate`]
//! rejects is an agent that has not picked one.
//!
//! Each round below is a request written against one of these, and the field names are theirs. When
//! a provider moves a knob or renames a field, the page to open is:
//!
//! - Anthropic — <https://platform.claude.com/docs/en/api/messages>
//! - `OpenAI` — <https://platform.openai.com/docs/api-reference/chat/create>
//! - Monshoot (Moonshot / Kimi) — <https://platform.kimi.ai/docs/api/chat>
//! - z.ai (Zhipu / GLM) — <https://docs.z.ai/api-reference/llm/chat-completion>
//! - Gemini — <https://ai.google.dev/api/generate-content>
//! - Ollama — <https://github.com/ollama/ollama/blob/main/docs/api.md#generate-a-chat-completion>
//!
//! **A turn is a loop, not a call.** The model is offered the [`tools`](super::tools) every coding
//! agent has — `Read`, `Write`, `Edit`, `Bash`, `Glob`, `Grep` — and while it asks for them, this
//! runs them and asks again, up to [`MAX_ROUNDS`]. Running out of rounds is not a failure: the
//! model gets one last round with the tools withheld, so the turn still ends in an answer that
//! says what it did and what is left. The four providers disagree about how tools are declared,
//! how a call comes back, how a result is handed over, and how they are taken away again; [`Wire`]
//! is where those disagreements live, so the loop itself is written once and reads the same for
//! all of them.
//!
//! **And a turn that is running can still be spoken to.** Because the loop is here rather than
//! inside a vendor CLI, a message typed while the model is working does not have to wait for the
//! answer: [`tool_loop`] takes whatever the conversation's queue is holding at the top of every
//! round, so the longest anyone waits is one model call and the tools it asked for. That is what
//! makes correcting a turn possible at all — a wrong direction spotted at round three is worth
//! saying at round three, not once sixty rounds of it have been paid for.

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};

use crate::StoredAgent;
use crate::arguments::{
    HarnessAdiArguments, HarnessProvider, HarnessResponseFormat, HarnessThinking,
};
use crate::backends::adi_events::{self, Sink};
use crate::error::{Error, Result};
use crate::progress::TurnMetrics;
use crate::runner::ImageDelivery;

use super::tools;
use crate::store::{Attachment, Turn};

/// Anthropic requires an explicit output cap, so default one when the agent sets none.
const DEFAULT_MAX_TOKENS: u64 = 4096;
/// The thinking chat-completions models — Kimi's, and z.ai's GLM — reason out of the same output
/// budget as the reply, so a 4k cap routinely ends the turn mid-thought with an empty answer. The
/// chat-completions dialects' default is roomier; an agent that sets `max_tokens` still wins.
const OPENAI_DIALECT_DEFAULT_MAX_TOKENS: u64 = 16_384;
/// A generous per-round ceiling — a local model can be slow, and a round is one blocking call.
const HTTP_TIMEOUT: Duration = Duration::from_secs(600);
/// How many model round-trips one turn may spend *calling tools* before it is asked to wrap up.
/// An agent's `max_turns` overrides it.
///
/// It was 16, which is under what ordinary work costs: reading a handful of files, editing three
/// and running the build is already 15 round-trips, so real turns hit the ceiling mid-task and
/// died there. Turns on this machine routinely run past a hundred steps. The number still exists
/// because a model stuck in a call/retry rut has to cost a bounded amount of money and time — but
/// the bound belongs well above the work, not inside it.
const MAX_ROUNDS: u64 = 64;

/// What the model is told once its rounds are spent. It is not a scolding: the point is to get the
/// work it *did* do written down, in the same message that admits what it did not.
const WRAP_UP: &str = "You have used every tool call this turn allows, and no more are possible — \
                       the tools are no longer available to you. Write your final answer now from \
                       what you already know: what you did, what you found, and what still needs \
                       doing. Do not ask to run anything else.";

/// Where `adi-mono` actually is, for spawning: **beside the running executable**, falling back to
/// the bare name for a PATH lookup.
///
/// The spawner is usually `adi-app`, not `adi-mono`, so `current_exe` is the wrong binary — but it
/// is in the right *directory*, and every packaging puts the two side by side (a node's
/// `~/.local/adi/bin`, the macOS bundle's `Contents/Resources`). The bare name alone was not
/// enough: a `systemd --user` unit inherits the manager's bare PATH, which contains no adi
/// directory, so on a Linux node every agent turn died with "couldn't spawn adi-mono: No such file
/// or directory" while the panel that spawned it was running from exactly that directory.
pub(crate) fn adi_mono_program() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("adi-mono")))
        .filter(|candidate| candidate.is_file())
        .map_or_else(
            || "adi-mono".to_string(),
            |p| p.to_string_lossy().into_owned(),
        )
}

/// The command a conversation turn spawns for an `adi` agent: re-enter this binary's hidden
/// `harness-turn` subcommand, which reads the transcript and calls the provider.
pub(crate) fn argv(agent_name: &str, conv_id: &str) -> Vec<String> {
    vec![
        adi_mono_program(),
        "harness-turn".to_string(),
        "--agent".to_string(),
        agent_name.to_string(),
        "--conv".to_string(),
        conv_id.to_string(),
    ]
}

/// Whether the loop can actually run these arguments. Every provider the manifest can name is
/// implemented, so the only thing left to reject is an agent that hasn't picked one — which is
/// not-yet-configured rather than broken, hence `NotRunnable` and a hidden run button.
pub(crate) fn validate(args: &HarnessAdiArguments) -> Result<()> {
    match args.provider {
        None => Err(Error::NotRunnable("harness:adi".to_string())),
        Some(_) => Ok(()),
    }
}

/// Run one turn to completion, writing its events to `sink` as they happen and returning the
/// answer. Called from the spawned `adi-mono harness-turn` child (a plain sync process — the
/// blocking HTTP client must not run inside an async runtime).
pub(crate) fn run_turn(
    agent: &StoredAgent,
    sessions_dir: &Path,
    conv_id: &str,
    sink: Sink<'_>,
) -> Result<String> {
    let mut args = agent.manifest.typed_arguments::<HarnessAdiArguments>()?;
    validate(&args)?;
    // The runner composes this run's system prompt — the agent's own instructions with its tools'
    // help behind them — and exports it, because this loop has no command-line flag to carry one.
    // It wins over the stored arguments; a turn spawned by anything that exported nothing falls
    // back to them, which is exactly what happened before the runner existed.
    if let Some(prompt) = composed_prompt() {
        args.system_prompt = Some(prompt);
    }
    let model = args
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| {
            Error::Unsupported("the adi loop needs a model — set one on the agent".to_string())
        })?;

    // The committed transcript ends with the user turn this reply answers (the agent layer appended
    // it before spawning us). Read straight from the session store: the turn child and the process
    // that launched it are different processes sharing one directory, so the store *is* the channel.
    let store = crate::store::SessionStore::new(sessions_dir);
    let turns = store.turns(&agent.name, conv_id);
    if turns.iter().all(|t| t.text.trim().is_empty()) {
        return Err(Error::Process(
            "the conversation has no messages to answer".to_string(),
        ));
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let agent_dir = store.agent_dir(&agent.name);
    let ctx = tools::Ctx::for_conversation(
        &agent.name,
        agent.manifest.unattended,
        &cwd,
        conv_id,
        &agent_dir,
        store.clone(),
    );
    let wire = Wire::of(&args, model)?;
    let images = ImageStore::new(&store);
    tool_loop(&wire, &args, &turns, &ctx, &images, sink)
}

// ---- the loop ----------------------------------------------------------------------

/// One tool call the model asked for.
struct ToolCall {
    /// The provider's own id where it has one; a synthesized `call-N` where it doesn't (Gemini and
    /// Ollama identify a call only by position, so the loop supplies what the transcript needs).
    id: String,
    name: String,
    input: Value,
}

/// What running one call produced. `ok` false is a tool that failed, which is an answer the model
/// is expected to read and act on — not a failure of the turn.
struct ToolResult {
    call_id: String,
    name: String,
    output: String,
    ok: bool,
}

/// One round-trip's outcome, in provider-neutral terms.
struct Reply {
    /// What the model wrote this round: commentary when calls follow, the answer when none do.
    text: String,
    calls: Vec<ToolCall>,
    /// The assistant message exactly as the provider sent it. Echoed back verbatim on the next
    /// round, because every one of these APIs wants its own message shape returned unaltered.
    raw: Value,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

/// Ask, run what was asked for, and ask again — until the model answers without calling anything.
fn tool_loop(
    wire: &Wire<'_>,
    args: &HarnessAdiArguments,
    turns: &[Turn],
    ctx: &tools::Ctx<'_>,
    images: &ImageStore<'_>,
    sink: Sink<'_>,
) -> Result<String> {
    let allowance = args.max_turns.filter(|n| *n > 0).unwrap_or(MAX_ROUNDS);
    let mut max_rounds = allowance;
    let mut messages = wire.seed(turns, images);
    let mut metrics = TurnMetrics::default();

    let mut round = 0;
    while round < max_rounds {
        round += 1;
        // Whatever was said since the last round joins the conversation here, before the model is
        // asked anything. This is the only boundary at which it can: a round is one blocking call
        // and then the tools it asked for, and a message injected into the middle of either would
        // not be read until this point anyway.
        if take_queued(ctx, |said| wire.interject_said(&mut messages, said, images)) {
            // A new instruction with no rounds left to carry it out is a message answered by "out
            // of tool calls" — so the person who interrupted buys the turn a fresh allowance. The
            // ceiling is there to bound a model stuck in a rut, and a human typing is not one.
            max_rounds = round.saturating_sub(1).saturating_add(allowance);
        }

        let reply = wire.round(&messages, Calls::Allowed)?;
        metrics.num_turns = Some(round);
        add(&mut metrics.input_tokens, reply.input_tokens);
        add(&mut metrics.output_tokens, reply.output_tokens);

        if reply.calls.is_empty() {
            if reply.text.trim().is_empty() {
                metrics.is_error = true;
                adi_events::metrics(sink, &metrics);
                return Err(Error::Process(format!(
                    "{} answered with neither text nor a tool call",
                    wire.provider()
                )));
            }
            adi_events::answer(sink, &reply.text);
            adi_events::metrics(sink, &metrics);
            return Ok(reply.text);
        }

        adi_events::message(sink, &reply.text);

        let mut results = Vec::with_capacity(reply.calls.len());
        for call in &reply.calls {
            adi_events::tool_started(sink, &call.id, &call.name, &call.input);
            let (output, ok) = match tools::execute(&call.name, &call.input, ctx) {
                Ok(out) => (out, true),
                Err(err) => (err, false),
            };
            adi_events::tool_finished(sink, &call.id, &call.name, &output, ok);
            results.push(ToolResult {
                call_id: call.id.clone(),
                name: call.name.clone(),
                output,
                ok,
            });
        }
        wire.append(&mut messages, &reply, &results);
    }

    // The rounds are spent and the model was still working. Failing the turn here is the wrong end
    // to stop at: every tool call it made was real work, and whoever asked gets an error where an
    // answer should be. So it gets one more round with the tools taken off the table — it can only
    // write — and what it writes is the turn's answer: what it did, and what it never reached.
    let rounds = if max_rounds == 1 {
        "1 round".to_string()
    } else {
        format!("{max_rounds} rounds")
    };
    adi_events::message(
        sink,
        &format!(
            "Out of tool calls after {rounds} — wrapping up with what I have. Raise this agent's \
             max turns if the work genuinely needs more."
        ),
    );
    wire.interject(&mut messages, WRAP_UP);
    let reply = wire.round(&messages, Calls::Withheld)?;
    metrics.num_turns = Some(max_rounds + 1);
    add(&mut metrics.input_tokens, reply.input_tokens);
    add(&mut metrics.output_tokens, reply.output_tokens);

    if reply.text.trim().is_empty() {
        metrics.is_error = true;
        adi_events::metrics(sink, &metrics);
        return Err(Error::Process(format!(
            "the turn was still calling tools after {rounds}, and wrote nothing when asked to \
             stop and summarise — the steps above are what it did; raise the agent's max turns if \
             the work genuinely needs more"
        )));
    }
    adi_events::answer(sink, &reply.text);
    adi_events::metrics(sink, &metrics);
    Ok(reply.text)
}

fn add(total: &mut Option<u64>, more: Option<u64>) {
    if let Some(n) = more {
        *total = Some(total.unwrap_or(0) + n);
    }
}

/// Hear everything said to this conversation since the last round, oldest first, handing each
/// message to `hear`. Reports whether anything was there.
///
/// Draining rather than taking one per round: three things typed in a row are three parts of one
/// thought, and spreading them over three model calls would have it act on the first before reading
/// the third.
///
/// Each message is recorded as a question as it is taken, so a reader watching the chat sees it move
/// from the queue into the transcript at the moment the turn actually hears it — and so a turn that
/// dies here leaves the message asked rather than silently swallowed. A store that will not give it
/// up simply ends the drain: what is still in the queue is offered again next round, and failing the
/// turn over it would throw away work the model has already done.
fn take_queued(ctx: &tools::Ctx<'_>, mut hear: impl FnMut(&crate::store::QueuedMessage)) -> bool {
    let mut heard = false;
    while let Ok(Some(message)) = ctx.sessions.take_queued_as_turn(ctx.agent, ctx.conv) {
        hear(&message);
        heard = true;
    }
    heard
}

// ---- the four wire formats ---------------------------------------------------------

/// Whether a round is allowed to reach for a tool.
///
/// [`Withheld`](Self::Withheld) is the wrap-up round, and it is *not* implemented by dropping the
/// declarations: Anthropic rejects a request whose history contains `tool_use` blocks unless that
/// same request also declares tools, and a wrap-up round's history is nothing but. So the tools
/// stay declared and the **choice** is closed instead — each provider spells that differently, and
/// Ollama, which has no such field and no such objection, simply doesn't get sent any.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Calls {
    Allowed,
    Withheld,
}

impl Calls {
    fn withheld(self) -> bool {
        self == Self::Withheld
    }
}

/// A provider's tool-calling dialect: how the conversation starts, how one round is sent and read,
/// and how a round plus its tool results is appended for the next one.
enum Wire<'a> {
    Anthropic {
        args: &'a HarnessAdiArguments,
        model: &'a str,
    },
    /// `OpenAI` and Monshoot: the same chat-completions shape, differing only in [`OpenAiDialect`].
    OpenAi {
        args: &'a HarnessAdiArguments,
        model: &'a str,
        dialect: &'static OpenAiDialect,
    },
    Gemini {
        args: &'a HarnessAdiArguments,
        model: &'a str,
    },
    Ollama {
        args: &'a HarnessAdiArguments,
        model: &'a str,
    },
}

impl<'a> Wire<'a> {
    fn of(args: &'a HarnessAdiArguments, model: &'a str) -> Result<Self> {
        Ok(match args.provider {
            Some(HarnessProvider::Anthropic) => Self::Anthropic { args, model },
            Some(HarnessProvider::Openai) => Self::OpenAi {
                args,
                model,
                dialect: &OPENAI,
            },
            Some(HarnessProvider::Monshoot) => Self::OpenAi {
                args,
                model,
                dialect: &MONSHOOT,
            },
            Some(HarnessProvider::Zai) => Self::OpenAi {
                args,
                model,
                dialect: &ZAI,
            },
            Some(HarnessProvider::Gemini) => Self::Gemini { args, model },
            Some(HarnessProvider::Ollama) => Self::Ollama { args, model },
            None => return Err(Error::NotRunnable("harness:adi".to_string())),
        })
    }

    fn provider(&self) -> &'static str {
        match self {
            Self::Anthropic { .. } => "anthropic",
            Self::OpenAi { dialect, .. } => dialect.provider,
            Self::Gemini { .. } => "gemini",
            Self::Ollama { .. } => "ollama",
        }
    }

    /// The transcript as this provider's opening message list.
    fn seed(&self, turns: &[Turn], images: &ImageStore<'_>) -> Vec<Value> {
        let plain = merged(turns, images.store);
        match self {
            // Gemini names the assistant `model` and carries the system prompt outside the list.
            Self::Gemini { .. } => plain
                .iter()
                .map(|said| {
                    let role = if said.role == "assistant" {
                        "model"
                    } else {
                        "user"
                    };
                    json!({ "role": role, "parts": self.parts(&said.text, said, images) })
                })
                .collect(),
            // Everyone else takes `{role, content}` with the system prompt as the first message —
            // except Anthropic, which has a `system` field; passing it as a message is accepted on
            // current models, but the dedicated field is what the API documents, so it goes there.
            Self::Anthropic { .. } => plain.iter().map(|said| self.said(said, images)).collect(),
            Self::OpenAi { args, .. } | Self::Ollama { args, .. } => {
                let mut messages: Vec<Value> =
                    plain.iter().map(|said| self.said(said, images)).collect();
                if let Some(system) = system_prompt(args) {
                    messages.insert(0, json!({ "role": "system", "content": system }));
                }
                messages
            }
        }
    }

    /// One `{role, content}` message, in the shape this provider takes.
    ///
    /// Text alone stays a **string**, which is what every one of these APIs took before images
    /// existed and what three quarters of a transcript still is. Only a message that actually
    /// carries a picture becomes a list of parts — so the ordinary request on the wire is byte for
    /// byte the one this loop has always sent.
    fn said(&self, said: &Said<'_>, images: &ImageStore<'_>) -> Value {
        let encoded = images.encode(&said.images);
        if encoded.is_empty() {
            return json!({ "role": said.role, "content": said.text });
        }
        match self {
            // Ollama is the odd one: the message keeps its plain string content and the pictures
            // ride beside it in their own field, as bare base64 with no type and no wrapper.
            Self::Ollama { .. } => json!({
                "role": said.role,
                "content": said.text,
                "images": encoded.iter().map(|i| Value::String(i.data.clone())).collect::<Vec<_>>(),
            }),
            _ => json!({ "role": said.role, "content": self.parts(&said.text, said, images) }),
        }
    }

    /// A message's content as a list of parts: its images first, then its words.
    ///
    /// Images lead because Anthropic documents that ordering as the one that answers better, and
    /// nothing else here minds it. A blank text part is left out rather than sent empty — a message
    /// that is only a screenshot is a real thing to send, and an empty string beside it is a
    /// rejected request on at least one of these APIs.
    fn parts(&self, text: &str, said: &Said<'_>, images: &ImageStore<'_>) -> Vec<Value> {
        let mut parts: Vec<Value> = images
            .encode(&said.images)
            .iter()
            .map(|image| self.image_part(image))
            .collect();
        if !text.trim().is_empty() {
            parts.push(self.text_part(text));
        }
        parts
    }

    /// One image, in the shape this provider's content lists take. Four APIs, four spellings of the
    /// same three facts — these bytes, this type, inline rather than by URL.
    fn image_part(&self, image: &Encoded) -> Value {
        match self {
            Self::Anthropic { .. } => json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": image.media_type,
                    "data": image.data,
                },
            }),
            // The chat-completions dialects take an image the same way they take a remote one: as a
            // URL, which for inline bytes is a `data:` URL.
            Self::OpenAi { .. } | Self::Ollama { .. } => json!({
                "type": "image_url",
                "image_url": { "url": format!("data:{};base64,{}", image.media_type, image.data) },
            }),
            Self::Gemini { .. } => json!({
                "inlineData": { "mimeType": image.media_type, "data": image.data },
            }),
        }
    }

    /// Send one round and read it back.
    fn round(&self, messages: &[Value], calls: Calls) -> Result<Reply> {
        match self {
            Self::Anthropic { args, model } => anthropic_round(args, model, messages, calls),
            Self::OpenAi {
                args,
                model,
                dialect,
            } => openai_round(args, model, messages, dialect, calls),
            Self::Gemini { args, model } => gemini_round(args, model, messages, calls),
            Self::Ollama { args, model } => ollama_round(args, model, messages, calls),
        }
    }

    /// Add one more thing from the user's side, mid-conversation: the wrap-up instruction, and a
    /// message somebody typed while the turn was still working.
    ///
    /// Where the last message is already the user's, this rides *inside* it rather than following
    /// it: both Anthropic and Gemini want the two roles to alternate, and a second user turn in a
    /// row is the one shape that would make the request fail outright. Which of the two shapes that
    /// turn holds depends on where in the loop we are — a list of blocks when it is answering tool
    /// calls, a bare string when it is the opening question the transcript seeded — so both are
    /// joined rather than only the one the wrap-up round used to meet.
    fn interject(&self, messages: &mut Vec<Value>, text: &str) {
        let field = self.user_field();
        if let Some(last) = messages.last_mut()
            && last.get("role").and_then(Value::as_str) == Some("user")
        {
            match last.get_mut(field) {
                Some(Value::Array(list)) => {
                    list.push(self.text_part(text));
                    return;
                }
                Some(slot @ Value::String(_)) => {
                    let joined = format!("{}\n\n{text}", slot.as_str().unwrap_or_default());
                    *slot = Value::String(joined);
                    return;
                }
                _ => {}
            }
        }
        messages.push(match self {
            Self::Gemini { .. } => json!({ "role": "user", "parts": [{ "text": text }] }),
            _ => json!({ "role": "user", "content": text }),
        });
    }

    /// Mid-turn, a whole message somebody typed while the model was working — its images with it.
    ///
    /// The plain [`interject`](Self::interject) is still what the wrap-up nudge uses, because a
    /// nudge is words and nothing else. This is the door a *person's* message comes through, and a
    /// person can paste a screenshot into the box at round three exactly as easily as at round one.
    ///
    /// A message with no images takes that same door, so the shape a running turn sees is unchanged
    /// for every conversation that never attaches one.
    fn interject_said(
        &self,
        messages: &mut Vec<Value>,
        said: &crate::store::QueuedMessage,
        images: &ImageStore<'_>,
    ) {
        if said.images.is_empty() {
            self.interject(messages, &said.text);
            return;
        }
        let said = Said {
            role: "user",
            // The same block a seeded turn gets ([`words_of`]): a file attached to a message typed
            // at round three is on disk exactly as one attached at round one, and the model has no
            // other way to learn where.
            text: crate::with_attachment_paths(
                images.store,
                &said.text,
                &said.images,
                ImageDelivery::Inline,
            ),
            images: said.images.clone(),
        };
        // Ollama keeps its pictures beside the message rather than inside it, so there is nothing to
        // fold into a previous turn — and it is the one provider here with no alternation rule to
        // break by appending.
        if matches!(self, Self::Ollama { .. }) {
            messages.push(self.said(&said, images));
            return;
        }
        let parts = self.parts(&said.text, &said, images);
        // Same rule as [`interject`]: two user messages in a row is the one shape Anthropic and
        // Gemini reject, so where the last message is already the user's this rides inside it. A
        // string content is *promoted* to a list on the way — the transcript seeds plain strings, so
        // that is the shape a first-round interjection actually meets.
        if let Some(last) = messages.last_mut()
            && last.get("role").and_then(Value::as_str) == Some("user")
        {
            match last.get_mut(self.user_field()) {
                Some(Value::Array(list)) => {
                    list.extend(parts);
                    return;
                }
                Some(slot @ Value::String(_)) => {
                    let existing = slot.as_str().unwrap_or_default().to_string();
                    let mut list = Vec::new();
                    if !existing.trim().is_empty() {
                        list.push(self.text_part(&existing));
                    }
                    list.extend(parts);
                    *slot = Value::Array(list);
                    return;
                }
                _ => {}
            }
        }
        messages.push(self.said(&said, images));
    }

    /// What this provider calls the list a message's content lives in.
    fn user_field(&self) -> &'static str {
        match self {
            Self::Gemini { .. } => "parts",
            _ => "content",
        }
    }

    /// One piece of plain text, in the shape this provider's content lists take.
    fn text_part(&self, text: &str) -> Value {
        match self {
            Self::Gemini { .. } => json!({ "text": text }),
            _ => json!({ "type": "text", "text": text }),
        }
    }

    /// Append the assistant's round and one result per call, in this provider's shape.
    fn append(&self, messages: &mut Vec<Value>, reply: &Reply, results: &[ToolResult]) {
        messages.push(reply.raw.clone());
        match self {
            Self::Anthropic { .. } => {
                // One user message carrying every result, which is the shape the API requires:
                // a tool_result block per tool_use, all in the turn that answers them.
                let blocks: Vec<Value> = results
                    .iter()
                    .map(|r| {
                        json!({
                            "type": "tool_result",
                            "tool_use_id": r.call_id,
                            "content": r.output,
                            "is_error": !r.ok,
                        })
                    })
                    .collect();
                messages.push(json!({ "role": "user", "content": blocks }));
            }
            Self::OpenAi { .. } => messages.extend(results.iter().map(
                |r| json!({ "role": "tool", "tool_call_id": r.call_id, "content": r.output }),
            )),
            // Ollama identifies a result by the tool's name rather than a call id.
            Self::Ollama { .. } => messages.extend(
                results
                    .iter()
                    .map(|r| json!({ "role": "tool", "tool_name": r.name, "content": r.output })),
            ),
            // Gemini answers a call with a functionResponse part, all of them in one user turn.
            Self::Gemini { .. } => {
                let parts: Vec<Value> = results
                    .iter()
                    .map(|r| {
                        json!({
                            "functionResponse": {
                                "name": r.name,
                                "response": { "output": r.output, "ok": r.ok },
                            }
                        })
                    })
                    .collect();
                messages.push(json!({ "role": "user", "parts": parts }));
            }
        }
    }
}

/// One message on its way to a provider: who said it, what they said, and what they attached.
struct Said<'a> {
    role: &'a str,
    text: String,
    images: Vec<Attachment>,
}

/// The transcript as messages, blank turns dropped and neighbours of the same role joined into one.
///
/// Joining is not tidiness. A turn that heard a message while it was working records that message as
/// its own question, so a conversation's history genuinely holds two questions in a row — and
/// replaying them as two messages is the shape Anthropic and Gemini reject outright. What the model
/// is owed is both texts in the order they were said, which is what one message carrying both is.
///
/// A turn with images but no words is **not** blank, whatever its text says: a pasted screenshot on
/// its own is the whole message, and dropping it would send a request that answers a picture nobody
/// attached.
fn merged<'a>(turns: &'a [Turn], store: &crate::store::SessionStore) -> Vec<Said<'a>> {
    let said = |turn: &Turn| {
        !turn.text.trim().is_empty() || !turn.images.is_empty() || !turn.steps.is_empty()
    };
    let mut out: Vec<Said<'a>> = Vec::with_capacity(turns.len());
    for turn in turns.iter().filter(|t| said(t)) {
        let words = words_of(turn, store);
        match out.last_mut() {
            Some(prev) if prev.role == turn.role => {
                if !words.trim().is_empty() {
                    if !prev.text.is_empty() {
                        prev.text.push_str("\n\n");
                    }
                    prev.text.push_str(&words);
                }
                prev.images.extend(turn.images.iter().cloned());
            }
            _ => out.push(Said {
                role: turn.role.as_str(),
                text: words,
                images: turn.images.clone(),
            }),
        }
    }
    out
}

/// One turn as this engine has to receive it: what was said, plus anything the platform ran on its
/// behalf before it was asked, plus where anything attached to it is on disk.
///
/// Every other engine is handed its message on a command line, and the launch appends both blocks
/// there. This loop is handed an agent and a conversation instead and replays what the store holds,
/// so they have to be rebuilt here — or the commands would have really run with the model never
/// learning what they said, and the PDF somebody attached would be a file nothing was ever told the
/// name of. Same renderers, so the text is byte-identical either way.
fn words_of(turn: &Turn, store: &crate::store::SessionStore) -> String {
    let words = match crate::prelude::block_of_steps(&turn.steps) {
        Some(block) if turn.role == "user" => {
            format!("{}\n\n{block}", turn.text.trim_end())
        }
        _ => turn.text.clone(),
    };
    crate::with_attachment_paths(store, &words, &turn.images, ImageDelivery::Inline)
}

/// One image, ready to go into a request body.
struct Encoded {
    media_type: String,
    data: String,
}

/// Where an attached image's bytes are read from, on the way into a request.
///
/// A thin thing on purpose: the transcript carries *references* so that a chat polled once a second
/// stays small, and this is the one place in the turn that turns a reference back into bytes. Once,
/// at the top of the turn — the same picture is in every round's message list, and re-reading and
/// re-encoding it per round would be the file read and the base64 pass repeated sixty times.
struct ImageStore<'a> {
    store: &'a crate::store::SessionStore,
    /// `id -> encoded`, filled as images are met. Interior mutability because encoding happens while
    /// the message list is being built, which is behind a `&self`.
    seen: std::cell::RefCell<std::collections::HashMap<String, Option<std::rc::Rc<Encoded>>>>,
}

impl<'a> ImageStore<'a> {
    fn new(store: &'a crate::store::SessionStore) -> Self {
        Self {
            store,
            seen: std::cell::RefCell::new(std::collections::HashMap::new()),
        }
    }

    /// These attachments as base64, dropping any whose bytes are gone.
    ///
    /// Dropping rather than failing the turn: an image deleted from under a recorded transcript is a
    /// message with one fewer picture, and refusing to answer a two-week-old conversation because a
    /// file was swept would be the worse of the two failures.
    ///
    /// **Only the images.** A turn may also carry a PDF or a CSV, which no provider here takes in a
    /// request body — those reach the model as a path in the message
    /// (`for_engine`), and putting their bytes in an image block would fail the
    /// whole request rather than one attachment.
    fn encode(&self, images: &[Attachment]) -> Vec<std::rc::Rc<Encoded>> {
        images
            .iter()
            .filter(|image| crate::store::is_image(&image.media_type))
            .filter_map(|image| {
                let mut seen = self.seen.borrow_mut();
                seen.entry(image.id.clone())
                    .or_insert_with(|| {
                        let bytes = self.store.attachment_bytes(image).ok()?;
                        Some(std::rc::Rc::new(Encoded {
                            media_type: image.media_type.clone(),
                            data: BASE64.encode(bytes),
                        }))
                    })
                    .clone()
            })
            .collect()
    }
}

/// The tool set as JSON Schema function declarations — the shape `OpenAI`, Ollama and (nested one
/// level deeper) Gemini all take.
fn function_declarations() -> Vec<Value> {
    tools::TOOLS
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "parameters": (t.schema)(),
            })
        })
        .collect()
}

// ---- Anthropic ---------------------------------------------------------------------

fn anthropic_round(
    args: &HarnessAdiArguments,
    model: &str,
    messages: &[Value],
    calls: Calls,
) -> Result<Reply> {
    let key = api_key(args, "ANTHROPIC_API_KEY", "Anthropic")?;

    let mut body = json!({
        "model": model,
        "max_tokens": args.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        "messages": messages,
        // Anthropic keeps the schema under `input_schema` rather than `parameters`.
        "tools": tools::TOOLS.iter().map(|t| json!({
            "name": t.name,
            "description": t.description,
            "input_schema": (t.schema)(),
        })).collect::<Vec<_>>(),
    });
    if calls.withheld() {
        body["tool_choice"] = json!({ "type": "none" });
    }
    if let Some(system) = system_prompt(args) {
        body["system"] = json!(system);
    }
    if let Some(t) = args.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(p) = args.top_p {
        body["top_p"] = json!(p);
    }
    if let Some(k) = args.top_k {
        body["top_k"] = json!(k);
    }
    if let Some(stops) = stop_sequences(args) {
        body["stop_sequences"] = json!(stops);
    }
    // Extended thinking is a mode, not a token budget: current models take `adaptive` (Claude
    // decides how much to think) or `disabled`, and reject the `budget_tokens` the older API
    // wanted. Left unset, the model's own default stands. Which models take which, and what
    // `budget_tokens` used to mean: <https://platform.claude.com/docs/en/build-with-claude/thinking>.
    if let Some(thinking) = args.thinking {
        let mode = match thinking {
            HarnessThinking::Adaptive => "adaptive",
            HarnessThinking::Disabled => "disabled",
        };
        body["thinking"] = json!({ "type": mode });
    }

    let base = base_url(args, "https://api.anthropic.com");
    let url = versioned_url(&base, "v1", "messages");
    let headers = [
        ("x-api-key", key.as_str()),
        ("anthropic-version", "2023-06-01"),
    ];
    let resp = post_json(&url, &headers, &body)?;

    // The reply is a list of content blocks: text ones make up what it said, tool_use ones are what
    // it wants run. Both can appear in the same round, which is exactly the "here's what I'm about
    // to do" narration the timeline wants to keep.
    let blocks = resp
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| provider_shape_error("anthropic", &resp))?;
    let mut text = String::new();
    let mut calls = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = block.get("text").and_then(Value::as_str) {
                    text.push_str(t);
                }
            }
            Some("tool_use") => calls.push(ToolCall {
                id: block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name: block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                input: block.get("input").cloned().unwrap_or_else(|| json!({})),
            }),
            _ => {}
        }
    }
    if text.trim().is_empty()
        && calls.is_empty()
        && resp.get("stop_reason").and_then(Value::as_str) == Some("max_tokens")
    {
        return Err(out_of_budget_error(
            model,
            args.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        ));
    }
    Ok(Reply {
        text,
        calls,
        raw: json!({ "role": "assistant", "content": blocks }),
        input_tokens: usage(&resp, &["usage", "input_tokens"]),
        output_tokens: usage(&resp, &["usage", "output_tokens"]),
    })
}

// ---- OpenAI dialect (OpenAI, Monshoot's Kimi, and z.ai's GLM) ----------------------

/// The providers that speak `OpenAI`'s `/chat/completions`. They agree on the whole request body
/// but disagree on where they live, which variable holds the key, what the path before the endpoint
/// is, and — the one that bites — what the output cap is called.
struct OpenAiDialect {
    /// Name used in error messages; also the manifest's `provider` value.
    provider: &'static str,
    default_base: &'static str,
    default_key_env: &'static str,
    /// What sits between the host and `chat/completions`: `v1` for the `OpenAI`-born, z.ai's
    /// `api/paas/v4` for GLM (its coding-plan base ends `api/coding/paas/v4`, which an override
    /// `base_url` reaches; only the version's last segment is deduplicated against the base).
    version: &'static str,
    /// `OpenAI`'s reasoning models **reject** `max_tokens` and want `max_completion_tokens`;
    /// Moonshot and z.ai only know `max_tokens`. An `OpenAI`-compatible third party that predates
    /// the rename is reachable through the `monshoot` provider with a `base_url` override.
    max_tokens_field: &'static str,
    /// The cap to send when the agent sets none — see [`OPENAI_DIALECT_DEFAULT_MAX_TOKENS`].
    default_max_tokens: u64,
    /// Whether `tool_choice: "none"` closes the tools on the wrap-up round. z.ai documents
    /// `tool_choice` as `auto`-only, so GLM instead withholds by declaring no tools at all — a
    /// shape every chat-completions endpoint accepts, where Anthropic's objection to it is
    /// Anthropic's alone.
    tool_choice_none: bool,
    /// Whether the provider takes `{"thinking": {"type": …}}`. z.ai does, and its GLM-4.5+ models
    /// think by default; `disabled` is the one mode worth sending, and *not* sending it means the
    /// model's own default stands.
    takes_thinking: bool,
}

const OPENAI: OpenAiDialect = OpenAiDialect {
    provider: "openai",
    default_base: "https://api.openai.com",
    default_key_env: "OPENAI_API_KEY",
    version: "v1",
    max_tokens_field: "max_completion_tokens",
    default_max_tokens: OPENAI_DIALECT_DEFAULT_MAX_TOKENS,
    tool_choice_none: true,
    takes_thinking: false,
};

const MONSHOOT: OpenAiDialect = OpenAiDialect {
    provider: "monshoot",
    default_base: "https://api.moonshot.ai",
    default_key_env: "MOONSHOT_API_KEY",
    version: "v1",
    max_tokens_field: "max_tokens",
    default_max_tokens: OPENAI_DIALECT_DEFAULT_MAX_TOKENS,
    tool_choice_none: true,
    takes_thinking: false,
};

const ZAI: OpenAiDialect = OpenAiDialect {
    provider: "zai",
    default_base: "https://api.z.ai",
    default_key_env: "Z_AI_API_KEY",
    version: "api/paas/v4",
    max_tokens_field: "max_tokens",
    default_max_tokens: OPENAI_DIALECT_DEFAULT_MAX_TOKENS,
    tool_choice_none: false,
    takes_thinking: true,
};

/// One round against an `OpenAI`-dialect chat-completions endpoint.
///
/// Two things about the reasoning models on these providers are worth knowing before reading the
/// parse below. They think first, and the scratchpad comes back *beside* the answer (`reasoning`
/// on `OpenAI`, `reasoning_content` on Kimi and GLM) while `content` holds the reply — so a round
/// that runs out of budget mid-thought returns an **empty** `content` with
/// `finish_reason: "length"`. That case gets its own error, because "raise max output tokens" is
/// the fix and nothing else says so. And several of them (`kimi-k2.6`, `OpenAI`'s o-series and
/// gpt-5) accept only the default temperature, which is why nothing is sent unless the agent asked
/// for it explicitly.
fn openai_round(
    args: &HarnessAdiArguments,
    model: &str,
    messages: &[Value],
    dialect: &OpenAiDialect,
    calls: Calls,
) -> Result<Reply> {
    let key = api_key(args, dialect.default_key_env, dialect.provider)?;

    let max_tokens = args.max_tokens.unwrap_or(dialect.default_max_tokens);
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": false,
    });
    if !calls.withheld() || dialect.tool_choice_none {
        body["tools"] = json!(
            function_declarations()
                .into_iter()
                .map(|f| json!({ "type": "function", "function": f }))
                .collect::<Vec<_>>()
        );
        if calls.withheld() {
            body["tool_choice"] = json!("none");
        }
    }
    body[dialect.max_tokens_field] = json!(max_tokens);
    if dialect.takes_thinking && args.thinking == Some(HarnessThinking::Disabled) {
        body["thinking"] = json!({ "type": "disabled" });
    }
    if let Some(t) = args.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(p) = args.top_p {
        body["top_p"] = json!(p);
    }
    if let Some(f) = args.frequency_penalty {
        body["frequency_penalty"] = json!(f);
    }
    if let Some(p) = args.presence_penalty {
        body["presence_penalty"] = json!(p);
    }
    if let Some(seed) = args.seed {
        body["seed"] = json!(seed);
    }
    if let Some(format) = args.response_format {
        body["response_format"] = json!({ "type": response_format_kind(format)? });
    }
    if let Some(stops) = stop_sequences(args) {
        body["stop"] = json!(stops);
    }

    let base = base_url(args, dialect.default_base);
    let url = versioned_url(&base, dialect.version, "chat/completions");
    let bearer = format!("Bearer {key}");
    let resp = post_json(&url, &[("authorization", bearer.as_str())], &body)?;

    let choice = resp
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .ok_or_else(|| provider_shape_error(dialect.provider, &resp))?;
    let message = choice
        .get("message")
        .ok_or_else(|| provider_shape_error(dialect.provider, &resp))?;
    let text = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // `arguments` is a JSON *string* here, not an object — the one place this dialect makes the
    // caller parse a second time.
    let calls: Vec<ToolCall> = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .enumerate()
                .filter_map(|(i, c)| {
                    let f = c.get("function")?;
                    Some(ToolCall {
                        id: c
                            .get("id")
                            .and_then(Value::as_str)
                            .map_or_else(|| format!("call-{i}"), str::to_string),
                        name: f.get("name").and_then(Value::as_str)?.to_string(),
                        input: parse_arguments(f.get("arguments")),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    if text.trim().is_empty()
        && calls.is_empty()
        && choice.get("finish_reason").and_then(Value::as_str) == Some("length")
    {
        return Err(out_of_budget_error(model, max_tokens));
    }
    Ok(Reply {
        text,
        calls,
        raw: message.clone(),
        input_tokens: usage(&resp, &["usage", "prompt_tokens"]),
        output_tokens: usage(&resp, &["usage", "completion_tokens"]),
    })
}

// ---- Gemini ------------------------------------------------------------------------

/// Google's `generateContent` — the one provider here that isn't a chat-completions clone. Its
/// differences, all visible below: the assistant role is called `model`, the system prompt is a
/// `systemInstruction` of its own, every sampling knob lives under `generationConfig`, the model
/// name is part of the URL rather than the body, tool declarations nest one level deeper, and a
/// 2.5-series reply interleaves *thought* parts with answer parts in one list — so the read keeps
/// only the parts that aren't thoughts.
fn gemini_round(
    args: &HarnessAdiArguments,
    model: &str,
    messages: &[Value],
    calls: Calls,
) -> Result<Reply> {
    let key = api_key(args, "GEMINI_API_KEY", "Gemini")?;

    let mut config = serde_json::Map::new();
    put_f64(&mut config, "temperature", args.temperature);
    put_f64(&mut config, "topP", args.top_p);
    put_u64(&mut config, "topK", args.top_k);
    put_u64(&mut config, "maxOutputTokens", args.max_tokens);
    if let Some(stops) = stop_sequences(args) {
        config.insert("stopSequences".to_string(), json!(stops));
    }
    if let Some(budget) = args.thinking_budget {
        config.insert(
            "thinkingConfig".to_string(),
            json!({ "thinkingBudget": budget }),
        );
    }

    let mut body = json!({
        "contents": messages,
        "tools": [{ "functionDeclarations": function_declarations() }],
    });
    if calls.withheld() {
        body["toolConfig"] = json!({ "functionCallingConfig": { "mode": "NONE" } });
    }
    if let Some(system) = system_prompt(args) {
        body["systemInstruction"] = json!({ "parts": [{ "text": system }] });
    }
    if !config.is_empty() {
        body["generationConfig"] = Value::Object(config);
    }

    let base = base_url(args, "https://generativelanguage.googleapis.com");
    let url = versioned_url(&base, "v1beta", &format!("models/{model}:generateContent"));
    // Two credential shapes reach this endpoint and they go in different headers: a plain API key
    // (`AIza…`) as `x-goog-api-key`, an OAuth access token (`ya29.…`) as a bearer. Pick by the
    // token's own prefix, so either kind of secret can be attached to the agent and just work.
    let bearer;
    let header = if key.starts_with("ya29.") {
        bearer = format!("Bearer {key}");
        ("authorization", bearer.as_str())
    } else {
        ("x-goog-api-key", key.as_str())
    };
    let resp = post_json(&url, &[header], &body)?;

    let candidate = resp
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .ok_or_else(|| provider_shape_error("gemini", &resp))?;
    let parts = candidate
        .get("content")
        .and_then(|c| c.get("parts"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut text = String::new();
    let mut calls = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        if part.get("thought").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        if let Some(t) = part.get("text").and_then(Value::as_str) {
            text.push_str(t);
        }
        // A function call here carries no id of its own — position is its only identity, so the
        // loop supplies one for the transcript and answers by name.
        if let Some(fc) = part.get("functionCall") {
            calls.push(ToolCall {
                id: format!("call-{i}"),
                name: fc
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                input: fc.get("args").cloned().unwrap_or_else(|| json!({})),
            });
        }
    }

    if text.trim().is_empty() && calls.is_empty() {
        // The two ways an answer goes missing: the budget went entirely on thinking, or the reply
        // was stopped (safety, recitation). Both name the reason Google gave, which is the whole
        // diagnosis.
        return match candidate.get("finishReason").and_then(Value::as_str) {
            Some("MAX_TOKENS") => Err(out_of_budget_error(
                model,
                args.max_tokens.unwrap_or_default(),
            )),
            Some(reason) if reason != "STOP" => Err(Error::Process(format!(
                "gemini stopped before writing an answer: {reason}"
            ))),
            _ => Err(provider_shape_error("gemini", &resp)),
        };
    }
    Ok(Reply {
        text,
        calls,
        raw: json!({ "role": "model", "parts": parts }),
        input_tokens: usage(&resp, &["usageMetadata", "promptTokenCount"]),
        output_tokens: usage(&resp, &["usageMetadata", "candidatesTokenCount"]),
    })
}

// ---- Ollama (local) ----------------------------------------------------------------

fn ollama_round(
    args: &HarnessAdiArguments,
    model: &str,
    messages: &[Value],
    calls: Calls,
) -> Result<Reply> {
    let mut options = serde_json::Map::new();
    put_f64(&mut options, "temperature", args.temperature);
    put_f64(&mut options, "top_p", args.top_p);
    put_u64(&mut options, "top_k", args.top_k);
    put_u64(&mut options, "num_ctx", args.num_ctx);
    put_f64(&mut options, "repeat_penalty", args.repeat_penalty);
    put_f64(&mut options, "min_p", args.min_p);
    put_u64(&mut options, "num_predict", args.max_tokens);
    if let Some(seed) = args.seed {
        options.insert("seed".to_string(), json!(seed));
    }
    if let Some(stops) = stop_sequences(args) {
        options.insert("stop".to_string(), json!(stops));
    }

    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": false,
    });
    // No tool_choice here, so withholding is literal: declare nothing and there is nothing to call.
    if !calls.withheld() {
        body["tools"] = json!(
            function_declarations()
                .into_iter()
                .map(|f| json!({ "type": "function", "function": f }))
                .collect::<Vec<_>>()
        );
    }
    if !options.is_empty() {
        body["options"] = Value::Object(options);
    }
    if args.format.is_some() {
        body["format"] = json!("json");
    }
    if args.think {
        // Only sent when asked for: a model that can't think rejects the field outright.
        body["think"] = json!(true);
    }
    if let Some(keep) = args.keep_alive.as_deref().filter(|k| !k.trim().is_empty()) {
        body["keep_alive"] = json!(keep);
    }

    let base = base_url(args, "http://localhost:11434");
    let url = format!("{base}/api/chat");
    let resp = post_json(&url, &[], &body)?;

    let message = resp
        .get("message")
        .ok_or_else(|| provider_shape_error("ollama", &resp))?;
    let text = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // Ollama returns already-parsed arguments (an object, not a string) and no call id.
    let calls: Vec<ToolCall> = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .enumerate()
                .filter_map(|(i, c)| {
                    let f = c.get("function")?;
                    Some(ToolCall {
                        id: format!("call-{i}"),
                        name: f.get("name").and_then(Value::as_str)?.to_string(),
                        input: parse_arguments(f.get("arguments")),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    if text.trim().is_empty() && calls.is_empty() {
        return Err(provider_shape_error("ollama", &resp));
    }
    Ok(Reply {
        text,
        calls,
        raw: message.clone(),
        input_tokens: usage(&resp, &["prompt_eval_count"]),
        output_tokens: usage(&resp, &["eval_count"]),
    })
}

// ---- shared HTTP + argument helpers ------------------------------------------------

/// Tool arguments as an object, whichever way the provider sent them: an object already (Ollama,
/// Gemini) or a JSON string to decode (`OpenAI`, Monshoot). A model that emits malformed JSON gets
/// an empty object and, a moment later, the tool's own complaint about the missing argument —
/// which is a better teacher than a parse error it never sees.
fn parse_arguments(raw: Option<&Value>) -> Value {
    match raw {
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| json!({})),
        Some(other) => other.clone(),
        None => json!({}),
    }
}

/// A usage counter from a response, by path.
fn usage(resp: &Value, path: &[&str]) -> Option<u64> {
    let mut node = resp;
    for key in path {
        node = node.get(key)?;
    }
    node.as_u64()
}

/// Install `ring` as the process-wide rustls provider, once.
///
/// reqwest is built with `rustls-no-provider` (see the workspace manifest), which means it picks
/// no crypto provider of its own — building any client fails until one is installed. A turn runs
/// in a freshly spawned `adi-mono harness-turn` child, so there is no earlier start-up path here
/// to rely on; the install happens on the way to the first request instead. `install_default`
/// errors only if a provider is already set, which is exactly the outcome wanted.
fn ensure_provider() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
}

/// POST `body` as JSON with the given extra headers, returning the decoded JSON response. A non-2xx
/// status surfaces the provider's own error body, which is what the caller needs to see.
fn post_json(url: &str, headers: &[(&str, &str)], body: &Value) -> Result<Value> {
    ensure_provider();
    let client = reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| Error::Process(format!("couldn't build HTTP client: {e}")))?;
    let mut req = client.post(url).json(body);
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    let resp = req
        .send()
        .map_err(|e| Error::Process(format!("request to {url} failed: {e}")))?;
    let status = resp.status();
    let text = resp
        .text()
        .map_err(|e| Error::Process(format!("reading response from {url} failed: {e}")))?;
    if !status.is_success() {
        return Err(Error::Process(format!(
            "{url} returned {status}: {}",
            text.trim()
        )));
    }
    serde_json::from_str(&text).map_err(|e| Error::Process(format!("invalid JSON from {url}: {e}")))
}

fn provider_shape_error(provider: &str, resp: &Value) -> Error {
    Error::Process(format!(
        "{provider} response had no answer text: {}",
        resp.to_string().chars().take(300).collect::<String>()
    ))
}

/// A reply that is all reasoning and no answer. Every thinking model can produce one, and the fix
/// is always the same, so they all say it the same way.
fn out_of_budget_error(model: &str, max_tokens: u64) -> Error {
    Error::Process(format!(
        "{model} spent its whole {max_tokens} token budget thinking and never wrote an answer — \
         raise the agent's max output tokens"
    ))
}

/// The provider's API key, read from the environment variable the agent named (or the provider's
/// conventional one). A missing key is a setup problem rather than a run failure — hence
/// `Unsupported`, and hence the pointer to where the key belongs.
fn api_key(args: &HarnessAdiArguments, default_env: &str, provider: &str) -> Result<String> {
    let key_env = args
        .api_key_env
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .unwrap_or(default_env);
    std::env::var(key_env).map_err(|_| {
        Error::Unsupported(format!(
            "no {provider} API key: environment variable {key_env} is unset (attach it as a secret on the agent)"
        ))
    })
}

/// The provider's endpoint: the agent's `base_url` override, or the provider's own host.
fn base_url(args: &HarnessAdiArguments, default: &str) -> String {
    args.base_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .unwrap_or(default)
        .trim_end_matches('/')
        .to_string()
}

/// `<base>/<version>/<path>`, tolerating a base that already ends in the version segment — the
/// panel's own hint for this field reads `https://api.moonshot.ai/v1`, and pasting exactly that
/// must not produce `/v1/v1/chat/completions`. Only the version's *last* segment is matched,
/// because a version can carry more than itself: z.ai's is `api/paas/v4`, and its coding-plan base
/// ends `api/coding/paas/v4` — both must land on the same URL as the bare host.
fn versioned_url(base: &str, version: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let last = version.rsplit('/').next().unwrap_or(version);
    if base.ends_with(&format!("/{last}")) {
        format!("{base}/{path}")
    } else {
        format!("{base}/{version}/{path}")
    }
}

/// The wire value for a structured-output mode. `json_schema` needs the schema itself, which this
/// argument set has nowhere to carry — so say that, rather than sending a request the provider
/// would reject for us.
fn response_format_kind(format: HarnessResponseFormat) -> Result<&'static str> {
    match format {
        HarnessResponseFormat::Text => Ok("text"),
        HarnessResponseFormat::JsonObject => Ok("json_object"),
        HarnessResponseFormat::JsonSchema => Err(Error::Unsupported(
            "the adi loop can't send a json_schema response format — it has no schema to send; \
             use json_object"
                .to_string(),
        )),
    }
}

/// The system prompt, trimmed to a non-empty value, or `None`.
fn system_prompt(args: &HarnessAdiArguments) -> Option<String> {
    args.system_prompt
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The prompt the runner composed for this turn and handed down in the environment, if it did.
fn composed_prompt() -> Option<String> {
    std::env::var(crate::runner::detached::SYSTEM_PROMPT_ENV)
        .ok()
        .map(|prompt| prompt.trim().to_string())
        .filter(|prompt| !prompt.is_empty())
}

/// The comma-separated `stop` argument split into a non-empty list of stop strings.
fn stop_sequences(args: &HarnessAdiArguments) -> Option<Vec<String>> {
    let stops: Vec<String> = args
        .stop
        .as_deref()?
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    (!stops.is_empty()).then_some(stops)
}

fn put_f64(map: &mut serde_json::Map<String, Value>, key: &str, value: Option<f64>) {
    if let Some(v) = value {
        map.insert(key.to_string(), json!(v));
    }
}

fn put_u64(map: &mut serde_json::Map<String, Value>, key: &str, value: Option<u64>) {
    if let Some(v) = value {
        map.insert(key.to_string(), json!(v));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_for(provider: HarnessProvider) -> HarnessAdiArguments {
        HarnessAdiArguments {
            provider: Some(provider),
            ..HarnessAdiArguments::default()
        }
    }

    fn turn(role: &str, text: &str) -> Turn {
        Turn {
            role: role.into(),
            text: text.into(),
            at: 1,
            pending: false,
            queued: false,
            images: Vec::new(),
            steps: Vec::new(),
            metrics: None,
        }
    }

    /// A scratch session store under a per-test directory. Seeding needs one because that is where
    /// an image's bytes are read from; nothing but the image cases ever puts a file in it.
    fn scratch(tag: &str) -> crate::store::SessionStore {
        let dir = std::env::temp_dir().join(format!(
            "adi-loop-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        crate::store::SessionStore::new(dir)
    }

    /// This loop is handed a conversation id, not a message: the launch's careful work of appending
    /// the pre-run output to the engine's command line reaches every other engine and none of it
    /// reaches this one. So the block is rebuilt from the turn's steps here — otherwise the
    /// commands really run, cost real time, and the model is never told a word of what they said.
    #[test]
    fn a_pre_run_reaches_the_loop_through_the_turn_it_was_recorded_on() {
        let mut opening = turn("user", "work the task");
        opening.steps = vec![crate::progress::Step::Tool {
            name: "Bash".into(),
            input: "adi-mono tasks show BUGBOUNTY-465".into(),
            status: crate::progress::ToolStatus::Ok,
            output: "Title: probe the auth flow".into(),
        }];

        let turns = [opening];
        let said = merged(&turns, &scratch("pre-run"));
        assert_eq!(said.len(), 1);
        assert!(
            said[0].text.starts_with("work the task"),
            "the task still leads: {}",
            said[0].text
        );
        assert!(
            said[0].text.contains("Title: probe the auth flow"),
            "the command's output must reach this engine too: {}",
            said[0].text
        );
        assert!(
            said[0]
                .text
                .contains("<pre-run command=\"adi-mono tasks show BUGBOUNTY-465\" status=\"ok\">"),
            "and framed exactly as every other engine sees it: {}",
            said[0].text
        );
    }

    /// An assistant turn's steps are its own tool calls — already in the transcript as the calls
    /// they were — so they must not be re-rendered into its words.
    #[test]
    fn an_assistant_turns_own_tool_calls_are_not_replayed_as_text() {
        let mut answer = turn("assistant", "done");
        answer.steps = vec![crate::progress::Step::Tool {
            name: "Bash".into(),
            input: "ls".into(),
            status: crate::progress::ToolStatus::Ok,
            output: "a.rs".into(),
        }];
        let turns = [answer];
        assert_eq!(merged(&turns, &scratch("own-calls"))[0].text, "done");
    }

    #[test]
    fn every_provider_runs_and_only_an_unconfigured_agent_does_not() {
        let mut args = HarnessAdiArguments::default();
        assert!(matches!(validate(&args), Err(Error::NotRunnable(b)) if b == "harness:adi"));
        for provider in [
            HarnessProvider::Anthropic,
            HarnessProvider::Openai,
            HarnessProvider::Gemini,
            HarnessProvider::Monshoot,
            HarnessProvider::Zai,
            HarnessProvider::Ollama,
        ] {
            args.provider = Some(provider);
            assert!(
                validate(&args).is_ok(),
                "{} must be runnable",
                provider.as_str()
            );
        }
    }

    #[test]
    fn a_base_url_that_already_carries_its_version_is_not_doubled() {
        // The panel hints the endpoint with its version segment, so both spellings must land on
        // the same URL.
        assert_eq!(
            versioned_url("https://api.moonshot.ai", "v1", "chat/completions"),
            "https://api.moonshot.ai/v1/chat/completions"
        );
        assert_eq!(
            versioned_url("https://api.moonshot.ai/v1/", "v1", "chat/completions"),
            "https://api.moonshot.ai/v1/chat/completions"
        );
        // z.ai's version carries its `api/paas` prefix with it, and the coding-plan host carries a
        // different one — three spellings, one endpoint.
        assert_eq!(
            versioned_url("https://api.z.ai", "api/paas/v4", "chat/completions"),
            "https://api.z.ai/api/paas/v4/chat/completions"
        );
        assert_eq!(
            versioned_url(
                "https://api.z.ai/api/paas/v4",
                "api/paas/v4",
                "chat/completions"
            ),
            "https://api.z.ai/api/paas/v4/chat/completions"
        );
        assert_eq!(
            versioned_url(
                "https://api.z.ai/api/coding/paas/v4",
                "api/paas/v4",
                "chat/completions"
            ),
            "https://api.z.ai/api/coding/paas/v4/chat/completions"
        );
        assert_eq!(
            versioned_url(
                "https://generativelanguage.googleapis.com",
                "v1beta",
                "models/g:x"
            ),
            "https://generativelanguage.googleapis.com/v1beta/models/g:x"
        );
    }

    #[test]
    fn the_openai_dialects_differ_only_where_the_providers_do() {
        // The rename is the whole reason the dialect struct exists: sending Moonshot's field name
        // to an OpenAI reasoning model is a 400, and vice versa.
        assert_eq!(OPENAI.max_tokens_field, "max_completion_tokens");
        assert_eq!(MONSHOOT.max_tokens_field, "max_tokens");
        assert_eq!(ZAI.max_tokens_field, "max_tokens");

        // z.ai is the one dialect that is not a `/v1` clone and not closable by `tool_choice`.
        assert_eq!(ZAI.version, "api/paas/v4");
        assert!(!ZAI.tool_choice_none);
        assert!(OPENAI.tool_choice_none && MONSHOOT.tool_choice_none);
        assert!(ZAI.takes_thinking);
        assert!(!OPENAI.takes_thinking && !MONSHOOT.takes_thinking);
    }

    #[test]
    fn gemini_seeds_the_assistant_turn_under_its_own_role_name() {
        let store = scratch("gemini-roles");
        let images = ImageStore::new(&store);
        let args = args_for(HarnessProvider::Gemini);
        let wire = Wire::of(&args, "gemini-2.5-pro").expect("wire");
        let seeded = wire.seed(&[turn("user", "hi"), turn("assistant", "hello")], &images);
        assert_eq!(seeded[0]["role"], "user");
        assert_eq!(seeded[1]["role"], "model");
        assert_eq!(seeded[1]["parts"][0]["text"], "hello");
    }

    #[test]
    fn blank_turns_are_dropped_and_a_system_prompt_leads_the_chat_dialects() {
        let store = scratch("blank-turns");
        let images = ImageStore::new(&store);
        let mut args = args_for(HarnessProvider::Monshoot);
        args.system_prompt = Some("be terse".into());
        let wire = Wire::of(&args, "kimi-k3").expect("wire");
        let seeded = wire.seed(
            &[
                turn("user", "hi"),
                turn("assistant", "  "),
                turn("user", "again"),
            ],
            &images,
        );
        // Two, not three: the empty answer goes, and the questions it stood between are then
        // neighbours of the same role, which replay as one message rather than as the pair
        // Anthropic and Gemini reject.
        assert_eq!(seeded.len(), 2, "the blank turn is dropped: {seeded:?}");
        assert_eq!(seeded[0]["role"], "system");
        assert_eq!(seeded[1]["content"], "hi\n\nagain");
    }

    #[test]
    fn each_provider_answers_a_call_the_way_its_api_expects() {
        let reply = |calls: Vec<ToolCall>| Reply {
            text: String::new(),
            calls,
            raw: json!({ "role": "assistant" }),
            input_tokens: None,
            output_tokens: None,
        };
        let call = || ToolCall {
            id: "c1".into(),
            name: "Read".into(),
            input: json!({}),
        };
        let results = [ToolResult {
            call_id: "c1".into(),
            name: "Read".into(),
            output: "contents".into(),
            ok: true,
        }];

        let args = args_for(HarnessProvider::Anthropic);
        let mut messages = Vec::new();
        Wire::of(&args, "m")
            .expect("wire")
            .append(&mut messages, &reply(vec![call()]), &results);
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"][0]["type"], "tool_result");
        assert_eq!(messages[1]["content"][0]["tool_use_id"], "c1");

        let args = args_for(HarnessProvider::Openai);
        let mut messages = Vec::new();
        Wire::of(&args, "m")
            .expect("wire")
            .append(&mut messages, &reply(vec![call()]), &results);
        assert_eq!(messages[1]["role"], "tool");
        assert_eq!(messages[1]["tool_call_id"], "c1");

        let args = args_for(HarnessProvider::Gemini);
        let mut messages = Vec::new();
        Wire::of(&args, "m")
            .expect("wire")
            .append(&mut messages, &reply(vec![call()]), &results);
        assert_eq!(messages[1]["parts"][0]["functionResponse"]["name"], "Read");
    }

    /// The wrap-up instruction has to reach the model without breaking the request that carries
    /// it: on the two block-shaped providers a second user turn in a row is rejected, so it rides
    /// inside the tool-result turn that is already there.
    #[test]
    fn the_wrap_up_nudge_never_makes_two_user_turns_in_a_row() {
        let anthropic = args_for(HarnessProvider::Anthropic);
        let mut messages = vec![
            json!({ "role": "assistant", "content": [] }),
            json!({ "role": "user", "content": [{ "type": "tool_result", "content": "out" }] }),
        ];
        Wire::of(&anthropic, "m")
            .expect("wire")
            .interject(&mut messages, "stop now");
        assert_eq!(messages.len(), 2, "it joined the turn: {messages:?}");
        assert_eq!(messages[1]["content"][1]["type"], "text");
        assert_eq!(messages[1]["content"][1]["text"], "stop now");

        let gemini = args_for(HarnessProvider::Gemini);
        let mut messages = vec![json!({ "role": "user", "parts": [{ "functionResponse": {} }] })];
        Wire::of(&gemini, "m")
            .expect("wire")
            .interject(&mut messages, "stop now");
        assert_eq!(messages.len(), 1, "it joined the turn: {messages:?}");
        assert_eq!(messages[0]["parts"][1]["text"], "stop now");

        let mut messages = vec![json!({ "role": "model", "parts": [] })];
        Wire::of(&gemini, "m")
            .expect("wire")
            .interject(&mut messages, "stop now");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["role"], "user");

        let openai = args_for(HarnessProvider::Openai);
        let mut messages = vec![json!({ "role": "tool", "tool_call_id": "c1", "content": "out" })];
        Wire::of(&openai, "m")
            .expect("wire")
            .interject(&mut messages, "stop now");
        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[1],
            json!({ "role": "user", "content": "stop now" })
        );
    }

    /// The round-1 shape, which the wrap-up round never met: the trailing user message is the
    /// opening question as a bare string, and a message that arrived a second after the launch has
    /// to join it. Pushing a second user turn there is what Anthropic rejects.
    #[test]
    fn a_message_heard_mid_turn_joins_the_question_whatever_shape_it_is_in() {
        let store = scratch("mid-turn");
        let images = ImageStore::new(&store);
        let anthropic = args_for(HarnessProvider::Anthropic);
        let wire = Wire::of(&anthropic, "m").expect("wire");
        let mut messages = wire.seed(&[turn("user", "fix the parser")], &images);
        wire.interject(&mut messages, "and handle CRLF");
        assert_eq!(messages.len(), 1, "it joined the question: {messages:?}");
        assert_eq!(messages[0]["content"], "fix the parser\n\nand handle CRLF");

        let gemini = args_for(HarnessProvider::Gemini);
        let wire = Wire::of(&gemini, "m").expect("wire");
        let mut messages = wire.seed(&[turn("user", "fix the parser")], &images);
        wire.interject(&mut messages, "and handle CRLF");
        assert_eq!(messages.len(), 1, "it joined the question: {messages:?}");
        assert_eq!(messages[0]["parts"][0]["text"], "fix the parser");
        assert_eq!(messages[0]["parts"][1]["text"], "and handle CRLF");

        // Mid-loop, where the trailing turn is the one answering tool calls, it rides in as one
        // more block — the wrap-up round's path, now shared.
        let wire = Wire::of(&anthropic, "m").expect("wire");
        let mut messages = vec![
            json!({ "role": "assistant", "content": [] }),
            json!({ "role": "user", "content": [{ "type": "tool_result", "content": "out" }] }),
        ];
        wire.interject(&mut messages, "actually, skip the tests");
        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[1]["content"][1]["text"],
            "actually, skip the tests"
        );
    }

    /// Four providers, four spellings of "here is a picture". This is the check that each one is
    /// spelled the way *its* API documents rather than the way the last one did — a wrong shape here
    /// is a 400 from the provider, at the one moment somebody has just pasted a screenshot.
    #[test]
    fn an_attached_image_goes_out_in_each_providers_own_shape() {
        let store = scratch("images");
        let image = store
            .put_attachment("shot.png", "image/png", b"\x89PNG")
            .expect("store the image");
        let asked = Turn {
            images: vec![image],
            ..turn("user", "what is wrong here?")
        };
        let images = ImageStore::new(&store);
        // The bytes, exactly as every provider takes them: base64 of what was stored.
        let data = BASE64.encode(b"\x89PNG");

        let anthropic = args_for(HarnessProvider::Anthropic);
        let seeded = Wire::of(&anthropic, "m")
            .expect("wire")
            .seed(std::slice::from_ref(&asked), &images);
        assert_eq!(seeded[0]["content"][0]["type"], "image");
        assert_eq!(seeded[0]["content"][0]["source"]["media_type"], "image/png");
        assert_eq!(seeded[0]["content"][0]["source"]["data"], data);
        // Images lead and the words follow — the order Anthropic documents as the better one. The
        // words carry the file's path behind them: the model can see the picture, and this is how
        // it can also *reach* it (`Agents::with_attachment_paths`).
        let words = seeded[0]["content"][1]["text"].as_str().unwrap_or_default();
        assert!(words.starts_with("what is wrong here?"), "{words}");
        assert!(words.contains("shot.png"), "{words}");

        let openai = args_for(HarnessProvider::Openai);
        let seeded = Wire::of(&openai, "m")
            .expect("wire")
            .seed(std::slice::from_ref(&asked), &images);
        assert_eq!(
            seeded[0]["content"][0]["image_url"]["url"],
            format!("data:image/png;base64,{data}"),
        );

        let gemini = args_for(HarnessProvider::Gemini);
        let seeded = Wire::of(&gemini, "m")
            .expect("wire")
            .seed(std::slice::from_ref(&asked), &images);
        assert_eq!(seeded[0]["parts"][0]["inlineData"]["mimeType"], "image/png");
        assert_eq!(seeded[0]["parts"][0]["inlineData"]["data"], data);

        // Ollama is the odd one: the message keeps plain string content and the pictures sit beside
        // it, as bare base64 with no type and no wrapper.
        let ollama = args_for(HarnessProvider::Ollama);
        let seeded = Wire::of(&ollama, "m")
            .expect("wire")
            .seed(std::slice::from_ref(&asked), &images);
        assert!(
            seeded[0]["content"]
                .as_str()
                .is_some_and(|c| c.starts_with("what is wrong here?")),
            "{seeded:?}",
        );
        assert_eq!(seeded[0]["images"][0], data);

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// A message that is only a screenshot is a real message. The blank-turn filter drops turns with
    /// nothing in them, and "nothing" has to mean no words *and* no pictures — otherwise the request
    /// that goes out answers an image nobody sent.
    #[test]
    fn a_turn_with_a_picture_and_no_words_is_not_a_blank_turn() {
        let store = scratch("wordless");
        let image = store
            .put_attachment("shot.png", "image/png", b"\x89PNG")
            .expect("store the image");
        let images = ImageStore::new(&store);
        let asked = Turn {
            images: vec![image],
            ..turn("user", "  ")
        };
        let args = args_for(HarnessProvider::Anthropic);
        let seeded = Wire::of(&args, "m").expect("wire").seed(&[asked], &images);
        assert_eq!(seeded.len(), 1, "the wordless turn survived: {seeded:?}");
        assert_eq!(seeded[0]["content"][0]["type"], "image");
        // The only text such a turn has is where the file is — never an empty part, which is what
        // a message with no words at all would otherwise send.
        assert_eq!(seeded[0]["content"].as_array().map(Vec::len), Some(2));
        assert!(
            seeded[0]["content"][1]["text"]
                .as_str()
                .is_some_and(|t| t.contains("shot.png")),
            "{seeded:?}",
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// An image whose bytes are gone — swept, or deleted from under an old transcript — costs the
    /// message a picture, not the turn an answer. The alternative is a two-week-old conversation
    /// that can never be replied to again because one file went missing.
    #[test]
    fn an_image_whose_bytes_are_gone_is_dropped_rather_than_failing_the_turn() {
        let store = scratch("missing");
        let images = ImageStore::new(&store);
        let asked = Turn {
            images: vec![Attachment {
                id: "nothing-here".into(),
                name: "gone.png".into(),
                media_type: "image/png".into(),
                size: 4,
            }],
            ..turn("user", "still worth answering")
        };
        let args = args_for(HarnessProvider::Anthropic);
        let seeded = Wire::of(&args, "m").expect("wire").seed(&[asked], &images);
        assert_eq!(seeded.len(), 1);
        // No image part at all — a plain string message, which is what a turn with no picture in it
        // has always been. The words still name where the file was: the row is what says it was
        // attached, and a missing file is the reader's to notice, not this turn's to fail on.
        assert!(
            seeded[0]["content"]
                .as_str()
                .is_some_and(|c| c.starts_with("still worth answering")),
            "{seeded:?}",
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// A turn that heard a message recorded it as a question, so the transcript it replays next time
    /// really does hold two in a row. Both texts have to reach the model, and as one message.
    #[test]
    fn two_questions_in_a_row_replay_as_one_message() {
        let store = scratch("two-questions");
        let images = ImageStore::new(&store);
        let anthropic = args_for(HarnessProvider::Anthropic);
        let history = [
            turn("user", "fix the parser"),
            turn("user", "and handle CRLF"),
            turn("assistant", "done"),
        ];
        let seeded = Wire::of(&anthropic, "m")
            .expect("wire")
            .seed(&history, &images);
        assert_eq!(seeded.len(), 2, "the two questions merged: {seeded:?}");
        assert_eq!(seeded[0]["role"], "user");
        assert_eq!(seeded[0]["content"], "fix the parser\n\nand handle CRLF");
        assert_eq!(seeded[1]["role"], "assistant");

        let gemini = args_for(HarnessProvider::Gemini);
        let seeded = Wire::of(&gemini, "m")
            .expect("wire")
            .seed(&history, &images);
        assert_eq!(seeded.len(), 2, "the two questions merged: {seeded:?}");
        assert_eq!(
            seeded[0]["parts"][0]["text"],
            "fix the parser\n\nand handle CRLF"
        );
        assert_eq!(seeded[1]["role"], "model");
    }

    #[test]
    fn tool_arguments_are_read_whichever_way_the_provider_sends_them() {
        assert_eq!(
            parse_arguments(Some(&json!("{\"path\":\"a.rs\"}")))["path"],
            "a.rs"
        );
        assert_eq!(
            parse_arguments(Some(&json!({"path": "a.rs"})))["path"],
            "a.rs"
        );
        assert_eq!(parse_arguments(Some(&json!("{not json"))), json!({}));
        assert_eq!(parse_arguments(None), json!({}));
    }

    #[test]
    fn argv_reenters_this_binary_for_the_turn() {
        let argv = argv("planner", "0000000000001-0000");
        // The program may be an absolute path beside the running executable or the bare name,
        // depending on what is installed next to the test binary — both name the same tool.
        let program = std::path::Path::new(&argv[0]);
        assert_eq!(
            program.file_name().and_then(|n| n.to_str()),
            Some("adi-mono")
        );
        assert_eq!(
            &argv[1..],
            [
                "harness-turn",
                "--agent",
                "planner",
                "--conv",
                "0000000000001-0000"
            ]
        );
    }

    /// Never a bare name when a sibling exists: a `systemd --user` unit's PATH has no adi
    /// directory, and resolving through it is what made every agent turn on a node fail to spawn.
    #[test]
    fn the_program_is_absolute_when_adi_mono_sits_beside_this_binary() {
        let dir = std::env::current_exe()
            .expect("current exe")
            .parent()
            .expect("a parent")
            .to_path_buf();
        let sibling = dir.join("adi-mono");
        let planted = if sibling.is_file() {
            false
        } else {
            std::fs::write(&sibling, b"#!/bin/sh\n").is_ok()
        };

        let program = adi_mono_program();
        if sibling.is_file() {
            assert_eq!(program, sibling.to_string_lossy());
        } else {
            // Could not plant one (read-only dir); the fallback is the documented behaviour.
            assert_eq!(program, "adi-mono");
        }

        if planted {
            let _ = std::fs::remove_file(&sibling);
        }
    }
}
