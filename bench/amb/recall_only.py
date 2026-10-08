#!/usr/bin/env python3
"""Recall-only run: ingest, query, write down the ranked sessions. No LLM, ever.

    ICM_AMB_ENGINE=v2 ICM_AMB_STORE_DATE=0 ICM_AMB_QUERY_NOW=0 \\
        python recall_only.py run --dataset longmemeval --split s --memory icm --output-dir out
    python recall_only.py run --dataset longmemeval --split s --memory bm25 --output-dir out
    python merge_recall.py out/longmemeval/icm/recall/s.json --expect-queries 500 --expect-docs 23867 -o merged.json

The harness' own modes cannot do this on LongMemEval or LoCoMo: its `retrieval`
mode still calls the judge model for an "open" dataset, and building its runner
builds a Gemini client. This script uses the harness only for what does not touch
a model: the dataset loaders and the provider base class. Every LLM entry point of
the harness is replaced by a function that raises, so a call cannot happen quietly.

Same command line as `run_amb.py run` where it makes sense, so `entrypoint.sh`
only swaps the script when MODE=recall: `--dataset --split --memory --name
--output-dir --description --skip-ingested --query-limit --category`. Sharding is
the launcher's (ICM_AMB_SHARD, ICM_AMB_SHARD_BY). The result file goes to
`<output-dir>/<dataset>/<name>/recall/<split>.json`, is rewritten after every
isolation unit (LongMemEval: after every question) and has the harness' shape
(`results` rows keyed by `query_id`), so `gcs_sync.py` protects it like any other.
`--skip-ingested` resumes at the first unit that is not complete in that file.

What is indexed (`--unit`, or ICM_AMB_RECALL_UNIT), for both providers alike:

  session-user  one unit per session: the user turns, joined by a newline. A session
                with no user turn is not indexed. This is the unit of MemPalace's
                published LongMemEval figure (benchmarks/longmemeval_bench.py,
                `build_palace_and_retrieve`, granularity `session`). Default.
  session-all   one unit per session: every turn as `role: content`, one per line.
                This is agentmemory's unit (benchmark/longmemeval-bench.ts).
  chunk         the units of the answer runs: the dataset's serialised document cut
                in ICM_AMB_CHUNK_TOKENS (512) cl100k tokens, each with the date and
                provenance line. Works for every dataset.

`session-*` need documents that are a JSON list of `{role, content}` turns
(LongMemEval). ICM_AMB_HEADER=1 adds the date/provenance line to session units
(default: none, as in both published protocols); ICM_AMB_HEADER=0 removes it from
chunks. A unit longer than 60,000 bytes is split at line breaks, for both
providers: ICM refuses a memory over 64 KiB (LongMemEval-S has one such session
out of 23,867).

Providers (`--memory`):

  icm    the ICM provider of the answer runs, driving `icm serve` (ICM_AMB_BIN,
         ICM_AMB_ENGINE, ICM_AMB_STORE_DATE, ICM_AMB_QUERY_NOW, ICM_AMB_NO_EMBEDDINGS
         as usual). ICM_AMB_ENGINE is required and is written in the result file
         (`provider.engine`, `provider.store_date`, `provider.query_now`) and in its
         description. Each unit is stored as one memory, text unchanged, without
         going through the tokenizer (session units need no tiktoken file).
         ICM_AMB_MAX_TOKENS is refused here: a token budget would cut the list.
         A returned memory whose id is not a document of the question's unit stops
         the run: it would be scored as a miss without a word.
  bm25   BM25 (rank_bm25 Okapi, the harness' library) over the very same units, one
         index per isolation unit. `--bm25-tokenizer words` (default) lower-cases and
         keeps runs of letters and digits; `harness` is the upstream bm25 provider's
         `lower().split()`, which leaves punctuation glued to the words. A unit that
         shares no token with the question is not returned.

What a row holds: `ranked_ids`, the document (session) id of each unit returned, in
rank order, up to `--depth` (50, MemPalace's own depth), repeats included: a session
indexed as several units can appear several times. `gold_ids` for LongMemEval are
the dataset's `answer_session_ids`, the definition both published protocols use;
the harness' own gold (`has_answer` turns) differs on 62 of the 500 questions and
is empty for 21, so it is kept aside as `gold_ids_harness`. Other datasets have
only the harness' gold. Metrics are computed by merge_recall.py.

`llm_calls` in the result file is a count, not a declaration: the number of times
anything reached one of the harness' LLM entry points during the run. Each of them
raised; a caller that swallowed the exception still leaves its trace here, and
merge_recall.py refuses a file where the count is not 0. `answer_llm` and
`judge_llm` are null because this script has no such model to name.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import signal
import sys
import time
from collections import OrderedDict
from pathlib import Path

import run_amb

UNITS = ("session-user", "session-all", "chunk")
MAX_UNIT_BYTES = 60_000  # ICM refuses a memory over 64 KiB (store layer and MCP tool)
_SHARED_UNIT = "_shared"
_WORD = re.compile(r"[a-z0-9]+")


def _env_flag(name: str, default: bool) -> bool:
    raw = os.environ.get(name)
    if raw is None or raw == "":
        return default
    return raw.strip().lower() in ("1", "true", "yes", "on")


# ------------------------------------------------------------------ indexed units

def split_bytes(text: str, limit: int = MAX_UNIT_BYTES) -> list[str]:
    """Cut `text` in parts of at most `limit` UTF-8 bytes, at line breaks when possible."""
    if len(text.encode()) <= limit:
        return [text]
    parts: list[str] = []
    current: list[str] = []
    size = 0
    for line in text.split("\n"):
        pieces = [line]
        if len(line.encode()) > limit:  # one line over the limit: cut it between characters
            pieces, buf, buf_size = [], [], 0
            for ch in line:
                n = len(ch.encode())
                if buf_size + n > limit:
                    pieces.append("".join(buf))
                    buf, buf_size = [], 0
                buf.append(ch)
                buf_size += n
            pieces.append("".join(buf))
        for piece in pieces:
            n = len(piece.encode()) + (1 if current else 0)
            if current and size + n > limit:
                parts.append("\n".join(current))
                current, size = [], 0
                n = len(piece.encode())
            current.append(piece)
            size += n
    if current:
        parts.append("\n".join(current))
    return [p for p in parts if p.strip()]


def _header(doc) -> str:
    """The date/provenance line the ICM provider writes at the top of a memory."""
    parts = []
    if doc.timestamp:
        parts.append(f"[{doc.timestamp}]")
    if doc.context:
        parts.append(doc.context)
    return " ".join(parts)


def _turns(doc) -> list[dict]:
    try:
        turns = json.loads(doc.content)
    except ValueError:
        turns = None
    if not isinstance(turns, list) or not all(isinstance(t, dict) and "role" in t and "content" in t for t in turns):
        sys.exit(f"recall_only: document {doc.id} is not a JSON list of {{role, content}} turns; "
                 f"`--unit session-user` and `session-all` only fit LongMemEval, use `--unit chunk`")
    return turns


def unit_texts(doc, unit: str, header: bool, chunk_tokens: int) -> list[str]:
    """The texts indexed for one harness document. An empty list: the document is not indexed."""
    from memory_bench.utils import chunk_text

    head = _header(doc) if header else ""
    if unit == "chunk":
        bodies = [chunk for chunk in chunk_text(doc.content, chunk_tokens) if chunk.strip()]
        return [f"{head}\n{body}" if head else body for body in bodies]
    turns = _turns(doc)
    if unit == "session-user":
        lines = [str(t["content"]) for t in turns if t["role"] == "user"]
        if not lines:
            return []
    else:
        lines = [f"{t['role']}: {t['content']}" for t in turns]
    text = "\n".join(lines)
    if not text.strip():
        return []
    room = MAX_UNIT_BYTES - (len(head.encode()) + 1 if head else 0)
    return [f"{head}\n{part}" if head else part for part in split_bytes(text, room)]


def build_units(docs: list, unit: str, header: bool, chunk_tokens: int) -> tuple[list, dict]:
    """Harness documents -> the `Document`s handed to the provider (one per indexed unit)."""
    from memory_bench.models import Document

    out, stats = [], {"docs": len(docs), "units": 0, "docs_not_indexed": 0, "docs_split": 0}
    for doc in docs:
        texts = unit_texts(doc, unit, header, chunk_tokens)
        if not texts:
            stats["docs_not_indexed"] += 1
        elif len(texts) > 1 and unit != "chunk":
            stats["docs_split"] += 1
        for text in texts:
            out.append(Document(id=doc.id, content=text, user_id=doc.user_id, timestamp=doc.timestamp))
    stats["units"] = len(out)
    return out, stats


# ---------------------------------------------------------------------- providers

def tokenize(text: str, tokenizer: str) -> list[str]:
    return text.lower().split() if tokenizer == "harness" else _WORD.findall(text.lower())


class UnitBm25:
    """BM25 over the units as given: no re-chunking, one index per isolation unit."""

    def __init__(self, tokenizer: str):
        if tokenizer not in ("words", "harness"):
            sys.exit(f"recall_only: --bm25-tokenizer {tokenizer!r}: expected `words` or `harness`")
        self.name = "bm25"
        self._tokenizer = tokenizer
        self.description = (f"BM25 Okapi (rank_bm25) over the same units as the memory under test, one index per "
                            f"isolation unit, tokenizer `{tokenizer}`. Units sharing no token with the question "
                            f"are not returned.")
        self._units: dict[str, tuple[list, object, list[set]]] = {}

    def prepare(self, store_dir: Path, unit_ids=None, reset: bool = True) -> None:
        self._units = {}

    def ingest(self, units: list) -> None:
        from rank_bm25 import BM25Okapi

        by_unit: OrderedDict[str, list] = OrderedDict()
        for doc in units:
            by_unit.setdefault(doc.user_id or _SHARED_UNIT, []).append(doc)
        for key, docs in by_unit.items():
            tokens = [tokenize(d.content, self._tokenizer) for d in docs]
            index = BM25Okapi(tokens) if any(tokens) else None
            self._units[key] = (docs, index, [set(t) for t in tokens])

    def retrieve(self, query: str, k: int = 10, user_id: str | None = None, query_timestamp: str | None = None):
        docs, index, vocab = self._units.get(user_id or _SHARED_UNIT, ([], None, []))
        terms = tokenize(query or "", self._tokenizer)
        if index is None or not terms:
            return [], None
        scores = index.get_scores(terms)
        wanted = set(terms)
        order = sorted((i for i in range(len(docs)) if wanted & vocab[i]), key=lambda i: scores[i], reverse=True)
        return [docs[i] for i in order[:k]], None

    def cleanup(self) -> None:
        self._units = {}


def make_provider(name: str, bm25_tokenizer: str):
    """Returns (provider, facts for the result header)."""
    if name == "bm25":
        return UnitBm25(bm25_tokenizer), {"bm25_tokenizer": bm25_tokenizer}
    if name != "icm":
        sys.exit(f"recall_only: --memory {name!r}: expected `icm` or `bm25` (other providers re-chunk what they "
                 f"are given, so the indexed unit would not be the one asked for)")
    if os.environ.get("ICM_AMB_MAX_TOKENS"):
        sys.exit("recall_only: unset ICM_AMB_MAX_TOKENS: a token budget would cut the ranked list before --depth")
    from icm_provider import IcmMemoryProvider

    try:
        provider = IcmMemoryProvider()  # refuses to start without ICM_AMB_ENGINE
    except RuntimeError as e:
        sys.exit(f"recall_only: {e}")
    # The units are final: one unit = one memory, text unchanged. Without this the
    # provider would cut a long session in 512-token chunks and add its own header.
    provider.store_documents_whole()
    settings = provider.settings
    return provider, {
        "icm_version": provider._version, "engine": settings.engine, "engine_label": settings.label,
        "embeddings": not provider._no_embeddings, "store_date": settings.store_date,
        "query_now": settings.query_now, "topic": provider._topic,
    }


# ------------------------------------------------------------------------- no LLM

_llm_attempts = {"count": 0, "earlier": 0}  # this process; the attempts a resumed file already recorded


def llm_attempts() -> int:
    """How many times an LLM entry point of the harness was reached since forbid_llm()."""
    return _llm_attempts["count"]


def forbid_llm() -> None:
    """Make every LLM entry point of the harness raise, and count the attempts.

    The raise is the guarantee. The count is what the result file reports: an
    attempt swallowed by a `try/except` somewhere would otherwise leave no trace."""
    import memory_bench.llm as llm_pkg
    from memory_bench.llm.base import LLM

    _llm_attempts.update(count=0, earlier=0)

    def refused(*_args, **_kwargs):
        _llm_attempts["count"] += 1
        raise RuntimeError("recall-only run: an LLM call was attempted; this mode never calls a model")

    for cls in {LLM, *llm_pkg.REGISTRY.values()}:
        for method in ("generate", "tool_loop", "_generate_raw"):
            if isinstance(cls, type) and hasattr(cls, method):
                setattr(cls, method, refused)
    llm_pkg.get_llm = llm_pkg.get_answer_llm = llm_pkg.get_judge_llm = refused


# --------------------------------------------------------------------------- gold

def gold_ids(dataset, queries: list) -> tuple[dict[str, list[str]], str]:
    """query id -> expected document ids, and the name of the definition used."""
    if dataset.name != "longmemeval":
        return {q.id: list(q.gold_ids) for q in queries}, "harness gold_ids"
    wanted = {q.id for q in queries}
    gold = {}
    for item in dataset._load_raw():
        qid = item.get("question_id")
        if qid in wanted:
            gold[qid] = [f"{qid}_{sid}" for sid in dict.fromkeys(item.get("answer_session_ids") or [])]
    missing = wanted - set(gold)
    if missing:
        sys.exit(f"recall_only: no `answer_session_ids` in the dataset file for {sorted(missing)[:5]}")
    return gold, "answer_session_ids"


# ---------------------------------------------------------------------------- run

_HEADER_KEYS = ("dataset", "split", "category", "mode", "memory_provider", "run_name", "description", "unit",
                "depth", "header", "chunk_tokens", "gold_definition", "provider")


def _save(path: Path, doc: dict) -> None:
    """Whole file or nothing, and never cut short by SIGTERM (same guard as the answer runs)."""
    doc["llm_calls"] = _llm_attempts["earlier"] + llm_attempts()
    run_amb._term["saving"] = True
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_name(path.name + ".part")
        tmp.write_text(json.dumps(doc, indent=1))
        os.replace(tmp, path)
    finally:
        run_amb._term["saving"] = False
        if run_amb._term["pending"]:
            sys.exit(143)


def run(args: argparse.Namespace) -> dict:
    if args.mode not in (None, "recall"):
        sys.exit(f"recall_only: --mode {args.mode!r}: this script only runs `recall`")
    if args.unit not in UNITS:
        sys.exit(f"recall_only: --unit {args.unit!r}: expected one of {', '.join(UNITS)}")
    if args.depth < 1:
        sys.exit("recall_only: --depth must be at least 1")

    run_amb.bootstrap()  # harness on sys.path, providers registered, sharding and trace as configured
    forbid_llm()
    from memory_bench.dataset import get_dataset

    dataset = get_dataset(args.dataset)
    provider, provider_facts = make_provider(args.memory, args.bm25_tokenizer)
    limit_max = 500 if provider_facts.get("engine") == "v2" else 100
    description = args.description
    if args.memory == "icm":
        description = provider.settings.describe(description)
    if args.memory == "icm" and args.depth > limit_max:
        sys.exit(f"recall_only: --depth {args.depth} is over what this engine returns ({limit_max})")
    name = args.name or provider.name
    header = _env_flag("ICM_AMB_HEADER", args.unit == "chunk")
    chunk_tokens = int(os.environ.get("ICM_AMB_CHUNK_TOKENS") or 512)

    queries = dataset.load_queries(args.split, category=args.category, limit=args.query_limit)
    by_unit: OrderedDict[str, list] = OrderedDict()
    for q in queries:
        by_unit.setdefault(q.user_id or _SHARED_UNIT, []).append(q)
    doc_category = args.category if args.category and dataset.category_type(args.split, args.category) == "doc" else None
    if dataset.isolation_unit is not None and args.query_limit is not None:
        documents = dataset.load_documents(args.split, category=doc_category, user_ids={q.user_id for q in queries if q.user_id})
    else:
        documents = dataset.load_documents(args.split, category=doc_category)
    docs_by_unit: dict[str, list] = {}
    for doc in documents:
        docs_by_unit.setdefault(dataset.get_isolation_id(doc) or _SHARED_UNIT, []).append(doc)
    del documents
    gold, gold_definition = gold_ids(dataset, queries)

    path = Path(args.output_dir) / dataset.name / name / "recall" / f"{args.split}.json"
    doc = {
        "dataset": dataset.name, "split": args.split, "category": args.category, "mode": "recall",
        "memory_provider": provider.name, "run_name": name, "description": description,
        "unit": args.unit, "depth": args.depth, "header": header,
        "chunk_tokens": chunk_tokens if args.unit == "chunk" else None,
        "gold_definition": gold_definition, "provider": dict(provider_facts, description=provider.description),
        "answer_llm": None, "judge_llm": None, "llm_calls": 0,
        "total_queries": 0, "units": {}, "results": [],
    }
    if args.skip_ingested and path.exists():
        try:
            previous = json.loads(path.read_text())
        except ValueError:
            sys.exit(f"recall_only: {path} is not readable JSON; delete it or drop --skip-ingested")
        for key in _HEADER_KEYS:
            if previous.get(key) != doc[key]:
                sys.exit(f"recall_only: {path} was written with {key}={previous.get(key)!r}, this run has "
                         f"{doc[key]!r}: not resuming a different configuration into the same file")
        doc["units"], doc["results"] = previous.get("units", {}), previous.get("results", [])
        _llm_attempts["earlier"] = int(previous.get("llm_calls") or 0)  # an earlier attempt's count is kept
    done = {row["query_id"] for row in doc["results"]}

    store_dir = Path(args.output_dir) / dataset.name / name / "_store" / args.split / (args.category or "all")
    provider.prepare(store_dir, unit_ids=set(by_unit), reset=True)
    print(f"[recall_only] {dataset.name}/{args.split} memory={provider.name} unit={args.unit} depth={args.depth}: "
          f"{len(queries)} questions in {len(by_unit)} isolation units, {len(done)} already in {path}", flush=True)
    started = time.perf_counter()
    try:
        for n, (unit, unit_queries) in enumerate(by_unit.items(), 1):
            if all(q.id in done for q in unit_queries):
                continue
            unit_docs = docs_by_unit.get(unit)
            if not unit_docs:
                sys.exit(f"recall_only: isolation unit {unit!r} has questions but no document")
            built, stats = build_units(unit_docs, args.unit, header, chunk_tokens)
            t0 = time.perf_counter()
            provider.ingest(built)
            stats["ingest_ms"] = round((time.perf_counter() - t0) * 1000, 1)
            doc["units"][unit] = stats
            doc["results"] = [row for row in doc["results"] if row["query_id"] not in {q.id for q in unit_queries}]
            known_ids = {d.id for d in unit_docs}
            for q in unit_queries:
                text = q.meta.get("retrieval_query") or q.query
                t0 = time.perf_counter()
                found, _ = provider.retrieve(text, args.depth, q.user_id, q.meta.get("query_timestamp"))
                elapsed = (time.perf_counter() - t0) * 1000
                foreign = [d.id for d in found if d.id not in known_ids]
                if foreign:
                    # Scored as it stands, each of these would be a miss nobody can see.
                    sys.exit(f"recall_only: question {q.id}: {len(foreign)} of the {len(found)} results are not documents "
                             f"of unit {unit!r} (e.g. {foreign[:3]}): the provider returned something this run did "
                             f"not store there. Nothing is written for this unit.")
                doc["results"].append({
                    "query_id": q.id, "user_id": q.user_id, "query": q.query,
                    "question_type": q.meta.get("question_type") or q.meta.get("category"),
                    "gold_ids": gold[q.id], "gold_ids_harness": list(q.gold_ids),
                    "ranked_ids": [d.id for d in found], "retrieve_time_ms": round(elapsed, 1),
                    "corpus_docs": stats["docs"], "corpus_units": stats["units"],
                })
            doc["total_queries"] = len(doc["results"])
            _save(path, doc)
            print(f"[recall_only] unit {n}/{len(by_unit)} {unit}: {stats['docs']} documents -> {stats['units']} units "
                  f"in {stats['ingest_ms'] / 1000:.1f}s, {len(unit_queries)} questions; "
                  f"{time.perf_counter() - started:.0f}s elapsed", flush=True)
    finally:
        provider.cleanup()
    doc["total_queries"] = len(doc["results"])
    if args.memory == "icm":
        doc["engine_field_in_binary"] = provider._engine_field  # None: not asked (see icm_provider.py)
    _save(path, doc)
    print(f"[recall_only] {len(doc['results'])} questions -> {path}", flush=True)
    return doc


def main(argv: list[str] | None = None) -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("command", choices=["run"])
    ap.add_argument("--dataset", required=True)
    ap.add_argument("--split", required=True)
    ap.add_argument("--memory", default="icm", help="icm or bm25")
    ap.add_argument("--mode", help="accepted for the entrypoint's sake; must be `recall`")
    ap.add_argument("--name", help="run name = output directory (default: the provider name)")
    ap.add_argument("--output-dir", default="outputs")
    ap.add_argument("--description")
    ap.add_argument("--category")
    ap.add_argument("--query-limit", type=int)
    ap.add_argument("--skip-ingested", action="store_true", help="resume: skip the units complete in the result file")
    ap.add_argument("--unit", default=os.environ.get("ICM_AMB_RECALL_UNIT") or "session-user", help=" | ".join(UNITS))
    ap.add_argument("--depth", type=int, default=int(os.environ.get("ICM_AMB_RECALL_DEPTH") or 50),
                    help="units asked per question (default 50)")
    ap.add_argument("--bm25-tokenizer", default=os.environ.get("ICM_AMB_BM25_TOKENIZER") or "words",
                    help="words (default) or harness")
    args = ap.parse_args(argv)
    signal.signal(signal.SIGTERM, run_amb._on_sigterm)
    run(args)


if __name__ == "__main__":
    main()
