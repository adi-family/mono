#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["requests>=2.31", "anthropic>=0.80"]
# ///
"""Hold the code constant, change only the delivery format, and count the tokens.

The end-to-end benchmark measures behaviour, which is confounded: the two arms write
*different* scripts, take different numbers of rounds, and think for different lengths.
This measures the format alone. Every script in the corpus is rendered twice — once as a
JSON tool-call argument, once as an `<execute>` block — and both are sent to the
provider's own tokenizer.

Each channel's per-action cost decomposes into two parts:

    fixed     the envelope: `{"script": …}` plus the tool_call and tool-message framing,
              versus the `<execute lang="…">` tags plus the `<result>` wrapper. Paid once
              per action regardless of what the script contains.
    marginal  what the script's own characters cost through that channel. The difference
              here *is* the escaping tax — same characters, different encoding.

Plus the per-request fixed overhead, which is paid on every single call: the tools arm
ships its JSON schemas, the execute arm ships a longer system prompt.

    uv run token_diff.py                     # corpus = the scripts from runs/
    uv run token_diff.py --provider anthropic --model claude-opus-5
    uv run token_diff.py --corpus runs --top 15
"""

from __future__ import annotations

import argparse
import hashlib
import json
import statistics
from pathlib import Path

import requests

import core
import local_tokenizer
import prompts

MOONSHOT_TOKENIZER = "https://api.moonshot.ai/v1/tokenizers/estimate-token-count"

#: A short fixed conversation the measured turn is appended to. Identical for both arms,
#: so it cancels out of every difference.
BASE = [{"role": "user", "content": "go"}]

#: Result body held constant across arms so the delta isolates the delivery format.
RESULT_BODY = core.format_result_body(0, "(no output)")

CACHE_PATH = Path(__file__).parent / ".tokencache.json"


# --------------------------------------------------------------------------------------
# Counters
# --------------------------------------------------------------------------------------


class MoonshotCounter:
    provider = "moonshot"

    def __init__(self, model: str):
        self.model = model
        self.key = core.resolve_key(
            ["KIMI_API_KEY", "MOONSHOT_API_KEY"], secret_name="KIMI_API_KEY"
        )

    def count(self, messages: list[dict], tools: list | None = None) -> int:
        body: dict = {"model": self.model, "messages": messages}
        if tools:
            body["tools"] = tools
        resp = requests.post(
            MOONSHOT_TOKENIZER,
            headers={"Authorization": f"Bearer {self.key}"},
            json=body,
            timeout=60,
        )
        resp.raise_for_status()
        payload = resp.json()
        if not payload.get("status"):
            raise RuntimeError(f"tokenizer refused: {payload}")
        return int(payload["data"]["total_tokens"])

    def tool_call_pair(self, lang: str, script: str) -> list[dict]:
        return [
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {
                        "id": "c1",
                        "type": "function",
                        "function": {
                            "name": lang,
                            "arguments": json.dumps({"script": script}),
                        },
                    }
                ],
            },
            {"role": "tool", "tool_call_id": "c1", "content": RESULT_BODY},
        ]

    def execute_pair(self, lang: str, script: str) -> list[dict]:
        return [
            {
                "role": "assistant",
                "content": f'<execute lang="{lang}">\n{script}\n</execute>',
            },
            {
                "role": "user",
                "content": f'<result index="1" lang="{lang}" exit="0">\n'
                f"{RESULT_BODY}\n</result>",
            },
        ]

    def request_overhead(self) -> tuple[int, int]:
        """(tools arm, execute arm) fixed tokens paid on every request."""
        tools_side = self.count(
            [{"role": "system", "content": prompts.system_prompt("tools")}] + BASE,
            tools=core.openai_tools(),
        )
        exec_side = self.count(
            [{"role": "system", "content": prompts.system_prompt("execute")}] + BASE
        )
        return tools_side, exec_side


class AnthropicCounter:
    provider = "anthropic"

    def __init__(self, model: str):
        import anthropic

        self.model = model
        self.client = anthropic.Anthropic(
            api_key=core.resolve_key(
                ["ANTHROPIC_API_KEY"], secret_name="ANTHROPIC_API_KEY"
            )
        )

    def count(self, messages: list[dict], tools: list | None = None) -> int:
        kwargs: dict = {"model": self.model, "messages": messages}
        if tools:
            kwargs["tools"] = tools
        return self.client.messages.count_tokens(**kwargs).input_tokens

    def tool_call_pair(self, lang: str, script: str) -> list[dict]:
        return [
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": lang,
                        "input": {"script": script},
                    }
                ],
            },
            {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_1",
                        "content": RESULT_BODY,
                    }
                ],
            },
        ]

    def execute_pair(self, lang: str, script: str) -> list[dict]:
        return [
            {
                "role": "assistant",
                "content": f'<execute lang="{lang}">\n{script}\n</execute>',
            },
            {
                "role": "user",
                "content": f'<result index="1" lang="{lang}" exit="0">\n'
                f"{RESULT_BODY}\n</result>",
            },
        ]

    def request_overhead(self) -> tuple[int, int]:
        tools_side = self.client.messages.count_tokens(
            model=self.model,
            system=prompts.system_prompt("tools"),
            messages=BASE,
            tools=core.anthropic_tools(),
        ).input_tokens
        exec_side = self.client.messages.count_tokens(
            model=self.model,
            system=prompts.system_prompt("execute"),
            messages=BASE,
        ).input_tokens
        return tools_side, exec_side


