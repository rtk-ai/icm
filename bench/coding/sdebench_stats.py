#!/usr/bin/env python3
"""Read sdebench result files and report the measures that separate the arms.

The solve rate does not: with five corrections allowed, every published run
solves 60 or 61 of the 61 tasks, with or without memory. What differs is how
much help the agent needed. This script reports, per run:

    solved        tasks green at the end (out of n)
    first_try     tasks solved with zero correction
    corrections   total test-feedback rounds, and the mean per task
    capped        tasks that used all corrections and still failed
    injected      tasks whose result carries the memory text the agent received
                  (reference arm: the plugin's own record). Not counted for the
                  ICM arm, whose rows carry a replay made before the agent started
    cost, wall    agent cost in USD and agent wall time in seconds, as recorded
                  by the harness (the memory system's own LLM cost is not in it)

and the same by `source` (history / conversation / conversation-amended), the
dataset's main axis. With --pair A B it compares two runs task by task: exact
sign test on the corrections, exact McNemar test on first-try.

For a run of the ICM arm (bench/coding/icm_coding.py) it adds what the adapter
measured on each task, out of the rows that carry the measure:

    hooks         tasks where the agent's own UserPromptSubmit reached ICM
                  (ICM's telemetry). A dry run starts no agent: 0.
    in seed       tasks whose seeded database holds at least one note taken from
                  the task's own documents (the conversation or commit that
                  carries the decision). Elsewhere memory cannot help.
    by position   tasks where the replayed SessionStart pack carries such a note.
                  That pack selects by import order, not by search: must be 0.
    by search     tasks where the replayed UserPromptSubmit hook carries such a
                  note for the first prompt.

    python sdebench_stats.py out/sdebench/*/coding/boltons.json
    python sdebench_stats.py --pair vanilla.json icm.json
    python sdebench_stats.py --json runs/*.json.gz
    python sdebench_stats.py --gate out/sdebench/icm-dry/coding/boltons.json

A run split over several processes (SDE_TASK_FILTER, one --name each) is given
as one comma-separated list: `a/coding/boltons.json,b/coding/boltons.json`. A
task present in two parts is an error.

--gate reads a dry run of the ICM arm and exits 1 unless a paid run can measure
retrieval: no task delivered by position, the decision in the seed for at least
--min-seed of the conversation tasks, found by search for at least --min-search
of them (defaults 0.9 and 0.5; conversation and conversation-amended together).

Input: the files the AMB runner writes (`outputs/sdebench/<run>/coding/boltons.json`,
plain or .gz). No model, no network.
"""

from __future__ import annotations

import argparse
import gzip
import json
import re
import sys
from math import comb
from pathlib import Path

SOURCES = ("history", "conversation", "conversation-amended")
CONVERSATION = ("conversation", "conversation-amended")
_FIELD = re.compile(r"(\w+)=(\S+)")
ICM_MEASURES = ("hooks", "in_seed", "by_position", "by_search")


def _load_one(path: str) -> dict:
    p = Path(path)
    opener = gzip.open if p.suffix == ".gz" else open
    with opener(p, "rt") as fh:
        return json.load(fh)


def load(spec: str) -> dict:
    """One result file, or the comma-separated parts of one run split by task."""
    parts = [_load_one(path) for path in spec.split(",") if path]
    if not parts:
        raise SystemExit(f"no result file in {spec!r}")
    data = dict(parts[0])
    rows, seen = [], set()
    for part in parts:
        for row in part.get("results") or []:
            if row["query_id"] in seen:
                raise SystemExit(f"{spec}: task {row['query_id']} is in two parts of the same run")
            seen.add(row["query_id"])
            rows.append(row)
    data["results"] = rows
    if len(parts) > 1:
        data["run_name"] = "+".join(str(part.get("run_name") or "?") for part in parts)
    return data


def _value(text: str):
    if text in ("True", "False", "None"):
        return {"True": True, "False": False, "None": None}[text]
    try:
        return int(text)
    except ValueError:
        return text


def icm_fields(reasoning: str | None) -> dict | None:
    """The `key=value` measures the ICM arm writes in a row's `reasoning`."""
    fields = {key: _value(value) for key, value in _FIELD.findall(reasoning or "")}
    return fields if fields.get("arm") == "icm" else None


def run_label(path: str, data: dict) -> str:
    return data.get("run_name") or Path(path.split(",")[0]).parent.parent.name


def task_rows(data: dict) -> dict[str, dict]:
    rows = {}
    for r in data.get("results") or []:
        meta = r.get("meta") or {}
        corrections = meta.get("interventions")
        icm = icm_fields(r.get("reasoning"))
        rows[r["query_id"]] = {
            "source": meta.get("source"),
            "solved": bool(meta.get("solved")),
            # A row without the harness' metrics (no result.json) is not a zero.
            "corrections": corrections,
            "first_try": bool(meta.get("solved")) and corrections == 0,
            "capped": bool(meta.get("capped")),
            "cost": meta.get("cost_usd"),
            "wall": meta.get("wall_s"),
            "injected": icm is None and (r.get("context") or "").startswith("## Memory"),
            "icm": icm,
        }
    return rows


