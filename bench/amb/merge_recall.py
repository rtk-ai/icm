#!/usr/bin/env python3
"""Check and merge the result files of a recall-only run (recall_only.py), then score it.

    python merge_recall.py out/RUN/shard-*/longmemeval/icm/recall/s.json \\
        --expect-queries 500 --expect-docs 23867 -o out/merged/longmemeval/icm/recall/s.json

A run that was not sharded (one file) goes through the same command. Refused,
exit 1, nothing written, as in merge_shards.py: a shard missing or given twice, a
question count other than --expect-queries (longmemeval/s: 500; locomo/locomo10:
1540), a question id present twice, shards that disagree on dataset, split,
provider, run name, description, indexed unit, depth, header, gold definition or
provider settings, a file that is not a recall-only result, or one that records an
LLM call. With --expect-docs (longmemeval/s: 23867; locomo/locomo10: 272) the
documents ingested must add up to that number too.

Metrics, for each k of --k (default 1 3 5 10), over the first k returned units:

  recall_any@k  share of questions with at least one expected session in the top k
  recall_all@k  share of questions with every expected session in the top k
  mrr           mean of 1 / rank of the first expected session (0 when none is returned)

"Top k" is the first k units returned, as MemPalace (`evaluate_retrieval`:
`rankings[:k]`) and agentmemory (`retrievedSessionIds.slice(0, k)`) count it: with
one unit per session that is the top k sessions. When a session is indexed as
several units (`--unit chunk`, or a session split because it was over the size
limit) the same figures are also given over the first k DISTINCT sessions, under
`distinct_sessions`; say which one is published.

Scopes: every question, the questions that are not abstention questions
(LongMemEval ids ending in `_abs`: 30 of 500; both published protocols keep them),
and each question type. A question without any expected session is left out of the
metrics and counted under `without_gold`. `short_lists@k` counts the questions
whose list is shorter than k although their corpus holds at least k units: those
are scored as returned (a miss stays a miss), the count is there to be read.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

from merge_shards import attempts, check_coverage, fail, percentile

_SAME = ("dataset", "split", "category", "mode", "memory_provider", "run_name", "description", "unit", "depth",
         "header", "chunk_tokens", "gold_definition", "provider")


def wilson(successes: int, n: int, z: float = 1.959964) -> tuple[float, float]:
    """95% Wilson score interval of a proportion."""
    if n == 0:
        return 0.0, 0.0
    p = successes / n
    centre = (p + z * z / (2 * n)) / (1 + z * z / n)
    half = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / (1 + z * z / n)
    return max(0.0, centre - half), min(1.0, centre + half)


def distinct(ids: list[str]) -> list[str]:
    return list(dict.fromkeys(ids))


def score(rows: list[dict], ks: list[int], gold_key: str = "gold_ids", dedup: bool = False) -> dict:
    """Recall figures of `rows`; a row without gold is counted apart, not scored."""
    scored = [r for r in rows if r.get(gold_key)]
    out: dict = {"questions": len(scored), "without_gold": len(rows) - len(scored)}
    n = len(scored)
    reciprocal = 0.0
    for row in scored:
        ranked = distinct(row["ranked_ids"]) if dedup else row["ranked_ids"]
        gold = set(row[gold_key])
        first = next((i for i, doc_id in enumerate(ranked) if doc_id in gold), None)
        reciprocal += 1 / (first + 1) if first is not None else 0.0
    out["mrr"] = round(reciprocal / n, 4) if n else None
    for k in ks:
        any_hit = all_hit = short = 0
        for row in scored:
            ranked = distinct(row["ranked_ids"]) if dedup else row["ranked_ids"]
            top = set(ranked[:k])
            gold = set(row[gold_key])
            any_hit += bool(gold & top)
            all_hit += gold <= top
            short += len(ranked) < k and (row.get("corpus_units") or 0) >= k
        for label, hits in (("recall_any", any_hit), ("recall_all", all_hit)):
            low, high = wilson(hits, n)
            out[f"{label}@{k}"] = round(hits / n, 4) if n else None
            out[f"{label}@{k}_count"] = hits
            out[f"{label}@{k}_ci95"] = [round(low, 4), round(high, 4)]
        out[f"short_lists@{k}"] = short
    return out


def is_abstention(row: dict) -> bool:
    return str(row["query_id"]).endswith("_abs")


def report(rows: list[dict], ks: list[int]) -> dict:
    types = sorted({r.get("question_type") or "unknown" for r in rows})
    multi_unit = any(len(distinct(r["ranked_ids"])) != len(r["ranked_ids"]) for r in rows)
    answerable = [r for r in rows if not is_abstention(r)]
    out = {
        "ks": ks,
        "all": score(rows, ks),
        "without_abstention": score(answerable, ks),
        "abstention_only": score([r for r in rows if is_abstention(r)], ks),
        "by_question_type": {t: score([r for r in rows if (r.get("question_type") or "unknown") == t], ks) for t in types},
        "distinct_sessions": {"all": score(rows, ks, dedup=True), "without_abstention": score(answerable, ks, dedup=True)},
        "a_session_takes_several_ranks": multi_unit,
    }
    if any(r.get("gold_ids_harness") != r.get("gold_ids") for r in rows):
        # Not the published definition: shown so the effect of the gold definition is known.
        out["harness_gold_definition"] = score(rows, ks, gold_key="gold_ids_harness")
    return out


def table(metrics: dict, ks: list[int]) -> str:
    def line(label: str, entry: dict) -> str:
        cells = []
        for k in ks:
            any_v, all_v = entry.get(f"recall_any@{k}"), entry.get(f"recall_all@{k}")
            cells.append(f"any@{k} {any_v:6.1%} all@{k} {all_v:6.1%}" if any_v is not None else f"any@{k}    n/a all@{k}    n/a")
        mrr = f"{entry['mrr']:.3f}" if entry.get("mrr") is not None else "n/a"
        return f"  {label:<28} n={entry['questions']:<4} " + "  ".join(cells) + f"  mrr {mrr}"

    lines = [line("all questions", metrics["all"]), line("without abstention", metrics["without_abstention"])]
    lines += [line(t, entry) for t, entry in metrics["by_question_type"].items()]
    if metrics["a_session_takes_several_ranks"]:
        lines.append(line("all, top k distinct sessions", metrics["distinct_sessions"]["all"]))
    if "harness_gold_definition" in metrics:
        lines.append(line("all, harness gold (not publ.)", metrics["harness_gold_definition"]))
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("shards", nargs="+", type=Path)
    ap.add_argument("-o", "--output", required=True, type=Path)
    ap.add_argument("--expect-queries", required=True, type=int,
                    help="questions the complete run must hold (longmemeval/s: 500, locomo/locomo10: 1540)")
    ap.add_argument("--expect-docs", type=int,
                    help="documents of the split (longmemeval/s: 23867, locomo/locomo10: 272)")
    ap.add_argument("--k", type=int, nargs="+", default=[1, 3, 5, 10])
    args = ap.parse_args(argv)

    n_shards = check_coverage(args.shards)
    parts = []
    for path in args.shards:
        try:
            part = json.loads(path.read_text())
        except (OSError, ValueError) as e:
            fail(f"{path} is not a readable result file: {e}")
        if part.get("mode") != "recall" or not isinstance(part.get("results"), list):
            fail(f"{path} is not a recall-only result file (mode={part.get('mode')!r}); answer runs go to merge_shards.py")
        if part.get("llm_calls") or part.get("answer_llm") or part.get("judge_llm"):
            fail(f"{path} records an LLM (llm_calls={part.get('llm_calls')!r}, answer_llm={part.get('answer_llm')!r}, "
                 f"judge_llm={part.get('judge_llm')!r}): not a recall-only result")
        parts.append(part)
    for key in _SAME:
        values = {json.dumps(p.get(key), sort_keys=True) for p in parts}
        if len(values) > 1:
            fail(f"shards disagree on {key}: {sorted(values)}")
    if max(args.k) > parts[0]["depth"]:
        fail(f"--k {max(args.k)} is deeper than the lists of this run (depth {parts[0]['depth']})")

    results: dict[str, dict] = {}
    units: dict[str, dict] = {}
    for path, part in zip(args.shards, parts):
        for row in part["results"]:
            if row["query_id"] in results:
                fail(f"query {row['query_id']} appears twice (second time in {path})")
            if not isinstance(row.get("ranked_ids"), list) or not isinstance(row.get("gold_ids"), list):
                fail(f"query {row['query_id']} in {path} has no ranked_ids or gold_ids list")
            results[row["query_id"]] = row
        for unit, stats in (part.get("units") or {}).items():
            if unit in units:
                fail(f"isolation unit {unit} was ingested by two shards (second time in {path})")
            units[unit] = stats
    rows = list(results.values())
    if len(rows) != args.expect_queries:
        sizes = ", ".join(str(len(p["results"])) for p in parts)
        fail(f"{len(rows)} questions, expected {args.expect_queries} (per file: {sizes}). "
             f"A shard stopped before its last unit, or the files are not the ones of this run.")
    missing_units = sorted({str(r.get("user_id") or "_shared") for r in rows} - set(units))
    if missing_units:
        fail(f"{len(missing_units)} isolation units have questions but no ingestion record (e.g. {missing_units[:3]})")
    ingested_docs = sum(u.get("docs", 0) for u in units.values())
    if args.expect_docs is not None and ingested_docs != args.expect_docs:
        fail(f"{ingested_docs} documents ingested, expected {args.expect_docs}")

    merged = {k: v for k, v in parts[0].items() if k not in ("results", "units")}
    merged["total_queries"] = len(rows)
    merged["shards"] = n_shards
    merged["ingested_docs"] = ingested_docs
    merged["ingested_units"] = sum(u.get("units", 0) for u in units.values())
    merged["docs_not_indexed"] = sum(u.get("docs_not_indexed", 0) for u in units.values())
    merged["docs_split"] = sum(u.get("docs_split", 0) for u in units.values())
    merged["ingestion_time_ms"] = round(sum(u.get("ingest_ms", 0.0) for u in units.values()), 1)
    latencies = sorted(r["retrieve_time_ms"] for r in rows if r.get("retrieve_time_ms") is not None)
    merged["avg_retrieve_time_ms"] = round(sum(latencies) / len(latencies), 1) if latencies else None
    merged["median_retrieve_time_ms"] = percentile(latencies, 0.5) if latencies else None
    merged["p95_retrieve_time_ms"] = percentile(latencies, 0.95) if latencies else None
    merged["metrics"] = report(rows, args.k)
    merged["units"] = units
    merged["results"] = rows

    warnings = []
    unfinished = [p.parents[3].name for p in args.shards
                  if (log := attempts(p)) and not (log[-1].get("event") == "exit" and log[-1].get("status") == 0)]
    if unfinished:
        warnings.append(f"the last pod of {', '.join(unfinished)} did not end with status 0 (see attempts.jsonl)")
    if args.expect_docs is None:
        warnings.append(f"ingested_docs={ingested_docs} not checked against the split (no --expect-docs)")
    without_gold = merged["metrics"]["all"]["without_gold"]
    if without_gold:
        warnings.append(f"{without_gold} questions have no expected session and are left out of the metrics")

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(merged, indent=1))
    per_unit = merged["ingestion_time_ms"] / 1000 / merged["ingested_units"] if merged["ingested_units"] else 0.0
    print(f"{n_shards} shards, {len(rows)} questions (expected {args.expect_queries}), provider {merged['memory_provider']}, "
          f"unit {merged['unit']}, gold = {merged['gold_definition']} -> {args.output}")
    print(f"ingested {ingested_docs} documents as {merged['ingested_units']} units in {merged['ingestion_time_ms'] / 1000:.0f}s "
          f"({per_unit:.2f}s per unit); {merged['docs_not_indexed']} documents not indexed, {merged['docs_split']} split; "
          f"retrieve median {merged['median_retrieve_time_ms']} ms")
    print(table(merged["metrics"], args.k))
    for warning in warnings:
        print(f"WARNING: {warning}")


if __name__ == "__main__":
    main()
