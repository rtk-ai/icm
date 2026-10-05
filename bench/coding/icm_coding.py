"""ICM as a coding-agent memory on sdebench, inside the Agent Memory Benchmark runner.

Two pieces, registered by run_sdebench.py:

- `IcmCodingProvider` (`--memory icm-coding`): the counterpart of upstream's
  `hindsight-coding` provider. Ingestion builds ONE ICM database per task from
  the task's corpus, with ICM's own `icm import` (no LLM), in the order
  icm_corpus.py defines. Retrieval is a no-op: memory reaches the agent inside
  its container, through the hooks and the MCP server that `icm init` installs
  (icm_sde_run.py).
- `IcmCodingMode` (`--mode coding`): upstream's coding mode, plus the dispatch of
  `icm-coding` to the `icm` arm. Every other provider goes through upstream's code
  unchanged, so `--memory vanilla` run from this launcher is upstream's own arm.
  A paid task whose agent never triggered ICM's hooks stops the run: it is not
  written as a memory row.

Isolation is per task, as the dataset declares (`isolation_unit = "task"`) and as
the reference arm does (one Hindsight bank per task).

Environment (all optional):

    ICM_SDE_SEED          `corpus` (default): import the task's corpus.
                          `empty`: no seed; measures the hooks and instructions
                          alone, i.e. what ICM costs when it knows nothing.
    ICM_SDE_GIT_INGEST    `messages` (default): the corpus' commit messages are
                          imported as text. `none`: conversations only. ICM has no
                          git-history ingestion of its own; see README.md.
    ICM_SDE_ORDER_SEED    key of the import order (default `icm-sde-1`), recorded
                          in seed.json; see icm_corpus.py.
    ICM_SDE_IMPORT        `default`: `icm import` as ICM ships it (sentences scored
                          and stored with the local embedding model; minutes per
                          task). `rules`: `icm import --no-embeddings` (keyword
                          rules; seconds per task). Recorded in seed.json.
    ICM_SDE_PROJECT       project name the seed is stored under. Default
                          `boltons`: what ICM derives inside the container from
                          the task repo's `origin` remote.
    ICM_SDE_SEED_IMAGE    image that runs the seeding (default: the agent image
                          of SDE_AGENT), so the seed is written by the very binary
                          the agent will use.
    ICM_SDE_DRY_RUN       1: every task goes through `icm_sde_run.py --dry-run`
                          (no agent, no model, no key).
    SDE_AGENT, SDE_MODEL  as upstream. SDE_CONCURRENCY has no effect on sdebench:
                          the runner handles one task at a time (one query per
                          isolation unit). See README.md for parallel shards.
"""

from __future__ import annotations

import asyncio
import json
import os
import re
import shutil
import subprocess
import sys
import time
import uuid
from pathlib import Path

from memory_bench.memory.base import MemoryProvider
from memory_bench.models import AnswerResult, Document
from memory_bench.modes.coding import CodingMode

import icm_corpus
import icm_sde_run

_HERE = Path(__file__).resolve().parent
_RUNNER = _HERE / "icm_sde_run.py"
_SEED_SH = _HERE / "container" / "seed.sh"
PROVIDER_NAME = "icm-coding"
# Fields of the result's `icm` block copied into the row's `reasoning`, as
# `key=value`: the runner keeps that string, sdebench_stats.py reads it back.
REPORTED = ("dry_run", "hook_fired", "session_start_fired", "seed_task_facts",
            "start_task_lines", "prompt_task_lines", "stored_during_task")
_SAFE = re.compile(r"[^A-Za-z0-9._-]+")


def _safe(name: str) -> str:
    return _SAFE.sub("_", name).strip("_") or "doc"


def seed_mode() -> str:
    mode = (os.environ.get("ICM_SDE_SEED") or "corpus").strip().lower()
    if mode not in ("corpus", "empty"):
        raise RuntimeError(f"ICM_SDE_SEED must be corpus or empty, got {mode!r}")
    return mode


def git_ingest() -> str:
    mode = (os.environ.get("ICM_SDE_GIT_INGEST") or "messages").strip().lower()
    if mode not in ("messages", "none"):
        raise RuntimeError(f"ICM_SDE_GIT_INGEST must be messages or none, got {mode!r}")
    return mode


def import_mode() -> str:
    mode = (os.environ.get("ICM_SDE_IMPORT") or "default").strip().lower()
    if mode not in ("default", "rules"):
        raise RuntimeError(f"ICM_SDE_IMPORT must be default or rules, got {mode!r}")
    return mode


def agent_image(agent: str | None = None) -> str:
    agent = agent or os.environ.get("SDE_AGENT", "opencode")
    if agent not in icm_sde_run.IMAGES:
        raise RuntimeError(f"unknown SDE_AGENT {agent!r}")
    env_name, default = icm_sde_run.IMAGES[agent]
    return os.environ.get("ICM_SDE_SEED_IMAGE") or os.environ.get(env_name) or default


