#!/usr/bin/env python3
"""Launcher: run the upstream Agent Memory Benchmark CLI with the ICM provider registered.

The harness is not forked or vendored. This script imports `memory_bench` from a
checkout (AMB_HOME, default ./agent-memory-benchmark next to this file), adds
`icm` and `full-context` to its provider REGISTRY, then delegates to the upstream
typer app, so every upstream command and flag works unchanged:

    ICM_AMB_ENGINE=v2 python run_amb.py run --dataset locomo --split locomo10 --memory icm --query-limit 20

`--memory full-context` is the no-retrieval baseline (full_context_provider.py): every
document of the question's unit, oldest first, to the same answer model and judge.

`--memory icm` needs ICM_AMB_ENGINE (`v2`, `legacy` or `binary-default-no-dates`, see
icm_provider.py): without it the launcher stops before the harness starts. The engine
and the dates it is given are appended to `--description`, so the result file says
which engine answered.

Launcher-only switches (environment variables, all optional):

  AMB_HOME             path to the agent-memory-benchmark checkout
  ICM_AMB_TRACE        JSONL file: one line per retrieve() call of the selected provider
                       (query, user_id, returned document ids, latency). Works for any
                       provider, so ICM and bm25 can be compared on gold-document recall.
  ICM_AMB_SHARD        "i/n": keep only the isolation units of shard i of n
                       (for a Kubernetes Indexed Job; merge with merge_shards.py).
  ICM_AMB_SHARD_BY     how units are dealt to shards. `hash` (default): sha1 of the unit
                       id modulo n; uneven, and some shards can be empty. `rank`: the
                       units of the split in sorted order, dealt round-robin, so n = the
                       number of units gives exactly one unit per shard (LoCoMo: n=10,
                       one conversation per pod). Every pod of a run must use the same.
  ICM_AMB_GCLOUD_AUTH  1: call Gemini on Vertex AI with the active gcloud account's
                       access token instead of Application Default Credentials.
                       Needs GOOGLE_CLOUD_PROJECT (and optionally GOOGLE_CLOUD_LOCATION).
  ICM_AMB_UNIT_CHECKPOINT
                       comma-separated dataset names (e.g. `personamem`) that the harness
                       runs in one batch and saves only at the very end. The launcher gives
                       them the query's `user_id` as isolation unit, so the harness ingests
                       one unit, answers its questions, saves, then moves on, and
                       `--skip-ingested` resumes at the first unfinished unit. Questions,
                       prompts and the documents each question can reach are unchanged for a
                       provider that isolates by `user_id` (ICM does: one database per
                       user_id); what changes is the order of the work, and the retrieve
                       latency, which no longer includes a server start per unit. Off by
                       default: it is a departure from the upstream schedule, to disclose.
  ICM_AMB_USAGE        JSONL file: one line per Gemini response with the token counts the
                       API reports (`usage_metadata`: prompt, output, thinking, cached,
                       total) and the model. The harness keeps none of them, and the
                       thinking tokens are billed as output: this file is the only
                       measure of what a run costs (read it with estimate_cost.py).
                       Counts only: no prompt and no answer text is written.
  ICM_AMB_LLM_TIMEOUT_S, ICM_AMB_LLM_ATTEMPTS, ICM_AMB_LLM_BACKOFF_S
                       network guard on the harness' Gemini client (defaults 300, 4, 5):
                       per-request timeout, and retries of connection-level failures. The
                       upstream client has neither: a dropped connection ends the process
                       and a frozen one blocks it for ever. HTTP 429/5xx stay with the
                       harness' own retry loops. ICM_AMB_LLM_TIMEOUT_S=0 turns the guard off.

Gemini on Vertex AI with ADC needs no switch: set GOOGLE_GENAI_USE_VERTEXAI=true,
GOOGLE_CLOUD_PROJECT and GOOGLE_CLOUD_LOCATION; the launcher then skips the upstream
check that insists on GEMINI_API_KEY.
"""

