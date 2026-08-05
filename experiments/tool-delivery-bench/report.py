#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# ///
"""Aggregate results.jsonl into an arm-vs-arm comparison.

    uv run report.py
    uv run report.py --in results.jsonl --csv summary.csv

Rows are one (provider, model, task, arm) cell averaged over its repeats; the deltas
section then reads `execute` against `tools` for each (provider, model, task).
"""

from __future__ import annotations

import argparse
import csv
import json
import statistics
from collections import defaultdict
from pathlib import Path

METRICS = [
    ("rounds", "rounds", "{:.2f}"),
    ("nudges", "nudge", "{:.2f}"),
    ("actions", "actions", "{:.2f}"),
    # act/rnd counts every round, including the 0-action stall rounds a nudge produces,
    # so it understates an arm that stalls. act/act-rnd divides by acting rounds only.
    ("actions_per_round", "act/rnd", "{:.2f}"),
    ("actions_per_acting_round", "act/act-rnd", "{:.2f}"),
    ("max_actions_in_one_round", "max/rnd", "{:.2f}"),
    ("input_tokens", "in", "{:.0f}"),
    ("cached_input_tokens", "cached", "{:.0f}"),
    ("output_tokens", "out", "{:.0f}"),
    ("reasoning_tokens", "reason", "{:.0f}"),
    ("cost_usd", "cost $", "{:.4f}"),
    ("api_latency_s", "api s", "{:.1f}"),
    ("payload_chars", "code ch", "{:.0f}"),
    ("wire_chars", "wire ch", "{:.0f}"),
    ("wire_overhead_ratio", "wire/code", "{:.3f}"),
    ("backslashes", "backsl", "{:.0f}"),
]

#: Lower is better for all of these, so a negative delta is a win for `execute`.
DELTA_METRICS = [
    ("actions_per_acting_round", "actions/acting rnd"),
    ("nudges", "stall nudges"),
    ("backslashes", "backslashes"),
    ("output_tokens", "output tokens"),
    ("input_tokens", "input tokens"),
    ("cost_usd", "cost"),
    ("rounds", "rounds"),
    ("wire_chars", "wire chars"),
    ("api_latency_s", "api latency"),
]


def flatten(record: dict) -> dict:
    """One record -> the flat metric namespace the report works in."""
    totals = record.get("totals") or {}
    usage = totals.get("usage") or {}
    flat = {k: v for k, v in totals.items() if not isinstance(v, dict)}
    flat.update(usage)
    flat["verified"] = 1.0 if record.get("verified") else 0.0
    flat["errored"] = 1.0 if record.get("error") else 0.0

    # Parallelism, measured only over rounds where the model actually asked for
    # something. A stalled turn is a failure of the channel, not a choice about batching,
    # and folding it in makes a stall-prone arm look like a serial one.
    rounds = record.get("rounds") or []
    acting = [r for r in rounds if (r.get("n_actions") or 0) > 0]
    flat["actions_per_acting_round"] = (
        sum(r["n_actions"] for r in acting) / len(acting) if acting else None
    )
    return flat


def mean_of(values: list[float | None]) -> float | None:
    real = [v for v in values if isinstance(v, (int, float))]
    return statistics.fmean(real) if real else None


def load(path: Path) -> list[dict]:
    if not path.exists():
        raise SystemExit(f"no results at {path} — run one of the run_*.py scripts first")
    out = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.strip():
            out.append(json.loads(line))
    return out


