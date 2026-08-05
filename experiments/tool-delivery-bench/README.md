# tool-delivery-bench

Does a model work better when it writes runnable code straight into its output instead of
into a JSON tool-call argument? This measures it.

Two arms, identical in every other respect:

| arm | how the model asks to run code |
| --- | --- |
| `tools` | native function calling — `sh(script)` / `py(script)`, the script arrives as a JSON string, so every quote, backslash and newline is escaped |
| `execute` | no tools declared — the model writes `<execute lang="py\|sh">…</execute>` into its reply and the harness runs whatever it finds, verbatim |

Both arms get the same capability (bash + Python 3 over one persistent working
directory), the same tasks, the same result formatting and the same system-prompt
scaffolding. Only the delivery channel differs.

The hypotheses being tested are that `execute` should (1) parallelize better, because
writing three blocks is as cheap as writing one, and (2) spend fewer output tokens,
because nothing is spent on escaping.

## Running it

```sh
cd experiments/tool-delivery-bench

uv run run_kimi_tools.py     --repeat 3      # baseline, kimi-k3
uv run run_kimi_execute.py   --repeat 3      # experiment, kimi-k3
uv run run_claude_tools.py   --repeat 3      # baseline, claude-opus-5
uv run run_claude_execute.py --repeat 3      # experiment, claude-opus-5

uv run report.py                             # arm-vs-arm comparison
uv run report.py --csv summary.csv
```

Every runner appends one JSON line per episode to `results.jsonl`; `report.py` reads that
file, so arms and providers accumulate and can be run in any order, at any time.

Useful flags (all runners): `--task fanout` (repeatable), `--repeat N`, `--model`,
`--max-rounds`, `--max-tokens`, `--no-prime`, `-v`. Claude runners also take
`--effort low|medium|high|xhigh|max|none`.

**Credentials.** `KIMI_API_KEY` / `MOONSHOT_API_KEY` from the environment, falling back to
`adi-mono secrets read KIMI_API_KEY` — which is where this machine already keeps it, so the
Kimi runners work with no setup. The Claude runners want `ANTHROPIC_API_KEY` (env or
`adi-mono secrets set ANTHROPIC_API_KEY`); there is no key on this machine yet, and a Claude
Code OAuth session is not usable as one.

## The tasks

Four, each deterministic — `setup` writes byte-identical fixtures every time and `verify`
recomputes the expected answer from them rather than diffing against a stored blob, so a
failed check always means the model got it wrong.

| task | what it probes |
| --- | --- |
| `fanout` | line count + SHA-256 of eight files → does the arm change how work is batched? |
| `escape` | a regex over quoted Windows paths full of backslashes → maximum escaping pain |
| `pipeline` | CSV → per-region revenue → sorted output; the sequential control case |
| `probe` | six unrelated facts about a directory → the purest parallelism signal |

## What gets measured

Per episode, into `results.jsonl`: rounds, actions, actions per round, max actions in one
round, input / cached / output / reasoning tokens, USD cost, API latency, wall clock,
whether verification passed, and per action the language, exit code and duration.

Two of the columns are the point of the whole thing:

- **`actions_per_round`** — hypothesis (1). How many independent things the model asks for
  per turn.
- **`wire_overhead_ratio`** — hypothesis (2). Characters the model had to emit to request an
  action, divided by characters of actual code. In the `tools` arm the numerator is the
  JSON argument blob; in `execute` it is the whole `<execute>` span. `backslashes` counts
  the escaping directly.

Cost comes from the `PRICING` table in `core.py` — $3.00 / $0.30 / $15.00 per MTok for
kimi-k3, first-party rates for the Claude models. Verify those against the provider before
quoting any figure; nothing re-checks them at runtime. Cost is derived, so raw token counts
are always in the record even when a model has no rate card.

Two accounting notes. Moonshot folds cached tokens into `prompt_tokens` and Anthropic does
not, so `usage_from_*` normalizes both to "input_tokens = uncached input". And Anthropic
returns `tool_use.input` already parsed, so `wire_chars` there is re-serialized rather than
the literal bytes the model emitted — identical for ASCII scripts, which these are;
Moonshot's runner does report the literal string.