from __future__ import annotations

import datetime as _dt
import hashlib
import importlib.abc
import importlib.machinery
import json
import os
import signal
import subprocess
import sys
import threading
import time
import types
from pathlib import Path

_HERE = Path(__file__).resolve().parent

# Third-party packages the harness imports at module load for providers we do not
# run (mem0, Hindsight, Qdrant hybrid search, ...). When they are not installed, a
# placeholder module is served so `import memory_bench` succeeds with the light
# requirements.txt; touching a placeholder at run time raises a clear error.
_OPTIONAL_PACKAGES = (
    "mem0", "cognee", "qdrant_client", "sentence_transformers", "fastembed", "supermemory",
    "hindsight", "hindsight_client", "hindsight_client_api", "hindsight_api", "groq", "openai",
    # Only BEAM scoring (scipy) and the BEAM / PersonaMem loaders (datasets) need these;
    # requirements.txt installs them, a minimal local install may leave them out.
    "scipy", "datasets",
)


class _Missing:
    def __init__(self, name: str):
        self._name = name

    def __call__(self, *args, **kwargs):
        raise ModuleNotFoundError(
            f"{self._name} is not installed (light install). Install the harness' full "
            f"dependencies (`uv sync` in AMB_HOME) to use this provider."
        )

    def __getattr__(self, item: str):
        if item.startswith("__"):
            raise AttributeError(item)
        return _Missing(f"{self._name}.{item}")


class _PlaceholderModule(types.ModuleType):
    __path__: list = []  # behave as a package so submodule imports resolve too

    def __getattr__(self, item: str):
        if item.startswith("__"):
            raise AttributeError(item)
        return _Missing(f"{self.__name__}.{item}")


class _PlaceholderFinder(importlib.abc.MetaPathFinder, importlib.abc.Loader):
    """Last-resort finder: only consulted when the real package is absent."""

    def find_spec(self, fullname, path=None, target=None):
        if fullname.split(".")[0] in _OPTIONAL_PACKAGES:
            return importlib.machinery.ModuleSpec(fullname, self, is_package=True)
        return None

    def create_module(self, spec):
        return _PlaceholderModule(spec.name)

    def exec_module(self, module):
        return None


def _amb_home() -> Path:
    home = Path(os.environ.get("AMB_HOME") or _HERE / "agent-memory-benchmark").resolve()
    if not (home / "src" / "memory_bench").is_dir():
        sys.exit(
            f"AMB checkout not found at {home} (expected src/memory_bench). "
            f"Clone https://github.com/vectorize-io/agent-memory-benchmark and set AMB_HOME."
        )
    return home


def _vertex_enabled() -> bool:
    return os.environ.get("GOOGLE_GENAI_USE_VERTEXAI", "").strip().lower() in ("1", "true")


def _env_float(name: str, default: float) -> float:
    raw = os.environ.get(name)
    return float(raw) if raw else default


