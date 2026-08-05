#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["anthropic>=0.80"]
# ///
"""Baseline arm on Claude: native tool calling over the Messages API.

Same two tools (`sh`, `py`) and the same tasks as the Kimi runs, so the numbers line up
across providers as well as across arms.

    uv run run_claude_tools.py --repeat 3

Note on the escaping metric: the Messages API returns `tool_use.input` already parsed,
so `wire_chars` here is re-serialized with `json.dumps(..., ensure_ascii=False)` rather
than being the literal bytes the model emitted. For ASCII scripts the two are identical;
Moonshot's runner does report the literal string.
"""

from __future__ import annotations

import argparse
import json
import time

import anthropic

import core
import prompts
import tasks

DEFAULT_MODEL = "claude-opus-5"


class ClaudeToolsDriver:
    provider = "anthropic"
    arm = "tools"

    def __init__(
        self,
        client: anthropic.Anthropic,
        model: str,
        max_tokens: int,
        effort: str | None,
        prime: bool = True,
    ):
        self.client = client
        self.model = model
        self.max_tokens = max_tokens
        self.effort = effort
        self.prime = prime
        self.system = ""
        self.messages: list[dict] = []

    def begin(self, system_prompt: str, user_prompt: str) -> None:
        self.system = system_prompt
        if not self.prime:
            self.messages = [{"role": "user", "content": user_prompt}]
            return
        # Same priming exchange as the execute arm, in tool-call shape. The task prompt
        # rides along in the tool_result turn so the history has no stray user turn.
        self.messages = [
            {"role": "user", "content": prompts.PRIME_USER},
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use",
                        "id": "toolu_prime",
                        "name": prompts.PRIME_LANG,
                        "input": {"script": prompts.PRIME_CODE},
                    }
                ],
            },
            {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_prime",
                        "content": prompts.PRIME_RESULT_BODY,
                    },
                    {"type": "text", "text": user_prompt},
                ],
            },
        ]

    def step(self) -> core.Step:
        kwargs: dict = {
            "model": self.model,
            "max_tokens": self.max_tokens,
            "system": self.system,
            "messages": self.messages,
            "tools": core.anthropic_tools(),
            "thinking": {"type": "adaptive"},
        }
        if self.effort:
            kwargs["output_config"] = {"effort": self.effort}

        started = time.monotonic()
        resp = self.client.messages.create(**kwargs)
        latency = time.monotonic() - started

        if resp.stop_reason == "refusal":
            detail = getattr(resp, "stop_details", None)
            raise RuntimeError(
                f"model refused (category={getattr(detail, 'category', None)})"
            )

        text_parts, thinking_chars = [], 0
        actions: list[core.Action] = []
        for block in resp.content:
            if block.type == "text":
                text_parts.append(block.text)
            elif block.type == "thinking":
                thinking_chars += len(getattr(block, "thinking", "") or "")
            elif block.type == "tool_use":
                payload = block.input if isinstance(block.input, dict) else {}
                actions.append(
                    core.Action(
                        lang=block.name,
                        code=payload.get("script") or payload.get("code") or "",
                        wire=json.dumps(payload, ensure_ascii=False),
                        ref=block.id,
                    )
                )

        # Echo the response back verbatim — thinking blocks must survive the round-trip.
        self.messages.append({"role": "assistant", "content": resp.content})

        return core.Step(
            text="\n".join(text_parts),
            actions=actions,
            usage=core.usage_from_anthropic(resp.usage),
            raw_usage=resp.usage.model_dump() if resp.usage else {},
            stop_reason=resp.stop_reason or "",
            latency_s=latency,
            reasoning_chars=thinking_chars,
        )

    def feed(self, outcomes: list[core.Outcome]) -> None:
        # All results go back in a single user turn — splitting them teaches the model
        # to stop asking for parallel calls.
        self.messages.append(
            {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": o.action.ref,
                        "content": core.format_result_body(o.exit_code, o.output),
                        "is_error": o.exit_code != 0,
                    }
                    for o in outcomes
                ],
            }
        )

    def feed_text(self, text: str) -> None:
        self.messages.append({"role": "user", "content": text})


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", default=DEFAULT_MODEL)
    parser.add_argument(
        "--effort",
        default="high",
        choices=["low", "medium", "high", "xhigh", "max", "none"],
        help="output_config.effort; 'none' omits the parameter",
    )
    core.add_common_args(parser)
    args = parser.parse_args()

    client = anthropic.Anthropic(
        api_key=core.resolve_key(["ANTHROPIC_API_KEY"], secret_name="ANTHROPIC_API_KEY")
    )
    effort = None if args.effort == "none" else args.effort
    core.drive(
        args,
        tasks,
        lambda: ClaudeToolsDriver(client, args.model, args.max_tokens, effort, args.prime),
    )


if __name__ == "__main__":
    main()
