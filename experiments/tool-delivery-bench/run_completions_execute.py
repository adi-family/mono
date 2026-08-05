#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["requests>=2.31"]
# ///
"""The execute arm on a raw text-completion endpoint, where it is the native channel.

Chat APIs put you inside someone else's template and someone else's tool channel. On
`/v1/completions` the prompt is ours byte for byte, which changes three things that the
chat-based arms could not fix:

  * There is no tool channel to reach for, so the kimi-k3 stall — decide to act, find no
    tool, emit empty content — cannot happen by construction. No priming, no nudges.
  * `stop: ["</execute>"]` halts generation exactly at the closing tag. Nothing is spent
    on a trailing narration the harness would discard.
  * Optionally the completion is *forced* to open with `<execute lang="` (--force-block),
    which removes the "announce instead of act" failure entirely.

Token accounting is exact on both sides because we wrote the prompt.

Needs a platform that serves the model over the legacy endpoint. All of the presets below
answer 401/403 rather than 404 unauthenticated, i.e. the route is live; whether the model
is enabled on it is what this script settles.

    uv run run_completions_execute.py --preset together   --repeat 3
    uv run run_completions_execute.py --preset openrouter --model moonshotai/kimi-k3
    uv run run_completions_execute.py --base-url https://…/v1/completions --model … \\
        --api-key-env MY_KEY --probe-only
"""

from __future__ import annotations

import argparse
import os
import time

import requests

import core
import prompts
import tasks

#: base URL, default model id, env var holding the key.
PRESETS = {
    "together": (
        "https://api.together.xyz/v1/completions",
        "moonshotai/Kimi-K3",
        "TOGETHER_API_KEY",
    ),
    "fireworks": (
        "https://api.fireworks.ai/inference/v1/completions",
        "accounts/fireworks/models/kimi-k3",
        "FIREWORKS_API_KEY",
    ),
    "openrouter": (
        "https://openrouter.ai/api/v1/completions",
        "moonshotai/kimi-k3",
        "OPENROUTER_API_KEY",
    ),
    "novita": (
        "https://api.novita.ai/openai/v1/completions",
        "moonshotai/kimi-k3",
        "NOVITA_API_KEY",
    ),
    "deepinfra": (
        "https://api.deepinfra.com/v1/openai/completions",
        "moonshotai/Kimi-K3",
        "DEEPINFRA_API_KEY",
    ),
    "moonshot": (
        "https://api.moonshot.ai/v1/completions",
        "kimi-k3",
        "KIMI_API_KEY",
    ),
}

STOP = ["</execute>", "\n### RESULT", "\n### TASK"]

#: Plain-text transcript. Deliberately not any provider's chat template — the point is
#: that we control every byte, and the same rendering is reused across platforms.
HEADER = "### INSTRUCTIONS\n{system}\n\n### TASK\n{task}\n\n### ASSISTANT\n"
RESULT = "\n\n### RESULT\n{body}\n\n### ASSISTANT\n"


class CompletionsExecuteDriver:
    arm = "execute"

    def __init__(
        self,
        *,
        base_url: str,
        model: str,
        api_key: str,
        provider: str,
        max_tokens: int,
        force_block: bool,
    ):
        self.base_url = base_url
        self.model = model
        self.api_key = api_key
        self.provider = provider
        self.max_tokens = max_tokens
        self.force_block = force_block
        self.prompt = ""

    def begin(self, system_prompt: str, user_prompt: str) -> None:
        self.prompt = HEADER.format(system=system_prompt, task=user_prompt)

    def _post(self, prompt: str) -> dict:
        response = requests.post(
            self.base_url,
            headers={
                "Authorization": f"Bearer {self.api_key}",
                "Content-Type": "application/json",
            },
            json={
                "model": self.model,
                "prompt": prompt,
                "max_tokens": self.max_tokens,
                "stop": STOP,
            },
            timeout=300,
        )
        if response.status_code != 200:
            raise RuntimeError(f"HTTP {response.status_code}: {response.text[:400]}")
        return response.json()

    def step(self) -> core.Step:
        # Forcing the opening tag makes acting the only continuation available.
        prefix = '<execute lang="' if self.force_block else ""
        started = time.monotonic()
        payload = self._post(self.prompt + prefix)
        latency = time.monotonic() - started

        choice = (payload.get("choices") or [{}])[0]
        text = prefix + (choice.get("text") or "")
        finish = choice.get("finish_reason") or ""
        # The stop sequence is consumed, so put the closing tag back before parsing.
        if finish == "stop" and "<execute" in text and "</execute>" not in text:
            text += "</execute>"

        self.prompt += text
        actions = [
            core.Action(lang=lang, code=code, wire=wire, ref=str(i))
            for i, (lang, code, wire) in enumerate(
                core.parse_execute_blocks(text), start=1
            )
        ]
        return core.Step(
            text=text,
            actions=actions,
            usage=core.usage_from_openai(payload.get("usage")),
            raw_usage=payload.get("usage") or {},
            stop_reason=finish,
            latency_s=latency,
        )

    def feed(self, outcomes: list[core.Outcome]) -> None:
        self.prompt += RESULT.format(body=core.render_execute_results(outcomes))

    def feed_text(self, text: str) -> None:
        self.prompt += RESULT.format(body=text)


def probe(base_url: str, model: str, api_key: str) -> None:
    """Answer the only question the docs will not: is this model live on /completions?"""
    print(f"POST {base_url}\n  model={model}")
    try:
        response = requests.post(
            base_url,
            headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"},
            json={"model": model, "prompt": "def fib(n):", "max_tokens": 24},
            timeout=90,
        )
    except Exception as exc:  # noqa: BLE001
        print(f"  transport error: {type(exc).__name__}: {exc}")
        return
    print(f"  HTTP {response.status_code}")
    body = response.text
    if response.status_code == 200:
        payload = response.json()
        text = (payload.get("choices") or [{}])[0].get("text", "")
        print(f"  usage: {payload.get('usage')}")
        print(f"  completion: {text!r}")
        print("  -> legacy completions is LIVE for this model")
    else:
        print(f"  {body[:400]}")
        print("  -> not usable; 404 means absent, 401/403 means auth or entitlement")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--preset", choices=sorted(PRESETS))
    parser.add_argument("--base-url")
    parser.add_argument("--model")
    parser.add_argument("--api-key-env")
    parser.add_argument(
        "--probe-only",
        action="store_true",
        help="just check whether the model answers on /completions, then exit",
    )
    parser.add_argument(
        "--force-block",
        action="store_true",
        help='seed each turn with `<execute lang="` so acting is the only continuation',
    )
    core.add_common_args(parser)
    args = parser.parse_args()

    if args.preset:
        base_url, model, key_env = PRESETS[args.preset]
    else:
        base_url, model, key_env = None, None, None
    base_url = args.base_url or base_url
    model = args.model or model
    key_env = args.api_key_env or key_env
    if not (base_url and model and key_env):
        raise SystemExit("need --preset, or all of --base-url --model --api-key-env")

    api_key = core.resolve_key([key_env], secret_name=key_env)
    provider = args.preset or base_url.split("/")[2]

    if args.probe_only:
        probe(base_url, model, api_key)
        return

    core.drive(
        args,
        tasks,
        lambda: CompletionsExecuteDriver(
            base_url=base_url,
            model=model,
            api_key=api_key,
            provider=provider,
            max_tokens=args.max_tokens,
            force_block=args.force_block,
        ),
    )


if __name__ == "__main__":
    main()
