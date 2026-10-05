#!/usr/bin/env python3
"""Replay one benchmark slice for two ICM builds and print a before/after table.

Both sides see the same questions, the same answer model and the same judge.
Each side is one ICM binary plus provider settings, so the script also compares
two engines of the same binary (legacy against v2).

    python compare_builds.py \\
        --before /path/to/icm-baseline --before-env ICM_AMB_ENGINE=legacy \\
        --after  /path/to/icm-next     --after-env  ICM_AMB_ENGINE=v2 \\
        --dataset locomo --split locomo10 --query-limit 20 --out /path/to/out

Each side names its recall engine: `--before-env ICM_AMB_ENGINE=...` and
`--after-env ICM_AMB_ENGINE=...` are required (`v2`, `legacy`, or
`binary-default-no-dates`; see icm_provider.py). A side without it is refused before
anything runs: the engine a binary picks by itself changed between builds, so an
unnamed side is neither the baseline nor v2. `legacy` is the value for a binary
older than v2 too. The engine and the dates sent are in each side's result file,
in the table and in the JSON written next to it.

Reported per side: accuracy, gold-session recall at k, context tokens (tiktoken,
as the harness counts them), memories returned, ingestion time, recall latency.

  --no-llm   skip answer generation and judging: ingest and recall only. No model
             cost, no accuracy row. Use it for gold recall on a full split.
  --reuse    keep a side whose result already exists in --out (e.g. the baseline).

Sides run one after the other (a warm ICM server holds the embedding model).
Every database lives under --out; the binaries are always started with --db.
Each side also leaves `http-trace-<dataset>-<split>-<label>.jsonl`: every /store
and /recall request as it was sent. The table counts in it the requests that
carried `created_at`, `now` and each `engine` value, so what a side really handed
to ICM is read from the wire, not from its settings.
"""

from __future__ import annotations

import argparse
import contextlib
import json
import os
import subprocess
import sys
import time
from pathlib import Path

from recall_at_k import gold_index, read_trace, recall_metrics
from run_amb import bootstrap

_HERE = Path(__file__).resolve().parent
_LLM_DEFAULTS = {
    # The models of the published leaderboard runs.
    "OMB_ANSWER_LLM": "gemini", "OMB_ANSWER_MODEL": "gemini-3.1-pro-preview",
    "OMB_JUDGE_LLM": "gemini", "OMB_JUDGE_MODEL": "gemini-2.5-flash-lite",
}


def _parse_env(pairs: list[str]) -> dict[str, str]:
    env = {}
    for pair in pairs:
        key, sep, value = pair.partition("=")
        if not sep or not key:
            sys.exit(f"expected KEY=VALUE, got {pair!r}")
        env[key] = value
    return env


@contextlib.contextmanager
def _environ(overrides: dict[str, str]):
    """Run a block with a clean set of ICM_AMB_* variables plus `overrides`."""
    saved = dict(os.environ)
    for key in [k for k in os.environ if k.startswith("ICM_AMB_")]:
        del os.environ[key]
    os.environ.update(overrides)
    try:
        yield
    finally:
        os.environ.clear()
        os.environ.update(saved)


def _select_queries(dataset, split: str, category: str | None, limit: int | None, unit: str | None):
    """Same selection rules as the harness runner (comma categories: `limit` per category)."""
    cats = [c.strip() for c in category.split(",") if c.strip()] if category else [None]
    if len(cats) > 1:
        seen, queries = set(), []
        for cat in cats:
            for q in dataset.load_queries(split, category=cat, limit=limit):
                if q.id not in seen:
                    seen.add(q.id)
                    queries.append(q)
        doc_category = None
    else:
        queries = dataset.load_queries(split, category=cats[0], limit=limit)
        doc_category = cats[0]
    if unit:
        queries = [q for q in queries if str(q.user_id) == str(unit)]
    return queries, doc_category


def _http_trace(args, label: str) -> Path:
    return args.out / f"http-trace-{args.dataset}-{args.split}-{label}.jsonl"


def sent_summary(path: Path) -> dict:
    """What the requests of one side carried, counted in its HTTP trace."""
    stores = dated = recalls = with_now = 0
    engines: dict[str, int] = {}
    if path.exists():
        for line in path.read_text().splitlines():
            if not line.strip():
                continue
            row = json.loads(line)
            body = row.get("body") or {}
            if row.get("path") == "/store":
                stores += 1
                dated += "created_at" in body
            elif row.get("path") == "/recall":
                recalls += 1
                with_now += "now" in body
                key = str(body.get("engine", "(no engine field)"))
                engines[key] = engines.get(key, 0) + 1
    return {"store_requests": stores, "store_with_created_at": dated, "recall_requests": recalls,
            "recall_with_now": with_now, "recall_engine_field": engines}


