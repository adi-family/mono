"""Kimi's tokenizer, run locally — no API call, no model, no cost.

`moonshotai/Kimi-K3` publishes its tokenizer on the Hub ungated: a `tiktoken.model` in
the standard `base64-token rank` format plus the split pattern in `tokenization_kimi.py`.
That is enough to rebuild the exact encoder with `tiktoken` and tokenize anything offline.

Verified against Moonshot's own `estimate-token-count` endpoint — identical counts on
every probe (200 backslashes 25/25, 200 quotes 50/50, 200 newlines 13/13).

Why it matters here: the hosted endpoint *parses* `tool_calls.arguments` and prices the
unescaped value, so it cannot see the escaping tax at all. The local encoder sees exactly
what the model decodes, token by token, and can show you the token IDs.

    python -c "import local_tokenizer as lt; print(lt.count('def f(x):\\n    return x'))"

Fetch the files once with `download_tokenizer.sh`, or let `ensure()` pull them.
"""

from __future__ import annotations

import base64
import json
import urllib.request
from functools import lru_cache
from pathlib import Path

HF_REPO = "moonshotai/Kimi-K3"
FILES = ("tiktoken.model", "tokenizer_config.json")
TOKENIZER_DIR = Path(__file__).parent / "tokenizer"

#: Copied verbatim from `tokenization_kimi.py` in the Hub repo. Do not "fix" the `&&`
#: character-class intersections — tiktoken's Rust engine accepts them as written.
PAT_STR = "|".join(
    [
        r"""[\p{Han}]+""",
        r"""[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}&&[^\p{Han}]]*[\p{Ll}\p{Lm}\p{Lo}\p{M}&&[^\p{Han}]]+(?i:'s|'t|'re|'ve|'m|'ll|'d)?""",
        r"""[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}&&[^\p{Han}]]+[\p{Ll}\p{Lm}\p{Lo}\p{M}&&[^\p{Han}]]*(?i:'s|'t|'re|'ve|'m|'ll|'d)?""",
        r"""\p{N}{1,3}""",
        r""" ?[^\s\p{L}\p{N}]+[\r\n]*""",
        r"""\s*[\r\n]+""",
        r"""\s+(?!\S)""",
        r"""\s+""",
    ]
)


def ensure(directory: Path = TOKENIZER_DIR) -> Path:
    """Download the tokenizer files if they are not already on disk."""
    directory.mkdir(parents=True, exist_ok=True)
    for name in FILES:
        target = directory / name
        if target.exists() and target.stat().st_size > 0:
            continue
        url = f"https://huggingface.co/{HF_REPO}/resolve/main/{name}"
        with urllib.request.urlopen(url, timeout=180) as response:
            target.write_bytes(response.read())
    return directory


@lru_cache(maxsize=1)
def encoder(directory: str | None = None):
    """The `tiktoken.Encoding` for Kimi. Cached; the BPE file is ~2.8 MB."""
    import tiktoken

    root = ensure(Path(directory) if directory else TOKENIZER_DIR)

    ranks: dict[bytes, int] = {}
    with (root / "tiktoken.model").open("rb") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            token, rank = line.split()
            ranks[base64.b64decode(token)] = int(rank)

    config = json.loads((root / "tokenizer_config.json").read_text(encoding="utf-8"))
    special = {
        entry["content"]: int(index)
        for index, entry in (config.get("added_tokens_decoder") or {}).items()
    }

    return tiktoken.Encoding(
        name="kimi-k3",
        pat_str=PAT_STR,
        mergeable_ranks=ranks,
        special_tokens=special,
    )


def encode(text: str) -> list[int]:
    return encoder().encode(text)


def count(text: str) -> int:
    return len(encoder().encode(text))


def explain(text: str, limit: int = 40) -> list[str]:
    """The first `limit` tokens as readable pieces — useful for seeing *why* a string
    is expensive (a run of `\\n` escapes shows up as one token per line)."""
    enc = encoder()
    return [enc.decode([tid]) for tid in enc.encode(text)[:limit]]