def _install_llm_guard() -> None:
    """Give the harness' Gemini client a request timeout and network retries.

    Upstream builds `genai.Client()` bare: no timeout (a frozen connection blocks
    the run for ever) and no retry of transport failures (one reset connection
    ends the process, and the answers of the unit in progress are paid again).
    Two layers that do not multiply each other:

    * the SDK's own options: `timeout` per request, and `retry_options` for what
      the SDK classifies as transient at connection level (timeouts, failed
      connects) plus HTTP 408 only, so 429/5xx keep the harness' retry loops;
    * a wrapper around `GeminiLLM._generate_raw` for the transport failures the
      SDK does not retry (reset or closed connection, token refresh transport).

    Prompts, models and generation settings are not touched."""
    timeout_s = _env_float("ICM_AMB_LLM_TIMEOUT_S", 300.0)
    if timeout_s <= 0:
        return
    attempts = max(1, int(_env_float("ICM_AMB_LLM_ATTEMPTS", 4)))
    backoff_s = _env_float("ICM_AMB_LLM_BACKOFF_S", 5.0)

    import google.auth.exceptions
    import httpx
    from google.genai import types
    import memory_bench.llm.gemini as gemini_mod

    inner = gemini_mod.genai
    sdk_retried = (httpx.TimeoutException, httpx.ConnectError)

    class _GenaiProxy:
        def __getattr__(self, item):
            return getattr(inner, item)

        @staticmethod
        def Client(*args, **kwargs):
            kwargs.setdefault("http_options", types.HttpOptions(
                timeout=int(timeout_s * 1000),
                retry_options=types.HttpRetryOptions(
                    attempts=attempts, initial_delay=backoff_s, max_delay=60.0,
                    http_status_codes=[408],  # an empty list would mean the SDK default (429, 5xx too)
                ),
            ))
            return inner.Client(*args, **kwargs)

    gemini_mod.genai = _GenaiProxy()

    original = gemini_mod.GeminiLLM._generate_raw

    def _generate_raw(self, contents, config=None):
        delay = backoff_s
        for attempt in range(attempts):
            try:
                return original(self, contents, config=config)
            except (httpx.TransportError, google.auth.exceptions.TransportError) as e:
                if isinstance(e, sdk_retried) or attempt == attempts - 1:
                    raise  # already retried by the SDK, or out of attempts
                print(f"[run_amb] Gemini transport error ({type(e).__name__}: {e}); "
                      f"retry {attempt + 1}/{attempts - 1} in {delay:.0f}s", file=sys.stderr, flush=True)
                time.sleep(delay)
                delay = min(delay * 2, 60.0)

    gemini_mod.GeminiLLM._generate_raw = _generate_raw


_USAGE_FIELDS = {  # usage_metadata attribute -> key in the usage file
    "prompt_token_count": "prompt_tokens",
    "candidates_token_count": "output_tokens",
    "thoughts_token_count": "thinking_tokens",
    "cached_content_token_count": "cached_tokens",
    "total_token_count": "total_tokens",
}


def usage_record(model: str, response) -> dict:
    """The token counts of one response as the API reports them; a missing count is 0."""
    usage = getattr(response, "usage_metadata", None)
    record = {"model": model, "reported": usage is not None}
    for attr, key in _USAGE_FIELDS.items():
        record[key] = int(getattr(usage, attr, None) or 0)
    return record


def _install_usage_log(path: str) -> None:
    """Append the API's own token counts of every Gemini response to a JSONL file.

    Wraps `GeminiLLM._generate_raw` outermost, so a response is logged once, after
    the retries of the layers below. A response the harness then fails to parse and
    asks again is a second line: it was billed too."""
    import memory_bench.llm.gemini as gemini_mod

    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    lock = threading.Lock()
    inner = gemini_mod.GeminiLLM._generate_raw

    def _generate_raw(self, contents, config=None):
        response = inner(self, contents, config=config)
        try:
            line = json.dumps(usage_record(getattr(self, "_model", "unknown"), response))
            with lock:
                with open(out, "a") as fh:
                    fh.write(line + "\n")
        except Exception as e:  # noqa: BLE001 - accounting must never cost an answer
            print(f"[run_amb] usage log: {type(e).__name__}: {e}", file=sys.stderr, flush=True)
        return response

    gemini_mod.GeminiLLM._generate_raw = _generate_raw