def fmt(value: float | None, spec: str) -> str:
    return "-" if value is None else spec.format(value)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--in",
        dest="infile",
        default=str(Path(__file__).parent / "results.jsonl"),
    )
    parser.add_argument("--csv", help="also write the per-cell table here")
    parser.add_argument(
        "--only-verified",
        action="store_true",
        help="drop failed episodes; a failed run ends early and looks artificially cheap",
    )
    args = parser.parse_args()

    records = load(Path(args.infile))
    if args.only_verified:
        kept = [r for r in records if r.get("verified")]
        print(f"(only-verified: kept {len(kept)} of {len(records)} episodes)")
        records = kept

    cells: dict[tuple, list[dict]] = defaultdict(list)
    for rec in records:
        cells[(rec["provider"], rec["model"], rec["task"], rec["arm"])].append(
            flatten(rec)
        )

    agg: dict[tuple, dict] = {}
    for key, flats in cells.items():
        row = {"n": len(flats)}
        row["ok"] = statistics.fmean(f["verified"] for f in flats)
        for metric, _, _ in METRICS:
            row[metric] = mean_of([f.get(metric) for f in flats])
        agg[key] = row

    # ---- per-cell table -------------------------------------------------------------
    headers = ["provider", "model", "task", "arm", "n", "ok"] + [h for _, h, _ in METRICS]
    table = []
    for (provider, model, task, arm) in sorted(agg):
        row = agg[(provider, model, task, arm)]
        table.append(
            [provider, model, task, arm, str(row["n"]), f"{row['ok']:.0%}"]
            + [fmt(row[m], spec) for m, _, spec in METRICS]
        )

    widths = [
        max(len(headers[i]), *(len(r[i]) for r in table)) if table else len(headers[i])
        for i in range(len(headers))
    ]

    def line(cols: list[str]) -> str:
        return "  ".join(c.rjust(widths[i]) if i >= 4 else c.ljust(widths[i])
                         for i, c in enumerate(cols))

    print("\n=== per (provider, model, task, arm), averaged over repeats ===\n")
    print(line(headers))
    print("  ".join("-" * w for w in widths))
    for row in table:
        print(line(row))

    # ---- execute vs tools -----------------------------------------------------------
    print("\n=== execute vs tools  (negative = execute is cheaper/fewer) ===\n")
    groups = sorted({(p, m, t) for (p, m, t, _) in agg})
    any_pair = False
    for provider, model, task in groups:
        tools = agg.get((provider, model, task, "tools"))
        execute = agg.get((provider, model, task, "execute"))
        if not tools or not execute:
            continue
        any_pair = True
        print(f"{provider}/{model}  {task}")
        print(
            f"    success:        tools {tools['ok']:.0%}   execute {execute['ok']:.0%}"
        )
        print(
            f"    actions/round:  tools {fmt(tools['actions_per_round'], '{:.2f}')}"
            f"   execute {fmt(execute['actions_per_round'], '{:.2f}')}"
        )
        for metric, label in DELTA_METRICS:
            a, b = tools.get(metric), execute.get(metric)
            if not isinstance(a, (int, float)) or not isinstance(b, (int, float)) or a == 0:
                continue
            pct = (b - a) / a * 100.0
            print(f"    {label:<15} {a:>10,.2f} -> {b:>10,.2f}   {pct:+6.1f}%")
        print()

    if not any_pair:
        print("(need both arms for the same provider/model/task to compare)\n")

    # ---- headline -------------------------------------------------------------------
    for provider, model in sorted({(p, m) for (p, m, _, _) in agg}):
        tools_rows = [v for k, v in agg.items() if k[:2] == (provider, model) and k[3] == "tools"]
        exec_rows = [v for k, v in agg.items() if k[:2] == (provider, model) and k[3] == "execute"]
        if not tools_rows or not exec_rows:
            continue
        t_out = mean_of([r["output_tokens"] for r in tools_rows])
        e_out = mean_of([r["output_tokens"] for r in exec_rows])
        t_cost = mean_of([r["cost_usd"] for r in tools_rows])
        e_cost = mean_of([r["cost_usd"] for r in exec_rows])
        t_ok = statistics.fmean([r["ok"] for r in tools_rows])
        e_ok = statistics.fmean([r["ok"] for r in exec_rows])
        print(f"{provider}/{model} across all tasks:")
        if t_out and e_out:
            print(f"    output tokens  {t_out:,.0f} -> {e_out:,.0f}  ({(e_out - t_out) / t_out * 100:+.1f}%)")
        if t_cost and e_cost:
            print(f"    cost           ${t_cost:.4f} -> ${e_cost:.4f}  ({(e_cost - t_cost) / t_cost * 100:+.1f}%)")
        print(f"    success        {t_ok:.0%} -> {e_ok:.0%}")
        print()

    if args.csv:
        with Path(args.csv).open("w", newline="", encoding="utf-8") as fh:
            writer = csv.writer(fh)
            writer.writerow(headers)
            writer.writerows(table)
        print(f"wrote {args.csv}")


if __name__ == "__main__":
    main()
