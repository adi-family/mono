#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["anthropic>=0.80"]
# ///
"""Experimental arm on Claude: code delivered as `<execute>` blocks in the output text.

No tools are declared, so the model never pays for a JSON-escaped argument blob — it
writes the script into its reply the way it would type it into an editor.

    uv run run_claude_execute.py --repeat 3
"""

from __future__ import annotations

import argparse
import time

import anthropic

import core
import prompts
import tasks

DEFAULT_MODEL = "claude-opus-5"


class ClaudeExecuteDriver:
    provider = "anthropic"
    arm = "execute"

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
        self.messages = []
        if self.prime:
            self.messages += [
                {"role": "user", "content": prompts.PRIME_USER},
                {
                    "role": "assistant",
                    "content": f'<execute lang="{prompts.PRIME_LANG}">\n'
                    f"{prompts.PRIME_CODE}\n</execute>",
                },
                {
                    "role": "user",
                    "content": f'<result index="1" lang="{prompts.PRIME_LANG}" exit="0">\n'
                    f"{prompts.PRIME_RESULT_BODY}\n</result>",
                },
            ]
        self.messages.append({"role": "user", "content": user_prompt})

    def step(self) -> core.Step:
        kwargs: dict = {
            "model": self.model,
            "max_tokens": self.max_tokens,
            "system": self.system,
            "messages": self.messages,
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
        for block in resp.content:
            if block.type == "text":
                text_parts.append(block.text)
            elif block.type == "thinking":
                thinking_chars += len(getattr(block, "thinking", "") or "")
        text = "\n".join(text_parts)

        self.messages.append({"role": "assistant", "content": resp.content})

        actions = [
            core.Action(lang=lang, code=code, wire=wire, ref=str(i))
            for i, (lang, code, wire) in enumerate(
                core.parse_execute_blocks(text), start=1
            )
        ]

        return core.Step(
            text=text,
            actions=actions,
            usage=core.usage_from_anthropic(resp.usage),
            raw_usage=resp.usage.model_dump() if resp.usage else {},
            stop_reason=resp.stop_reason or "",
            latency_s=latency,
            reasoning_chars=thinking_chars,
        )

    def feed(self, outcomes: list[core.Outcome]) -> None:
        self.messages.append(
            {"role": "user", "content": core.render_execute_results(outcomes)}
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
        lambda: ClaudeExecuteDriver(client, args.model, args.max_tokens, effort, args.prime),
    )


if __name__ == "__main__":
    main()
