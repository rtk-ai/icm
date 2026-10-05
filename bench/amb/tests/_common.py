"""Shared pieces of the offline tests of bench/amb. Nothing here calls a model or a cluster.

The tests need a checkout of the harness: set AMB_HOME (see ../README.md). Without it
they are skipped, not failed. Everything else they need is installed by the two
requirements files (../requirements.txt and requirements.txt here): a missing module
is an error, never a skip.
"""
import atexit
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
PY = sys.executable
_DROP = ("ICM_AMB_", "OMB_", "GOOGLE_", "GEMINI_", "LONGMEMEVAL_", "LOCOMO_", "JOB_COMPLETION", "RESULTS_", "RUN_", "SHARDS",
         "MODE", "MEMORY", "DATASET", "SPLIT", "EXTRA_ARGS")


def amb_home() -> Path:
    home = Path(os.environ.get("AMB_HOME") or BENCH / "agent-memory-benchmark")
    if not (home / "src" / "memory_bench").is_dir():
        raise unittest.SkipTest("no harness checkout: set AMB_HOME")
    return home.resolve()


def env(**extra) -> dict:
    """The caller's environment without any benchmark setting, plus AMB_HOME and `extra`."""
    out = {k: v for k, v in os.environ.items() if not k.startswith(_DROP)}
    out.update(AMB_HOME=str(amb_home()), PYTHONUNBUFFERED="1")
    out.update({k: str(v) for k, v in extra.items()})
    return out


def run(*cmd, env_: dict | None = None, check: bool = True, **kw) -> subprocess.CompletedProcess:
    proc = subprocess.run([str(c) for c in cmd], env=env_ or env(), capture_output=True, text=True, **kw)
    if check and proc.returncode != 0:
        raise AssertionError(f"exit {proc.returncode}: {' '.join(map(str, cmd))}\n{(proc.stdout + proc.stderr)[-3000:]}")
    return proc


# Where the tests write. Read once, at import: several test classes clear every
# ICM_AMB_* variable of the process before they start. ICM_AMB_TEST_WORK keeps the
# files for reading afterwards; without it each test process gets a directory of
# its own, removed at exit, so two runs at the same time never share one.
_WORK = os.environ.get("ICM_AMB_TEST_WORK")
_WORK_ROOT: list[Path] = []


def work_root() -> Path:
    if not _WORK_ROOT:
        if _WORK:
            _WORK_ROOT.append(Path(_WORK) / "icm-amb-tests")
        else:
            _WORK_ROOT.append(Path(tempfile.mkdtemp(prefix="icm-amb-tests-")))
            atexit.register(shutil.rmtree, _WORK_ROOT[0], ignore_errors=True)
    return _WORK_ROOT[0]


def workdir(name: str) -> Path:
    root = work_root() / name
    shutil.rmtree(root, ignore_errors=True)
    root.mkdir(parents=True)
    return root


def path_with_python(root: Path) -> str:
    """A PATH whose `python` is this interpreter.

    entrypoint.sh calls `python`, as the image does. A virtualenv has one; a system
    interpreter may only be `python3`. A shim makes the tests run with either."""
    bin_dir = root / "shim-bin"
    bin_dir.mkdir(parents=True, exist_ok=True)
    shim = bin_dir / "python"
    shim.write_text(f"#!/bin/sh\nexec '{PY}' \"$@\"\n")
    shim.chmod(0o755)
    return f"{bin_dir}:{os.environ['PATH']}"


def fake_icm_bin(root: Path) -> Path:
    """An executable named like a binary that runs fake_icm.py with this interpreter."""
    path = root / "icm"
    path.write_text(f"#!/bin/sh\nexec '{PY}' '{HERE / 'fake_icm.py'}' \"$@\"\n")
    path.chmod(0o755)
    return path


def jsonl(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()] if path.exists() else []


# ----------------------------------------------------------- a tiny LongMemEval file

FILLER = "\n".join(f"filler row number {i} about nothing at all" for i in range(2600))  # about 100 kB, no query word


def _session(*turns) -> list[dict]:
    return [{"role": role, "content": text, **({"has_answer": True} if flag else {})} for role, text, flag in turns]


def lme_items() -> list[dict]:
    """Six questions in the LongMemEval-S file format, each one built to exercise one rule."""
    def item(qid, qtype, question, sessions, gold, date="2023/05/30 (Tue) 10:00"):
        return {"question_id": qid, "question_type": qtype, "question": question, "answer": "n/a", "question_date": date,
                "answer_session_ids": gold, "haystack_session_ids": [s for s, _ in sessions],
                "haystack_dates": [f"2023/05/{10 + i:02d} (Mon) 09:{i:02d}" for i in range(len(sessions))],
                "haystack_sessions": [turns for _, turns in sessions]}

    return [
        # qa: the session that matches best has no user turn: indexed by session-all, not by session-user
        item("qa", "single-session-user", "which zebra did alice adopt", [
            ("d1", _session(("user", "weather looks sunny today", False), ("assistant", "indeed", False))),
            ("answer_qa_1", _session(("user", "alice chose to adopt a zebra named stripes", True), ("assistant", "nice", False))),
            ("d2", _session(("user", "pasta recipe with tomato", False), ("assistant", "which pasta", False))),
            ("d3", _session(("assistant", "which zebra did alice adopt zebra zebra", False))),
        ], ["answer_qa_1"]),
        # qb: two expected sessions, one of them shares no word with the question
        item("qb", "multi-session", "list my marathon cities", [
            ("answer_qb_1", _session(("user", "ran the marathon so here are cities to list", True))),
            ("d1", _session(("user", "my cat sleeps", False))),
            ("answer_qb_2", _session(("user", "another race happened near chicago", True))),
        ], ["answer_qb_1", "answer_qb_2"]),
        # qc_abs: abstention question, expected session known to the dataset but no has_answer turn
        item("qc_abs", "single-session-user", "what violin brand did i buy", [
            ("d1", _session(("user", "the garden needs water", False))),
            ("answer_qc_1", _session(("user", "i bought a guitar brand yamaha", False))),
        ], ["answer_qc_1"]),
        # qd: a session over 64 KiB (the words of the question are at its very end), and a session listed twice
        item("qd", "temporal-reasoning", "where is the hidden xylophone", [
            ("dup_1", _session(("user", "where did summer go", False))),
            ("answer_qd_1", _session(("user", FILLER + "\nthe hidden xylophone is in the attic", True))),
            ("dup_1", _session(("user", "where did summer go", False))),
        ], ["answer_qd_1"]),
        item("qe", "knowledge-update", "what colour is my bicycle", [
            ("answer_qe_1", _session(("user", "my bicycle is now painted green colour", True), ("assistant", "what a change", False))),
            ("d1", _session(("user", "bread needs flour", False))),
        ], ["answer_qe_1"]),
        item("qf", "single-session-assistant", "recommend a telescope", [
            ("d1", _session(("user", "nothing relevant here", False))),
            ("answer_qf_1", _session(("user", "please recommend a telescope for beginners", False),
                                     ("assistant", "the dobsonian telescope", True))),
        ], ["answer_qf_1"]),
    ]


def write_lme(root: Path) -> Path:
    path = root / "longmemeval_tiny.json"
    path.write_text(json.dumps(lme_items()))
    return path