def icm_block(items: list[dict]) -> dict | None:
    """The ICM arm's own measures over some rows; None when no row is an ICM row.

    Each measure is [tasks where it holds, tasks where it was measured]."""
    rows = [v["icm"] for v in items if v.get("icm") is not None]
    if not rows:
        return None

    def count(key: str, holds) -> list[int]:
        known = [r[key] for r in rows if r.get(key) is not None]
        return [sum(1 for value in known if holds(value)), len(known)]

    return {
        "rows": len(rows),
        "dry_run": sum(1 for r in rows if r.get("dry_run") is True),
        "hooks": count("hook_fired", lambda v: v is True),
        "in_seed": count("seed_task_facts", lambda v: v > 0),
        "by_position": count("start_task_lines", lambda v: v > 0),
        "by_search": count("prompt_task_lines", lambda v: v > 0),
        "stored_during_task": sum(r.get("stored_during_task") or 0 for r in rows),
    }


def summarize(rows: dict[str, dict]) -> dict:
    values = list(rows.values())
    known = [v for v in values if v["corrections"] is not None]

    def block(items: list[dict]) -> dict:
        measured = [v for v in items if v["corrections"] is not None]
        total = sum(v["corrections"] for v in measured)
        return {
            "n": len(items),
            "solved": sum(v["solved"] for v in items),
            "first_try": sum(v["first_try"] for v in items),
            "corrections": total,
            "corrections_mean": round(total / len(measured), 3) if measured else None,
            "capped": sum(v["capped"] for v in items),
            "injected": sum(v["injected"] for v in items),
            "icm": icm_block(items),
        }

    out = block(values)
    out["missing_metrics"] = len(values) - len(known)
    out["cost_usd"] = round(sum(v["cost"] or 0 for v in values), 2)
    out["wall_s"] = round(sum(v["wall"] or 0 for v in values))
    out["by_source"] = {s: block([v for v in values if v["source"] == s]) for s in SOURCES
                        if any(v["source"] == s for v in values)}
    return out


def sign_test(wins: int, losses: int) -> float:
    """Two-sided exact binomial test of wins vs losses (ties dropped)."""
    n = wins + losses
    if n == 0:
        return 1.0
    k = min(wins, losses)
    tail = sum(comb(n, i) for i in range(k + 1)) / 2 ** n
    return min(1.0, 2 * tail)


def pair(a: dict[str, dict], b: dict[str, dict]) -> dict:
    """Task-by-task comparison of run B against run A (same tasks only)."""
    common = sorted(t for t in set(a) & set(b)
                    if a[t]["corrections"] is not None and b[t]["corrections"] is not None)

    def compare(tasks: list[str]) -> dict:
        fewer = sum(b[t]["corrections"] < a[t]["corrections"] for t in tasks)
        more = sum(b[t]["corrections"] > a[t]["corrections"] for t in tasks)
        gained = sum(b[t]["first_try"] and not a[t]["first_try"] for t in tasks)
        lost = sum(a[t]["first_try"] and not b[t]["first_try"] for t in tasks)
        return {
            "n": len(tasks),
            "corrections_a": sum(a[t]["corrections"] for t in tasks),
            "corrections_b": sum(b[t]["corrections"] for t in tasks),
            "b_fewer": fewer, "b_more": more, "tied": len(tasks) - fewer - more,
            "p_sign": sign_test(fewer, more),
            "first_try_a": sum(a[t]["first_try"] for t in tasks),
            "first_try_b": sum(b[t]["first_try"] for t in tasks),
            "first_try_gained": gained, "first_try_lost": lost,
            "p_mcnemar": sign_test(gained, lost),
        }

    out = compare(common)
    out["only_in_a"] = len(set(a) - set(b))
    out["only_in_b"] = len(set(b) - set(a))
    out["by_source"] = {s: compare([t for t in common if a[t]["source"] == s]) for s in SOURCES
                        if any(a[t]["source"] == s for t in common)}
    return out


def gate(s: dict, min_seed: float, min_search: float) -> list[str]:
    """Reasons a paid run of the ICM arm would not measure retrieval (empty = go)."""
    whole = s.get("icm")
    if whole is None:
        return ["not a run of the ICM arm"]
    reasons = []
    if whole["by_position"][1] < s["n"]:
        reasons.append(f"SessionStart was replayed on {whole['by_position'][1]} of {s['n']} tasks")
    if whole["by_position"][0]:
        reasons.append(f"{whole['by_position'][0]} task(s) delivered by position at SessionStart")
    parts = [b["icm"] for source, b in s["by_source"].items() if source in CONVERSATION and b["icm"]]
    n = sum(b["n"] for source, b in s["by_source"].items() if source in CONVERSATION)
    if not n:
        return reasons + ["no conversation task in this run"]
    for key, floor, what in (("in_seed", min_seed, "the decision is in the seed"),
                             ("by_search", min_search, "the first prompt retrieves it")):
        have = sum(p[key][0] for p in parts)
        if have < floor * n:
            reasons.append(f"{what} on {have} of {n} conversation tasks, below {floor:.0%}")
    return reasons