def _store_dir(args, label: str) -> Path:
    return args.out / args.dataset / label / "_store" / args.split / (args.category or "all")


def _result_path(args, label: str) -> Path:
    name = "recall-only" if args.no_llm else "rag"
    return args.out / args.dataset / label / name / f"{args.split}.json"


def _run_harness(args, label: str, side_env: dict[str, str]) -> None:
    """Full run through the upstream harness (answers and judging)."""
    trace = args.out / f"trace-{args.dataset}-{args.split}-{label}.jsonl"
    trace.unlink(missing_ok=True)
    env = {k: v for k, v in os.environ.items() if not k.startswith("ICM_AMB_") or k == "ICM_AMB_GCLOUD_AUTH"}
    for key, value in _LLM_DEFAULTS.items():
        env.setdefault(key, value)
    env.update(side_env)
    env["ICM_AMB_TRACE"] = str(trace)
    env["ICM_AMB_HTTP_TRACE"] = str(_http_trace(args, label))
    _http_trace(args, label).unlink(missing_ok=True)
    # run_amb.py appends the engine of this side to the description (ICM_AMB_ENGINE is in side_env).
    cmd = [sys.executable, str(_HERE / "run_amb.py"), "run", "--dataset", args.dataset, "--split", args.split,
           "--memory", "icm", "--mode", "rag", "--name", label, "--output-dir", str(args.out),
           "--description", f"compare_builds side {label}"]
    for flag, value in (("--category", args.category), ("--query-limit", args.query_limit), ("--unit", args.unit)):
        if value is not None:
            cmd += [flag, str(value)]
    log = args.out / f"log-{args.dataset}-{args.split}-{label}.txt"
    with open(log, "w") as fh:
        code = subprocess.run(cmd, env=env, stdout=fh, stderr=subprocess.STDOUT).returncode
    if code != 0:
        sys.exit(f"harness run for {label!r} failed with exit code {code}; see {log}")


def _run_recall_only(args, label: str, side_env: dict[str, str], dataset, queries, doc_category) -> None:
    """Ingest and recall without any LLM, driving the provider the way the runner does."""
    from icm_provider import IcmMemoryProvider
    from memory_bench.utils import count_tokens

    _http_trace(args, label).unlink(missing_ok=True)
    with _environ(dict(side_env, ICM_AMB_HTTP_TRACE=str(_http_trace(args, label)))):
        provider = IcmMemoryProvider()
        settings = provider.settings
        try:
            units = {q.user_id for q in queries if q.user_id}
            if dataset.isolation_unit is not None:
                documents = dataset.load_documents(args.split, category=doc_category, user_ids=units)
            else:
                documents = dataset.load_documents(args.split, category=doc_category)
            provider.prepare(_store_dir(args, label), unit_ids=units or None, reset=True)

            rows, ingest_s = [], 0.0

            def recall(q) -> None:
                text = q.meta.get("retrieval_query") or q.query
                t0 = time.perf_counter()
                docs, _ = provider.retrieve(text, None, q.user_id, q.meta.get("query_timestamp"))
                ms = (time.perf_counter() - t0) * 1000
                context = "\n\n".join(f"## Memory {i + 1}\n{d.content}" for i, d in enumerate(docs))
                rows.append({"query_id": q.id, "user_id": q.user_id, "query": text,
                             "doc_ids": [d.id for d in docs], "ms": round(ms, 1),
                             "context_tokens": count_tokens(context)})

            if dataset.isolation_unit is not None:
                for unit in dict.fromkeys(q.user_id for q in queries):
                    unit_docs = [d for d in documents if dataset.get_isolation_id(d) == unit]
                    t0 = time.perf_counter()
                    provider.ingest(unit_docs)
                    ingest_s += time.perf_counter() - t0
                    for q in queries:
                        if q.user_id == unit:
                            recall(q)
                    print(f"  [{label}] unit {unit}: {len(unit_docs)} docs ingested, "
                          f"{sum(1 for q in queries if q.user_id == unit)} queries", flush=True)
            else:
                t0 = time.perf_counter()
                provider.ingest(documents)
                ingest_s = time.perf_counter() - t0
                for q in queries:
                    recall(q)
        finally:
            provider.cleanup()

    out = _result_path(args, label)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps({
        "dataset": args.dataset, "split": args.split, "mode": "recall-only", "run_name": label,
        "description": settings.describe(f"compare_builds side {label}"),
        "engine": settings.engine, "store_date": settings.store_date, "query_now": settings.query_now,
        "engine_field_in_binary": provider._engine_field,
        "ingestion_time_ms": round(ingest_s * 1000, 1), "ingested_docs": len(documents), "results": rows,
    }, indent=2))


