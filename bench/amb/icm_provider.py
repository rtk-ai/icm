"""ICM memory provider for the Agent Memory Benchmark (AMB) harness.

Harness: https://github.com/vectorize-io/agent-memory-benchmark (not vendored here:
the upstream repo declares no license). `run_amb.py` registers this class in the
harness REGISTRY at start-up and then hands over to the upstream CLI.

What is measured: ICM as a user gets it through `icm serve --http` (warm
embedder, default config, hybrid recall), with the recall engine named in
ICM_AMB_ENGINE. The engine is never implicit: the provider refuses to start without
ICM_AMB_ENGINE, because "whatever the binary defaults to" changed between builds
and is neither the baseline nor the full v2 setting (see "Recall engine" below).
No dataset-specific branch, no LLM at ingest, no reranker, no per-dataset tuning.

Design (see README.md for the rationale and the measurements behind each point):

* Driving ICM  - one long-lived `icm serve --http 127.0.0.1:<port>` process per
  isolation unit. A cold CLI call reloads the embedding model on every process;
  the warm server answers a recall in tens of milliseconds.
* Isolation    - one SQLite database per unit (harness `user_id`), under the
  `store_dir` the harness hands to `prepare()`. Never the user's real database:
  `--db` is always passed and always points inside `store_dir`.
* Timestamps   - ICM has no "event date" field on a memory (`created_at` is the
  ingestion time), so the document date and the harness-provided provenance line
  are written as a one-line header in the memory text, like a user would.
* Chunking     - the harness' own `chunk_text` (512 cl100k tokens, no overlap),
  i.e. the same unit as the upstream `bm25` and `hybrid-search` baselines.
* Recall       - POST /recall with `limit=k`; hybrid (FTS5 + vector) unless
  ICM_AMB_NO_EMBEDDINGS=1.

Environment variables:

  ICM_AMB_BIN            path to the `icm` binary to measure (default: `icm` on PATH)
  ICM_AMB_K              memories requested per query (default 50)
  ICM_AMB_CHUNK_TOKENS   chunk size in cl100k tokens (default 512)
  ICM_AMB_TOPIC          topic the chunks are stored under (default `conversations`)
  ICM_AMB_HEADER         0 to store chunks without the date/provenance header
  ICM_AMB_NO_EMBEDDINGS  1 to run `icm serve --no-embeddings` (keyword-only recall)
  ICM_AMB_CONFIG         ICM config.toml to use (default: an empty file, i.e. ICM defaults)
  ICM_AMB_MAX_SERVERS    warm servers kept alive at once (default 1; each holds the model,
                         about 3 GiB). A dataset the harness runs in one batch (PersonaMem)
                         visits each unit once, after all ingestion: every unit then starts
                         a server and loads the model inside the timed retrieve, whatever
                         this value. See "Recall latency" in README.md.
  ICM_AMB_NAME           provider name written in the result file (default `icm`)
  ICM_AMB_HTTP_TRACE     JSONL file: one line per /store and /recall request as it was
                         sent (`path`, `unit`, and the JSON body with `content` replaced
                         by its length). The proof of what a run handed to ICM: which
                         `engine`, whether `created_at` and `now` were there.

Recall engine:

  ICM_AMB_ENGINE         REQUIRED, one of:
                         `v2`      `"engine": "v2"` on /recall, the document date as
                                   `created_at` on /store and the question date as `now`
                                   on /recall (each date can be withheld, see
                                   ICM_AMB_STORE_DATE and ICM_AMB_QUERY_NOW).
                         `legacy`  `"engine": "legacy"` on /recall, no date: the
                                   baseline. A build older than v2 ignores the field and
                                   runs its only engine, so the same value measures the
                                   previous engine on every build.
                         `binary-default-no-dates`
                                   no `engine` field and no date: the engine the binary
                                   picks by itself (v2 since the default changed, the
                                   previous engine before). It is what a bare HTTP client
                                   gets; it is NOT the baseline and NOT the v2 setting
                                   above, and two builds measured this way may not run
                                   the same engine.
                         The value is written in the provider description, in
                         ingest-stats.json and, by the launchers, in the run description
                         of the result file.
  ICM_AMB_MAX_TOKENS     token budget sent as `max_tokens` instead of cutting at k
                         (needs ICM_AMB_ENGINE=v2). Unit: ICM's estimate of what it returns. This
                         provider asks /recall?format=json, where a memory costs
                         ceil(characters of its whole indented JSON row / 4): the summary
                         plus all the other fields, roughly 110 to 140 tokens of envelope
                         per 512-token chunk. The harness counts the summaries with
                         tiktoken, so the context is SMALLER than the budget: simulated
                         ICM-cost / tiktoken is 1.13 on LoCoMo and 1.60 on PersonaMem
                         (a Python replica of the formula, not measured on the binary;
                         max_tokens=32768 gives about 28,900 tokens on LoCoMo). Do not
                         derive the parameter from a target: calibrate it with
                         `compare_builds.py --no-llm` and publish the `avg_context_tokens`
                         of the result file, not this parameter.
  ICM_AMB_STORE_DATE     0 to keep `created_at` out of /store under v2
  ICM_AMB_QUERY_NOW      0 to keep `now` out of /recall under v2. Both at 0 give v2
                         what a system that is handed no date gets: the setting to
                         compare with published figures whose protocol sends the text
                         of the question only. They are refused with another engine,
                         which sends no date anyway.
  ICM_AMB_ORDER          `chrono` returns the recalled memories oldest first instead of
                         in rank order (experiment; rank order is the default, and gold
                         recall at k is only meaningful in rank order)

Before the first request of a run the provider sends an unknown engine name: a
build with the v2 fields answers HTTP 400, an older one ignores the field and
answers 200. With `v2` the run stops on an older build instead of measuring the
previous engine under a v2 label; with the two other values the answer is only
recorded (`engine_field_in_binary` in ingest-stats.json).
"""

