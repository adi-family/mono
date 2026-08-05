"""Shared machinery for the tool-delivery benchmark.

The experiment compares two ways of handing a model the ability to run code:

  arm "tools"    — native function calling. The model emits a structured tool call
                   whose arguments are a JSON object; the code is a JSON string value,
                   so every quote, backslash and newline inside it must be escaped.

  arm "execute"  — no tools declared. The model writes the code straight into its
                   output text inside `<execute lang="py|sh">...</execute>` blocks,
                   verbatim, and the harness runs whatever it finds.

Both arms get exactly the same capability (a bash runner and a python runner over a
persistent working directory), the same tasks and the same result formatting, so the
only variable is the delivery mechanism.

Everything here is provider-agnostic. Each of the four runner scripts supplies a
`Driver` that knows how to talk to one provider in one arm.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import time
import uuid
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Iterable, Protocol

# --------------------------------------------------------------------------------------
# Execution sandbox
# --------------------------------------------------------------------------------------

#: Merged stdout+stderr handed back to the model is clipped to this many characters.
MAX_OUTPUT_CHARS = 4000

#: Wall-clock ceiling for a single block of model-written code.
EXEC_TIMEOUT_S = 60

LANG_ALIASES = {
    "py": "py",
    "python": "py",
    "python3": "py",
    "sh": "sh",
    "bash": "sh",
    "shell": "sh",
    "zsh": "sh",
}

#: Names matching these are stripped from the environment the model's code inherits, so
#: a task can never read the credentials that are paying for the run.
SECRET_ENV_PATTERN = re.compile(
    r"(API_KEY|SECRET|TOKEN|PASSWORD|CREDENTIAL)", re.IGNORECASE
)


def child_env() -> dict[str, str]:
    """The environment model-written code runs under: ours, minus anything secret."""
    return {k: v for k, v in os.environ.items() if not SECRET_ENV_PATTERN.search(k)}


def clip(text: str, limit: int = MAX_OUTPUT_CHARS) -> tuple[str, bool]:
    """Clip long output head+tail so the model still sees both ends of it."""
    if len(text) <= limit:
        return text, False
    head = text[: (limit * 2) // 3]
    tail = text[-(limit // 3) :]
    omitted = len(text) - len(head) - len(tail)
    return f"{head}\n...[{omitted} characters omitted]...\n{tail}", True


class Sandbox:
    """A per-episode working directory plus a runner for model-written code.

    The model's cwd is `workdir`. Scratch files holding the code itself live in a
    sibling directory, so listing the working directory shows only task artifacts.
    """

    def __init__(self, root: Path):
        self.root = root
        self.workdir = root / "work"
        self.scratch = root / "_exec"
        self.workdir.mkdir(parents=True, exist_ok=True)
        self.scratch.mkdir(parents=True, exist_ok=True)
        self._n = 0

    def run(self, lang: str, code: str) -> tuple[int, str, float, bool]:
        """Run one block. Returns (exit_code, combined_output, seconds, truncated)."""
        kind = LANG_ALIASES.get(lang.lower())
        self._n += 1
        if kind is None:
            return 2, f"unsupported lang {lang!r}; use \"py\" or \"sh\"", 0.0, False

        suffix = ".py" if kind == "py" else ".sh"
        path = self.scratch / f"block{self._n:03d}{suffix}"
        path.write_text(code, encoding="utf-8")
        argv = [sys.executable, str(path)] if kind == "py" else ["bash", str(path)]

        started = time.monotonic()
        try:
            proc = subprocess.run(
                argv,
                cwd=self.workdir,
                env=child_env(),
                capture_output=True,
                text=True,
                timeout=EXEC_TIMEOUT_S,
            )
            code_out, out, err = proc.returncode, proc.stdout, proc.stderr
        except subprocess.TimeoutExpired as exc:
            code_out = 124
            out = exc.stdout or ""
            err = (exc.stderr or "") + f"\n[timed out after {EXEC_TIMEOUT_S}s]"
        duration = time.monotonic() - started

        parts = []
        if out.strip():
            parts.append(f"--- stdout ---\n{out.rstrip()}")
        if err.strip():
            parts.append(f"--- stderr ---\n{err.rstrip()}")
        if not parts:
            parts.append("(no output)")
        body, truncated = clip("\n".join(parts))
        return code_out, body, duration, truncated


def format_result_body(exit_code: int, output: str) -> str:
    """The result text both arms hand back — identical wording, so input tokens compare."""
    return f"exit={exit_code}\n{output}"


# --------------------------------------------------------------------------------------
# The `<execute>` protocol
# --------------------------------------------------------------------------------------

EXECUTE_RE = re.compile(
    r"<execute\s+lang\s*=\s*[\"']?(?P<lang>[A-Za-z0-9_+-]+)[\"']?\s*>"
    r"(?P<body>.*?)"
    r"</execute\s*>",
    re.DOTALL | re.IGNORECASE,
)


def parse_execute_blocks(text: str) -> list[tuple[str, str, str]]:
    """Extract (lang, code, wire_text) for every execute block, in order.

    `wire_text` is the whole `<execute …>…</execute>` span — what the model actually
    had to emit to request this action, which is what the escaping metric compares.
    """
    found = []
    for m in EXECUTE_RE.finditer(text):
        body = m.group("body")
        # A leading newline right after the opening tag is formatting, not code.
        if body.startswith("\r\n"):
            body = body[2:]
        elif body.startswith("\n"):
            body = body[1:]
        found.append((m.group("lang"), body.rstrip() + "\n", m.group(0)))
    return found


def render_execute_results(outcomes: list["Outcome"]) -> str:
    """The user turn fed back after a batch of execute blocks."""
    chunks = []
    for i, o in enumerate(outcomes, start=1):
        chunks.append(
            f'<result index="{i}" lang="{o.action.lang}" exit="{o.exit_code}">\n'
            f"{o.output}\n"
            f"</result>"
        )
    return "\n".join(chunks)


# --------------------------------------------------------------------------------------
# Tool schemas for the "tools" arm (same capability as `<execute>`, JSON-shaped)
# --------------------------------------------------------------------------------------

TOOL_SPECS = [
    {
        "name": "sh",
        "description": (
            "Run a bash script in the working directory. The whole script source is "
            "passed as one string. The working directory persists between calls; each "
            "call is a fresh process. Returns the exit code plus stdout and stderr."
        ),
        "parameter": "script",
        "parameter_description": "Full bash source to execute.",
    },
    {
        "name": "py",
        "description": (
            "Run a Python 3 script in the working directory. The whole script source is "
            "passed as one string. The working directory persists between calls; each "
            "call is a fresh process. Returns the exit code plus stdout and stderr."
        ),
        "parameter": "script",
        "parameter_description": "Full Python 3 source to execute.",
    },
]


def openai_tools() -> list[dict[str, Any]]:
    """Tool definitions in the OpenAI / Moonshot chat-completions dialect."""
    return [
        {
            "type": "function",
            "function": {
                "name": spec["name"],
                "description": spec["description"],
                "parameters": {
                    "type": "object",
                    "properties": {
                        spec["parameter"]: {
                            "type": "string",
                            "description": spec["parameter_description"],
                        }
                    },
                    "required": [spec["parameter"]],
                },
            },
        }
        for spec in TOOL_SPECS
    ]


def anthropic_tools() -> list[dict[str, Any]]:
    """Tool definitions in the Anthropic Messages dialect."""
    return [
        {
            "name": spec["name"],
            "description": spec["description"],
            "input_schema": {
                "type": "object",
                "properties": {
                    spec["parameter"]: {
                        "type": "string",
                        "description": spec["parameter_description"],
                    }
                },
                "required": [spec["parameter"]],
                "additionalProperties": False,
            },
        }
        for spec in TOOL_SPECS
    ]


# --------------------------------------------------------------------------------------
# Usage accounting and pricing
# --------------------------------------------------------------------------------------


@dataclass
class Usage:
    """Normalized across providers.

    `input_tokens` is always the *uncached* input billed at the full rate, and
    `cached_input_tokens` the part served from cache. Anthropic reports it that way
    already; Moonshot folds cached tokens into `prompt_tokens`, so we subtract.
    """

    input_tokens: int = 0
    cached_input_tokens: int = 0
    cache_write_tokens: int = 0
    output_tokens: int = 0
    reasoning_tokens: int = 0

    def __add__(self, other: "Usage") -> "Usage":
        return Usage(
            input_tokens=self.input_tokens + other.input_tokens,
            cached_input_tokens=self.cached_input_tokens + other.cached_input_tokens,
            cache_write_tokens=self.cache_write_tokens + other.cache_write_tokens,
            output_tokens=self.output_tokens + other.output_tokens,
            reasoning_tokens=self.reasoning_tokens + other.reasoning_tokens,
        )


def usage_from_openai(raw: Any) -> Usage:
    """Normalize a Moonshot / OpenAI-dialect `usage` object."""
    d = raw if isinstance(raw, dict) else (raw.model_dump() if raw else {})
    prompt = int(d.get("prompt_tokens") or 0)
    cached = int(
        (d.get("prompt_tokens_details") or {}).get("cached_tokens")
        or d.get("cached_tokens")
        or 0
    )
    reasoning = int(
        (d.get("completion_tokens_details") or {}).get("reasoning_tokens") or 0
    )
    return Usage(
        # Moonshot counts cached tokens inside prompt_tokens; split them out.
        input_tokens=max(prompt - cached, 0),
        cached_input_tokens=cached,
        output_tokens=int(d.get("completion_tokens") or 0),
        reasoning_tokens=reasoning,
    )


def usage_from_anthropic(raw: Any) -> Usage:
    """Normalize an Anthropic `usage` object."""
    d = raw if isinstance(raw, dict) else (raw.model_dump() if raw else {})
    return Usage(
        input_tokens=int(d.get("input_tokens") or 0),
        cached_input_tokens=int(d.get("cache_read_input_tokens") or 0),
        cache_write_tokens=int(d.get("cache_creation_input_tokens") or 0),
        output_tokens=int(d.get("output_tokens") or 0),
        # Anthropic bills thinking inside output_tokens and does not break it out.
        reasoning_tokens=0,
    )


#: USD per 1M tokens. Verify against the provider's own pricing page before quoting
#: these numbers anywhere that matters — they are the input to every cost figure the
#: report prints, and nothing here re-checks them.
#:
#:   kimi-k3        https://platform.kimi.ai/docs/pricing/chat  ($3.00 / $0.30 / $15.00)
#:   claude-opus-5  cache read = 0.1x input, 5-minute cache write = 1.25x input
PRICING: dict[str, dict[str, float]] = {
    "kimi-k3": {"input": 3.00, "cached_input": 0.30, "output": 15.00},
    "kimi-k2.7-code": {"input": 1.15, "cached_input": 0.15, "output": 8.00},
    "kimi-k2.6": {"input": 0.60, "cached_input": 0.15, "output": 2.50},
    "claude-opus-5": {
        "input": 5.00,
        "cached_input": 0.50,
        "cache_write": 6.25,
        "output": 25.00,
    },
    "claude-opus-4-8": {
        "input": 5.00,
        "cached_input": 0.50,
        "cache_write": 6.25,
        "output": 25.00,
    },
    "claude-sonnet-5": {
        "input": 3.00,
        "cached_input": 0.30,
        "cache_write": 3.75,
        "output": 15.00,
    },
    "claude-haiku-4-5": {
        "input": 1.00,
        "cached_input": 0.10,
        "cache_write": 1.25,
        "output": 5.00,
    },
}


def cost_usd(model: str, usage: Usage) -> float | None:
    """Dollar cost of a run, or None when we have no verified rate card for `model`."""
    rates = PRICING.get(model)
    if not rates:
        return None
    per_m = 1_000_000.0
    total = usage.input_tokens / per_m * rates["input"]
    total += usage.cached_input_tokens / per_m * rates.get("cached_input", 0.0)
    total += usage.cache_write_tokens / per_m * rates.get("cache_write", 0.0)
    total += usage.output_tokens / per_m * rates["output"]
    return total


# --------------------------------------------------------------------------------------
# Records
# --------------------------------------------------------------------------------------


@dataclass
class Action:
    """One requested code execution, in whichever shape the arm delivers it."""

    lang: str
    code: str
    #: Exactly the characters the model emitted to request this action — the escaped
    #: JSON argument blob in the tools arm, the whole `<execute>` span in the other.
    wire: str
    #: Provider handle needed to answer it (tool_use id, or the block index).
    ref: str | None = None


@dataclass
class Outcome:
    action: Action
    exit_code: int
    output: str
    duration_s: float
    truncated: bool


@dataclass
class Step:
    """What one model turn produced."""

    text: str
    actions: list[Action]
    usage: Usage
    raw_usage: dict[str, Any]
    stop_reason: str
    latency_s: float
    reasoning_chars: int = 0


@dataclass
class ActionRecord:
    round: int
    index: int
    lang: str
    payload_chars: int
    wire_chars: int
    backslashes: int
    escaped_newlines: int
    exit_code: int
    duration_s: float
    truncated: bool


@dataclass
class RoundRecord:
    index: int
    latency_s: float
    n_actions: int
    text_chars: int
    reasoning_chars: int
    stop_reason: str
    usage: dict[str, int]
    raw_usage: dict[str, Any]


@dataclass
class RunRecord:
    run_id: str
    provider: str
    model: str
    arm: str
    task: str
    repeat: int
    verified: bool
    verify_detail: str
    rounds: list[RoundRecord] = field(default_factory=list)
    actions: list[ActionRecord] = field(default_factory=list)
    totals: dict[str, Any] = field(default_factory=dict)
    final_text: str = ""
    error: str | None = None

    def to_json(self) -> str:
        return json.dumps(asdict(self), ensure_ascii=False)


class Driver(Protocol):
    """What a provider+arm pair must implement for `run_episode` to drive it."""

    provider: str
    model: str
    arm: str

    def begin(self, system_prompt: str, user_prompt: str) -> None: ...

    def step(self) -> Step: ...

    def feed(self, outcomes: list[Outcome]) -> None: ...

    def feed_text(self, text: str) -> None: ...


#: Sent, identically in both arms, when a turn neither acts nor declares completion.
#: kimi-k3 in particular will sometimes plan an action in its reasoning and then emit
#: empty content; without this a single stalled turn ends the episode.
STALL_NUDGE = (
    "Your last reply neither ran anything nor reported completion, so nothing happened. "
    "Either issue the actions you need now, or reply starting with `DONE:`."
)


def is_done(text: str) -> bool:
    return "DONE:" in text.upper()


def run_episode(
    *,
    driver: Driver,
    task,
    sandbox: Sandbox,
    max_rounds: int,
    repeat: int,
    max_nudges: int = 2,
    verbose: bool = False,
) -> RunRecord:
    """Drive one (driver, task) episode to completion and score the result."""
    record = RunRecord(
        run_id=uuid.uuid4().hex[:12],
        provider=driver.provider,
        model=driver.model,
        arm=driver.arm,
        task=task.name,
        repeat=repeat,
        verified=False,
        verify_detail="not reached",
    )

    started = time.monotonic()
    driver.begin(task.system_prompt(driver.arm), task.user_prompt)
    nudges = 0

    try:
        for round_index in range(1, max_rounds + 1):
            step = driver.step()
            record.rounds.append(
                RoundRecord(
                    index=round_index,
                    latency_s=round(step.latency_s, 3),
                    n_actions=len(step.actions),
                    text_chars=len(step.text),
                    reasoning_chars=step.reasoning_chars,
                    stop_reason=step.stop_reason,
                    usage=asdict(step.usage),
                    raw_usage=step.raw_usage,
                )
            )
            if verbose:
                print(
                    f"  round {round_index}: {len(step.actions)} action(s), "
                    f"in={step.usage.input_tokens} out={step.usage.output_tokens} "
                    f"stop={step.stop_reason}",
                    flush=True,
                )

            if not step.actions:
                # A turn that acts on nothing and claims nothing is a stall, not an
                # answer. Nudge once or twice before giving up — symmetrically in both
                # arms, and counted, so the cost of stalling stays visible.
                if not is_done(step.text) and nudges < max_nudges:
                    nudges += 1
                    if verbose:
                        print(f"  nudge {nudges}: empty turn", flush=True)
                    driver.feed_text(STALL_NUDGE)
                    continue
                record.final_text = step.text
                if not is_done(step.text):
                    record.error = "stalled: no action and no DONE"
                break

            outcomes: list[Outcome] = []
            for i, action in enumerate(step.actions, start=1):
                exit_code, output, duration, truncated = sandbox.run(
                    action.lang, action.code
                )
                outcomes.append(
                    Outcome(action, exit_code, output, duration, truncated)
                )
                record.actions.append(
                    ActionRecord(
                        round=round_index,
                        index=i,
                        lang=action.lang,
                        payload_chars=len(action.code),
                        wire_chars=len(action.wire),
                        backslashes=action.wire.count("\\"),
                        escaped_newlines=action.wire.count("\\n"),
                        exit_code=exit_code,
                        duration_s=round(duration, 3),
                        truncated=truncated,
                    )
                )
            driver.feed(outcomes)
        else:
            record.error = f"hit max_rounds={max_rounds} without finishing"
    except Exception as exc:  # noqa: BLE001 - a failed episode is a datum, not a crash
        record.error = f"{type(exc).__name__}: {exc}"

    ok, detail = task.verify(sandbox.workdir)
    record.verified = ok
    record.verify_detail = detail

    total_usage = Usage()
    for r in record.rounds:
        total_usage = total_usage + Usage(**r.usage)
    payload = sum(a.payload_chars for a in record.actions)
    wire = sum(a.wire_chars for a in record.actions)
    record.totals = {
        "rounds": len(record.rounds),
        "nudges": nudges,
        "actions": len(record.actions),
        "actions_per_round": round(len(record.actions) / max(len(record.rounds), 1), 3),
        "max_actions_in_one_round": max(
            (r.n_actions for r in record.rounds), default=0
        ),
        "failed_actions": sum(1 for a in record.actions if a.exit_code != 0),
        "usage": asdict(total_usage),
        "cost_usd": cost_usd(driver.model, total_usage),
        "wall_s": round(time.monotonic() - started, 3),
        "api_latency_s": round(sum(r.latency_s for r in record.rounds), 3),
        "payload_chars": payload,
        "wire_chars": wire,
        # >1 means the delivery format cost extra characters on top of the code itself.
        "wire_overhead_ratio": round(wire / payload, 4) if payload else None,
        "backslashes": sum(a.backslashes for a in record.actions),
        "escaped_newlines": sum(a.escaped_newlines for a in record.actions),
        "reasoning_chars": sum(r.reasoning_chars for r in record.rounds),
    }
    return record


# --------------------------------------------------------------------------------------
# Runner plumbing shared by the four scripts
# --------------------------------------------------------------------------------------


def resolve_key(env_names: Iterable[str], secret_name: str | None = None) -> str:
    """Environment first, then this machine's `adi-mono` secret store."""
    for name in env_names:
        value = os.environ.get(name)
        if value:
            return value
    if secret_name and shutil.which("adi-mono"):
        proc = subprocess.run(
            ["adi-mono", "secrets", "read", secret_name],
            capture_output=True,
            text=True,
            timeout=20,
        )
        if proc.returncode == 0 and proc.stdout.strip():
            return proc.stdout.strip()
    raise SystemExit(
        f"no credential found: set one of {', '.join(env_names)}"
        + (f" or `adi-mono secrets set {secret_name}`" if secret_name else "")
    )