class LocalCounter:
    """Kimi's tokenizer, run here. Exact, free, and — unlike the hosted estimator —
    it sees the escaped bytes rather than the parsed argument value."""

    provider = "local"

    def __init__(self, model: str):
        self.model = model
        local_tokenizer.encoder()  # fail fast if the BPE file is missing

    def count(self, messages: list[dict], tools: list | None = None) -> int:
        total = 0
        for message in messages:
            content = message.get("content")
            if isinstance(content, str):
                total += local_tokenizer.count(content)
            elif isinstance(content, list):
                total += local_tokenizer.count(json.dumps(content, ensure_ascii=False))
            for call in message.get("tool_calls") or []:
                # The escaped argument string is what the model decodes.
                total += local_tokenizer.count(call["function"]["arguments"])
                total += local_tokenizer.count(call["function"]["name"])
        if tools:
            total += local_tokenizer.count(json.dumps(tools, ensure_ascii=False))
        return total

    tool_call_pair = MoonshotCounter.tool_call_pair
    execute_pair = MoonshotCounter.execute_pair

    def request_overhead(self) -> tuple[int, int]:
        tools_side = self.count(
            [{"role": "system", "content": prompts.system_prompt("tools")}] + BASE,
            tools=core.openai_tools(),
        )
        exec_side = self.count(
            [{"role": "system", "content": prompts.system_prompt("execute")}] + BASE
        )
        return tools_side, exec_side


# --------------------------------------------------------------------------------------