## First results — kimi-k3, 3 repeats per cell

24 episodes, `--repeat 3`, priming on. Read with `uv run report.py --only-verified`: a
failed episode ends early and so looks artificially cheap, which would flatter whichever
arm fails more.

**Hypothesis 2 (escaping) holds in characters, and much more modestly in tokens.**
Backslashes emitted per episode collapse by roughly an order of magnitude:

| task | tools | execute | |
| --- | ---: | ---: | ---: |
| `escape` | 80.3 | 6.0 | −92.5% |
| `probe` | 87.0 | 7.3 | −91.6% |
| `fanout` | 53.0 | 5.3 | −89.9% |
| `pipeline` | 39.3 | 1.3 | −96.6% |

But characters are not the billing unit, and on K3 they are not paid at all: the model
emits tool arguments as raw text and the JSON encoding is added by the wire adapter
afterwards (see *Correction* below). The marginal token difference for the code body is
**+0.4%** — nothing. End-to-end output tokens came out at `escape` −32%, `pipeline` −41%,
`fanout` +6%, `probe` +33%; overall −15% output and −17% cost, so on this model the
end-to-end saving is **behavioural**, not an escaping saving.

**Hypothesis 1 (parallelism) is not really supported.** Measured over acting rounds only,
`actions_per_acting_round` is **1.00** for the tools arm on three of four tasks — kimi-k3
with native tools issues one call at a time, essentially never batching. The execute arm
does slightly better (1.50 on `escape`, 1.17 on `fanout`, 1.11 on `probe`, 1.00 on
`pipeline`), but it is a small effect, and on `probe` — the task built specifically to
invite six independent checks — it is marginally *worse*. Neither channel makes this model
parallelize; the text channel just doesn't stop it either.

**The execute channel has a running cost.** Even with priming, the execute arm needed
1.0–1.67 stall nudges per episode against 0 for tools, and 1 of 12 episodes failed outright
(`escape`, stalled with no action and no `DONE:`). Those are extra round-trips, extra
latency, and a success rate of 92% against 100% before filtering.

Caveats: n=3 per cell, one model, one machine — treat everything in this section as
directional. The section below is the part that does not depend on sample size: it holds
the code constant and measures the encodings against each other.

## The 1:1 token comparison — `token_diff.py`

The end-to-end run measures behaviour, which is confounded: the two arms write different
scripts, take different numbers of rounds and think for different lengths. `token_diff.py`
removes all of that. It takes the 69 scripts the benchmark runs actually produced, renders
each one both ways, and counts tokens with the provider's own tokenizer.

```sh
uv run token_diff.py                                     # kimi-k3
uv run token_diff.py --provider anthropic --model claude-opus-5
```

Three separate costs, kimi-k3:

| what | tools | execute | |
| --- | ---: | ---: | ---: |
| **per request** — system prompt + 2 JSON tool schemas vs system prompt with the protocol paragraph | 585 | 490 | **−95** |
| **per action** — `{"script": …}` + tool_call + tool message vs `<execute>` tags + `<result>` wrapper | 83 | 67 | **−16** |
| **per action** — the code body itself, 69 real scripts | 7,089 | 6,248 | **−841 (−11.9%)** |

The execute channel is cheaper on all three axes, so there is no break-even to clear — but
the totals are modest. A four-round, three-action episode saves roughly 4×95 + 3×28 ≈ 460
tokens, most of it on the input side, which is billed at a fifth of the output rate.

Multi-line Python is where the encoding tax concentrates: the worst scripts in the corpus
lose 25–69 tokens each, and they are all long `.py` bodies. Newlines are the reason — a run
of real newlines merges into very few BPE tokens, while `\n` as two literal characters is
one token per line.

### Correction: on K3 the escaping tax is a wire-format artifact, not a model cost

An earlier version of this file claimed the opposite, on the strength of this measurement:
the same string tokenized as text costs far more once JSON-escaped.