def add_common_args(parser) -> None:
    parser.add_argument("--task", action="append", help="run only these tasks (repeatable)")
    parser.add_argument("--repeat", type=int, default=1, help="trials per task")
    parser.add_argument("--max-rounds", type=int, default=12)
    parser.add_argument("--max-tokens", type=int, default=16000)
    parser.add_argument(
        "--out",
        default=str(Path(__file__).parent / "results.jsonl"),
        help="JSONL file results are appended to",
    )
    parser.add_argument(
        "--runs-dir",
        default=str(Path(__file__).parent / "runs"),
        help="where per-episode sandboxes are created",
    )
    parser.add_argument(
        "--no-prime",
        dest="prime",
        action="store_false",
        help="skip the one-shot channel primer (kimi-k3 stalls without it)",
    )
    parser.add_argument("-v", "--verbose", action="store_true")


def drive(args, tasks_module, make_driver) -> None:
    """Run the selected tasks against one driver factory and append results."""
    selected = tasks_module.select(args.task)
    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    runs_dir = Path(args.runs_dir)

    for task in selected:
        for repeat in range(1, args.repeat + 1):
            driver = make_driver()
            slug = f"{driver.provider}-{driver.arm}-{task.name}-r{repeat}-{uuid.uuid4().hex[:6]}"
            sandbox = Sandbox(runs_dir / slug)
            task.setup(sandbox.workdir)
            print(f"[{driver.provider}/{driver.arm}] {task.name} #{repeat}", flush=True)

            record = run_episode(
                driver=driver,
                task=task,
                sandbox=sandbox,
                max_rounds=args.max_rounds,
                repeat=repeat,
                verbose=args.verbose,
            )
            with out_path.open("a", encoding="utf-8") as fh:
                fh.write(record.to_json() + "\n")

            t = record.totals
            cost = t.get("cost_usd")
            print(
                f"  -> {'ok ' if record.verified else 'FAIL'} "
                f"rounds={t['rounds']} actions={t['actions']} "
                f"in={t['usage']['input_tokens']} cached={t['usage']['cached_input_tokens']} "
                f"out={t['usage']['output_tokens']} "
                f"cost={'$%.4f' % cost if cost is not None else 'n/a'} "
                f"overhead={t['wire_overhead_ratio']}"
                + (f"  [{record.error}]" if record.error else ""),
                flush=True,
            )
            if not record.verified:
                print(f"     verify: {record.verify_detail}", flush=True)

    print(f"\nappended to {out_path}\nreport with:  uv run report.py --in {out_path}")
