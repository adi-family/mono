"""The task set.

Every task is deterministic: the same `setup` produces byte-identical inputs on every
run, and `verify` recomputes the expected answer from those inputs rather than comparing
against a stored blob. So a failed verification means the model got it wrong, never that
the fixture drifted.

The four tasks probe different things:

  fanout    fan-out over many files — does the arm change how work gets batched?
  escape    regexes, backslashes and quoted Windows paths — maximum escaping pain
  pipeline  a sequential multi-step computation — the control case
  probe     six unrelated facts — the purest parallelism signal
"""

from __future__ import annotations

import csv
import hashlib
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

import prompts


@dataclass
class Task:
    name: str
    user_prompt: str
    setup: Callable[[Path], None]
    verify: Callable[[Path], tuple[bool, str]]

    def system_prompt(self, arm: str) -> str:
        return prompts.system_prompt(arm)


def _sha256_text(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


# --------------------------------------------------------------------------------------
# fanout
# --------------------------------------------------------------------------------------

_FANOUT_WORDS = [
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
    "india", "juliet", "kilo", "lima", "mike", "november", "oscar", "papa",
]


def _fanout_setup(work: Path) -> None:
    data = work / "data"
    data.mkdir(parents=True, exist_ok=True)
    for i in range(1, 9):
        lines = [
            f"{_FANOUT_WORDS[(i * 3 + j) % len(_FANOUT_WORDS)]} {i}-{j}"
            for j in range(2 + i * 2)
        ]
        (data / f"part{i:02d}.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")


def _fanout_verify(work: Path) -> tuple[bool, str]:
    report = work / "report.tsv"
    if not report.exists():
        return False, "report.tsv missing"
    expected = []
    for path in sorted((work / "data").glob("*.txt")):
        text = path.read_text(encoding="utf-8")
        expected.append((path.name, str(text.count("\n")), _sha256_file(path)))
    got = [
        tuple(line.split("\t"))
        for line in report.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    if got and got[0][0].lower() in {"filename", "file", "name"}:
        got = got[1:]  # tolerate a header row
    if got != expected:
        return False, f"expected {len(expected)} rows, got {len(got)}; first diff: " + str(
            next((f"{e} != {g}" for e, g in zip(expected, got) if e != g), "length only")
        )
    return True, "ok"


FANOUT = Task(
    name="fanout",
    user_prompt=(
        "In ./data there are several .txt files. For each one, compute its line count "
        "and the SHA-256 hex digest of its bytes.\n\n"
        "Write ./report.tsv with one tab-separated row per file and no header:\n"
        "  <filename>\\t<line count>\\t<sha256 hex>\n"
        "Rows sorted by filename ascending. `filename` is the base name, not a path."
    ),
    setup=_fanout_setup,
    verify=_fanout_verify,
)


# --------------------------------------------------------------------------------------
# escape
# --------------------------------------------------------------------------------------

_LOG_LINES = [
    r'2026-08-04T09:00:01Z [INFO] service started, config at "C:\etc\svc\main.cfg"',
    r'2026-08-04T09:00:07Z [ERROR] failed to open "C:\Users\ivan\My Docs\a b.txt" after 3 tries',
    r"2026-08-04T09:01:12Z [WARN] slow read on \\share\vol1\data.bin",
    r'2026-08-04T09:01:44Z [WARN] retrying "D:\tmp\q\"uoted\".log" in 5s',
    r"2026-08-04T09:02:00Z [DEBUG] heartbeat ok",
    r'2026-08-04T09:02:31Z [ERROR] permission denied for "/var/log/app/err.log"',
    r'2026-08-04T09:03:03Z [INFO] rotated "C:\logs\app.1.log"',
    r"2026-08-04T09:03:59Z [ERROR] no quoted path in this one at all",
    r'2026-08-04T09:04:20Z [WARN] checksum mismatch on "E:\backup\2026\08\04\snap.tar.gz"',
    r'2026-08-04T09:05:05Z [ERROR] cannot stat "C:\Program Files\Some App\bin\run.exe"',
]

_LOG_RULE = (
    "A line is interesting when its level is ERROR or WARN and its message contains a "
    "double-quoted path. The quoted path runs from the first double quote on the line to "
    "the last double quote on the line."
)


def _escape_expected(work: Path) -> list[dict[str, str]]:
    out = []
    for line in (work / "log.txt").read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        ts, rest = line.split(" ", 1)
        level = rest[rest.index("[") + 1 : rest.index("]")]
        if level not in {"ERROR", "WARN"}:
            continue
        first, last = line.find('"'), line.rfind('"')
        if first == -1 or last == first:
            continue
        out.append({"ts": ts, "level": level, "path": line[first + 1 : last]})
    return out


def _escape_setup(work: Path) -> None:
    (work / "log.txt").write_text("\n".join(_LOG_LINES) + "\n", encoding="utf-8")


def _escape_verify(work: Path) -> tuple[bool, str]:
    if not (work / "extract.py").exists():
        return False, "extract.py missing"
    out = work / "out.jsonl"
    if not out.exists():
        return False, "out.jsonl missing"
    try:
        got = [
            json.loads(line)
            for line in out.read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]
    except json.JSONDecodeError as exc:
        return False, f"out.jsonl is not valid JSONL: {exc}"
    expected = _escape_expected(work)
    if len(got) != len(expected):
        return False, f"expected {len(expected)} records, got {len(got)}"
    for i, (e, g) in enumerate(zip(expected, got)):
        slim = {k: g.get(k) for k in ("ts", "level", "path")}
        if slim != e:
            return False, f"record {i}: expected {e}, got {slim}"
    return True, "ok"


ESCAPE = Task(
    name="escape",
    user_prompt=(
        "./log.txt holds lines shaped `<ISO timestamp> [<LEVEL>] <message>`.\n\n"
        f"{_LOG_RULE}\n\n"
        "Write a Python 3 script ./extract.py that uses a regular expression to pull the "
        "interesting lines out of ./log.txt, then run it. It must write ./out.jsonl with "
        "one JSON object per interesting line, in input order, with exactly these keys:\n"
        '  {"ts": <timestamp>, "level": <ERROR|WARN>, "path": <path without the surrounding quotes>}\n'
        "The path is copied through character for character — backslashes, spaces and any "
        "inner quotes stay exactly as they appear in the log."
    ),
    setup=_escape_setup,
    verify=_escape_verify,
)


# --------------------------------------------------------------------------------------
# pipeline
# --------------------------------------------------------------------------------------

_SALES_ROWS = [
    ("north", "widget", 12, 9.99),
    ("south", "widget", 30, 9.99),
    ("north", "gadget", 4, 129.50),
    ("east", "widget", 7, 9.99),
    ("south", "gadget", 11, 129.50),
    ("west", "doohickey", 250, 0.75),
    ("east", "gadget", 2, 129.50),
    ("north", "doohickey", 90, 0.75),
    ("west", "widget", 18, 9.99),
    ("south", "doohickey", 40, 0.75),
]


def _pipeline_setup(work: Path) -> None:
    with (work / "sales.csv").open("w", newline="", encoding="utf-8") as fh:
        writer = csv.writer(fh)
        writer.writerow(["region", "product", "units", "price"])
        writer.writerows(_SALES_ROWS)


def _pipeline_expected(work: Path) -> list[tuple[str, str]]:
    totals: dict[str, float] = {}
    with (work / "sales.csv").open(encoding="utf-8") as fh:
        for row in csv.DictReader(fh):
            totals[row["region"]] = totals.get(row["region"], 0.0) + int(
                row["units"]
            ) * float(row["price"])
    ordered = sorted(totals.items(), key=lambda kv: (-kv[1], kv[0]))
    return [(region, f"{value:.2f}") for region, value in ordered]


def _pipeline_verify(work: Path) -> tuple[bool, str]:
    revenue = work / "revenue.csv"
    top = work / "top.txt"
    if not revenue.exists():
        return False, "revenue.csv missing"
    if not top.exists():
        return False, "top.txt missing"
    expected = _pipeline_expected(work)
    rows = [
        line.split(",")
        for line in revenue.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    if not rows or [c.strip() for c in rows[0]] != ["region", "revenue"]:
        return False, f"expected header region,revenue; got {rows[0] if rows else '(empty)'}"
    got = [(r[0].strip(), r[1].strip()) for r in rows[1:]]
    if got != expected:
        return False, f"expected {expected}, got {got}"
    if top.read_text(encoding="utf-8").strip() != expected[0][0]:
        return False, (
            f"top.txt should be {expected[0][0]!r}, "
            f"got {top.read_text(encoding='utf-8').strip()!r}"
        )
    return True, "ok"


PIPELINE = Task(
    name="pipeline",
    user_prompt=(
        "./sales.csv has the columns region,product,units,price.\n\n"
        "Revenue for a row is units * price. Write ./revenue.csv with the header line "
        "`region,revenue` followed by one row per region, revenue rounded to exactly two "
        "decimal places, sorted by revenue descending (ties broken by region name "
        "ascending). Then write ./top.txt containing only the name of the highest-revenue "
        "region."
    ),
    setup=_pipeline_setup,
    verify=_pipeline_verify,
)


# --------------------------------------------------------------------------------------
# probe
# --------------------------------------------------------------------------------------

_PROBE_FILES = {
    "readme.md": "# probe fixture\nthis directory is generated\n",
    "a-fairly-long-filename.txt": "one\ntwo\nthree\nfour\nfive\n",
    "tiny.txt": "x\n",
    "notes.log": "\n".join(f"line {i}" for i in range(1, 41)) + "\n",
    "table.csv": "id,value\n1,10\n2,20\n3,30\n",
}


def _probe_setup(work: Path) -> None:
    data = work / "data"
    data.mkdir(parents=True, exist_ok=True)
    for name, body in _PROBE_FILES.items():
        (data / name).write_text(body, encoding="utf-8")


def _probe_expected(work: Path) -> dict[str, str]:
    data = work / "data"
    files = sorted(p for p in data.iterdir() if p.is_file())
    names = [p.name for p in files]
    longest = sorted(names, key=lambda n: (-len(n), n))[0]
    largest = sorted(files, key=lambda p: (-p.stat().st_size, p.name))[0]
    return {
        "n_files": str(len(files)),
        "total_bytes": str(sum(p.stat().st_size for p in files)),
        "longest_name": longest,
        "max_lines": str(
            max(p.read_text(encoding="utf-8").count("\n") for p in files)
        ),
        "manifest_sha256": _sha256_text("\n".join(names) + "\n"),
        "first_line_of_largest": largest.read_text(encoding="utf-8").splitlines()[0],
    }


def _probe_verify(work: Path) -> tuple[bool, str]:
    facts = work / "facts.txt"
    if not facts.exists():
        return False, "facts.txt missing"
    got: dict[str, str] = {}
    for line in facts.read_text(encoding="utf-8").splitlines():
        if "=" in line:
            key, _, value = line.partition("=")
            got[key.strip()] = value.strip()
    expected = _probe_expected(work)
    wrong = {k: (v, got.get(k)) for k, v in expected.items() if got.get(k) != v}
    if wrong:
        return False, "; ".join(f"{k}: expected {e!r} got {g!r}" for k, (e, g) in wrong.items())
    return True, "ok"


PROBE = Task(
    name="probe",
    user_prompt=(
        "Look at the files directly inside ./data (regular files only, no recursion) and "
        "write ./facts.txt with exactly these six lines, in this order, as `key=value` "
        "with no spaces around the `=`:\n\n"
        "  n_files=                 how many files there are\n"
        "  total_bytes=             sum of their sizes in bytes\n"
        "  longest_name=            the file name with the most characters (ties: "
        "lexicographically smallest)\n"
        "  max_lines=               the highest line count among them\n"
        "  manifest_sha256=         SHA-256 hex of the UTF-8 bytes of the file names "
        "sorted ascending, joined by a newline, with a trailing newline\n"
        "  first_line_of_largest=   the first line of the largest file by byte size "
        "(ties: lexicographically smallest name)"
    ),
    setup=_probe_setup,
    verify=_probe_verify,
)


# --------------------------------------------------------------------------------------

# --------------------------------------------------------------------------------------
# chain — the fixed task for the native-vs-chat comparison
# --------------------------------------------------------------------------------------
#
# Deliberately a dependency chain rather than fan-out: write a script, make it
# executable, run it so its output lands in a file, read the file back. Nothing here can
# be parallelised away, so the two encodings are compared over the same four actions.

_CHAIN_FILES = {
    "alpha.txt": "one\ntwo\nthree\n",
    "beta.txt": "x\ny\n",
    "gamma.txt": "1\n2\n3\n4\n5\n",
}


def _chain_setup(work: Path) -> None:
    data = work / "data"
    data.mkdir(parents=True, exist_ok=True)
    for name, body in _CHAIN_FILES.items():
        (data / name).write_text(body, encoding="utf-8")


def _chain_expected(work: Path) -> list[str]:
    rows = []
    for path in sorted((work / "data").glob("*.txt")):
        rows.append(f"{path.name}:{path.read_text(encoding='utf-8').count(chr(10))}")
    return rows


def _chain_verify(work: Path) -> tuple[bool, str]:
    script = work / "collect.sh"
    out = work / "out.txt"
    if not script.exists():
        return False, "collect.sh missing"
    if not script.stat().st_mode & 0o111:
        return False, "collect.sh is not executable (chmod step skipped)"
    if not out.exists():
        return False, "out.txt missing"
    expected = _chain_expected(work)
    got = [line.strip() for line in out.read_text(encoding="utf-8").splitlines() if line.strip()]
    if got != expected:
        return False, f"expected {expected}, got {got}"
    return True, "ok"


CHAIN = Task(
    name="chain",
    user_prompt=(
        "Do these four steps in order, each depending on the one before it:\n\n"
        "1. Write a shell script ./collect.sh that, for every .txt file directly inside "
        "./data, prints one line `<filename>:<line count>` — filenames sorted ascending, "
        "no path, no header.\n"
        "2. Make ./collect.sh executable.\n"
        "3. Run it so that its stdout is redirected into ./out.txt.\n"
        "4. Read ./out.txt back and report its contents in your final message."
    ),
    setup=_chain_setup,
    verify=_chain_verify,
)


ALL: list[Task] = [FANOUT, ESCAPE, PIPELINE, PROBE, CHAIN]
BY_NAME = {t.name: t for t in ALL}


def select(names: list[str] | None) -> list[Task]:
    if not names:
        return ALL
    unknown = [n for n in names if n not in BY_NAME]
    if unknown:
        raise SystemExit(
            f"unknown task(s): {', '.join(unknown)}; available: {', '.join(BY_NAME)}"
        )
    return [BY_NAME[n] for n in names]
