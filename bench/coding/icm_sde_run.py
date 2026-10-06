#!/usr/bin/env python3
"""ICM arm for sdebench: one task, one coding agent, ICM installed by `icm init`.

sdebench is the coding-agent benchmark of the Agent Memory Benchmark
(https://github.com/vectorize-io/agent-memory-benchmark, sdebench/harness/run.py).
Upstream declares no license, so nothing of it is copied here: this script loads
the upstream harness from a checkout (AMB_HOME) and adds one arm, `--history icm`,
by wrapping four of its functions. Building the repo, the prompt, the correction
loop, the grading and the result file all stay upstream's.

What the `icm` arm does, next to upstream's reference memory arm (`hscoding`):

    reference arm (Hindsight plugin)          this arm (ICM)
    full repo, git history present            same
    past chats NOT seeded on disk             same
    memory store filled before the task       same (ICM_SDE_DB, built by `icm import`)
    plugin's UserPromptSubmit hook wired      `icm init --mode all` inside the
      by the harness                            container: hooks + MCP + instructions
    plugin write-back disabled                ICM's own write hooks stay on (PostToolUse,
                                                PreCompact, SessionEnd): what a turn
                                                stores can be recalled in the next
                                                correction round of the same task. The
                                                database is copied INTO the container
                                                and discarded with it: nothing crosses
                                                tasks, the seed is never modified.
                                                Their extractor is ICM's local one
                                                (ICM_SDE_EXTRACTION=none): with ICM's
                                                default, SessionEnd would call the
                                                agent's CLI, a model call of its own
    reflect diagnostics read at the end       both injecting hooks are replayed before
                                                the agent starts (SessionStart, then
                                                UserPromptSubmit on the first prompt),
                                                ICM's hook telemetry is read at the end
                                                (did they fire, what was written)

What reaches the agent must come from retrieval, not from the order the corpus
was imported in (icm_corpus.py). The SessionStart pack selects by insertion
order, so its replay is checked against the task's own documents: a paid run
refuses a task whose pack carries one of their lines.

Usage (same flags as upstream run.py, `--history icm` is the default here):

    python icm_sde_run.py --task <task.json> --agent claude-code [--model M] [--run-id R]
    python icm_sde_run.py --task <task.json> --agent claude-code --dry-run

`--dry-run` runs everything except the agent: the repo is built, the container
starts, `icm init` runs in it, the wiring is verified, the MCP server answers a
tools/list, the SessionStart and UserPromptSubmit hooks are replayed (the second
on the real prompt), the tests are graded. No model is called and no API key is
needed: when none is set, a placeholder is passed so that upstream does not
mount a credentials file. A dry run always reports solved=false.

Exit status: 0, or 3 when a paid run finished but ICM's telemetry holds no
UserPromptSubmit row from the agent (the result file says `memory_run: false`).

Environment:

    AMB_HOME                  agent-memory-benchmark checkout, with the sde-bench
                              submodule at sdebench/datasets (required)
    SDEBENCH_BOLTONS_HOST     clone of https://github.com/vectorize-io/boltons
                              (required by every task's build.py)
    ICM_SDE_DB                seeded database for this task (see icm_coding.py);
                              unset = the agent starts with an empty memory
    ICM_SDE_IMAGE_CLAUDE      agent images (defaults icm-sde-agent-claude,
    ICM_SDE_IMAGE_CODEX       icm-sde-agent-codex, icm-sde-agent-opencode),
    ICM_SDE_IMAGE_OPENCODE    built from Dockerfile.agent-icm
    ICM_SDE_INIT_MODE         `icm init --mode` value, default `all`
                              (hooks + MCP + instructions + skills); `standard`
                              is ICM's own default (no MCP)
    ICM_SDE_EXTRACTION        the `extraction.summarizer.provider` the image must
                              run with, default `none` (no LLM). The task stops
                              when `icm config` in the container says otherwise
    ICM_SDE_ALLOW_UNPINNED    1: accept an AMB_HOME that is not at the pinned commit

Agent credentials are read by the upstream harness, not by this script
(ANTHROPIC_API_KEY or CLAUDE_CODE_OAUTH_TOKEN, OPENAI_API_KEY, GEMINI_API_KEY).
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import subprocess
import sys
import types
from pathlib import Path

HERE = Path(__file__).resolve().parent
CONTAINER_DIR = HERE / "container"
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import icm_corpus  # noqa: E402

ARM = "icm"
# Commit of agent-memory-benchmark the seams below were written against
# (the same pin as bench/amb).
PINNED_AMB = "f618ed7b1f0eb9cad7b42e876f91a42f0eadb150"
IMAGES = {
    "claude-code": ("ICM_SDE_IMAGE_CLAUDE", "icm-sde-agent-claude"),
    "codex": ("ICM_SDE_IMAGE_CODEX", "icm-sde-agent-codex"),
    "opencode": ("ICM_SDE_IMAGE_OPENCODE", "icm-sde-agent-opencode"),
}
CONTAINER_DB = "/root/icm/memories.db"
CONTAINER_WORKDIR = "/work"
# Upstream attributes this script relies on. A missing one means upstream moved
# and the wrapper must be re-read against it, not that it can be skipped.
SEAMS = ("main", "start_agent_container", "stop_agent_container", "run_agent",
         "_AGENT_IMAGES", "build_repo", "grade", "PROMPT", "argparse", "subprocess")
_ZERO_TOKENS = {"input": 0, "output": 0, "reasoning": 0, "cache_read": 0, "cache_write": 0}
# Passed to upstream in a dry run when no Claude credential is set: upstream then
# skips its read-write mount of ~/.sdebench/claude_creds.json. No agent starts.
DRY_RUN_KEY = "dry-run-no-agent-is-started"
_CLAUDE_KEYS = ("CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_API_KEY")
_MEMORIES = re.compile(r"Memories:\s*(\d+)")
HOOK_QUERY_BYTES = 200      # ICM's UserPromptSubmit hook searches on the head of the prompt
EXIT_NO_HOOK = 3


class WiringError(RuntimeError):
    """The ICM arm could not be set up. Never scored as a memory run."""


# ── upstream loading ─────────────────────────────────────────────────────────

def amb_home() -> Path:
    raw = os.environ.get("AMB_HOME")
    if not raw:
        raise SystemExit("AMB_HOME is not set: point it at an agent-memory-benchmark checkout")
    home = Path(raw).expanduser().resolve()
    if not (home / "sdebench" / "harness" / "run.py").is_file():
        raise SystemExit(f"{home} has no sdebench/harness/run.py: not an agent-memory-benchmark checkout")
    return home


def checkout_commit(home: Path) -> str | None:
    """HEAD of the checkout, or None when it cannot be read (not a git checkout)."""
    try:
        out = subprocess.run(["git", "-C", str(home), "rev-parse", "HEAD"],
                             capture_output=True, text=True, timeout=30)
    except (OSError, subprocess.SubprocessError):
        return None
    sha = out.stdout.strip()
    return sha if out.returncode == 0 and len(sha) == 40 else None


def check_pin(home: Path) -> str | None:
    sha = checkout_commit(home)
    if sha != PINNED_AMB and os.environ.get("ICM_SDE_ALLOW_UNPINNED") != "1":
        raise SystemExit(
            f"AMB_HOME is at {sha or 'an unknown commit'}, this wrapper was written against "
            f"{PINNED_AMB}. Check out that commit, or set ICM_SDE_ALLOW_UNPINNED=1 after "
            f"re-reading sdebench/harness/run.py against the wrapper's seams.")
    return sha


def load_upstream(home: Path) -> types.ModuleType:
    path = home / "sdebench" / "harness" / "run.py"
    spec = importlib.util.spec_from_file_location("sdebench_upstream_run", path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    missing = [name for name in SEAMS if not hasattr(module, name)]
    if missing:
        raise SystemExit(f"upstream run.py no longer has {missing}: the ICM arm must be "
                         f"re-read against the new harness before any run")
    return module


# ── docker helpers ───────────────────────────────────────────────────────────

def _docker(*args: str, stdin: str | None = None, timeout: int = 600) -> subprocess.CompletedProcess:
    return subprocess.run(["docker", *args], input=stdin, capture_output=True, text=True,
                          errors="replace", timeout=timeout)


def parse_sections(text: str) -> dict[str, str]:
    """Split the "@@name" sections the container scripts print."""
    sections: dict[str, list[str]] = {}
    current = None
    for line in (text or "").splitlines():
        if line.startswith("@@"):
            current = line[2:].strip()
            sections.setdefault(current, [])
        elif current is not None:
            sections[current].append(line)
    return {name: "\n".join(lines).strip() for name, lines in sections.items()}


def _int(text: str | None) -> int | None:
    try:
        return int((text or "").strip().splitlines()[-1])
    except (ValueError, IndexError):
        return None


def _memories(stats: str | None) -> int | None:
    """Memory count in the output of `icm stats`."""
    found = _MEMORIES.search(stats or "")
    return int(found.group(1)) if found else None


def seed_facts(seed_db: Path | None, task: dict) -> dict:
    """What the seeding left next to the database: the import order (seed.json) and
    how many stored notes come from the task's own documents (seed-export.jsonl)."""
    out = {"order_seed": None, "task_documents": None, "seed_task_facts": None}
    if seed_db is None:
        return out
    try:
        corpus = json.loads((seed_db.parent / "seed.json").read_text()).get("corpus") or {}
        out["order_seed"] = corpus.get("order_seed")
        out["task_documents"] = corpus.get("task_documents")
    except (OSError, ValueError):
        pass
    facts = icm_corpus.exported_task_facts(seed_db.parent / "seed-export.jsonl",
                                           icm_corpus.task_texts(task))
    out["seed_task_facts"] = None if facts is None else len(facts)
    return out


