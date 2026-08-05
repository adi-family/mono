#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["tiktoken>=0.7"]
# ///
"""Declared tools versus console commands behind one `sh` — priced exactly, offline.

Two ways to give an agent the same capabilities:

  declared   one JSON-schema tool per capability. The schemas ride in every request, and
             each call pays `<|open|>argument key=… type=…<|sep|>` per argument.

  cli        a single `sh(script)` tool, with the capabilities documented as commands.
             One schema regardless of how many commands exist, and a call is just shell —
             which also means several capabilities can compose in a single action.

The token side of that choice is computable. The parts that are not — gating, rendering,
auditing — are in the README; this only settles the arithmetic.

    uv run tools_vs_cli.py
    uv run tools_vs_cli.py --calls 20
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent / "tokenizer"))

import local_tokenizer  # noqa: E402

import encoding_k3  # noqa: E402  isort:skip


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


#: A realistic filesystem/search toolset, written the way such schemas normally are.
DECLARED = [
    tool(
        "read_file",
        "Read a file from the filesystem. Returns its contents with line numbers. "
        "Use offset and limit to read part of a large file.",
        {
            "path": {"type": "string", "description": "Absolute path to the file."},
            "offset": {"type": "integer", "description": "First line to read."},
            "limit": {"type": "integer", "description": "How many lines to read."},
        },
        ["path"],
    ),
    tool(
        "write_file",
        "Write a file to the filesystem, overwriting it if it already exists.",
        {
            "path": {"type": "string", "description": "Absolute path to the file."},
            "content": {"type": "string", "description": "Full contents to write."},
        },
        ["path", "content"],
    ),
    tool(
        "edit_file",
        "Replace an exact string in a file. The old string must appear exactly once.",
        {
            "path": {"type": "string", "description": "Absolute path to the file."},
            "old_string": {"type": "string", "description": "Text to replace."},
            "new_string": {"type": "string", "description": "Replacement text."},
        },
        ["path", "old_string", "new_string"],
    ),
    tool(
        "glob",
        "Find files matching a glob pattern, sorted by modification time.",
        {
            "pattern": {"type": "string", "description": "Glob pattern, e.g. **/*.rs"},
            "path": {"type": "string", "description": "Directory to search in."},
        },
        ["pattern"],
    ),
    tool(
        "grep",
        "Search file contents with a regular expression. Returns matching lines.",
        {
            "pattern": {"type": "string", "description": "Regular expression."},
            "path": {"type": "string", "description": "File or directory to search."},
            "glob": {"type": "string", "description": "Restrict to matching files."},
        },
        ["pattern"],
    ),
    tool(
        "list_dir",
        "List the entries of a directory.",
        {"path": {"type": "string", "description": "Absolute path to the directory."}},
        ["path"],
    ),
]

#: The same six capabilities as commands. Terser because the shell conventions are
#: already known to the model — it needs the names and the local specifics, not a
#: description of what `grep` is.
CLI_DOC = """\
# Commands
Everything below is available in the shell.

- `read <path> [--offset N] [--limit N]` — print a file, numbered
- `write <path>` — write stdin to a file, overwriting
- `edit <path> <old> <new>` — replace an exact string, which must occur once
- `glob <pattern> [dir]` — list matching files, newest first
- `grep <regex> [path] [--glob G]` — search file contents
- plus the usual coreutils; compose them with pipes and redirects.
"""

SH_TOOL = tool(
    "sh",
    "Run a bash script. Returns exit code, stdout and stderr.",
    {"script": {"type": "string", "description": "Full source to execute."}},
    ["script"],
)


def render(messages: list[dict], tools=None) -> str:
    segments = encoding_k3.build_chat_segments(
        messages=messages, tools=tools, add_generation_prompt=False, thinking=False
    )
    return "".join(segment.text for segment in segments)


def count(text: str) -> int:
    return len(local_tokenizer.encoder().encode(text, allowed_special="all"))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--calls", type=int, default=10)
    args = parser.parse_args()

    stub = [{"role": "user", "content": "x"}]
    base = count(render(stub))
    empty_turn = count(render(stub + [{"role": "assistant", "content": ""}]))

    def declare(tools) -> int:
        return count(render(stub, tools)) - base

    def call_cost(name: str, arguments: dict) -> int:
        return (
            count(
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
                                    "function": {"name": name, "arguments": arguments},
                                }
                            ],
                        }
                    ]
                )
            )
            - empty_turn
        )

    print("\n=== standing cost: schemas vs one schema + a command doc ===\n")
    print(f"  {'capabilities':>12}  {'declared':>9}  {'sh + doc':>9}")
    print("  " + "-" * 36)
    sh_declare = declare([SH_TOOL])
    for n in range(1, len(DECLARED) + 1):
        doc_lines = CLI_DOC.splitlines()
        doc = "\n".join(doc_lines[:3] + doc_lines[3 : 3 + n] + doc_lines[-1:])
        cli_side = sh_declare + count(
            render([{"role": "system", "content": doc}] + stub)
        ) - base
        print(f"  {n:>12}  {declare(DECLARED[:n]):>9,}  {cli_side:>9,}")

    print("\n=== one action, same work ===\n")
    pairs = [
        ("read one file", ("read_file", {"path": "/w/data/a.txt"}), "read /w/data/a.txt\n"),
        (
            "grep, then count hits",
            ("grep", {"pattern": "TODO", "path": "/w/src", "glob": "*.rs"}),
            "grep TODO /w/src --glob '*.rs' | wc -l\n",
        ),
        (
            "three files, line counts",
            ("read_file", {"path": "/w/data/a.txt"}),
            "wc -l /w/data/*.txt\n",
        ),
    ]
    print(f"  {'operation':<26} {'declared':>9} {'shell':>7} {'delta':>7}")
    print("  " + "-" * 54)
    for label, (name, arguments), script in pairs:
        declared_cost = call_cost(name, arguments)
        shell_cost = call_cost("sh", {"script": script})
        print(
            f"  {label:<26} {declared_cost:>9} {shell_cost:>7} {shell_cost - declared_cost:>+7}"
        )
    print(
        "\n  note: the third row is not a fair per-call comparison — the shell does all\n"
        "  three files in one action, where the declared toolset needs three calls."
    )

    print(f"\n=== projection over {args.calls} actions, 6 capabilities ===\n")
    doc_cost = sh_declare + count(render([{"role": "system", "content": CLI_DOC}] + stub)) - base
    declared_total = declare(DECLARED)
    read_declared = call_cost("read_file", {"path": "/w/data/a.txt"})
    read_shell = call_cost("sh", {"script": "read /w/data/a.txt\n"})
    print(f"  declared : {declared_total:,} standing + {read_declared} x {args.calls} "
          f"= {declared_total + read_declared * args.calls:,}")
    print(f"  sh + doc : {doc_cost:,} standing + {read_shell} x {args.calls} "
          f"= {doc_cost + read_shell * args.calls:,}")
    print()


if __name__ == "__main__":
    main()
