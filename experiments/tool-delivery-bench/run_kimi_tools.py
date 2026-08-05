#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["openai>=1.60"]
# ///
"""Baseline arm on Kimi: native tool calling over Moonshot's chat-completions API.

The model asks for code by emitting a function call whose argument is a JSON object,
so every newline, quote and backslash in the script has to survive JSON escaping.

    uv run run_kimi_tools.py --repeat 3
"""

from __future__ import annotations

import argparse
import json
import time

from openai import OpenAI

import core
import prompts
import tasks

MOONSHOT_BASE_URL = "https://api.moonshot.ai/v1"
DEFAULT_MODEL = "kimi-k3"


class KimiToolsDriver:
    #: Set from the base URL so a run through a gateway is not filed under "moonshot".
    provider = "moonshot"
    arm = "tools"

    def __init__(
        self,
        client: OpenAI,
        model: str,
        max_tokens: int,
        prime: bool = True,
        provider: str = "moonshot",
    ):
        self.provider = provider
        self.client = client
        self.model = model
        self.max_tokens = max_tokens
        self.prime = prime
        self.messages: list[dict] = []

    def begin(self, system_prompt: str, user_prompt: str) -> None:
        self.messages = [{"role": "system", "content": system_prompt}]
        if self.prime:
            # The same priming exchange the execute arm gets, in tool-call shape, so
            # both arms pay the same teaching cost in input tokens.
            self.messages += [
                {"role": "user", "content": prompts.PRIME_USER},
                {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [
                        {
                            "id": "call_prime",
                            "type": "function",
                            "function": {
                                "name": prompts.PRIME_LANG,
                                "arguments": json.dumps({"script": prompts.PRIME_CODE}),
                            },
                        }
                    ],
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_prime",
                    "content": prompts.PRIME_RESULT_BODY,
                },
            ]
        self.messages.append({"role": "user", "content": user_prompt})

    def step(self) -> core.Step:
        started = time.monotonic()
        resp = self.client.chat.completions.create(
            model=self.model,
            messages=self.messages,
            tools=core.openai_tools(),
            # Moonshot only understands `max_tokens`, and Kimi's reasoning tokens come
            # out of the same budget — too small a cap returns empty content.
            max_tokens=self.max_tokens,
        )
        latency = time.monotonic() - started

        choice = resp.choices[0]
        msg = choice.message
        text = msg.content or ""
        extra = getattr(msg, "model_extra", None) or {}
        reasoning = getattr(msg, "reasoning_content", None) or extra.get(
            "reasoning_content"
        ) or ""

        assistant: dict = {"role": "assistant", "content": text}
        actions: list[core.Action] = []
        if msg.tool_calls:
            assistant["tool_calls"] = [
                {
                    "id": tc.id,
                    "type": "function",
                    "function": {
                        "name": tc.function.name,
                        "arguments": tc.function.arguments,
                    },
                }
                for tc in msg.tool_calls
            ]
            for tc in msg.tool_calls:
                raw = tc.function.arguments or "{}"
                try:
                    parsed = json.loads(raw)
                except json.JSONDecodeError:
                    parsed = {}
                actions.append(
                    core.Action(
                        lang=tc.function.name,
                        code=parsed.get("script") or parsed.get("code") or "",
                        # The literal argument string the model produced — escaping included.
                        wire=raw,
                        ref=tc.id,
                    )
                )
        self.messages.append(assistant)

        return core.Step(
            text=text,
            actions=actions,
            usage=core.usage_from_openai(resp.usage),
            raw_usage=resp.usage.model_dump() if resp.usage else {},
            stop_reason=choice.finish_reason or "",
            latency_s=latency,
            reasoning_chars=len(reasoning),
        )

    def feed(self, outcomes: list[core.Outcome]) -> None:
        for outcome in outcomes:
            self.messages.append(
                {
                    "role": "tool",
                    "tool_call_id": outcome.action.ref,
                    "content": core.format_result_body(
                        outcome.exit_code, outcome.output
                    ),
                }
            )

    def feed_text(self, text: str) -> None:
        self.messages.append({"role": "user", "content": text})


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", default=DEFAULT_MODEL)
    parser.add_argument("--base-url", default=MOONSHOT_BASE_URL)
    parser.add_argument(
        "--api-key-env",
        default=None,
        help="credential to use instead of KIMI_API_KEY — lets these runners drive any "
        "OpenAI-dialect endpoint (e.g. --base-url https://openrouter.ai/api/v1 "
        "--api-key-env OPENROUTER_API_KEY --model moonshotai/kimi-k3)",
    )
    core.add_common_args(parser)
    args = parser.parse_args()

    key_env = args.api_key_env
    client = OpenAI(
        api_key=core.resolve_key(
            [key_env] if key_env else ["KIMI_API_KEY", "MOONSHOT_API_KEY"],
            secret_name=key_env or "KIMI_API_KEY",
        ),
        base_url=args.base_url,
    )
    host = args.base_url.split("/")[2]
    provider = "moonshot" if "moonshot" in host else host.replace("api.", "").split(".")[0]
    core.drive(
        args,
        tasks,
        lambda: KimiToolsDriver(client, args.model, args.max_tokens, args.prime, provider),
    )


if __name__ == "__main__":
    main()