from __future__ import annotations

import atexit
import hashlib
import json
import logging
import os
import re
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
from collections import OrderedDict
from dataclasses import dataclass
from pathlib import Path

from memory_bench.memory.base import MemoryProvider
from memory_bench.models import Document
from memory_bench.utils import chunk_text

logger = logging.getLogger("icm_amb")

_SHARED_UNIT = "_shared"
_HTTP_LIMIT_MAX = 100     # the legacy /recall clamps `limit` to 1..=100
_HTTP_LIMIT_MAX_V2 = 500  # the v2 pipeline accepts up to 500

ENGINE_V2, ENGINE_LEGACY, ENGINE_BINARY_DEFAULT = "v2", "legacy", "binary-default-no-dates"
ENGINES = (ENGINE_V2, ENGINE_LEGACY, ENGINE_BINARY_DEFAULT)
_OFF = ("0", "false", "no", "off")


@dataclass(frozen=True)
class EngineSettings:
    """Which recall engine a run names, and which dates it hands to ICM."""

    engine: str
    store_date: bool      # document date sent as `created_at` on /store
    query_now: bool       # question date sent as `now` on /recall
    max_tokens: int | None

    @property
    def label(self) -> str:
        """One line for a run description: enough to tell two runs apart without the launch command."""
        if self.engine == ENGINE_BINARY_DEFAULT:
            return "engine: the binary's own default (no `engine` field sent), no date sent"
        if self.engine == ENGINE_LEGACY:
            return "engine legacy, no date sent"
        if not self.store_date and not self.query_now:
            return "engine v2, no date sent"
        return (f"engine v2, document date {'sent (created_at)' if self.store_date else 'not sent'}, "
                f"question date {'sent (now)' if self.query_now else 'not sent'}")

    def describe(self, description: str | None) -> str:
        """`description` with the engine label appended, once."""
        if description and self.label in description:
            return description
        return f"{description} | {self.label}" if description else self.label