# ── the arm ──────────────────────────────────────────────────────────────────

class IcmArm:
    """State of one task run and the four wrappers installed on the upstream module."""

    def __init__(self, run: types.ModuleType, *, dry_run: bool, seed_db: Path | None,
                 init_mode: str, amb_commit: str | None, task: dict | None = None,
                 extraction: str = "none"):
        self.run = run
        self.dry_run = dry_run
        self.seed_db = seed_db
        self.init_mode = init_mode
        self.extraction = extraction
        self.amb_commit = amb_commit
        self.task = task or {}
        self.task_texts = icm_corpus.task_texts(self.task)
        self.seed = seed_facts(seed_db, self.task)
        self.events: list[dict] = []
        self.rounds = 0
        self._orig_start = run.start_agent_container
        self._orig_stop = run.stop_agent_container
        self._orig_run_agent = run.run_agent

    # -- installation on the upstream module ---------------------------------
    def install(self) -> None:
        run = self.run

        class _Parser(argparse.ArgumentParser):
            def add_argument(self, *names, **kwargs):
                if names and names[0] == "--history" and kwargs.get("choices") is not None:
                    kwargs["choices"] = [*kwargs["choices"], ARM]
                return super().add_argument(*names, **kwargs)

        # Upstream's parser lists the arms it knows; any other value of --history falls
        # through to its plain branch (full repo, no memory of its own), which is the
        # base this arm needs. Only the module's own view of argparse is replaced.
        run.argparse = types.SimpleNamespace(ArgumentParser=_Parser)
        for agent, (env_name, default) in IMAGES.items():
            run._AGENT_IMAGES[agent] = os.environ.get(env_name) or default
        run.start_agent_container = self.start
        run.stop_agent_container = self.stop
        run.run_agent = self.run_agent

    # -- container start: seed + `icm init` + wiring check --------------------
    def start(self, workdir, env, agent="opencode"):
        if agent == "claude-code" and not any(env.get(k) for k in _CLAUDE_KEYS):
            if self.dry_run:
                env = {**env, "ANTHROPIC_API_KEY": DRY_RUN_KEY}
            else:
                creds = Path(getattr(self.run, "_CLAUDE_CREDS", "") or "/nonexistent")
                if not creds.is_file():
                    raise WiringError(
                        f"no Claude credential: set ANTHROPIC_API_KEY or CLAUDE_CODE_OAUTH_TOKEN "
                        f"(upstream would mount {creds}, which does not exist, and Docker "
                        f"would create a directory there)")
        cid = self._orig_start(workdir, env, agent)
        try:
            self._install_icm(cid, agent)
            self._probe_start(cid)
        except Exception:
            self._orig_stop(cid)
            raise
        return cid

    def _install_icm(self, cid: str, agent: str) -> None:
        made = _docker("exec", cid, "mkdir", "-p", str(Path(CONTAINER_DB).parent))
        if made.returncode != 0:
            raise WiringError(f"cannot prepare the database directory: {made.stderr.strip()[:200]}")
        seeded = False
        if self.seed_db is not None:
            copied = _docker("cp", str(self.seed_db), f"{cid}:{CONTAINER_DB}")
            if copied.returncode != 0:
                raise WiringError(f"cannot copy the seeded database into the container: "
                                  f"{copied.stderr.strip()[:200]}")
            seeded = True
        setup = _docker("exec", "-i", "-e", f"ICM_SDE_INIT_MODE={self.init_mode}",
                        "-e", f"ICM_SDE_EXTRACTION={self.extraction}", cid,
                        "sh", "-s", "--", agent,
                        stdin=(CONTAINER_DIR / "setup.sh").read_text())
        sections = parse_sections(setup.stdout)
        event = {
            "event": "icm_setup",
            "agent": agent,
            "init_mode": self.init_mode,
            "seeded": seeded,
            "icm_version": sections.get("icm_version"),
            "agent_version": sections.get("agent_version"),
            "extraction": sections.get("extraction"),
            "wiring": sections.get("wiring", "").splitlines(),
            "mcp_tools": sections.get("mcp_tools", "").split(),
            "stats": sections.get("stats"),
            "ok": setup.returncode == 0 and sections.get("ok") == "1",
        }
        self.events.append(event)
        if not event["ok"]:
            reason = sections.get("error") or setup.stderr.strip()[-300:] or setup.stdout.strip()[-300:]
            raise WiringError(f"ICM setup failed in the agent container: {reason}")
        if self.init_mode in ("all", "mcp") and not event["mcp_tools"]:
            raise WiringError("the MCP server listed no icm_* tool: this would not be a hooks+MCP run")
        if seeded and "present" not in sections.get("seed", ""):
            raise WiringError("the seeded database is not where ICM_DB points")

    # -- one agent turn -------------------------------------------------------
    def run_agent(self, cid, model, timeout, message, resume=False, agent="opencode",
                  system_append=None):
        self.rounds += 1
        if not resume:
            self._probe_prompt(cid, message)
        if self.dry_run:
            return {"elapsed": 0.0, "tokens": dict(_ZERO_TOKENS), "turns": 0, "cost": 0.0,
                    "trajectory": [{"k": "say", "text": "[dry-run] no agent was started, no model was called"}]}
        return self._orig_run_agent(cid, model, timeout, message, resume=resume, agent=agent,
                                    system_append=system_append)

    # -- what the two injecting hooks emit, replayed without any model ---------
    def _replay(self, cid: str, event: str, payload: dict) -> dict:
        """Run `icm hook <event>` on a copy of the database (container/probe.sh)."""
        put = _docker("exec", "-i", cid, "sh", "-c", f"cat > /tmp/icm-sde-probe-{event}.json",
                      stdin=json.dumps(payload))
        done = _docker("exec", "-i", "-w", CONTAINER_WORKDIR, cid, "sh", "-s", "--", event,
                       stdin=(CONTAINER_DIR / "probe.sh").read_text())
        ok = put.returncode == 0 and done.returncode == 0
        text = done.stdout if ok else ""
        return {
            "ok": ok,
            "exit": done.returncode,
            "chars": len(text),
            "injects": bool(text.strip()),
            # Injected lines that are passages of the task's own documents.
            "task_lines": icm_corpus.task_lines(text, self.task_texts),
            "answer": text,
            "stderr": done.stderr.strip()[-300:] or None,
        }

    def _probe_start(self, cid: str) -> None:
        """SessionStart: the pack selects by insertion order, not by search. It must
        not carry the task's own documents, or the task is answered by its position."""
        start = self._replay(cid, "start", {
            "session_id": "icm-sde-probe", "cwd": CONTAINER_WORKDIR,
            "hook_event_name": "SessionStart", "source": "startup"})
        self.events.append({"event": "icm_probe_start", **start})
        if self.dry_run:
            return
        if not start["ok"]:
            raise WiringError(f"the SessionStart hook could not be replayed (exit {start['exit']}): "
                              f"what it injects is unknown, the task is not run")
        if start["task_lines"]:
            raise WiringError(
                f"the SessionStart pack carries {len(start['task_lines'])} line(s) of the task's own "
                f"documents: it selects by import order, so the task would be answered by position. "
                f"Re-seed with another ICM_SDE_ORDER_SEED. First line: {start['task_lines'][0][:120]!r}")

    def _probe_prompt(self, cid: str, message: str) -> None:
        """UserPromptSubmit on the first prompt: a search, on the head of the prompt."""
        prompt = self._replay(cid, "prompt", {
            "session_id": "icm-sde-probe", "cwd": CONTAINER_WORKDIR,
            "hook_event_name": "UserPromptSubmit", "prompt": message})
        head = message.encode()[:HOOK_QUERY_BYTES].decode(errors="ignore")
        self.events.append({"event": "icm_probe_prompt", **prompt, "query": head})

    # -- container stop: read ICM's telemetry first ---------------------------
    def stop(self, cid):
        if cid:
            try:
                collected = _docker("exec", "-i", cid, "sh", "-s",
                                    stdin=(CONTAINER_DIR / "collect.sh").read_text())
                sections = parse_sections(collected.stdout)
                self.events.append({
                    "event": "icm_collect",
                    "ok": collected.returncode == 0,
                    "hook_prompt_rows": _int(sections.get("hook_prompt_rows")),
                    "hook_start_rows": _int(sections.get("hook_start_rows")),
                    "hook_post_rows": _int(sections.get("hook_post_rows")),
                    "hook_end_rows": _int(sections.get("hook_end_rows")),
                    "hook_stats": sections.get("hook_stats"),
                    "stats": sections.get("stats"),
                    "memories": _memories(sections.get("stats")),
                    "topics": sections.get("topics"),
                })
            except Exception as exc:  # telemetry must never cost the result
                self.events.append({"event": "icm_collect", "ok": False, "error": str(exc)[:300]})
        self._orig_stop(cid)

    # -- result ---------------------------------------------------------------
    def summary(self) -> dict:
        def first(name: str) -> dict:
            return next((e for e in self.events if e["event"] == name), {})

        setup, start, prompt = first("icm_setup"), first("icm_probe_start"), first("icm_probe_prompt")
        collect = next((e for e in reversed(self.events) if e["event"] == "icm_collect"), {})
        fired = collect.get("hook_prompt_rows")
        started = collect.get("hook_start_rows")
        before, after = _memories(setup.get("stats")), collect.get("memories")
        hook_fired = None if fired is None else fired > 0

        def lines(probe: dict) -> int | None:
            return len(probe["task_lines"]) if probe.get("ok") else None

        return {
            "arm": ARM,
            "dry_run": self.dry_run,
            "amb_commit": self.amb_commit,
            "init_mode": self.init_mode,
            "seeded": setup.get("seeded"),
            "icm_version": setup.get("icm_version"),
            "agent_version": setup.get("agent_version"),
            # Who extracts facts in ICM's write hooks; `none` = no model call by ICM.
            "extraction": setup.get("extraction"),
            "mcp_tools": len(setup.get("mcp_tools") or []),
            # The seed: import order, and the stored notes that are passages of the
            # task's own documents (0 = the decision is not in the database at all).
            "order_seed": self.seed["order_seed"],
            "task_documents": self.seed["task_documents"],
            "seed_task_facts": self.seed["seed_task_facts"],
            # What each hook returns, measured by replaying it on a copy of the database
            # before the agent starts. `*_task_lines` counts the injected lines that come
            # from the task's own documents. SessionStart selects by insertion order:
            # its count must be 0. UserPromptSubmit is a search: its count is the recall.
            "start_chars": start.get("chars"),
            "start_task_lines": lines(start),
            "prompt_chars": prompt.get("chars"),
            "prompt_task_lines": lines(prompt),
            "probe_injects": prompt.get("injects"),
            # Whether the agent's own hook invocations reached ICM (telemetry rows written
            # during the agent's turns; the replays run on a copy and write none).
            "hook_prompt_rows": fired,
            "hook_start_rows": started,
            "hook_post_rows": collect.get("hook_post_rows"),
            "rounds": self.rounds,
            "hook_fired": hook_fired,
            "session_start_fired": None if started is None else started > 0,
            # Notes written during the task by ICM's own write hooks or by the agent.
            "stored_during_task": (after - before if before is not None and after is not None
                                   else None),
            # False: the agent ran without ICM's automatic recall. Not a memory run.
            "memory_run": None if self.dry_run else hook_fired is True,
        }

    def annotate(self, work: Path) -> None:
        """Add the ICM diagnostics to upstream's result.json and trace.json."""
        summary = self.summary()
        for name in ("result.json", "trace.json"):
            path = work / name
            if not path.is_file():
                continue
            data = json.loads(path.read_text())
            data["icm"] = summary
            data["memory_diag"] = self.events
            path.write_text(json.dumps(data, indent=2))