class IcmCodingProvider(MemoryProvider):
    name = PROVIDER_NAME
    description = ("ICM installed in the coding agent by `icm init` (hooks + MCP); "
                   "one database per task, seeded by `icm import`, no LLM.")
    kind = "local"
    provider = "icm"
    variant = "coding-hooks-mcp"
    link = "https://github.com/rtk-ai/icm"
    concurrency = 1         # the runner handles one sdebench task at a time anyway

    def __init__(self) -> None:
        self._store: Path | None = None
        self._skip = False

    # -- lifecycle ------------------------------------------------------------
    def initialize(self) -> None:
        seed_mode()
        git_ingest()
        import_mode()
        for script in (_RUNNER, _SEED_SH):
            if not script.is_file():
                raise RuntimeError(f"missing {script}")
        if seed_mode() == "corpus":
            image = agent_image()
            found = subprocess.run(["docker", "image", "inspect", image],
                                   capture_output=True, text=True)
            if found.returncode != 0:
                raise RuntimeError(
                    f"agent image {image!r} not found: build it from "
                    f"bench/coding/Dockerfile.agent-icm (see bench/coding/README.md)")

    def prepare(self, store_dir: Path, unit_ids: set[str] | None = None, reset: bool = True) -> None:
        self._store = Path(store_dir) / "icm-coding"
        self._skip = not reset                      # --skip-ingestion: reuse the seeded databases
        os.environ["ICM_SDE_STORE"] = str(self._store)   # read by IcmCodingMode
        if reset:
            for unit in unit_ids or set():
                shutil.rmtree(self.unit_dir(unit), ignore_errors=True)
        self._store.mkdir(parents=True, exist_ok=True)

    def unit_dir(self, task_id: str) -> Path:
        if self._store is None:
            raise RuntimeError("prepare() was not called")
        return self._store / _safe(task_id)

    # -- ingestion ------------------------------------------------------------
    def ingest(self, documents: list[Document]) -> None:
        asyncio.run(self.async_ingest(documents))

    async def async_ingest(self, documents: list[Document]) -> None:
        if not documents:
            return
        task_id = documents[0].user_id
        if not task_id:
            return
        if any(d.user_id != task_id for d in documents):
            raise RuntimeError("icm-coding ingests one task at a time (isolation_unit = task)")
        unit = self.unit_dir(task_id)
        db = unit / "memories.db"
        if self._skip and db.is_file():
            return
        shutil.rmtree(unit, ignore_errors=True)
        unit.mkdir(parents=True)
        if seed_mode() == "empty":
            (unit / "seed.json").write_text(json.dumps({"task_id": task_id, "seed": "empty"}))
            return
        counts = icm_corpus.write_corpus(documents, unit / "corpus",
                                         commits=git_ingest() == "messages")
        t0 = time.perf_counter()
        await asyncio.to_thread(self._seed, unit)
        report = (unit / "seed-report.txt").read_text() if (unit / "seed-report.txt").is_file() else ""
        sections = icm_sde_run.parse_sections(report)
        # A memory arm whose database is missing measures no memory at all.
        if not db.is_file() or sections.get("ok") != "1":
            raise RuntimeError(f"seeding failed for {task_id}: {report[-400:]}")
        facts = icm_corpus.exported_task_facts(unit / "seed-export.jsonl",
                                               icm_corpus.document_texts(documents))
        (unit / "seed.json").write_text(json.dumps({
            "task_id": task_id, "seed": "corpus", "git_ingest": git_ingest(),
            "import": sections.get("import_mode") or import_mode(),
            "project": self.project(), "image": agent_image(), "corpus": counts,
            # Stored notes that are passages of the task's own documents; null when
            # the seeding left no export to read.
            "task_facts": None if facts is None else len(facts),
            "seconds": round(time.perf_counter() - t0, 1),
            "icm_version": sections.get("icm_version"), "stats": sections.get("stats"),
        }, indent=2))

    @staticmethod
    def project() -> str:
        return os.environ.get("ICM_SDE_PROJECT") or "boltons"

    def _seed(self, unit: Path) -> None:
        cmd = ["docker", "run", "--rm", "-i",
               "-v", f"{unit / 'corpus'}:/corpus:ro", "-v", f"{unit}:/out",
               "-e", f"ICM_SDE_PROJECT={self.project()}", "-e", f"ICM_SDE_IMPORT={import_mode()}",
               agent_image(), "sh", "-s"]
        done = subprocess.run(cmd, input=_SEED_SH.read_text(), capture_output=True, text=True,
                              errors="replace", timeout=3600)
        if done.returncode != 0:
            raise RuntimeError(f"seed container failed (exit {done.returncode}): "
                               f"{(done.stderr or done.stdout).strip()[-300:]}")

    # -- retrieval: agent-side (hooks + MCP); nothing to serve here -----------
    def retrieve(self, query: str, k: int = 10, user_id: str | None = None,
                 query_timestamp: str | None = None) -> tuple[list[Document], dict | None]:
        return [], None