def _collect(args, label: str, gold: dict, settings) -> dict:
    """Read one side's outputs back into a flat metrics dict.

    The engine comes from what the run wrote, never from what was asked: a result
    kept by --reuse that was produced with another engine is refused."""
    result = json.loads(_result_path(args, label).read_text())
    stats_file = _store_dir(args, label) / "icm" / "ingest-stats.json"
    stats = json.loads(stats_file.read_text()) if stats_file.exists() else {}
    rows = result["results"]
    recorded = {key: result.get(key, stats.get(key)) for key in ("engine", "store_date", "query_now")}
    asked = {"engine": settings.engine, "store_date": settings.store_date, "query_now": settings.query_now}
    if recorded != asked:
        sys.exit(f"side {label!r}: {_result_path(args, label)} was produced with {recorded}, this command asks for "
                 f"{asked}. Use another --out or label, or drop --reuse.")

    if args.no_llm:
        trace_rows = rows
        accuracy = None
    else:
        trace = args.out / f"trace-{args.dataset}-{args.split}-{label}.jsonl"
        trace_rows = [r for rs in read_trace(trace).values() for r in rs]
        accuracy = (result["correct"], result["total_queries"], result["accuracy"])

    tokens = [r["context_tokens"] for r in rows if r.get("context_tokens") is not None]
    chunks = stats.get("chunks") or 0
    metrics = recall_metrics(trace_rows, gold, args.k)
    metrics.update({
        "label": label,
        "icm_version": stats.get("icm_version"),
        "binary": stats.get("binary"),
        "engine": recorded["engine"],
        "engine_label": settings.label,
        "store_date": recorded["store_date"],
        "query_now": recorded["query_now"],
        "engine_field_in_binary": result.get("engine_field_in_binary", stats.get("engine_field_in_binary")),
        "description": result.get("description"),
        "sent": sent_summary(_http_trace(args, label)),
        "max_tokens": stats.get("max_tokens"),
        "k": stats.get("k"),
        "chunk_tokens": stats.get("chunk_tokens"),
        "queries": len(rows),
        "accuracy": accuracy,
        "answer_llm": result.get("answer_llm"),
        "judge_llm": result.get("judge_llm"),
        "avg_context_tokens": round(sum(tokens) / len(tokens), 1) if tokens else None,
        "ingestion_s": round((result.get("ingestion_time_ms") or 0) / 1000, 1),
        "chunks": chunks,
        "s_per_chunk": round((result.get("ingestion_time_ms") or 0) / 1000 / chunks, 2) if chunks else None,
    })
    return metrics


