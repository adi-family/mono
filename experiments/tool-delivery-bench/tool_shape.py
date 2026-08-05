#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["tiktoken>=0.7"]
# ///
"""How should the code-running tool actually be shaped? Priced exactly, offline.

Once you accept the native tool channel (see README), the remaining design question is
what to declare. The schema is paid once per request; the call envelope is paid once per
action. Those pull in opposite directions:

  * more tools, or longer descriptions  -> bigger declaration, same call
  * more arguments per tool             -> smaller declaration, bigger call, because K3
                                          renders `<|open|>argument key=… type=…<|sep|>`
                                          around every single argument

So a shape that looks tidy can lose on a long agent run, and the crossover is computable
rather than a matter of taste.

    uv run tool_shape.py
    uv run tool_shape.py --calls 40
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent / "tokenizer"))

import local_tokenizer  # noqa: E402

import encoding_k3  # noqa: E402  isort:skip

SCRIPT = "chmod +x collect.sh && ./collect.sh > out.txt\n"

LONG_DESC_SH = (
    "Run a bash script in the working directory. The whole script source is passed as "
    "one string. The working directory persists between calls; each call is a fresh "
    "process. Returns the exit code plus stdout and stderr."
)
SHORT_DESC_SH = "Run a bash script. Returns exit code, stdout and stderr."


def tool(name: str, description: str, properties: dict, required: list[str]) -> dict:
    return {
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": {
                "type": "object",
                "properties": properties,
                "required": required,
            },
        },
    }


SCRIPT_PROP = {"script": {"type": "string", "description": "Full source to execute."}}

SHAPES: dict[str, tuple[list[dict], dict]] = {
    "two tools, 1 arg (current)": (
        [
            tool("sh", LONG_DESC_SH, SCRIPT_PROP, ["script"]),
            tool(
                "py",
                LONG_DESC_SH.replace("bash script", "Python 3 script"),
                SCRIPT_PROP,
                ["script"],
            ),
        ],
        {"name": "sh", "arguments": {"script": SCRIPT}},
    ),
    "one tool, 2 args (lang+script)": (
        [
            tool(
                "run",
                "Run a script in the working directory. Returns exit code, stdout and stderr.",
                {
                    "lang": {"type": "string", "enum": ["sh", "py"]},
                    "script": {"type": "string", "description": "Full source to execute."},
                },
                ["lang", "script"],
            )
        ],
        {"name": "run", "arguments": {"lang": "sh", "script": SCRIPT}},
    ),
    "one tool, 1 arg (sh only)": (
        [tool("sh", LONG_DESC_SH, SCRIPT_PROP, ["script"])],
        {"name": "sh", "arguments": {"script": SCRIPT}},
    ),
    "one tool, 1 arg, short desc": (
        [tool("sh", SHORT_DESC_SH, SCRIPT_PROP, ["script"])],
        {"name": "sh", "arguments": {"script": SCRIPT}},
    ),
}


def render(messages: list[dict], tools=None) -> str:
    segments = encoding_k3.build_chat_segments(
        messages=messages, tools=tools, add_generation_prompt=False, thinking=False
    )
    return "".join(segment.text for segment in segments)


def count(text: str) -> int:
    return len(local_tokenizer.encoder().encode(text, allowed_special="all"))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--calls", type=int, default=10, help="actions in the projection")
    args = parser.parse_args()

    stub = [{"role": "user", "content": "x"}]
    baseline = count(render(stub))
    body = count(SCRIPT)

    print(f"\nscript body, identical in every shape: {body} tokens\n")
    print(f"  {'shape':<32} {'declare':>8} {'per call':>9} {'@1':>7} {'@'+str(args.calls):>7}")
    print("  " + "-" * 68)

    rows = []
    for label, (tools, call) in SHAPES.items():
        declare = count(render(stub, tools)) - baseline

        with_call = count(
            render(
                stub
                + [
                    {
                        "role": "assistant",
                        "content": "",
                        "tool_calls": [{"id": "c1", "type": "function", "function": call}],
                    }
                ]
            )
        )
        empty_turn = count(
            render(stub + [{"role": "assistant", "content": ""}])
        )
        per_call = with_call - empty_turn

        rows.append((label, declare, per_call))
        print(
            f"  {label:<32} {declare:>8,} {per_call:>9,} "
            f"{declare + per_call:>7,} {declare + per_call * args.calls:>7,}"
        )

    # What actually drives the per-call cost: argument *count*. K3 renders
    # `<|open|>argument key=… type=…<|sep|>` around each one, and `_xtml_type` derives
    # that type from the runtime value, not the schema — so an enum and a free string
    # are byte-identical at call time. Richness is free; arity is not.
    def per_call_of(arguments: dict) -> int:
        with_call = count(
            render(
                stub
                + [
                    {
                        "role": "assistant",
                        "content": "",
                        "tool_calls": [
                            {
                                "id": "c1",
                                "type": "function",
                                "function": {"name": "run", "arguments": arguments},
                            }
                        ],
                    }
                ]
            )
        )
        return with_call - count(render(stub + [{"role": "assistant", "content": ""}]))

    print("\n=== what per-call cost is really made of ===\n")
    ladder = [
        ("1 argument  {script}", {"script": SCRIPT}),
        ("2 arguments {lang, script}", {"lang": "sh", "script": SCRIPT}),
        ("3 arguments {lang, script, timeout}", {"lang": "sh", "script": SCRIPT, "timeout": 60}),
    ]
    for label, arguments in ladder:
        print(f"  {label:<38} {per_call_of(arguments):>4} tokens")
    print("\n  -> linear in argument count, about +15 tokens each, on every call")

    best_1 = min(rows, key=lambda r: r[1] + r[2])
    best_n = min(rows, key=lambda r: r[1] + r[2] * args.calls)
    print(f"\n  cheapest at 1 action           : {best_1[0]}")
    print(f"  cheapest at {args.calls} actions{'':<10}: {best_n[0]}")

    two = next(r for r in rows if r[0].startswith("two tools"))
    one = next(r for r in rows if r[0].startswith("one tool, 2 args"))
    saved = two[1] - one[1]
    extra = one[2] - two[2]
    print(
        f"\n  collapsing two tools into one with a `lang` argument saves {saved:,} tokens "
        f"of declaration\n  and costs {extra:,} more per call"
        + (
            f" -> it pays off below {saved / extra:.0f} actions per conversation"
            if extra > 0
            else " -> it wins on both axes"
        )
    )
    print()


if __name__ == "__main__":
    main()
