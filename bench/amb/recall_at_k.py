#!/usr/bin/env python3
"""Gold-document recall from a retrieve() trace: a retrieval metric with no LLM in the loop.

The trace is written by `run_amb.py` when ICM_AMB_TRACE is set (any provider).
For each traced query that has gold document ids in the dataset, report at each
cut-off k (over the returned memories, in rank order):

  any@k   share of queries with at least one gold document in the top k
  all@k   share of queries with every gold document in the top k
  frac@k  mean fraction of gold documents found in the top k

    python recall_at_k.py --dataset locomo --split locomo10 --trace out/trace-icm.jsonl
    python recall_at_k.py --dataset locomo --split locomo10 --trace out/RUN/shard-*/trace-*.jsonl

A trace is append-only and survives a pod restart, so a question replayed after a
restart (the whole unit in progress) or after a harness retry (HTTP 429/5xx) is in
it more than once. Only the last line of each question is aggregated, and the number
of lines dropped is printed. "Question" means (user_id, retrieval query) as often as
the split holds it: LoCoMo asks 11 questions twice in the same conversation, and
those 22 lines are all kept.
"""

from __future__ import annotations

import argparse
import json
from collections import Counter, defaultdict
from pathlib import Path

from run_amb import bootstrap


def load_gold(dataset: str, split: str) -> tuple[dict[tuple[str | None, str], list[str]], Counter]:
    """Gold ids per (user_id, query text), and how many questions of the split are traced
    under each (user_id, retrieval query)."""
    from memory_bench.dataset import get_dataset

    gold: dict[tuple[str | None, str], list[str]] = {}
    expected: Counter = Counter()
    for q in get_dataset(dataset).load_queries(split):
        traced = q.meta.get("retrieval_query") or q.query
        expected[(q.user_id, traced)] += 1
        for text in {q.query, traced}:
            gold[(q.user_id, text)] = q.gold_ids
    return gold, expected


def gold_index(dataset: str, split: str) -> dict[tuple[str | None, str], list[str]]:
    """(user_id, query text) -> gold document ids, for every query of the split."""
    return load_gold(dataset, split)[0]


def dedup_trace(rows: list[dict], expected: Counter) -> tuple[list[dict], int, int]:
    """Keep the last line(s) of each question; later lines are the replays.

    Returns (kept rows in trace order, lines dropped, questions of the split with
    no line at all). A key unknown to the split counts as one question."""
    seen: Counter = Counter()
    keep = [False] * len(rows)
    for i in range(len(rows) - 1, -1, -1):
        key = (rows[i]["user_id"], rows[i]["query"])
        if seen[key] < (expected.get(key) or 1):
            keep[i] = True
        seen[key] += 1
    kept = [row for row, k in zip(rows, keep) if k]
    missing = sum(max(0, n - seen[key]) for key, n in expected.items())
    return kept, len(rows) - len(kept), missing


def recall_metrics(rows: list[dict], gold: dict, ks: list[int]) -> dict:
    """Aggregate retrieve() rows (`user_id`, `query`, `doc_ids`, `ms`) against gold ids."""
    scored = [(r, gold[(r["user_id"], r["query"])]) for r in rows if gold.get((r["user_id"], r["query"]))]
    latencies = sorted(r["ms"] for r in rows)
    entry = {
        "traced_queries": len(rows),
        "queries_with_gold": len(scored),
        "avg_returned": round(sum(len(r["doc_ids"]) for r in rows) / len(rows), 1) if rows else 0,
        "avg_retrieve_ms": round(sum(latencies) / len(rows), 1) if rows else 0,
        "median_retrieve_ms": latencies[len(latencies) // 2] if rows else 0,
    }
    for k in ks:
        any_hit = all_hit = frac = 0.0
        for row, gold_ids in scored:
            top = set(row["doc_ids"][:k])
            found = sum(1 for g in gold_ids if g in top)
            any_hit += found > 0
            all_hit += found == len(gold_ids)
            frac += found / len(gold_ids)
        n = len(scored) or 1
        entry[f"any@{k}"] = round(any_hit / n, 4)
        entry[f"all@{k}"] = round(all_hit / n, 4)
        entry[f"frac@{k}"] = round(frac / n, 4)
    return entry


def read_trace(*paths: Path) -> dict[str, list[dict]]:
    """Trace rows per provider, files in the order given, lines in file order."""
    per_provider: dict[str, list[dict]] = defaultdict(list)
    for path in paths:
        for line in path.read_text().splitlines():
            if line.strip():
                row = json.loads(line)
                per_provider[row["provider"]].append(row)
    return per_provider


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--dataset", required=True)
    ap.add_argument("--split", required=True)
    ap.add_argument("--trace", required=True, type=Path, nargs="+",
                    help="one trace file, or the trace of every shard of a run")
    ap.add_argument("--k", type=int, nargs="+", default=[5, 10, 20, 50])
    ap.add_argument("--json", action="store_true", help="print machine-readable JSON")
    args = ap.parse_args()

    bootstrap()
    gold, expected = load_gold(args.dataset, args.split)
    report = {}
    for provider, rows in read_trace(*args.trace).items():
        kept, dropped, missing = dedup_trace(rows, expected)
        report[provider] = dict(recall_metrics(kept, gold, args.k), trace_lines=len(rows),
                                duplicates_dropped=dropped, questions_without_trace=missing)

    if args.json:
        print(json.dumps(report, indent=2))
        return
    for provider, entry in report.items():
        print(f"{provider}: {entry['trace_lines']} trace lines, {entry['duplicates_dropped']} duplicates dropped "
              f"(questions replayed after a restart or a retry), {entry['questions_without_trace']} of the "
              f"{sum(expected.values())} questions of the split have no line")
        print(f"{provider}: {entry['queries_with_gold']}/{entry['traced_queries']} queries with gold ids, "
              f"avg returned {entry['avg_returned']}, avg retrieve {entry['avg_retrieve_ms']} ms "
              f"(median {entry['median_retrieve_ms']} ms)")
        for k in args.k:
            print(f"  k={k:<3} any={entry[f'any@{k}']:.1%}  all={entry[f'all@{k}']:.1%}  frac={entry[f'frac@{k}']:.1%}")


if __name__ == "__main__":
    main()