| content | raw | JSON-escaped | |
| --- | ---: | ---: | ---: |
| 200 backslashes | 25 | 50 | +100% |
| 200 double quotes | 50 | 200 | +300% |
| 200 newlines | 13 | 200 | **+1438%** |
| a realistic Windows-path regex | 128 | 144 | +12.5% |

That is true of text, but the model never emits that text. `encoding_k3.py` in the Hub repo
is Kimi's own prompt encoder, and it shows K3's native tool-call format is **not JSON** — it
is an XTML tag language built on single-token controls:

```
<|open|>call tool="sh" index="1"<|sep|>
<|open|>argument key="script" type="string"<|sep|>
echo hi                     ← raw text, verbatim, nothing escaped
<|close|>argument<|sep|>
<|close|>call<|sep|>
```

`_append_text(segments, _xtml_value(value))` writes the argument body straight through.
Only *attribute* values get escaped, and only for `&` and `"`. So the model generates the
script verbatim in both arms; Moonshot's OpenAI-compatible adapter JSON-encodes it
afterwards, on the way out. The escaping is paid by the wire format, not by the decoder.

Consequences: `estimate-token-count` was **right** to price the parsed value — it matches
the real token stream — and `token_diff.py`'s text-level number (−11.9%) **overstates** the
tools arm on K3. The honest marginal difference for the code body is the estimator's
**+0.4%**, i.e. nothing. Tool *declarations* are still JSON in a system message
(`_render_tool_declare` emits ```` ```json ```` with the schemas), so the 209-token
per-request schema cost is real and unaffected by this correction.

This is model-specific. A provider whose model really does decode escaped JSON would show
the text-level number; check the model's own encoder before assuming either way.

### Why the stall happens — the same finding, from the other end

When K3 decides to act it emits `<|open|>tools<|sep|>…` — control tokens. Declare no tools
and the adapter has nowhere to route them, so they are stripped: empty `content`,
`finish_reason: stop`, ~30 completion tokens billed for a turn that returned nothing. That
is exactly the pathology the chat arms hit, and it reproduces on a raw completions endpoint
too. The model was never "refusing to write text" — it was making a tool call into a
channel that had been removed.

### `<execute>` versus the native channel, in tokens

The controls are one token each; our delimiters are ordinary text:

| | tokens |
| --- | ---: |
| `<\|open\|>` / `<\|sep\|>` / `<\|close\|>` / `<\|end_of_msg\|>` | 1 each |
| `<execute lang="sh">` | 6 |
| `</execute>` | 3 |
| `<result index="1" lang="sh" exit="0">` | 14 |
| `</result>` | 3 |
| native call framing, one action, opening only | 22 |

So the hand-rolled protocol is actually *leaner* per action than K3's own framing (6 tokens
to open versus 22) — it just is not what the model was trained to emit, which is why it
needs priming and still stalls. The genuinely zero-overhead path on this model is to drive
the native XTML channel directly; `local_tokenizer.py` can encode those control tokens, and
whether any OpenAI-compatible gateway passes them through is the open question.

## Native tool channel vs `<execute>` — `native_vs_chat.py`

Because `encoding_k3.py` renders the exact token stream the model reads, both encodings can
be priced **offline and exactly**, with no API call and no behavioural noise. Same task
(`chain`: write a script → chmod → run into a file → read it back), same four scripts, the
only difference being the envelope around them.

| | tokens |
| --- | ---: |
| standing preamble — native: system prompt + JSON schemas | 499 |
| standing preamble — execute: system prompt with the protocol | 404 |
| **whole 4-action conversation, native tool channel** | **1,040** |
| **whole 4-action conversation, `<execute>` in response text** | **882** |
| script bodies (identical in both) | 75 |
| difference per action, envelope only | **−39.5** |

`<execute>` is **15.2% cheaper than K3's own tool channel** on this task. The native channel
is verbose where ours is terse: opening one call is `<|open|>call tool="sh" index="1"<|sep|>`
+ `<|open|>argument key="script" type="string"<|sep|>` — 22 tokens of attribute text before
a single byte of script — plus a full `role="tool"` message envelope for every result.
`<execute lang="sh">` opens in 6.

So the trade is now sharp, and it is not about escaping at all:

* **`<execute>`** — ~15% fewer tokens, but it is not the channel the model was trained on,
  so it needs priming and still stalls (see below).
* **native** — ~15% more tokens, but it fires reliably every time, because emitting
  `<|open|>call…` is what the model does when it decides to act.

Caveat: this prices what the model *reads*. Generation cost tracks the assistant turns
proportionally, but a real run also differs in rounds and reasoning, which is what the
end-to-end benchmark measures.

### Driving the native channel directly is not possible through a hosted gateway

Worth knowing before trying. OpenRouter's `/v1/completions` **accepts** a prompt containing
`<|open|>`/`<|sep|>` control tokens — no error. But the model's reply comes back with
`text: ''` and ~37 billed completion tokens: the adapter parses the native output, finds a
tool call for tools that were never declared through the API's `tools` parameter, and drops
it. Only the think channel survives, via `include_reasoning`. Reported `prompt_tokens` (375)
also matches neither the special-aware local count (240) nor the literal-text one (290), so
the gateway is not passing the prompt through untouched either.

To drive the native channel end to end you need the model itself — self-hosted vLLM/SGLang
with `skip_special_tokens=False` — not an OpenAI-compatible gateway.

## Findings while building it

**kimi-k3 will not use the `<execute>` channel cold.** Given a real agentic task and no
`tools` parameter it reasons *"let me look at the files first"* and then returns **empty
content** with `finish_reason: stop` — 32 completion tokens, nothing in `content`. It is not
a parsing problem; there is genuinely nothing there. The action pathway appears wired to
native tool calls, and with no tool behind it the turn just ends. Trivial one-shot prompts
("print the current directory") produce a correct block every time, so the model knows the
syntax perfectly well — it stalls specifically when it decides to *begin a task*.

Two things fix it, both in the harness rather than the protocol:

1. **A one-shot priming exchange** (`--prime`, on by default): one example turn where the
   assistant runs `echo ready` and gets a result back. This takes the execute arm from
   2/4 tasks to 4/4. The identical primer is injected into the `tools` arm in tool-call
   shape, so both arms pay the same input-token cost and the comparison stays honest.
   Run with `--no-prime` to measure the cold behaviour — that is a result in its own right.
2. **A stall nudge** (`STALL_NUDGE` in `core.py`, max 2 per episode, counted in
   `totals.nudges`): when a turn neither acts nor says `DONE:`, the harness says so and asks
   again. Applied identically in both arms.

Also: Moonshot rejects an empty `assistant` message in the history, which is exactly what a
stalled turn produces — the Kimi execute driver drops that turn and folds the nudge into the
preceding user message rather than sending two user turns back to back.

## Sandboxing

Each episode gets its own directory under `runs/`. Model-written code runs with that
directory as cwd, a 60-second timeout, and an environment stripped of anything matching
`API_KEY|SECRET|TOKEN|PASSWORD|CREDENTIAL`, so a task can never read the credentials paying
for the run. The scripts themselves are written to a sibling `_exec/` directory so a
directory listing shows only task artifacts. Output handed back to the model is clipped to
4000 characters, head and tail.

This is still real code execution on a real machine — the system prompt tells the model to
stay in its directory, but nothing enforces that. Read a diff of `prompts.py` before
pointing this at a model you trust less.

## Layout

```
core.py                 sandbox, execute-block parser, usage/pricing, records, episode loop
tasks.py                the four tasks — setup, prompt, verifier
prompts.py              the two system prompts and the priming exchange
run_kimi_tools.py       kimi-k3   + native tool calls
run_kimi_execute.py     kimi-k3   + <execute> blocks
run_claude_tools.py     opus-5    + native tool calls
run_claude_execute.py   opus-5    + <execute> blocks
token_diff.py           same code, both encodings -> where the tokens actually go
report.py               results.jsonl -> comparison table
```
