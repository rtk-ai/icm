#!/usr/bin/env python3
"""Merge the per-shard result files of a sharded run into one harness-shaped result,
and refuse to do it on an incomplete set.

    python merge_shards.py out/RUN/shard-*/locomo/icm/rag/locomo10.json \\
        --expect-queries 1540 --expect-docs 272 -o out/merged/locomo/icm/rag/locomo10.json

A run that was not sharded (one file under `all/`) goes through the same command:
nothing is merged, the checks still apply.

Refused, exit 1, nothing written:
  * a shard is missing or given twice (the `shard-<i>-of-<n>` of each path must
    cover 0..n-1 exactly once, with one n);
  * the number of questions differs from --expect-queries
    (locomo/locomo10: 1540, personamem/32k: 589);
  * a question id appears twice, inside a shard or across shards;
  * the shards disagree on dataset, split, mode, models, provider, run name or
    description (two builds or two settings mixed in one table row).

Written, exit 3 (exit 0 with --allow-fallback): some rows went through the harness'
silent fallback. After six unparseable model outputs the harness uses the raw text
as the value of every field: an open answer equals its own reasoning, a judge verdict
is `bool("<raw text>")`, i.e. CORRECT whatever the text says, and a multiple-choice
answer is not a letter. The ids are printed and stored under `fallback_check`.

Warned about, exit status unchanged: ingestion figures that do not cover every
document. After a pod restart the harness counts only the units ingested by the
last attempt. With --expect-docs a mismatch sets `ingestion_time_ms` to null.

Totals are recomputed the way the harness does it: accuracy is the mean of `score`
when the dataset scores continuously (BEAM), else correct / total. The merged file
also gets `median_retrieve_time_ms` and `p95_retrieve_time_ms`: on a dataset the
harness runs in one batch (PersonaMem) the mean includes one `icm serve` start and
model load per unit, so the median is the figure to quote for ICM's recall latency.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

_SHARD = re.compile(r"^shard-(\d+)-of-(\d+)$")
_SAME = ("dataset", "split", "mode", "category", "oracle", "answer_llm", "judge_llm",
         "memory_provider", "run_name", "description")
_MCQ_LETTERS = ("a", "b", "c", "d")


def fail(message: str) -> None:
    sys.exit(f"merge_shards: REFUSED: {message}")


def shard_of(path: Path) -> tuple[int, int] | None:
    for part in path.parts:
        m = _SHARD.match(part)
        if m:
            return int(m.group(1)), int(m.group(2))
    return None


def check_coverage(paths: list[Path]) -> int:
    """Every shard index exactly once. Returns the number of shards."""
    found = [shard_of(p) for p in paths]
    if all(f is None for f in found):
        if len(paths) != 1:
            fail(f"{len(paths)} files and no `shard-<i>-of-<n>` in their paths: cannot tell whether a shard is missing")
        return 1
    if any(f is None for f in found):
        fail("some paths have a `shard-<i>-of-<n>` directory and some do not: "
             + ", ".join(str(p) for p, f in zip(paths, found) if f is None))
    totals = sorted({n for _, n in found})
    if len(totals) != 1:
        fail(f"shards of different runs are mixed (of-{' and of-'.join(map(str, totals))})")
    n = totals[0]
    indices = [i for i, _ in found]
    twice = sorted({i for i in indices if indices.count(i) > 1})
    missing = sorted(set(range(n)) - set(indices))
    stray = sorted(set(indices) - set(range(n)))
    if twice or missing or stray:
        fail(f"need shards 0..{n - 1} exactly once: missing {missing or 'none'}, given twice {twice or 'none'}"
             + (f", out of range {stray}" if stray else ""))
    return n


def fallback_rows(rows: list[dict]) -> dict[str, list[str]]:
    """Rows that carry the signature of the harness' last-resort parse (see module docstring)."""
    out: dict[str, list[str]] = {"answer_fallback": [], "judge_fallback": [], "invalid_mcq_answer": [], "empty_context": []}
    for row in rows:
        reason = row.get("judge_reason") or ""
        answer = row.get("answer") or ""
        reasoning = row.get("reasoning") or ""
        if reason.startswith("empty context"):
            out["empty_context"].append(row["query_id"])
        elif reason == "letter match" or reason.startswith("expected one of"):
            # Multiple choice, scored without a judge: the fallback leaves the first character of the raw text.
            if answer.strip().lower() not in _MCQ_LETTERS:
                out["invalid_mcq_answer"].append(row["query_id"])
        elif reason.startswith(("score=", "interventions=")):
            continue  # continuous or coding score: no judge verdict to inspect
        else:
            if reasoning.strip() and reasoning == answer:
                out["answer_fallback"].append(row["query_id"])
            verdict = reason.strip()
            if row.get("correct") and (not verdict or verdict[0] in "{[`" or '"correct"' in verdict or len(verdict) > 1500):
                out["judge_fallback"].append(row["query_id"])
    return out


def attempts(result_path: Path) -> list[dict]:
    """The pod start/exit log written by entrypoint.sh next to <dataset>/<run>/<mode>/<split>.json."""
    if len(result_path.parents) < 4:
        return []
    log = result_path.parents[3] / "attempts.jsonl"
    if not log.is_file():
        return []
    out = []
    for line in log.read_text().splitlines():
        try:
            out.append(json.loads(line))
        except ValueError:
            continue
    return out