def engine_settings(environ=None) -> EngineSettings:
    """The engine named by ICM_AMB_ENGINE; RuntimeError when it is missing or inconsistent.

    Launchers call this before any work so that a run without an explicit engine
    stops at once, and use `.label` to write the choice in the result file."""
    environ = os.environ if environ is None else environ
    engine = (environ.get("ICM_AMB_ENGINE") or "").strip().lower()
    if engine not in ENGINES:
        got = f"ICM_AMB_ENGINE={engine!r} is not one of them" if engine else "ICM_AMB_ENGINE is not set"
        raise RuntimeError(
            f"{got}. Name the recall engine: `v2` (engine v2, document and question dates sent), `legacy` "
            f"(the previous engine, the baseline; also the value for a build older than v2) or "
            f"`binary-default-no-dates` (no engine field and no date: whatever the binary picks, "
            f"neither the baseline nor the v2 setting)."
        )
    raw_budget = (environ.get("ICM_AMB_MAX_TOKENS") or "").strip()
    max_tokens = int(raw_budget) if raw_budget else None
    if max_tokens is not None and max_tokens <= 0:
        max_tokens = None
    if max_tokens and engine != ENGINE_V2:
        raise RuntimeError(f"ICM_AMB_MAX_TOKENS needs ICM_AMB_ENGINE=v2 (got {engine}): only v2 cuts by a token budget")
    dates = {}
    for name in ("ICM_AMB_STORE_DATE", "ICM_AMB_QUERY_NOW"):
        raw = (environ.get(name) or "").strip().lower()
        if raw and engine != ENGINE_V2:
            raise RuntimeError(f"{name} is only read with ICM_AMB_ENGINE=v2: `{engine}` sends no date")
        dates[name] = engine == ENGINE_V2 and raw not in _OFF
    return EngineSettings(engine, dates["ICM_AMB_STORE_DATE"], dates["ICM_AMB_QUERY_NOW"], max_tokens)


def _env_int(name: str, default: int) -> int:
    raw = os.environ.get(name)
    return int(raw) if raw else default


def _env_flag(name: str, default: bool = False) -> bool:
    raw = os.environ.get(name)
    if raw is None or raw == "":
        return default
    return raw.strip().lower() in ("1", "true", "yes", "on")


def _free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _unit_slug(unit: str) -> str:
    """Filesystem-safe, collision-free name for an isolation unit."""
    safe = re.sub(r"[^A-Za-z0-9._-]+", "_", unit)[:80]
    return f"{safe}-{hashlib.sha1(unit.encode(), usedforsecurity=False).hexdigest()[:8]}"


