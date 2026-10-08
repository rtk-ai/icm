#!/usr/bin/env python3
"""k8s/render.sh + k8s/job.yaml + entrypoint.sh for the five LoCoMo runs (full context,
bm25, ICM at 5, 10 and 20) and the recall-only LongMemEval runs, and the rule that a
Job with MEMORY=icm names its recall engine. Offline: nothing talks to a cluster; a
stub stands in for the benchmark process.

    python tests/test_k8s_runs.py
    K8S_JOB_SCHEMA=/path/job-v1.33.0.json python tests/test_k8s_runs.py   # also validates the Job schema

Needs `envsubst` (gettext) on PATH and the modules of tests/requirements.txt. Neither is
optional: without them the tests fail, they are not skipped. The Job schema is the one
optional input (a standalone `job-batch-v1.json` of github.com/yannh/kubernetes-json-schema).
"""
import json
import os
import shutil
import subprocess
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import yaml  # tests/requirements.txt: a missing module is an error here, not four skipped tests

from _common import BENCH, path_with_python, workdir

IMAGE = "europe-west9-docker.pkg.dev/rtk-ai-labs-01/rtk-bench/icm-amb@sha256:" + "ab" * 32
BASE = dict(IMAGE=IMAGE, RESULTS_BUCKET="some-bucket", DATASET="locomo", SPLIT="locomo10", SHARDS="10", PARALLELISM="3",
            ICM_AMB_SHARD_BY="rank")
LOCOMO_RUNS = {
    "full-context": dict(RUN_ID="s4-full", MEMORY="full-context", RUN_NAME="full-context"),
    "bm25": dict(RUN_ID="s4-bm25", MEMORY="bm25", RUN_NAME="bm25"),
    "icm-k5": dict(RUN_ID="s4-icm-k5", MEMORY="icm", RUN_NAME="icm-v2-k5", ICM_AMB_ENGINE="v2", ICM_AMB_K="5"),  # dated: the published setting
    "icm-k10": dict(RUN_ID="s4-icm-k10", MEMORY="icm", RUN_NAME="icm-v2-k10", ICM_AMB_ENGINE="v2", ICM_AMB_K="10"),
    "icm-k20": dict(RUN_ID="s4-icm-k20", MEMORY="icm", RUN_NAME="icm-v2-k20", ICM_AMB_ENGINE="v2", ICM_AMB_K="20"),
}
RECALL_RUNS = {
    # the run to put next to the published figures: v2 given no date, as the two published protocols give none
    "lme-icm": dict(RUN_ID="s6-lme-icm-nodate", DATASET="longmemeval", SPLIT="s", MODE="recall", MEMORY="icm",
                    RUN_NAME="icm-v2-nodate-user", ICM_AMB_ENGINE="v2", ICM_AMB_STORE_DATE="0", ICM_AMB_QUERY_NOW="0",
                    ICM_AMB_RECALL_UNIT="session-user", SHARDS="12", BACKOFF_LIMIT_PER_INDEX="3"),
    # the second, labelled run: v2 with the session and question dates
    "lme-icm-dated": dict(RUN_ID="s6-lme-icm-dated", DATASET="longmemeval", SPLIT="s", MODE="recall", MEMORY="icm",
                          RUN_NAME="icm-v2-dated-user", ICM_AMB_ENGINE="v2", ICM_AMB_RECALL_UNIT="session-user", SHARDS="12",
                          BACKOFF_LIMIT_PER_INDEX="3"),
    "lme-bm25": dict(RUN_ID="s6-lme-bm25-user", DATASET="longmemeval", SPLIT="s", MODE="recall", MEMORY="bm25", RUN_NAME="bm25-user",
                     SHARDS="1", PARALLELISM="1"),
}
_STUB = '''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
a = sys.argv[1:]
get = lambda flag: a[a.index(flag) + 1]
out = Path(get("--output-dir")) / get("--dataset") / get("--name") / get("--mode") / (get("--split") + ".json")
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(json.dumps({"script": Path(__file__).name, "argv": a, "mode": get("--mode"),
                           "env": {k: v for k, v in os.environ.items() if k.startswith(("ICM_AMB_", "OMB_", "LONGMEMEVAL_"))},
                           "results": [{"query_id": "q1"}]}))
'''


def render(**settings) -> subprocess.CompletedProcess:
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("ICM_AMB_", "RUN_", "MODE", "MEMORY", "DATASET", "SPLIT", "SHARDS", "PARALLELISM", "EXTRA_ARGS",
                                "BACKOFF_", "IMAGE", "RESULTS_"))}
    env.update(BASE)
    env.update(settings)
    return subprocess.run(["sh", str(BENCH / "k8s" / "render.sh")], env=env, capture_output=True, text=True)