def _table(before: dict, after: dict, ks: list[int]) -> str:
    def cell(value, pct: bool = False) -> str:
        if value is None:
            return "n/a"
        return f"{value:.1%}" if pct else str(value)

    def delta(a, b, pct: bool = False) -> str:
        if a is None or b is None or isinstance(a, str) or isinstance(b, str):
            return ""
        return f"{(b - a) * 100:+.1f} pts" if pct else f"{b - a:+.1f}"

    def setting(m: dict) -> str:
        cut = f"max_tokens={m['max_tokens']}" if m.get("max_tokens") else f"k={m.get('k')}"
        return f"{m.get('engine_label')}, {cut}, chunks {m.get('chunk_tokens')}"

    def sent(m: dict) -> str:
        t = m.get("sent") or {}
        if not t.get("store_requests") and not t.get("recall_requests"):
            return "no HTTP trace (side reused)"
        fields = ", ".join(f"{name} x{n}" for name, n in sorted(t["recall_engine_field"].items()))
        return (f"/store with created_at {t['store_with_created_at']}/{t['store_requests']}; /recall with now "
                f"{t['recall_with_now']}/{t['recall_requests']}; engine field: {fields or 'n/a'}")

    lines = [f"| Metric | {before['label']} | {after['label']} | Delta |", "|---|---|---|---|",
             f"| ICM version | {before.get('icm_version')} | {after.get('icm_version')} | |",
             f"| Settings | {setting(before)} | {setting(after)} | |",
             f"| Sent to ICM (HTTP trace) | {sent(before)} | {sent(after)} | |",
             f"| Queries | {before['queries']} | {after['queries']} | |"]
    if before["accuracy"] and after["accuracy"]:
        (bc, bt, ba), (ac, at, aa) = before["accuracy"], after["accuracy"]
        lines.append(f"| Accuracy | {bc}/{bt} ({ba:.1%}) | {ac}/{at} ({aa:.1%}) | {delta(ba, aa, True)} |")
    lines.append(f"| Queries with gold ids | {before['queries_with_gold']} | {after['queries_with_gold']} | |")
    for k in ks:
        for name in ("any", "all", "frac"):
            key = f"{name}@{k}"
            lines.append(f"| Gold sessions {key} | {cell(before[key], True)} | {cell(after[key], True)} "
                         f"| {delta(before[key], after[key], True)} |")
    for key, title in (("avg_returned", "Memories returned (avg)"),
                       ("avg_context_tokens", "Context tokens (avg, tiktoken)"),
                       ("ingestion_s", "Ingestion (s)"), ("chunks", "Memories stored"),
                       ("s_per_chunk", "Ingestion (s per memory)"),
                       ("avg_retrieve_ms", "Recall latency avg (ms)"),
                       ("median_retrieve_ms", "Recall latency median (ms)")):
        lines.append(f"| {title} | {cell(before[key])} | {cell(after[key])} | {delta(before[key], after[key])} |")
    return "\n".join(lines)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--before", required=True, help="ICM binary of the reference side")
    ap.add_argument("--after", required=True, help="ICM binary of the candidate side")
    ap.add_argument("--before-label", default="before")
    ap.add_argument("--after-label", default="after")
    ap.add_argument("--before-env", action="append", default=[], metavar="KEY=VALUE",
                    help="provider setting for the reference side (repeatable); ICM_AMB_ENGINE=... is required, "
                         "e.g. ICM_AMB_ENGINE=legacy")
    ap.add_argument("--after-env", action="append", default=[], metavar="KEY=VALUE",
                    help="provider setting for the candidate side (repeatable); ICM_AMB_ENGINE=... is required, "
                         "e.g. ICM_AMB_ENGINE=v2")
    ap.add_argument("--dataset", required=True)
    ap.add_argument("--split", required=True)
    ap.add_argument("--category", help="category filter(s), comma-separated, as in `amb run`")
    ap.add_argument("--query-limit", type=int, help="max queries (per category when several are given)")
    ap.add_argument("--unit", help="one isolation unit")
    ap.add_argument("--out", required=True, type=Path, help="output directory (results, traces, databases)")
    ap.add_argument("--k", type=int, nargs="+", default=[5, 10, 20, 50])
    ap.add_argument("--no-llm", action="store_true", help="ingest and recall only: no answers, no judge")
    ap.add_argument("--reuse", action="store_true", help="skip a side whose result file already exists")
    args = ap.parse_args()
    args.out = args.out.resolve()
    args.out.mkdir(parents=True, exist_ok=True)
    if args.before_label == args.after_label:
        sys.exit("--before-label and --after-label must differ")

    bootstrap()
    from icm_provider import engine_settings
    from memory_bench.dataset import get_dataset

    # Both sides are checked before either runs: an hour of ingestion is not spent on a
    # comparison whose second side would be refused.
    engines = {}
    for label, flag, pairs in ((args.before_label, "--before-env", args.before_env),
                               (args.after_label, "--after-env", args.after_env)):
        try:
            engines[label] = engine_settings(_parse_env(pairs))
        except RuntimeError as e:
            sys.exit(f"side {label!r}: {e}\nGive it with {flag} ICM_AMB_ENGINE=<engine>; the caller's environment "
                     f"is not read for a side.")

    dataset = get_dataset(args.dataset)
    queries, doc_category = _select_queries(dataset, args.split, args.category, args.query_limit, args.unit)
    if not queries:
        sys.exit("no query selected")
    gold = gold_index(args.dataset, args.split)

    sides = []
    for label, binary, pairs in ((args.before_label, args.before, args.before_env),
                                 (args.after_label, args.after, args.after_env)):
        binary_path = Path(binary).resolve()
        if not binary_path.is_file():
            sys.exit(f"ICM binary not found: {binary}")
        side_env = dict(_parse_env(pairs), ICM_AMB_BIN=str(binary_path))
        if args.reuse and _result_path(args, label).exists():
            print(f"[{label}] reusing {_result_path(args, label)}", flush=True)
        else:
            print(f"[{label}] {binary_path} {' '.join(pairs)} - {len(queries)} queries", flush=True)
            if args.no_llm:
                _run_recall_only(args, label, side_env, dataset, queries, doc_category)
            else:
                _run_harness(args, label, side_env)
        sides.append(_collect(args, label, gold, engines[label]))

    table = _table(sides[0], sides[1], args.k)
    mode = "recall only, no LLM" if args.no_llm else \
        f"answers {sides[0].get('answer_llm')}, judge {sides[0].get('judge_llm')}"
    header = f"## {args.dataset}/{args.split}: {sides[0]['queries']} queries ({mode})\n\n"
    stem = f"compare-{args.dataset}-{args.split}-{args.before_label}-vs-{args.after_label}"
    (args.out / f"{stem}.md").write_text(header + table + "\n")
    (args.out / f"{stem}.json").write_text(json.dumps({"before": sides[0], "after": sides[1]}, indent=2))
    print("\n" + header + table)
    print(f"\nSaved: {args.out / (stem + '.md')}")


if __name__ == "__main__":
    main()