# ── entry point ──────────────────────────────────────────────────────────────

def split_args(argv: list[str]) -> tuple[bool, list[str]]:
    """Take --dry-run out, default --history to the ICM arm, pin a dry run to one grade."""
    dry_run = "--dry-run" in argv
    rest = [a for a in argv if a != "--dry-run"]
    if not any(a == "--history" or a.startswith("--history=") for a in rest):
        rest += ["--history", ARM]
    if dry_run:
        cleaned, skip = [], False
        for a in rest:
            if skip:
                skip = False
                continue
            if a == "--max-interventions":
                skip = True
                continue
            if a.startswith("--max-interventions="):
                continue
            cleaned.append(a)
        rest = cleaned + ["--max-interventions", "0"]
    return dry_run, rest


def _arg(argv: list[str], name: str, default: str | None = None) -> str | None:
    for i, a in enumerate(argv):
        if a == name and i + 1 < len(argv):
            return argv[i + 1]
        if a.startswith(name + "="):
            return a.split("=", 1)[1]
    return default


def work_dir(task_path: str, history: str, run_id: str) -> Path:
    """Where upstream writes this run's result (run.py: /tmp/sdebench/run/<task>_<arm>_<id>)."""
    task = json.loads(Path(task_path).read_text())
    return Path("/tmp/sdebench/run") / f"{task['task_id']}_{history}_{run_id}"


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if "-h" in argv or "--help" in argv:
        print(__doc__)
        return 0
    dry_run, rest = split_args(argv)
    home = amb_home()
    amb_commit = check_pin(home)
    run = load_upstream(home)

    history = _arg(rest, "--history", ARM)
    arm = None
    if history == ARM:
        seed = os.environ.get("ICM_SDE_DB")
        seed_db = Path(seed).expanduser().resolve() if seed else None
        if seed_db is not None and not seed_db.is_file():
            raise SystemExit(f"ICM_SDE_DB={seed_db} does not exist: seed the task first (icm_coding.py)")
        task_file = _arg(rest, "--task")
        try:
            task = json.loads(Path(task_file).read_text()) if task_file else {}
        except (OSError, ValueError):
            task = {}                    # upstream reports an unreadable task file itself
        arm = IcmArm(run, dry_run=dry_run, seed_db=seed_db, task=task,
                     init_mode=os.environ.get("ICM_SDE_INIT_MODE") or "all", amb_commit=amb_commit,
                     extraction=os.environ.get("ICM_SDE_EXTRACTION") or "none")
        arm.install()
    elif dry_run:
        raise SystemExit("--dry-run is only defined for the icm arm")

    task_path = _arg(rest, "--task")
    saved_argv = sys.argv
    sys.argv = [str(home / "sdebench" / "harness" / "run.py"), *rest]
    try:
        run.main()
    finally:
        sys.argv = saved_argv
        if arm is not None and task_path:
            arm.annotate(work_dir(task_path, history, _arg(rest, "--run-id", "r1")))
    if arm is not None:
        s = arm.summary()
        print("ICM " + json.dumps(s))
        if s["memory_run"] is False:
            print("ERROR: no UserPromptSubmit row in ICM's hook telemetry: the agent ran "
                  "WITHOUT ICM's automatic recall. This is not a memory run.", file=sys.stderr)
            return EXIT_NO_HOOK
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