class Render(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not shutil.which("envsubst"):
            raise AssertionError("envsubst is not on PATH: render.sh cannot run (install gettext); see README, Local setup")
        cls.root = workdir("k8s")
        cls.dataset = cls.root / "lme.json"
        cls.dataset.write_text("[]")
        cls.tools = cls.root / "tools"
        cls.tools.mkdir()
        for name in ("run_amb.py", "recall_only.py"):
            (cls.tools / name).write_text(_STUB)
        shutil.copy(BENCH / "gcs_sync.py", cls.tools / "gcs_sync.py")

    def manifest(self, **settings) -> tuple[dict, dict]:
        proc = render(**settings)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertNotIn("${", "".join(l for l in proc.stdout.splitlines() if not l.lstrip().startswith("#")))
        doc = yaml.safe_load(proc.stdout)
        env = {e["name"]: e["value"] for e in doc["spec"]["template"]["spec"]["containers"][0]["env"]}
        self.assertTrue(all(isinstance(v, str) for v in env.values()))
        schema = os.environ.get("K8S_JOB_SCHEMA")
        if schema:
            import jsonschema
            errors = list(jsonschema.Draft7Validator(json.loads(Path(schema).read_text())).iter_errors(doc))
            self.assertEqual([e.message for e in errors], [])
        return doc, env

    def pod(self, env: dict, index: int = 0) -> dict:
        """The rendered environment, through the real entrypoint, to what the benchmark process receives."""
        work, remote = self.root / f"work-{env['RUN_ID']}", self.root / "remote"
        proc = self.entrypoint(env, index)
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        work, remote = self.root / f"work-{env['RUN_ID']}", self.root / "remote"
        shard = "all" if env["SHARDS"] == "1" else f"shard-{index}-of-{env['SHARDS']}"
        mode = env["MODE"]
        path = remote / env["RUN_ID"] / shard / env["DATASET"] / env["RUN_NAME"] / mode / f"{env['SPLIT']}.json"
        return json.loads(path.read_text())

    def entrypoint(self, env: dict, index: int = 0, **extra) -> subprocess.CompletedProcess:
        work, remote = self.root / f"work-{env['RUN_ID']}", self.root / "remote"
        clean = {k: v for k, v in os.environ.items() if not k.startswith(("ICM_AMB_", "LONGMEMEVAL_", "RUN_", "MODE", "MEMORY"))}
        run_env = dict(clean, **env, TOOLS=str(self.tools), WORK_DIR=str(work), RESULTS_REMOTE=str(remote),
                       AMB_HOME=str(self.root), JOB_COMPLETION_INDEX=str(index), SYNC_INTERVAL="1000",
                       LONGMEMEVAL_DATA_PATH=str(self.dataset), PATH=path_with_python(self.root), **extra)
        (self.root / "src" / "memory_bench").mkdir(parents=True, exist_ok=True)  # so the entrypoint does not clone
        return subprocess.run(["sh", str(BENCH / "entrypoint.sh")], env=run_env, capture_output=True, text=True)

    def test_the_five_locomo_runs(self):
        names = set()
        for label, settings in LOCOMO_RUNS.items():
            doc, env = self.manifest(**settings)
            names.add(doc["metadata"]["name"])
            self.assertEqual(doc["metadata"]["name"], f"amb-{settings['RUN_ID']}-locomo-locomo10-{settings['MEMORY']}")
            self.assertEqual((doc["spec"]["completions"], doc["spec"]["parallelism"], doc["spec"]["backoffLimitPerIndex"]), (10, 3, 1))
            self.assertEqual((env["MEMORY"], env["MODE"], env["RUN_NAME"]), (settings["MEMORY"], "rag", settings["RUN_NAME"]))
            self.assertEqual(env["ICM_AMB_K"], settings.get("ICM_AMB_K", ""))
            self.assertEqual(env["ICM_AMB_ENGINE"], settings.get("ICM_AMB_ENGINE", ""))
            self.assertEqual((env["OMB_ANSWER_MODEL"], env["OMB_JUDGE_MODEL"]), ("gemini-3.1-pro-preview", "gemini-2.5-flash-lite"))
            got = self.pod(env, index=3)
            argv = got["argv"]
            self.assertEqual(got["script"], "run_amb.py", label)
            self.assertEqual(argv[:11], ["run", "--dataset", "locomo", "--split", "locomo10", "--memory", settings["MEMORY"],
                                         "--mode", "rag", "--name", settings["RUN_NAME"]])
            self.assertEqual(got["env"].get("ICM_AMB_K", ""), settings.get("ICM_AMB_K", ""))
            self.assertEqual(got["env"]["ICM_AMB_SHARD"], "3/10")
            self.assertEqual(got["env"]["ICM_AMB_SHARD_BY"], "rank")
            self.assertEqual(got["env"]["OMB_ANSWER_MODEL"], "gemini-3.1-pro-preview")
            # every answer run keeps the API's token counts next to its result: what it cost
            self.assertTrue(got["env"]["ICM_AMB_USAGE"].endswith(f"/shard-3-of-10/usage-locomo-locomo10-{settings['RUN_NAME']}.jsonl"))
            # the engine reaches the process for ICM, and only for ICM
            self.assertEqual(got["env"].get("ICM_AMB_ENGINE", ""), settings.get("ICM_AMB_ENGINE", ""))
            self.assertEqual("ICM_AMB_HTTP_TRACE" in got["env"], settings["MEMORY"] == "icm")
        self.assertEqual(len(names), 5, "one Job name per run")

    def test_icm_without_an_engine_is_refused_at_render_and_at_pod_start(self):
        icm = dict(RUN_ID="x", MEMORY="icm")
        for settings, needle in (
            (icm, "ICM_AMB_ENGINE is required with MEMORY=icm"),
            (dict(RUN_ID="x"), "ICM_AMB_ENGINE is required with MEMORY=icm"),  # MEMORY defaults to icm
            (dict(icm, ICM_AMB_ENGINE=""), "ICM_AMB_ENGINE is required with MEMORY=icm"),
            (dict(icm, ICM_AMB_ENGINE="default"), "must be v2, legacy or binary-default-no-dates"),
            (dict(icm, ICM_AMB_ENGINE="legacy", ICM_AMB_STORE_DATE="0"), "ICM_AMB_STORE_DATE is only read with ICM_AMB_ENGINE=v2"),
            (dict(icm, ICM_AMB_ENGINE="binary-default-no-dates", ICM_AMB_QUERY_NOW="0"), "ICM_AMB_QUERY_NOW is only read with"),
            (dict(icm, ICM_AMB_ENGINE="v2", ICM_AMB_STORE_DATE="no"), "ICM_AMB_STORE_DATE must be 0, 1 or empty"),
            (dict(icm, ICM_AMB_ENGINE="v2", ICM_AMB_NO_EMBEDDINGS="yes"), "ICM_AMB_NO_EMBEDDINGS must be 0, 1 or empty"),
            (dict(icm, ICM_AMB_ENGINE="legacy", ICM_AMB_MAX_TOKENS="8000"), "ICM_AMB_MAX_TOKENS needs ICM_AMB_ENGINE=v2"),
        ):
            proc = render(**settings)
            self.assertNotEqual(proc.returncode, 0, settings)
            self.assertIn(needle, proc.stderr)
            self.assertEqual(proc.stdout, "")
        for engine in ("v2", "legacy", "binary-default-no-dates"):
            _, env = self.manifest(RUN_ID=f"e-{engine}", ICM_AMB_ENGINE=engine)
            self.assertEqual(env["ICM_AMB_ENGINE"], engine)
        # a manifest written without render.sh (plain envsubst, a hand edit): the pod stops
        # before it downloads anything or starts the benchmark process
        _, env = self.manifest(RUN_ID="by-hand", ICM_AMB_ENGINE="v2")
        for bad in ("", "default"):
            proc = self.entrypoint(dict(env, ICM_AMB_ENGINE=bad))
            self.assertEqual(proc.returncode, 2, proc.stdout + proc.stderr)
            self.assertIn("ICM_AMB_ENGINE", proc.stdout)
            self.assertFalse((self.root / "work-by-hand").exists(), "nothing started")
        self.assertFalse((self.root / "remote" / "by-hand").exists())

    def test_a_baseline_does_not_carry_icm_settings_left_in_the_shell(self):
        proc = render(RUN_ID="s4-bm25", MEMORY="bm25", RUN_NAME="bm25", ICM_AMB_ENGINE="v2", ICM_AMB_STORE_DATE="0")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("ICM_AMB_ENGINE=v2 is not read with MEMORY=bm25", proc.stderr)
        env = {e["name"]: e["value"] for e in yaml.safe_load(proc.stdout)["spec"]["template"]["spec"]["containers"][0]["env"]}
        self.assertEqual((env["ICM_AMB_ENGINE"], env["ICM_AMB_STORE_DATE"]), ("", ""))

    def test_recall_runs(self):
        doc, env = self.manifest(**RECALL_RUNS["lme-icm"])
        self.assertEqual(doc["metadata"]["name"], "amb-s6-lme-icm-nodate-longmemeval-s-icm")
        self.assertEqual((doc["spec"]["completions"], doc["spec"]["parallelism"], doc["spec"]["backoffLimitPerIndex"]), (12, 3, 3))
        self.assertEqual((env["MODE"], env["ICM_AMB_RECALL_UNIT"], env["ICM_AMB_ENGINE"]), ("recall", "session-user", "v2"))
        # the date switches travel from the shell to the manifest to the process
        self.assertEqual((env["ICM_AMB_STORE_DATE"], env["ICM_AMB_QUERY_NOW"], env["ICM_AMB_NO_EMBEDDINGS"]), ("0", "0", ""))
        got = self.pod(env, index=11)
        self.assertEqual(got["script"], "recall_only.py")
        self.assertEqual(got["argv"][:11], ["run", "--dataset", "longmemeval", "--split", "s", "--memory", "icm", "--mode", "recall",
                                            "--name", "icm-v2-nodate-user"])
        self.assertEqual((got["env"]["ICM_AMB_SHARD"], got["env"]["ICM_AMB_RECALL_UNIT"]), ("11/12", "session-user"))
        self.assertEqual((got["env"]["ICM_AMB_ENGINE"], got["env"]["ICM_AMB_STORE_DATE"], got["env"]["ICM_AMB_QUERY_NOW"]),
                         ("v2", "0", "0"))
        self.assertEqual(got["env"]["LONGMEMEVAL_DATA_PATH"], str(self.dataset), "the file already there is used, not fetched")
        doc, env = self.manifest(**RECALL_RUNS["lme-icm-dated"])
        self.assertEqual(doc["metadata"]["name"], "amb-s6-lme-icm-dated-longmemeval-s-icm")
        self.assertEqual((env["ICM_AMB_ENGINE"], env["ICM_AMB_STORE_DATE"], env["ICM_AMB_QUERY_NOW"]), ("v2", "", ""))
        _, env = self.manifest(RUN_ID="kw", ICM_AMB_ENGINE="v2", ICM_AMB_NO_EMBEDDINGS="1")
        self.assertEqual(env["ICM_AMB_NO_EMBEDDINGS"], "1")
        self.assertEqual(self.pod(env)["env"]["ICM_AMB_NO_EMBEDDINGS"], "1")
        doc, env = self.manifest(**RECALL_RUNS["lme-bm25"])
        self.assertEqual((env["MODE"], env["MEMORY"], env["ICM_AMB_RECALL_UNIT"]), ("recall", "bm25", ""))
        self.assertEqual(self.pod(env)["script"], "recall_only.py")

    def test_defaults_are_the_answer_run_of_before(self):
        doc, env = self.manifest(RUN_ID="v2-20261004", ICM_AMB_ENGINE="v2")
        self.assertEqual((env["MODE"], env["MEMORY"], env["RUN_NAME"], env["ICM_AMB_K"], env["ICM_AMB_RECALL_UNIT"]),
                         ("rag", "icm", "icm", "", ""))
        self.assertEqual(doc["metadata"]["name"], "amb-v2-20261004-locomo-locomo10-icm")
        self.assertEqual(self.pod(env)["script"], "run_amb.py")

    def test_refusals(self):
        for settings, needle in (
            (dict(RUN_ID="x", MODE="recall", MEMORY="full-context"), "MODE=recall runs MEMORY=icm or MEMORY=bm25"),
            (dict(RUN_ID="x", MODE="recall", ICM_AMB_ENGINE="v2", ICM_AMB_RECALL_UNIT="turn"), "ICM_AMB_RECALL_UNIT must be"),
            (dict(RUN_ID="x", MODE="recall", ICM_AMB_ENGINE="v2", ICM_AMB_MAX_TOKENS="8000"), "refuses ICM_AMB_MAX_TOKENS"),
            (dict(RUN_ID="x", MODE="recal", ICM_AMB_ENGINE="v2"), "MODE must be"),
            (dict(RUN_ID="x", ICM_AMB_ENGINE="v2", ICM_AMB_RECALL_UNIT="chunk"), "only read with MODE=recall"),
            (dict(RUN_ID="x", MEMORY="full_context"), "may only hold lowercase letters"),
        ):
            proc = render(**settings)
            self.assertNotEqual(proc.returncode, 0, settings)
            self.assertIn(needle, proc.stderr)
            self.assertEqual(proc.stdout, "")


if __name__ == "__main__":
    unittest.main(verbosity=2)