def _install_gcloud_auth() -> None:
    """Route the harness' `genai.Client()` to Vertex AI with the gcloud account token."""
    import google.auth.credentials
    import memory_bench.llm.gemini as gemini_mod

    genai = gemini_mod.genai  # the module, or the proxy of _install_llm_guard: both compose

    project = os.environ.get("GOOGLE_CLOUD_PROJECT")
    if not project:
        sys.exit("ICM_AMB_GCLOUD_AUTH=1 needs GOOGLE_CLOUD_PROJECT")
    location = os.environ.get("GOOGLE_CLOUD_LOCATION", "global")

    class GcloudCredentials(google.auth.credentials.Credentials):
        def __init__(self):
            super().__init__()
            self._refresh_lock = threading.Lock()

        def refresh(self, request):
            with self._refresh_lock:
                out = subprocess.run(["gcloud", "auth", "print-access-token"],
                                     capture_output=True, text=True, timeout=60)
                if out.returncode != 0 or not out.stdout.strip():
                    raise RuntimeError("`gcloud auth print-access-token` failed; run `gcloud auth login`")
                self.token = out.stdout.strip()
                # gcloud hands back its cached token; re-ask well before it can expire.
                self.expiry = _dt.datetime.utcnow() + _dt.timedelta(minutes=10)

    credentials = GcloudCredentials()

    class _GenaiProxy:
        def __getattr__(self, item):
            return getattr(genai, item)

        @staticmethod
        def Client(*args, **kwargs):
            kwargs.setdefault("vertexai", True)
            kwargs.setdefault("project", project)
            kwargs.setdefault("location", location)
            kwargs.setdefault("credentials", credentials)
            return genai.Client(*args, **kwargs)

    gemini_mod.genai = _GenaiProxy()


def _install_unit_checkpoint(spec: str) -> None:
    """Make the harness save after each `user_id` for datasets it runs in one batch.

    A dataset without `isolation_unit` (PersonaMem) is ingested whole, then every
    question is answered, then the result file is written once: a pod that dies at
    the last question has saved nothing. With an isolation unit the harness goes
    unit by unit and saves after each one. This sets the unit to the query's
    `user_id` and writes that id in `meta["user_id"]`, the field the harness'
    `--skip-ingested` reads to recognise a finished unit.

    The harness' unit path silently drops what does not fit a unit, where the batch
    path would have answered it. Those cases stop the run here instead: a query
    without `user_id`, and a unit that has queries but no document."""
    from memory_bench.dataset import REGISTRY as DATASETS

    for name in [part.strip() for part in spec.split(",") if part.strip()]:
        cls = DATASETS.get(name)
        if cls is None:
            sys.exit(f"ICM_AMB_UNIT_CHECKPOINT={spec!r}: unknown dataset {name!r}")
        if cls.isolation_unit is not None:
            continue  # the harness already checkpoints this one per unit

        def wrap(cls=cls, name=name):
            orig_queries, orig_docs = cls.load_queries, cls.load_documents

            def load_queries(self, split, category=None, limit=None):
                queries = orig_queries(self, split, category=category, limit=limit)
                for q in queries:
                    if not q.user_id:
                        sys.exit(f"ICM_AMB_UNIT_CHECKPOINT: {name} query {q.id} has no user_id; "
                                 f"the unit-sequential path would drop it")
                    # The harness reads sample_id, then user_id, then conversation_id.
                    for key in ("sample_id", "user_id", "conversation_id"):
                        if q.meta.get(key) not in (None, q.user_id):
                            sys.exit(f"ICM_AMB_UNIT_CHECKPOINT: {name} query {q.id} has meta[{key!r}]="
                                     f"{q.meta[key]!r}, not its user_id; resume would misfile it")
                    q.meta["user_id"] = q.user_id
                self._icm_amb_query_units = {q.user_id for q in queries}
                return queries

            def load_documents(self, split, category=None, limit=None, ids=None, user_ids=None):
                docs = orig_docs(self, split, category=category, limit=limit, ids=ids, user_ids=user_ids)
                if limit is None and ids is None:  # a deliberate partial load is the caller's business
                    wanted = getattr(self, "_icm_amb_query_units", set())
                    if user_ids is not None:
                        wanted = wanted & set(user_ids)
                    missing = wanted - {d.user_id for d in docs}
                    if missing:
                        sys.exit(f"ICM_AMB_UNIT_CHECKPOINT: {name} units {sorted(missing)[:5]} have queries "
                                 f"but no document; the unit-sequential path would drop their queries")
                return docs

            cls.load_queries, cls.load_documents = load_queries, load_documents
            cls.isolation_unit = "user_id"
        wrap()
        print(f"[run_amb] {name}: one checkpoint per user_id (ICM_AMB_UNIT_CHECKPOINT)", file=sys.stderr, flush=True)


