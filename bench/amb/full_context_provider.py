"""Full-context baseline for the Agent Memory Benchmark (AMB) harness: no retrieval.

`run_amb.py` registers this class as `--memory full-context`. For every question it
hands the answer model EVERY document of the question's isolation unit (LoCoMo: all
the sessions of the conversation), oldest first, whole (no chunking), whatever the
query and whatever `k`. It is the upper reference for "how much of the score is the
reader model": same harness, same answer model, same judge, same prompt as a memory
run, only the retrieval step is replaced by "everything".

What it is given: the `Document` objects the harness passes to `ingest()`, i.e. the
exact list `bm25`, `icm` and every other provider receive for the same unit. It never
sees a `Query` object, a gold answer or a gold document id: `retrieve()` uses its
`user_id` argument and nothing else (the query text is ignored).

What the answer model reads for one document, in the harness' usual rendering
(`## Memory <n>` + content): the same one-line header the ICM provider writes in each
memory, then the document as the dataset serialises it.

    [<document timestamp>] <provenance line of the harness>
    <document content>

Environment variables:

  ICM_AMB_HEADER   0 to hand the documents without the date/provenance line (the
                   upstream `bm25` baseline gives the answer model no date either).
                   Default on, as for the ICM provider, so both show the same dates.

Limits to state when publishing a figure:

* Documents live in the process: `--skip-ingestion` cannot work (the run stops with
  a message), `--skip-ingested` does (a finished unit is not asked again).
* A document without a timestamp goes after the dated ones, in ingestion order.
* A unit may be handed in several `ingest()` calls: the documents add up. A document
  handed again in a later call (same id, date, provenance and text) is not added a
  second time, so replaying a unit never doubles its context.
* The context is as long as the unit: check `avg_context_tokens` of the result file
  against the answer model's window before a paid run (LoCoMo: see README.md).
"""

from __future__ import annotations

import hashlib
import json
import os
import threading
from collections import OrderedDict
from datetime import datetime
from pathlib import Path

from memory_bench.memory.base import MemoryProvider
from memory_bench.models import Document

_SHARED_UNIT = "_shared"


def _env_flag(name: str, default: bool) -> bool:
    raw = os.environ.get(name)
    if raw is None or raw == "":
        return default
    return raw.strip().lower() in ("1", "true", "yes", "on")


def document_text(doc: Document, header: bool = True) -> str:
    """The text of one document as the answer model reads it (same header as ICM memories)."""
    if not header:
        return doc.content
    parts = []
    if doc.timestamp:
        parts.append(f"[{doc.timestamp}]")
    if doc.context:
        parts.append(doc.context)
    return f"{' '.join(parts)}\n{doc.content}" if parts else doc.content


def _digest(doc: Document) -> str:
    payload = json.dumps([doc.id, doc.user_id, doc.timestamp, doc.context, doc.content], ensure_ascii=False)
    return hashlib.sha256(payload.encode()).hexdigest()


def _chrono_key(indexed: tuple[int, Document]) -> tuple:
    index, doc = indexed
    if not doc.timestamp:
        return (1, 0.0, index)
    try:
        when = datetime.fromisoformat(doc.timestamp.replace("Z", "+00:00"))
        stamp = when.timestamp()
    except ValueError:
        return (1, 0.0, index)  # unreadable date: with the undated ones, in ingestion order
    return (0, stamp, index)


class FullContextMemoryProvider(MemoryProvider):
    name = "full-context"
    description = ("No retrieval: every document of the question's isolation unit, whole, oldest first, "
                   "with its date and provenance line. Baseline that isolates the answer model.")
    kind = "local"
    link = "https://github.com/rtk-ai/icm/tree/main/bench/amb"
    logo = None
    concurrency = 4

    def __init__(self) -> None:
        self._header = _env_flag("ICM_AMB_HEADER", True)
        if not self._header:
            self.description = self.description.replace("with its date and provenance line", "without any date line")
        self._units: dict[str, list[Document]] = {}
        self._held: dict[str, set[str]] = {}  # unit -> digests of the documents of earlier ingest() calls
        self._lock = threading.Lock()
        self._store_dir: Path | None = None
        self._stats = {"units": 0, "docs": 0, "retrieves": 0, "docs_returned": 0}

    def prepare(self, store_dir: Path, unit_ids: set[str] | None = None, reset: bool = True) -> None:
        self._store_dir = Path(store_dir).resolve() / "full-context"
        if not reset:
            raise RuntimeError(
                "full-context keeps the documents in memory: --skip-ingestion cannot reuse a previous "
                "run. Run without it (ingestion costs nothing here); --skip-ingested works."
            )
        with self._lock:
            self._units = {}
            self._held = {}

    def ingest(self, documents: list[Document]) -> None:
        by_unit: OrderedDict[str, list[Document]] = OrderedDict()
        for doc in documents:
            by_unit.setdefault(doc.user_id or _SHARED_UNIT, []).append(doc)
        with self._lock:
            for unit, docs in by_unit.items():
                # A unit can arrive in several calls: what is new is added, never
                # replaced, so no document disappears. What an earlier call already
                # handed is not added again, so none is counted twice. The documents
                # of one call are all kept, including two that are identical.
                earlier = self._held.setdefault(unit, set())
                fresh = [doc for doc in docs if _digest(doc) not in earlier]
                self._units.setdefault(unit, []).extend(fresh)
                earlier.update(_digest(doc) for doc in docs)
            self._stats["units"] = len(self._units)
            self._stats["docs"] = sum(len(docs) for docs in self._units.values())

    def retrieve(self, query: str, k: int | None = None, user_id: str | None = None,
                 query_timestamp: str | None = None, filters: dict | None = None) -> tuple[list[Document], dict | None]:
        unit = user_id or _SHARED_UNIT
        with self._lock:
            docs = self._units.get(unit)
        if not docs:
            # An empty context would be scored "wrong" without a word and read as a reader failure.
            raise RuntimeError(f"full-context: no document was ingested for unit {unit!r}")
        ordered = [doc for _, doc in sorted(enumerate(docs), key=_chrono_key)]
        out = [Document(id=doc.id, content=document_text(doc, self._header), user_id=doc.user_id,
                        timestamp=doc.timestamp) for doc in ordered]
        with self._lock:
            self._stats["retrieves"] += 1
            self._stats["docs_returned"] += len(out)
        # raw_response stays None: LoCoMo and LongMemEval would otherwise send
        # `json.dumps(raw_response)` to the answer model instead of the rendered documents.
        return out, None

    def cleanup(self) -> None:
        if self._store_dir is None:
            return
        self._store_dir.mkdir(parents=True, exist_ok=True)
        (self._store_dir / "full-context-stats.json").write_text(json.dumps(dict(self._stats, header=self._header), indent=2))