def _icm_text(b: dict) -> str:
    def frac(key: str) -> str:
        return f"{b[key][0]}/{b[key][1]}"
    return (f"hooks {frac('hooks')}, in seed {frac('in_seed')}, by position {frac('by_position')}, "
            f"by search {frac('by_search')}")


def print_summary(label: str, llm: str | None, s: dict) -> None:
    icm = s.get("icm")
    shown = f"injected {s['injected']}" if icm is None else _icm_text(icm)
    print(f"{label} [{llm or '?'}]: solved {s['solved']}/{s['n']}, first try {s['first_try']}, "
          f"corrections {s['corrections']} (mean {s['corrections_mean']}), capped {s['capped']}, "
          f"{shown}, cost ${s['cost_usd']}, wall {s['wall_s']} s"
          + (f", {s['missing_metrics']} rows WITHOUT metrics" if s["missing_metrics"] else ""))
    if icm is not None:
        if icm["dry_run"]:
            print(f"    DRY RUN on {icm['dry_run']} of {icm['rows']} rows: no agent ran, nothing here is a score")
        elif icm["hooks"][0] < icm["rows"]:
            print(f"    {icm['rows'] - icm['hooks'][0]} row(s) WITHOUT ICM's hooks: not memory runs")
        if icm["stored_during_task"]:
            print(f"    {icm['stored_during_task']} notes written to memory during the tasks")
    for source, b in s["by_source"].items():
        shown = f"injected {b['injected']}" if b["icm"] is None else _icm_text(b["icm"])
        print(f"    {source:<21} n={b['n']:<3} first try {b['first_try']:<3} "
              f"corrections {b['corrections']:<3} {shown}")


def print_pair(label_a: str, label_b: str, p: dict) -> None:
    def line(name: str, c: dict) -> None:
        print(f"  {name:<21} n={c['n']:<3} corrections {c['corrections_a']} -> {c['corrections_b']} "
              f"(B fewer on {c['b_fewer']}, more on {c['b_more']}, tied {c['tied']}, sign test p={c['p_sign']:.2g}); "
              f"first try {c['first_try_a']} -> {c['first_try_b']} "
              f"(+{c['first_try_gained']} / -{c['first_try_lost']}, McNemar p={c['p_mcnemar']:.2g})")

    print(f"A = {label_a}, B = {label_b}")
    line("all", p)
    for source, c in p["by_source"].items():
        line(source, c)
    if p["only_in_a"] or p["only_in_b"]:
        print(f"  tasks not shared: {p['only_in_a']} only in A, {p['only_in_b']} only in B")


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("files", nargs="*", help="result files to summarize")
    ap.add_argument("--pair", nargs=2, metavar=("A", "B"), help="compare run B against run A task by task")
    ap.add_argument("--json", action="store_true", help="machine-readable output")
    ap.add_argument("--gate", action="store_true",
                    help="exit 1 unless every given file is an ICM dry run a paid run can follow")
    ap.add_argument("--min-seed", type=float, default=0.9, metavar="F",
                    help="--gate: share of conversation tasks whose decision must be in the seed")
    ap.add_argument("--min-search", type=float, default=0.5, metavar="F",
                    help="--gate: share of conversation tasks the first prompt must retrieve it for")
    args = ap.parse_args(argv)
    if not args.files and not args.pair:
        ap.error("give result files, or --pair A B")

    out: dict = {"runs": {}, "pair": None}
    refused = False
    for path in args.files:
        data = load(path)
        label = run_label(path, data)
        summary = summarize(task_rows(data))
        out["runs"][label] = {"file": path, "llm": data.get("answer_llm"),
                              "memory": data.get("memory_provider"), **summary}
        if not args.json:
            print_summary(label, data.get("answer_llm"), summary)
        if args.gate:
            reasons = gate(summary, args.min_seed, args.min_search)
            out["runs"][label]["gate"] = reasons
            refused = refused or bool(reasons)
            if not args.json:
                print(f"    gate: {'NO GO: ' + '; '.join(reasons) if reasons else 'go'}")
    if args.pair:
        data_a, data_b = load(args.pair[0]), load(args.pair[1])
        result = pair(task_rows(data_a), task_rows(data_b))
        out["pair"] = {"a": run_label(args.pair[0], data_a), "b": run_label(args.pair[1], data_b), **result}
        if not args.json:
            print_pair(out["pair"]["a"], out["pair"]["b"], result)
    if args.json:
        json.dump(out, sys.stdout, indent=2)
        print()
    return 1 if refused else 0


if __name__ == "__main__":
    raise SystemExit(main())