# SIGTERM handling: exit at once, except while the result file is being written.
_term = {"saving": False, "pending": False}


def _on_sigterm(*_) -> None:
    if _term["saving"]:
        _term["pending"] = True
        return
    sys.exit(143)


def _install_save_guard() -> None:
    """Let a result save that is under way finish before SIGTERM ends the process.

    The harness rewrites the whole result file in place. An exit raised in the
    middle of that write leaves a truncated file, and the unit that had just been
    answered (and paid for) is lost."""
    from memory_bench.runner import EvalRunner

    original = EvalRunner._save

    def _save(self, *args, **kwargs):
        _term["saving"] = True
        try:
            return original(self, *args, **kwargs)
        finally:
            _term["saving"] = False
            if _term["pending"]:
                sys.exit(143)

    EvalRunner._save = _save


def _shard_of(key: str, n: int) -> int:
    return int(hashlib.sha1(key.encode()).hexdigest(), 16) % n


def _install_sharding(spec: str, by: str = "hash") -> None:
    """Restrict every dataset to the isolation units of shard i of n."""
    from memory_bench.dataset import REGISTRY as DATASETS

    index, total = (int(x) for x in spec.split("/"))
    if not 0 <= index < total:
        sys.exit(f"ICM_AMB_SHARD={spec!r}: expected i/n with 0 <= i < n")
    if by not in ("hash", "rank"):
        sys.exit(f"ICM_AMB_SHARD_BY={by!r}: expected `hash` or `rank`")

    for cls in set(DATASETS.values()):
        def wrap(cls=cls):
            orig_queries, orig_docs = cls.load_queries, cls.load_documents

            def keeper(self, split):
                """unit -> does it belong to this shard. A document without unit is in every shard."""
                if by == "hash":
                    return lambda unit: unit is None or _shard_of(str(unit), total) == index
                # rank: position of the unit among the units that have a question in the
                # split, whatever the category filter of this particular call.
                cache = self.__dict__.setdefault("_icm_amb_shard_of", {})
                if split not in cache:
                    units = sorted({str(q.user_id or q.id) for q in orig_queries(self, split, category=None, limit=None)})
                    cache[split] = {unit: i % total for i, unit in enumerate(units)}
                return lambda unit: unit is None or cache[split].get(str(unit)) == index

            def load_queries(self, split, category=None, limit=None):
                keep = keeper(self, split)
                queries = [q for q in orig_queries(self, split, category=category, limit=None)
                           if keep(q.user_id or q.id)]
                return queries[:limit] if limit else queries

            def load_documents(self, split, *args, **kwargs):
                keep = keeper(self, split)
                return [d for d in orig_docs(self, split, *args, **kwargs) if keep(d.user_id)]

            cls.load_queries, cls.load_documents = load_queries, load_documents
        wrap()


def _install_trace(path: str, registry: dict) -> None:
    """Log every retrieve() of every provider class to a JSONL file."""
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    lock = threading.Lock()

    for cls in set(registry.values()):
        def wrap(cls=cls):
            original = cls.retrieve

            def retrieve(self, query, *args, **kwargs):
                t0 = time.perf_counter()
                docs, raw = original(self, query, *args, **kwargs)
                user_id = kwargs.get("user_id", args[1] if len(args) > 1 else None)
                line = json.dumps({
                    "provider": getattr(self, "name", cls.__name__), "user_id": user_id, "query": query,
                    "doc_ids": [d.id for d in docs], "ms": round((time.perf_counter() - t0) * 1000, 1),
                })
                with lock:
                    with open(out, "a") as fh:
                        fh.write(line + "\n")
                return docs, raw

            cls.retrieve = retrieve
        wrap()