def percentile(sorted_values: list[float], q: float) -> float:
    return sorted_values[min(len(sorted_values) - 1, int(q * len(sorted_values)))]


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("shards", nargs="+", type=Path)
    ap.add_argument("-o", "--output", required=True, type=Path)
    ap.add_argument("--expect-queries", required=True, type=int,
                    help="questions the complete run must hold (locomo/locomo10: 1540, personamem/32k: 589)")
    ap.add_argument("--expect-docs", type=int,
                    help="documents of the split (locomo/locomo10: 272, personamem/32k: 195); "
                         "checks that the ingestion figures cover all of them")
    ap.add_argument("--allow-fallback", action="store_true",
                    help="exit 0 even when rows went through the harness' silent fallback")
    args = ap.parse_args()

    n_shards = check_coverage(args.shards)
    parts = []
    for path in args.shards:
        try:
            parts.append(json.loads(path.read_text()))
        except (OSError, ValueError) as e:
            fail(f"{path} is not a readable result file: {e}")
    for key in _SAME:
        values = {json.dumps(p.get(key)) for p in parts}
        if len(values) > 1:
            fail(f"shards disagree on {key}: {sorted(values)}")

    results: dict[str, dict] = {}
    for path, part in zip(args.shards, parts):
        for row in part.get("results", []):
            if row["query_id"] in results:
                fail(f"query {row['query_id']} appears twice (second time in {path})")
            results[row["query_id"]] = row
    rows = list(results.values())
    if len(rows) != args.expect_queries:
        sizes = ", ".join(f"{len(p.get('results', []))}" for p in parts)
        fail(f"{len(rows)} questions, expected {args.expect_queries} (per file: {sizes}). "
             f"A shard stopped before its last unit, or the files are not the ones of this run.")

    merged = {k: v for k, v in parts[0].items() if k != "results"}
    merged["total_queries"] = len(rows)
    merged["correct"] = sum(1 for r in rows if r.get("correct"))
    scores = [r["score"] for r in rows if r.get("score") is not None]
    merged["accuracy"] = (sum(scores) / len(scores)) if scores else (merged["correct"] / len(rows) if rows else 0.0)
    merged["ingestion_time_ms"] = round(sum(p.get("ingestion_time_ms") or 0 for p in parts), 1)
    merged["ingested_docs"] = sum(p.get("ingested_docs") or 0 for p in parts)
    for field in ("retrieve_time_ms", "context_tokens"):
        values = [r[field] for r in rows if r.get(field) is not None]
        merged[f"avg_{field}"] = round(sum(values) / len(values), 1) if values else None
    latencies = sorted(r["retrieve_time_ms"] for r in rows if r.get("retrieve_time_ms") is not None)
    merged["median_retrieve_time_ms"] = percentile(latencies, 0.5) if latencies else None
    merged["p95_retrieve_time_ms"] = percentile(latencies, 0.95) if latencies else None

    warnings: list[str] = []
    resumed, unfinished = [], []
    for path in args.shards:
        log = attempts(path)
        if any(a.get("event") == "start" and a.get("resume") for a in log):
            resumed.append(str(path.parents[3].name))
        if log and not (log[-1].get("event") == "exit" and log[-1].get("status") == 0):
            unfinished.append(str(path.parents[3].name))
    if unfinished:
        warnings.append(f"the last pod of {', '.join(unfinished)} did not end with status 0 (see attempts.jsonl)")
    if args.expect_docs is not None and merged["ingested_docs"] != args.expect_docs:
        warnings.append(f"ingestion figures cover {merged['ingested_docs']} documents out of {args.expect_docs}: "
                        f"ingestion_time_ms set to null (a resumed shard counts only its last attempt)")
        merged["ingestion_incomplete"] = {"ingested_docs": merged["ingested_docs"], "expected_docs": args.expect_docs,
                                          "partial_ingestion_time_ms": merged["ingestion_time_ms"]}
        merged["ingestion_time_ms"] = None
    elif args.expect_docs is None and resumed:
        warnings.append(f"{', '.join(resumed)} resumed after a restart: ingestion_time_ms and ingested_docs "
                        f"({merged['ingested_docs']}) may cover only the last attempt; pass --expect-docs to check")
    elif args.expect_docs is None:
        warnings.append(f"ingested_docs={merged['ingested_docs']} not checked against the split (no --expect-docs)")

    flagged = fallback_rows(rows)
    merged["fallback_check"] = {key: {"count": len(ids), "query_ids": ids} for key, ids in flagged.items()}
    merged["results"] = rows

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(merged, indent=2))
    print(f"{n_shards} shards, {len(rows)} queries (expected {args.expect_queries}), "
          f"accuracy {merged['accuracy']:.4f} -> {args.output}")
    print(f"retrieve time: mean {merged['avg_retrieve_time_ms']} ms, median {merged['median_retrieve_time_ms']} ms, "
          f"p95 {merged['p95_retrieve_time_ms']} ms; ingested_docs {merged['ingested_docs']}, "
          f"ingestion_time_ms {merged['ingestion_time_ms']}")
    print("harness fallback rows (expected 0): " + ", ".join(
        f"{key}={len(flagged[key])}" for key in ("answer_fallback", "judge_fallback", "invalid_mcq_answer"))
        + f"; empty_context={len(flagged['empty_context'])}")
    for warning in warnings:
        print(f"WARNING: {warning}")
    fallback = sum(len(flagged[key]) for key in ("answer_fallback", "judge_fallback", "invalid_mcq_answer"))
    if fallback:
        for key in ("answer_fallback", "judge_fallback", "invalid_mcq_answer"):
            if flagged[key]:
                print(f"FALLBACK {key}: {', '.join(flagged[key][:20])}{' ...' if len(flagged[key]) > 20 else ''}")
        print("A judge_fallback row is counted CORRECT by the harness whatever the judge wrote: read its "
              "judge_reason, and publish the score with those rows counted wrong or state their number.")
        if not args.allow_fallback:
            sys.exit(3)


if __name__ == "__main__":
    main()
