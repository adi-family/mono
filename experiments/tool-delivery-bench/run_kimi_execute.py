#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["openai>=1.60"]
# ///
"""Experimental arm on Kimi: code delivered as `<execute>` blocks in the output text.

No tools are declared. The system prompt teaches the block syntax; the harness parses
whatever blocks the reply contains, runs them all, and feeds every result back in one
user turn. The script is written verbatim, so nothing is JSON-escaped.

    uv run run_kimi_execute.py --repeat 3
"""

from __future__ import annotations

import argparse
import time

from openai import OpenAI

import core
import prompts
import tasks

MOONSHOT_BASE_URL = "https://api.moonshot.ai/v1"
DEFAULT_MODEL = "kimi-k3"


class KimiExecuteDriver:
    #: Set from the base URL so a run through a gateway is not filed under "moonshot".
    provider = "moonshot"
    arm = "execute"

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
        started = time.monotonic()
        resp = self.client.chat.completions.create(
            model=self.model,
            messages=self.messages,
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

        # Moonshot rejects an empty assistant turn in the history, and kimi-k3 does
        # sometimes return one — it plans an action in `reasoning_content` and then
        # emits nothing. Drop the turn; the stall nudge follows as a user message.
        if text.strip():
            self.messages.append({"role": "assistant", "content": text})

        actions = [
            core.Action(lang=lang, code=code, wire=wire, ref=str(i))
            for i, (lang, code, wire) in enumerate(
                core.parse_execute_blocks(text), start=1
            )
        ]

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
        self.messages.append(
            {"role": "user", "content": core.render_execute_results(outcomes)}
        )

    def feed_text(self, text: str) -> None:
        # Fold into the previous user turn when the assistant turn was dropped, so the
        # history never carries two user messages back to back.
        if self.messages and self.messages[-1]["role"] == "user":
            self.messages[-1]["content"] += "\n\n" + text
        else:
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
        lambda: KimiExecuteDriver(client, args.model, args.max_tokens, args.prime, provider),
    )


if __name__ == "__main__":
    main()