def bootstrap():
    """Make `memory_bench` importable and return its (patched) CLI module."""
    home = _amb_home()
    sys.path.insert(0, str(home / "src"))
    sys.path.insert(0, str(_HERE))
    sys.meta_path.append(_PlaceholderFinder())

    import memory_bench.memory as memory_pkg
    from full_context_provider import FullContextMemoryProvider
    from icm_provider import IcmMemoryProvider

    memory_pkg.REGISTRY["icm"] = IcmMemoryProvider  # before the CLI builds its help text
    memory_pkg.REGISTRY["full-context"] = FullContextMemoryProvider  # no retrieval: the reader-model baseline

    import memory_bench.cli as cli

    if _vertex_enabled() or os.environ.get("ICM_AMB_GCLOUD_AUTH") == "1":
        os.environ.setdefault("GOOGLE_GENAI_USE_VERTEXAI", "true")
        cli._resolve_gemini_key = lambda: None  # upstream insists on an API key otherwise
    _install_llm_guard()
    if os.environ.get("ICM_AMB_USAGE"):
        _install_usage_log(os.environ["ICM_AMB_USAGE"])
    _install_save_guard()
    if os.environ.get("ICM_AMB_GCLOUD_AUTH") == "1":
        _install_gcloud_auth()
    if os.environ.get("ICM_AMB_UNIT_CHECKPOINT"):
        _install_unit_checkpoint(os.environ["ICM_AMB_UNIT_CHECKPOINT"])
    if os.environ.get("ICM_AMB_SHARD"):
        _install_sharding(os.environ["ICM_AMB_SHARD"], os.environ.get("ICM_AMB_SHARD_BY") or "hash")
    if os.environ.get("ICM_AMB_TRACE"):
        _install_trace(os.environ["ICM_AMB_TRACE"], memory_pkg.REGISTRY)
    return cli


def _option(argv: list[str], *names: str) -> tuple[int | None, str | None]:
    """(index of the value's argument, value) of the last `--name value` or `--name=value` in argv."""
    found: tuple[int | None, str | None] = (None, None)
    for i, arg in enumerate(argv):
        if arg in names and i + 1 < len(argv):
            found = (i + 1, argv[i + 1])
        else:
            for name in names:
                if name.startswith("--") and arg.startswith(name + "="):
                    found = (i, arg[len(name) + 1:])
    return found


def name_engine(argv: list[str], environ=None) -> list[str]:
    """`amb run --memory icm ...` with the engine written in `--description`.

    Stops the process when the run names no engine (ICM_AMB_ENGINE): the result
    file would not say which engine answered. Other commands and other providers
    go through unchanged."""
    if not argv or argv[0] != "run" or _option(argv, "--memory", "-m")[1] != "icm":
        return list(argv)
    from icm_provider import engine_settings

    try:
        settings = engine_settings(environ)
    except RuntimeError as e:
        sys.exit(f"run_amb: {e}")
    out = list(argv)
    index, description = _option(out, "--description", "-d")
    described = settings.describe(description)
    if index is None:
        out += ["--description", described]
    elif out[index].startswith("--description="):
        out[index] = f"--description={described}"
    else:
        out[index] = described
    return out


def main() -> None:
    # Turn SIGTERM (pod eviction, `kill`) into a normal exit so the provider's
    # atexit hook stops its `icm serve` child instead of orphaning it. A result
    # save under way is allowed to finish first (see _install_save_guard).
    signal.signal(signal.SIGTERM, _on_sigterm)
    cli = bootstrap()
    sys.argv[1:] = name_engine(sys.argv[1:])
    cli.app()


if __name__ == "__main__":
    main()