class IcmCodingMode(CodingMode):
    """Upstream's coding mode, with `icm-coding` dispatched to the `icm` arm."""

    async def async_answer(self, query: str, memory: MemoryProvider, task_type: str = "coding",
                           user_id: str | None = None, meta: dict | None = None) -> AnswerResult:
        if memory.name != PROVIDER_NAME:
            return await super().async_answer(query, memory, task_type=task_type,
                                              user_id=user_id, meta=meta)
        meta = meta or {}
        task_json = meta.get("task_json")
        task_id = user_id or meta.get("task_id") or "task"
        if not task_json:
            raise RuntimeError(f"no task_json for {task_id}")
        run_id = f"icm-{uuid.uuid4().hex[:8]}"
        store = os.environ.get("ICM_SDE_STORE")
        if not store:
            raise RuntimeError("ICM_SDE_STORE is not set: the icm-coding provider was not prepared")
        env = {**os.environ}
        db = Path(store) / _safe(task_id) / "memories.db"
        if seed_mode() == "corpus":
            if not db.is_file():
                raise RuntimeError(f"no seeded database for {task_id} at {db}")
            env["ICM_SDE_DB"] = str(db)
        else:
            env.pop("ICM_SDE_DB", None)
        cmd = [sys.executable, str(_RUNNER), "--task", str(task_json), "--history", icm_sde_run.ARM,
               "--agent", self._agent, "--model", self._model, "--run-id", run_id]
        if os.environ.get("ICM_SDE_DRY_RUN") == "1":
            cmd.append("--dry-run")
        t0 = time.perf_counter()
        proc = await asyncio.to_thread(subprocess.run, cmd, capture_output=True, text=True,
                                       errors="replace", env=env)
        elapsed_ms = (time.perf_counter() - t0) * 1000
        work = Path("/tmp/sdebench/run") / f"{task_id}_{icm_sde_run.ARM}_{run_id}"
        result_path = work / "result.json"
        if not result_path.is_file():
            # Upstream scores a run without result file as "unsolved". For a memory arm that
            # hides a broken setup behind a wrong-looking score, so it stops the run instead.
            raise RuntimeError(f"icm arm produced no result for {task_id} (exit {proc.returncode}): "
                               f"{(proc.stderr or proc.stdout or '')[-600:]}")
        result = json.loads(result_path.read_text())
        trace_path = work / "trace.json"
        if trace_path.is_file():
            trace = json.loads(trace_path.read_text())
            flat: list = []
            for i, rnd in enumerate(trace.get("trace") or []):
                if i:
                    flat.append({"k": "say", "text": "[feedback] " + (rnd.get("prompt") or "")[:400]})
                flat.extend(rnd.get("trajectory") or [])
            result["trajectory"] = flat
            result["git_history"] = trace.get("git_history")
            result["final_patch"] = trace.get("final_patch")
        icm = result.get("icm") or {}
        dry_run = os.environ.get("ICM_SDE_DRY_RUN") == "1"
        if not dry_run and icm.get("memory_run") is not True:
            # The agent's own hook invocations left no row in ICM's telemetry: it ran
            # without ICM's recall. Written as a row, it would be read as a memory run.
            raise RuntimeError(
                f"{task_id}: the agent ran without ICM's hooks (hook_prompt_rows="
                f"{icm.get('hook_prompt_rows')}, exit {proc.returncode}); not a memory run, "
                f"nothing is scored. Result kept at {result_path}")
        blocks = []
        for event, title in (("icm_probe_start", "SessionStart hook"),
                             ("icm_probe_prompt", "UserPromptSubmit hook, first prompt")):
            probe = next((e for e in result.get("memory_diag") or [] if e.get("event") == event), None)
            if probe and probe.get("answer"):
                blocks.append(f"## Memory (ICM {title}; replayed before the agent started)\n"
                              + probe["answer"].strip())
        if blocks:
            result["memory_context"] = "\n\n".join(blocks)
        solved = bool(result.get("solved"))
        return AnswerResult(
            answer="solved" if solved else "unsolved",
            reasoning=(f"arm={icm_sde_run.ARM} interventions={result.get('interventions')} "
                       f"cost=${result.get('cost_usd')} turns={result.get('turns')} "
                       + " ".join(f"{key}={icm.get(key)}" for key in REPORTED)),
            context=f"memory={memory.name} arm={icm_sde_run.ARM}",
            retrieve_time_ms=float(result.get("wall_s", 0.0)) * 1000 or elapsed_ms,
            raw_response=result,
        )
