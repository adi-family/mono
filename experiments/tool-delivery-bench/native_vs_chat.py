#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["tiktoken>=0.7"]
# ///
"""Native K3 tool calls versus the OpenAI-compatible chat shape — same task, exact tokens.

`encoding_k3.py` from the Hub repo is Kimi's own prompt encoder: it turns a conversation
into the exact token stream the model sees, control tokens and all. Combined with the
local tokenizer that means both encodings can be priced **offline and exactly**, with no
API call and no credit — and, more usefully, with no behavioural noise, because both arms
are handed the identical four-step conversation.

Two encodings of the same work:

  native   the tool channel the model was trained on. Tools declared once as JSON schemas
           in a `type="tool-declare"` system message; each call is
           `<|open|>call tool="sh"…<|sep|><|open|>argument key="script"…<|sep|>` with the
           script written through **verbatim**; each result comes back as a
           `role="tool"` message.

  execute  no tools declared at all. The protocol lives in the system prompt and every
           call is `<execute lang="sh">…</execute>` inside the ordinary response channel,
           with results fed back as `<result …>` text in the user turn.

Both are rendered through `build_chat_segments`, so what is compared is what the model
actually reads, not what the HTTP body happens to look like.

    uv run native_vs_chat.py
    uv run native_vs_chat.py --task chain --verbose
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent / "tokenizer"))

import core  # noqa: E402
import local_tokenizer  # noqa: E402
import prompts  # noqa: E402
import tasks  # noqa: E402

try:
    import encoding_k3  # noqa: E402
except ImportError as exc:  # pragma: no cover
    raise SystemExit(
        "tokenizer/encoding_k3.py is missing — fetch it with:\n"
        "  curl -sLO --output-dir tokenizer "
        "https://huggingface.co/moonshotai/Kimi-K3/resolve/main/encoding_k3.py"
    ) from exc


#: The four actions the `chain` task needs, written the way a model would write them.
#: Identical source in both arms — only the envelope around it changes.
ACTIONS = [
    (
        "sh",
        'cat > collect.sh <<\'EOF\'\n#!/bin/bash\nfor f in $(ls data/*.txt | sort); do\n'
        '  printf "%s:%s\\n" "$(basename "$f")" "$(wc -l < "$f" | tr -d \' \')"\ndone\nEOF\n',
        "exit=0\n(no output)",
    ),
    ("sh", "chmod +x collect.sh\n", "exit=0\n(no output)"),
    ("sh", "./collect.sh > out.txt\n", "exit=0\n(no output)"),
    (
        "sh",
        "cat out.txt\n",
        "exit=0\n--- stdout ---\nalpha.txt:3\nbeta.txt:2\ngamma.txt:5",
    ),
]

FINAL = "DONE: wrote collect.sh, made it executable, ran it into out.txt and read it back."


def native_conversation(task_prompt: str) -> tuple[list[dict], list[dict]]:
    """The conversation as the tool channel renders it, plus the tools to declare."""
    messages: list[dict] = [
        {"role": "system", "content": prompts.system_prompt("tools")},
        {"role": "user", "content": task_prompt},
    ]
    for index, (lang, script, result) in enumerate(ACTIONS, start=1):
        messages.append(
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {
                        "id": f"call_{index}",
                        "type": "function",
                        "function": {"name": lang, "arguments": {"script": script}},
                    }
                ],
            }
        )
        messages.append({"role": "tool", "tool": lang, "content": result})
    messages.append({"role": "assistant", "content": FINAL})
    return messages, core.openai_tools()


def execute_conversation(task_prompt: str) -> tuple[list[dict], None]:
    """The same work carried in ordinary response text, with no tools declared."""
    messages: list[dict] = [
        {"role": "system", "content": prompts.system_prompt("execute")},
        {"role": "user", "content": task_prompt},
    ]
    for lang, script, result in ACTIONS:
        messages.append(
            {
                "role": "assistant",
                "content": f'<execute lang="{lang}">\n{script}</execute>',
            }
        )
        messages.append(
            {
                "role": "user",
                "content": f'<result index="1" lang="{lang}" exit="0">\n{result}\n</result>',
            }
        )
    messages.append({"role": "assistant", "content": FINAL})
    return messages, None


def render(messages: list[dict], tools) -> str:
    segments = encoding_k3.build_chat_segments(
        messages=messages,
        tools=tools,
        add_generation_prompt=False,
        thinking=False,
    )
    return "".join(segment.text for segment in segments)


def count(text: str) -> int:
    return len(local_tokenizer.encoder().encode(text, allowed_special="all"))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--task", default="chain")
    parser.add_argument("--verbose", action="store_true", help="dump both renderings")
    args = parser.parse_args()

    task = tasks.BY_NAME[args.task]

    native_messages, native_tools = native_conversation(task.user_prompt)
    execute_messages, _ = execute_conversation(task.user_prompt)

    native_text = render(native_messages, native_tools)
    execute_text = render(execute_messages, None)

    # Like for like: each arm's whole standing preamble — system prompt, plus the JSON
    # schemas where the tool channel is used at all.
    empty = count(render([{"role": "user", "content": "x"}], None))
    schema_cost = (
        count(
            render(
                [
                    {"role": "system", "content": prompts.system_prompt("tools")},
                    {"role": "user", "content": "x"},
                ],
                native_tools,
            )
        )
        - empty
    )
    protocol_cost = (
        count(
            render(
                [
                    {"role": "system", "content": prompts.system_prompt("execute")},
                    {"role": "user", "content": "x"},
                ],
                None,
            )
        )
        - empty
    )

    native_total, execute_total = count(native_text), count(execute_text)

    print(f"\ntask: {task.name}   actions: {len(ACTIONS)}   (rendered by encoding_k3.py)\n")
    print("=== fixed setup cost ===\n")
    print(f"  native   system prompt + JSON tool schemas : {schema_cost:>6,} tokens")
    print(f"  execute  system prompt with the protocol   : {protocol_cost:>6,} tokens")
    print(f"  difference                                 : {protocol_cost - schema_cost:>+6,} tokens")

    print("\n=== whole conversation, four actions carried through ===\n")
    print(f"  native  (tool channel)      : {native_total:>6,} tokens")
    print(f"  execute (response channel)  : {execute_total:>6,} tokens")
    delta = execute_total - native_total
    print(f"  difference                  : {delta:>+6,} tokens ({delta / native_total * 100:+.1f}%)")

    print("\n=== per action, envelope only (script bodies are identical) ===\n")
    body = sum(count(script) for _, script, _ in ACTIONS)
    print(f"  script bodies, identical in both : {body:>6,} tokens")
    print(f"  native envelope + results        : {native_total - body:>6,} tokens")
    print(f"  execute envelope + results       : {execute_total - body:>6,} tokens")
    per_action = (execute_total - native_total) / len(ACTIONS)
    print(f"  difference per action            : {per_action:>+6.1f} tokens")

    if args.verbose:
        print("\n=== native rendering ===\n")
        print(native_text)
        print("\n=== execute rendering ===\n")
        print(execute_text)
    print()


if __name__ == "__main__":
    main()