class _IcmServer:
    """One warm `icm serve --http` process bound to one database file."""

    def __init__(self, binary: str, db_path: Path, config_path: Path, no_embeddings: bool):
        self.db_path = db_path
        self.port = _free_port()
        self._base = f"http://127.0.0.1:{self.port}"
        cmd = [binary, "--db", str(db_path), "serve", "--http", f"127.0.0.1:{self.port}"]
        if no_embeddings:
            cmd.append("--no-embeddings")
        env = dict(os.environ)
        # The benchmark must not read the user's ICM setup: explicit config,
        # explicit --db, default (SQLite) backend.
        env["ICM_CONFIG"] = str(config_path)
        for var in ("ICM_DB", "ICM_DB_BACKEND", "ICM_READONLY", "ICM_NO_EMBEDDINGS"):
            env.pop(var, None)
        self.inflight = 0  # requests currently using this server (guarded by the provider lock)
        self._log = open(db_path.with_suffix(".server.log"), "ab")
        self._proc = subprocess.Popen(
            cmd, env=env, cwd=str(db_path.parent), stdin=subprocess.DEVNULL,
            stdout=self._log, stderr=self._log,
        )
        self._wait_ready()

    def _wait_ready(self, timeout_s: float = 120.0) -> None:
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            if self._proc.poll() is not None:
                raise RuntimeError(
                    f"icm serve exited with code {self._proc.returncode}; see {self._log.name}"
                )
            try:
                with urllib.request.urlopen(f"{self._base}/health", timeout=2) as resp:
                    if resp.status == 200:
                        return
            except (urllib.error.URLError, OSError):
                time.sleep(0.1)
        self.stop()
        raise RuntimeError(f"icm serve did not become healthy within {timeout_s:.0f}s")

    def post(self, path: str, body: dict, timeout_s: float = 600.0) -> str:
        req = urllib.request.Request(
            f"{self._base}{path}?format=json",
            data=json.dumps(body).encode(),
            headers={"Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(req, timeout=timeout_s) as resp:
                return resp.read().decode()
        except urllib.error.HTTPError as e:
            detail = e.read().decode(errors="replace")[:500]
            raise RuntimeError(f"icm {path} failed with HTTP {e.code}: {detail}") from e

    def stop(self) -> None:
        if self._proc.poll() is None:
            self._proc.terminate()
            try:
                self._proc.wait(timeout=20)
            except subprocess.TimeoutExpired:
                self._proc.kill()
                self._proc.wait(timeout=10)
        self._log.close()


class IcmMemoryProvider(MemoryProvider):
    kind = "local"
    link = "https://github.com/rtk-ai/icm"
    logo = None
    # Queries of one unit hit the same warm server concurrently; a server is only
    # swapped out once its in-flight requests are done (see `_acquire`).
    concurrency = 4

    def __init__(self) -> None:
        self._binary = os.environ.get("ICM_AMB_BIN") or shutil.which("icm") or "icm"
        self._k = _env_int("ICM_AMB_K", 50)
        self._chunk_tokens = _env_int("ICM_AMB_CHUNK_TOKENS", 512)
        self._topic = os.environ.get("ICM_AMB_TOPIC", "conversations")
        self._header = _env_flag("ICM_AMB_HEADER", True)
        self._no_embeddings = _env_flag("ICM_AMB_NO_EMBEDDINGS", False)
        self._max_servers = max(1, _env_int("ICM_AMB_MAX_SERVERS", 1))
        self._k_explicit = bool(os.environ.get("ICM_AMB_K"))
        # Before the binary is even looked at: a run that names no engine must not start.
        self.settings = engine_settings()
        self._engine = self.settings.engine
        self._max_tokens = self.settings.max_tokens
        self._v2 = self._engine == ENGINE_V2
        self._store_date = self.settings.store_date
        self._query_now = self.settings.query_now
        self._order = (os.environ.get("ICM_AMB_ORDER") or "rank").strip().lower()
        if self._order not in ("rank", "chrono"):
            raise RuntimeError(f"ICM_AMB_ORDER={self._order!r}: expected `rank` or `chrono`")
        self._engine_field: bool | None = None  # does the binary know `engine`? None = not asked
        self._probed = False
        self._whole_documents = False
        self._unknown_ids_warned = False
        self._http_trace = os.environ.get("ICM_AMB_HTTP_TRACE") or None
        if self._http_trace:
            Path(self._http_trace).parent.mkdir(parents=True, exist_ok=True)
        self.name = os.environ.get("ICM_AMB_NAME", "icm")
        self._version = self._probe_version()
        self.description = self._describe()

        self._store_dir: Path | None = None
        self._config_path: Path | None = None
        self._servers: OrderedDict[str, _IcmServer] = OrderedDict()
        self._maps: dict[str, dict[str, dict]] = {}  # unit -> memory id -> chunk record
        self._lock = threading.Condition(threading.RLock())
        self._stats = {"docs": 0, "chunks": 0, "store_seconds": 0.0, "units": 0,
                       "recalls": 0, "recall_memories": 0, "unknown_memory_ids": 0}
        atexit.register(self._stop_all)

    # ------------------------------------------------------------------ setup

    def _describe(self) -> str:
        mode = "keyword-only (FTS5)" if self._no_embeddings else "hybrid (FTS5 + vector)"
        if self._max_tokens:
            cut = f"Recall cut by a token budget (max_tokens={self._max_tokens}, ICM estimate)."
        else:
            cut = f"Requests top-k={self._k}."
        if self._whole_documents:
            unit = "Each document stored as one memory, text unchanged."
        else:
            unit = (f"Documents chunked into {self._chunk_tokens}-token windows, each stored as one memory "
                    f"{'with its date in the text' if self._header else 'without any date line in the text'}.")
        return (f"ICM {self._version} via `icm serve --http`, default config, {self.settings.label}, {mode} recall. "
                f"One SQLite database per isolation unit. {unit} {cut} No LLM at ingest, no reranker.")

    def store_documents_whole(self) -> None:
        """One document = one memory, text unchanged: no chunking, no header.

        For a caller that hands final units (recall_only.py). The tokenizer is then
        never loaded: a unit does not go through tiktoken to be handed back as is."""
        self._whole_documents = True
        self._header = False
        self.description = self._describe()

    def _probe_version(self) -> str:
        try:
            # `--version` exits during argument parsing, before any store is opened. The
            # explicit --db is belt and braces: no invocation of the binary under test may
            # ever fall back to the user's real database.
            unused_db = Path(tempfile.gettempdir()) / "icm-amb-unused.db"
            env = {k: v for k, v in os.environ.items() if k not in ("ICM_DB", "ICM_DB_BACKEND")}
            out = subprocess.run([self._binary, "--db", str(unused_db), "--version"],
                                 capture_output=True, text=True, timeout=30, env=env)
        except (OSError, subprocess.TimeoutExpired) as e:
            raise RuntimeError(
                f"cannot run ICM binary {self._binary!r} (set ICM_AMB_BIN to the binary to measure): {e}"
            ) from e
        if out.returncode != 0:
            raise RuntimeError(f"`{self._binary} --version` failed: {out.stderr.strip()[:300]}")
        return out.stdout.strip().removeprefix("icm ").strip() or "unknown"

    def prepare(self, store_dir: Path, unit_ids: set[str] | None = None, reset: bool = True) -> None:
        self._stop_all()
        self._store_dir = Path(store_dir).resolve() / "icm"
        if reset and self._store_dir.exists():
            shutil.rmtree(self._store_dir)
        self._store_dir.mkdir(parents=True, exist_ok=True)

        override = os.environ.get("ICM_AMB_CONFIG")
        if override:
            self._config_path = Path(override).resolve()
            if not self._config_path.is_file():
                raise RuntimeError(f"ICM_AMB_CONFIG={override} is not a file")
        else:
            self._config_path = self._store_dir / "icm-config.toml"
            self._config_path.touch()  # empty file = ICM built-in defaults

        self._maps = {}
        if not reset:  # --skip-ingestion: reuse the databases and chunk maps of the previous run
            for map_file in self._store_dir.glob("*.map.jsonl"):
                records = [json.loads(line) for line in map_file.read_text().splitlines() if line.strip()]
                if records:
                    self._maps[records[0]["unit"]] = {r["memory_id"]: r for r in records}

    def _require_store_dir(self) -> Path:
        if self._store_dir is None:
            raise RuntimeError("IcmMemoryProvider.prepare() was not called: no storage directory")
        return self._store_dir

    def _db_path(self, unit: str) -> Path:
        return self._require_store_dir() / f"{_unit_slug(unit)}.db"

    def _acquire(self, unit: str) -> _IcmServer:
        """Return the warm server of `unit` with its in-flight count raised.

        Starts the server if needed, first evicting the least recently used one
        once nothing is in flight on it. Callers must `_release` the server."""
        with self._lock:
            while True:
                server = self._servers.get(unit)
                if server is not None:
                    self._servers.move_to_end(unit)
                    break
                if len(self._servers) < self._max_servers:
                    assert self._config_path is not None
                    db_path = self._db_path(unit)
                    fresh = not db_path.exists()
                    server = _IcmServer(self._binary, db_path, self._config_path, self._no_embeddings)
                    if not self._probed:
                        self._probe_engine_field(server, fresh)
                    self._servers[unit] = server
                    break
                idle = next((u for u, s in self._servers.items() if s.inflight == 0), None)
                if idle is None:
                    self._lock.wait()
                    continue
                self._servers.pop(idle).stop()
            server.inflight += 1
            return server

    def _probe_engine_field(self, server: _IcmServer, fresh_db: bool) -> None:
        """Ask the binary whether it knows the `engine` field, once per run.

        A build with the v2 fields rejects an unknown engine name with HTTP 400
        before any search; an older one drops the unknown field and answers 200.
        Under `v2` an older build stops the run: it would measure its only engine
        under a v2 label. Under the other values the answer is recorded. The older
        build runs a real recall for the probe, so outside `v2` it is only sent to
        a database that was just created (not to one reused by --skip-ingestion)."""
        if not self._v2 and not fresh_db:
            return
        self._probed = True
        try:
            server.post("/recall", {"query": "capability probe", "engine": "__amb_probe__"})
        except RuntimeError as e:
            if "HTTP 400" in str(e):
                self._engine_field = True
                return
            server.stop()
            raise
        self._engine_field = False
        if self._v2:
            server.stop()
            raise RuntimeError(
                f"{self._binary} accepted an unknown `engine` on /recall: this build has no v2 HTTP "
                f"fields. Measure it with ICM_AMB_ENGINE=legacy (its only engine)."
            )
        logger.info("[icm] %s has no `engine` field: %s runs its only engine", self._binary, self._engine)

    def _post(self, server: _IcmServer, path: str, unit: str, body: dict) -> str:
        """POST one request, and write what was sent to ICM_AMB_HTTP_TRACE when it is set."""
        if self._http_trace:
            sent = {k: (len(v) if k == "content" else v) for k, v in body.items()}
            line = json.dumps({"path": path, "unit": unit, "body": sent})
            with self._lock:
                with open(self._http_trace, "a") as fh:
                    fh.write(line + "\n")
        return server.post(path, body)

    def _release(self, server: _IcmServer) -> None:
        with self._lock:
            server.inflight -= 1
            self._lock.notify_all()

    def _stop_all(self) -> None:
        with self._lock:
            for server in self._servers.values():
                server.stop()
            self._servers.clear()

    # ----------------------------------------------------------------- ingest

    def _memory_text(self, doc: Document, chunk: str) -> str:
        if not self._header:
            return chunk
        parts = []
        if doc.timestamp:
            parts.append(f"[{doc.timestamp}]")
        if doc.context:
            parts.append(doc.context)
        return f"{' '.join(parts)}\n{chunk}" if parts else chunk

    def ingest(self, documents: list[Document]) -> None:
        by_unit: OrderedDict[str, list[Document]] = OrderedDict()
        for doc in documents:
            by_unit.setdefault(doc.user_id or _SHARED_UNIT, []).append(doc)

        for unit, docs in by_unit.items():
            server = self._acquire(unit)
            try:
                with self._lock:
                    unit_map = self._maps.setdefault(unit, {})
                map_file = self._db_path(unit).with_suffix(".map.jsonl")
                t0 = time.perf_counter()
                n_chunks = 0
                with open(map_file, "a") as fh:
                    for doc in docs:
                        # A whole document never goes through the tokenizer.
                        chunks = [doc.content] if self._whole_documents else chunk_text(doc.content, self._chunk_tokens)
                        for idx, chunk in enumerate(chunks):
                            if not chunk.strip():
                                continue
                            payload = {"topic": self._topic, "content": self._memory_text(doc, chunk)}
                            if self._store_date and doc.timestamp:
                                payload["created_at"] = doc.timestamp
                            stored = json.loads(self._post(server, "/store", unit, payload))
                            record = {
                                "memory_id": stored[0]["id"], "unit": unit, "doc_id": doc.id,
                                "chunk": idx, "timestamp": doc.timestamp,
                            }
                            unit_map[record["memory_id"]] = record
                            fh.write(json.dumps(record) + "\n")
                            n_chunks += 1
                elapsed = time.perf_counter() - t0
                self._stats["docs"] += len(docs)
                self._stats["chunks"] += n_chunks
                self._stats["store_seconds"] += elapsed
                self._stats["units"] += 1
                logger.info("[icm] unit %s: %d docs -> %d memories in %.1fs", unit, len(docs), n_chunks, elapsed)
            finally:
                self._release(server)

    # --------------------------------------------------------------- retrieve

    async def async_retrieve(self, query: str, k: int | None = None, user_id: str | None = None,
                             query_timestamp: str | None = None, filters: dict | None = None):
        # The base class forwards its own default (k=10) when the mode passes no k;
        # override so the provider's configured k applies, as upstream `hybrid-search` does.
        import asyncio
        return await asyncio.to_thread(self.retrieve, query, k, user_id, query_timestamp)

    def retrieve(self, query: str, k: int | None = None, user_id: str | None = None,
                 query_timestamp: str | None = None, filters: dict | None = None) -> tuple[list[Document], dict | None]:
        unit = user_id or _SHARED_UNIT
        if self._max_tokens and k is None and not self._k_explicit:
            limit = _HTTP_LIMIT_MAX_V2  # the budget cuts; `limit` is only the item ceiling
        else:
            limit = max(1, min(k or self._k, _HTTP_LIMIT_MAX_V2 if self._v2 else _HTTP_LIMIT_MAX))
        request: dict = {"query": query, "limit": limit}
        if self._engine != ENGINE_BINARY_DEFAULT:
            request["engine"] = self._engine
        if self._max_tokens:
            request["max_tokens"] = self._max_tokens
        if self._query_now and query_timestamp:
            request["now"] = query_timestamp
        if not (query or "").strip():
            return [], None
        with self._lock:
            unit_map = self._maps.get(unit)
        if unit_map is None:
            return [], None  # nothing was ever ingested for this unit
        server = self._acquire(unit)
        try:
            body = self._post(server, "/recall", unit, request)
        finally:
            self._release(server)
        try:
            rows = json.loads(body)
        except json.JSONDecodeError:
            return [], None  # ICM answers with a plain "no memories" message when nothing matches
        if not isinstance(rows, list):
            return [], None

        docs = []
        unknown = 0
        for row in rows:
            record = unit_map.get(row["id"])
            if record is None:
                # A memory this run did not store in this unit. Its text still reaches
                # the answer model, but its id is no document id: gold recall counts it
                # as a miss. Counted, so a caller can refuse the run (recall_only.py does).
                unknown += 1
                record = {}
            docs.append(Document(
                id=record.get("doc_id", row["id"]),
                content=row["summary"],
                user_id=user_id,
                timestamp=record.get("timestamp"),
            ))
        if unknown:
            with self._lock:
                self._stats["unknown_memory_ids"] += unknown
                warn, self._unknown_ids_warned = not self._unknown_ids_warned, True
            if warn:
                logger.warning("[icm] unit %s: /recall returned %d memories that were not stored by this run in this "
                               "unit; their ids match no document (total in ingest-stats.json)", unit, unknown)
        if self._order == "chrono":
            # Stable sort: undated memories keep their rank order at the end.
            docs.sort(key=lambda d: (d.timestamp is None, d.timestamp or ""))
        with self._lock:
            self._stats["recalls"] += 1
            self._stats["recall_memories"] += len(docs)
        # raw_response stays None on purpose: some datasets (LongMemEval, LoCoMo) feed
        # `json.dumps(raw_response)` to the answer LLM instead of the rendered memories.
        # Returning None keeps the answer context identical to what `context_tokens` counts.
        return docs, None

    # ---------------------------------------------------------------- cleanup

    def cleanup(self) -> None:
        self._stop_all()
        if self._store_dir is not None:
            stats = dict(self._stats, icm_version=self._version, binary=self._binary, k=self._k,
                         chunk_tokens=self._chunk_tokens, topic=self._topic, header=self._header,
                         whole_documents=self._whole_documents,
                         embeddings=not self._no_embeddings, engine=self._engine,
                         engine_label=self.settings.label, engine_field_in_binary=self._engine_field,
                         max_tokens=self._max_tokens, store_date=self._store_date,
                         query_now=self._query_now, order=self._order)
            (self._store_dir / "ingest-stats.json").write_text(json.dumps(stats, indent=2))