def load_corpus(root: Path) -> list[tuple[str, str, str]]:
    """(name, lang, source) for every script the benchmark runs actually produced."""
    out = []
    for path in sorted(root.glob("*/_exec/*")):
        if path.suffix == ".py":
            lang = "py"
        elif path.suffix == ".sh":
            lang = "sh"
        else:
            continue
        source = path.read_text(encoding="utf-8")
        if source.strip():
            out.append((f"{path.parent.parent.name}/{path.name}", lang, source))
    return out


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--provider",
        default="moonshot",
        choices=["moonshot", "anthropic", "local"],
        help="'local' runs Kimi's own tokenizer offline — free, exact, no API",
    )
    parser.add_argument("--model", default=None, help="defaults per provider")
    parser.add_argument("--corpus", default=str(Path(__file__).parent / "runs"))
    parser.add_argument("--top", type=int, default=10, help="worst-offender rows to show")
    parser.add_argument("--no-cache", action="store_true")
    args = parser.parse_args()

    model = args.model or (
        "claude-opus-5" if args.provider == "anthropic" else "kimi-k3"
    )
    counter = {
        "moonshot": MoonshotCounter,
        "anthropic": AnthropicCounter,
        "local": LocalCounter,
    }[args.provider](model)

    cache: dict[str, int] = {}
    if CACHE_PATH.exists() and not args.no_cache:
        cache = json.loads(CACHE_PATH.read_text(encoding="utf-8"))

    def counted(kind: str, lang: str, script: str) -> int:
        key = hashlib.sha256(
            f"{counter.provider}|{model}|{kind}|{lang}|{script}".encode()
        ).hexdigest()
        if key in cache:
            return cache[key]
        pair = (
            counter.tool_call_pair(lang, script)
            if kind == "tools"
            else counter.execute_pair(lang, script)
        )
        value = counter.count(BASE + pair) - base_tokens
        cache[key] = value
        return value

    base_tokens = counter.count(BASE)

    # ---- fixed per-request overhead --------------------------------------------------
    tools_req, exec_req = counter.request_overhead()
    print(f"\nprovider={counter.provider} model={model}\n")
    print("=== fixed cost per REQUEST (paid on every single call) ===\n")
    print(f"  tools arm    system prompt + 2 JSON tool schemas : {tools_req:>6,} tokens")
    print(f"  execute arm  system prompt (protocol paragraph)  : {exec_req:>6,} tokens")
    print(f"  difference                                       : {exec_req - tools_req:>+6,} tokens")

    # ---- fixed per-action framing ----------------------------------------------------
    fixed_tools = counted("tools", "sh", "")
    fixed_exec = counted("execute", "sh", "")
    if counter.provider == "local":
        print(
            "\n  NOTE: the local encoder tokenizes text exactly but does not model the\n"
            "  chat template, so the two fixed sections below undercount both arms.\n"
            "  The code-body section is text-only and is exact."
        )
    print("\n=== fixed cost per ACTION (envelope, empty script) ===\n")
    print(f"  tools arm    {{\"script\": …}} + tool_call + tool message : {fixed_tools:>4,} tokens")
    print(f"  execute arm  <execute> tags + <result> wrapper         : {fixed_exec:>4,} tokens")
    print(f"  difference                                            : {fixed_exec - fixed_tools:>+4,} tokens")

    # ---- marginal cost of the code itself --------------------------------------------
    corpus = load_corpus(Path(args.corpus))
    if not corpus:
        raise SystemExit(f"no scripts under {args.corpus} — run a benchmark arm first")

    # Generation-side measurement. The model decodes the escaped JSON literally, one
    # token at a time, so the honest cost of the tools channel is the escaped source
    # tokenized *as text*. Counting it through `tool_calls.arguments` instead gives a
    # much lower number, because the tokenizer endpoint parses the argument string and
    # prices the unescaped value — it understates the tools arm badly. Newlines are
    # where it hurts: a run of real newlines merges into few tokens, while `\n` as two
    # literal characters is one token per line.
    def as_text(text: str) -> int:
        key = hashlib.sha256(
            f"{counter.provider}|{model}|text|{text}".encode()
        ).hexdigest()
        if key in cache:
            return cache[key]
        value = counter.count([{"role": "user", "content": "x" + text}]) - text_base
        cache[key] = value
        return value

    text_base = counter.count([{"role": "user", "content": "x"}])

    rows = []
    for name, lang, source in corpus:
        escaped = json.dumps(source)[1:-1]
        m_tools = as_text(escaped)
        m_exec = as_text(source)
        rows.append(
            {
                "name": name,
                "lang": lang,
                "chars": len(source),
                "escapes": source.count("\\") + source.count('"') + source.count("\n"),
                "m_tools": m_tools,
                "m_exec": m_exec,
                "delta": m_exec - m_tools,
            }
        )

    if not args.no_cache:
        CACHE_PATH.write_text(json.dumps(cache), encoding="utf-8")

    m_tools = sum(r["m_tools"] for r in rows)
    m_exec = sum(r["m_exec"] for r in rows)
    print(f"\n=== marginal cost of the CODE itself ({len(rows)} real scripts) ===\n")
    print("  (tokenized as generated text: escaped JSON vs the raw block body)\n")
    print(f"  total code characters                : {sum(r['chars'] for r in rows):>8,}")
    print(f"  JSON-escaped, as a tool argument     : {m_tools:>8,} tokens")
    print(f"  raw, as an <execute> block body      : {m_exec:>8,} tokens")
    saved = m_tools - m_exec
    print(
        f"  difference                           : {-saved:>+8,} tokens "
        f"({-saved / m_tools * 100:+.1f}%)"
    )
    per_action = statistics.fmean(r["delta"] for r in rows)
    print(f"  mean per action                      : {per_action:>+8.1f} tokens")

    # ---- where the escaping tax actually lands ---------------------------------------
    heavy = sorted(rows, key=lambda r: r["delta"])[: args.top]
    print(f"\n=== {args.top} scripts the encoding punishes most ===\n")
    print(f"  {'script':<34} {'lang':<5} {'chars':>6} {'esc':>5} {'tools':>7} {'exec':>7} {'delta':>7}")
    print("  " + "-" * 76)
    for r in heavy:
        print(
            f"  {r['name'][:34]:<34} {r['lang']:<5} {r['chars']:>6,} {r['escapes']:>5,} "
            f"{r['m_tools']:>7,} {r['m_exec']:>7,} {r['delta']:>+7,}"
        )

    # ---- break-even ------------------------------------------------------------------
    print("\n=== break-even ===\n")
    request_penalty = exec_req - tools_req
    per_action_saving = -(per_action + (fixed_exec - fixed_tools))
    print(
        f"  execute pays {request_penalty:+,} tokens per request and saves "
        f"{per_action_saving:,.1f} per action"
    )
    if per_action_saving > 0 and request_penalty > 0:
        print(
            f"  -> it wins from action {request_penalty / per_action_saving:.1f} onward "
            "in a conversation"
        )
    elif per_action_saving > 0:
        print("  -> it is cheaper on both axes; no break-even to clear")
    else:
        print("  -> it does not save per action for this corpus")
    print()


if __name__ == "__main__":
    main()
