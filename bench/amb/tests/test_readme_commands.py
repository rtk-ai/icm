#!/usr/bin/env python3
"""The commands of README.md, run as written. The GKE blocks go through the real
k8s/render.sh with `kubectl`, `gcloud` and `python` replaced by stubs, and every Job
they would apply is read back: its engine, its mode, its description. Offline.

    python tests/test_readme_commands.py
"""
import os
import re
import shutil
import subprocess
import sys
import unittest
from pathlib import Path

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parent))
from _common import BENCH, workdir

README = (BENCH / "README.md").read_text()
REPO = BENCH.parent.parent
PLACEHOLDERS = {"<digest>": "ab" * 32, "<bucket>": "test-bucket", "<git sha>": "abc1234", "<in>": "1", "<out>": "1"}


def bash_blocks(text: str) -> list[str]:
    return re.findall(r"```bash\n(.*?)```", text, re.S)


def commands(block: str) -> list[str]:
    """Logical command lines: continuations joined, comments dropped."""
    joined = re.sub(r"\\\n\s*", " ", block)
    return [line.strip() for line in joined.splitlines() if line.strip() and not line.strip().startswith("#")]


class Gke(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not shutil.which("envsubst"):
            raise AssertionError("envsubst is not on PATH: render.sh cannot run (install gettext)")
        cls.root = workdir("readme")
        stubs, cls.captured = cls.root / "stubs", cls.root / "applied"
        stubs.mkdir()
        cls.captured.mkdir()
        (stubs / "kubectl").write_text(
            "#!/bin/sh\ncase \"$*\" in\n  *'-f -') n=$(ls \"$CAPTURE\" | wc -l | tr -d ' '); cat > \"$CAPTURE/apply-$n.yaml\" ;;\nesac\n")
        for name in ("gcloud", "python"):
            (stubs / name).write_text("#!/bin/sh\nexit 0\n")
        for stub in stubs.iterdir():
            stub.chmod(0o755)
        section = README[README.index("\n## GKE\n"):]
        script = "\n".join(bash_blocks(section))
        for placeholder, value in PLACEHOLDERS.items():
            script = script.replace(placeholder, value)
        assert "<" not in re.sub(r"<<|2?>&?\d?", "", script), "a placeholder of the README is not known to this test"
        keep = {k: v for k, v in os.environ.items()
                if not k.startswith(("ICM_AMB_", "RUN_", "MODE", "MEMORY", "DATASET", "SPLIT", "SHARDS", "PARALLELISM", "EXTRA_ARGS",
                                     "BACKOFF_", "IMAGE", "RESULTS_"))}
        cls.proc = subprocess.run(["sh", "-e", "-c", script], cwd=REPO, capture_output=True, text=True,
                                  env=dict(keep, PATH=f"{stubs}:{os.environ['PATH']}", CAPTURE=str(cls.captured)))
        cls.jobs = []
        for path in sorted(cls.captured.glob("apply-*.yaml"), key=lambda p: int(p.stem.split("-")[1])):
            doc = yaml.safe_load(path.read_text())
            env = {e["name"]: e["value"] for e in doc["spec"]["template"]["spec"]["containers"][0]["env"]}
            cls.jobs.append((doc["metadata"]["name"], env))

    def test_every_block_runs_and_every_job_renders(self):
        self.assertEqual(self.proc.returncode, 0, self.proc.stderr[-3000:])
        self.assertEqual(self.proc.stderr, "", "render.sh had nothing to say: no stale variable, no refusal")
        names = [name for name, _ in self.jobs]
        self.assertEqual(len(names), 9, names)  # LoCoMo, PersonaMem, five LoCoMo runs, two LongMemEval recall runs
        self.assertEqual(len(set(names)), len(names), "one Job name per run")

    def test_every_icm_job_names_its_engine_and_no_baseline_carries_one(self):
        for name, env in self.jobs:
            if env["MEMORY"] == "icm":
                self.assertIn(env["ICM_AMB_ENGINE"], ("v2", "legacy", "binary-default-no-dates"), name)
            else:
                self.assertEqual((env["ICM_AMB_ENGINE"], env["ICM_AMB_STORE_DATE"], env["ICM_AMB_QUERY_NOW"]), ("", "", ""), name)

    def test_each_job_carries_its_own_description_and_nothing_of_the_previous_one(self):
        words = {"locomo": "LoCoMo", "personamem": "PersonaMem", "longmemeval": "LongMemEval-S"}
        descriptions = [env["RUN_DESCRIPTION"] for _, env in self.jobs]
        self.assertEqual(len(set(descriptions)), len(descriptions), "no description used twice")
        for name, env in self.jobs:
            description = env["RUN_DESCRIPTION"]
            self.assertIn(words[env["DATASET"]], description, name)
            for other in set(words.values()) - {words[env["DATASET"]]}:
                self.assertNotIn(other, description, name)
            if env["MODE"] == "recall":
                self.assertIn("recall only", description, name)
                self.assertNotIn("k=50", description, name)
                self.assertNotIn("chunks 512", description, name)
                self.assertEqual(env["DATASET"], "longmemeval")
            else:
                self.assertEqual(env["MODE"], "rag", name)
                self.assertNotIn("recall only", description, name)
                self.assertEqual(env["ICM_AMB_RECALL_UNIT"], "", name)
            if env["MEMORY"] == "icm" and env["MODE"] == "rag":
                self.assertIn(f"k={env['ICM_AMB_K'] or 50}", description, name)
            # a setting of one run does not reach the next: only PersonaMem asks for the unit checkpoint
            self.assertEqual(env["ICM_AMB_UNIT_CHECKPOINT"], "personamem" if env["DATASET"] == "personamem" else "", name)

    def test_the_first_longmemeval_run_sends_no_date_and_the_dated_one_is_labelled(self):
        recall = [(name, env) for name, env in self.jobs if env["MODE"] == "recall"]
        self.assertEqual(len(recall), 2)
        (first_name, first), (second_name, second) = recall
        self.assertEqual((first["ICM_AMB_ENGINE"], first["ICM_AMB_STORE_DATE"], first["ICM_AMB_QUERY_NOW"]), ("v2", "0", "0"))
        self.assertIn("nodate", first["RUN_NAME"])
        self.assertIn("nodate", first_name)
        self.assertEqual((second["ICM_AMB_ENGINE"], second["ICM_AMB_STORE_DATE"], second["ICM_AMB_QUERY_NOW"]), ("v2", "", ""))
        self.assertIn("dated", second["RUN_NAME"])
        self.assertIn("dated variant", second["RUN_DESCRIPTION"])
        self.assertNotEqual(first["RUN_ID"], second["RUN_ID"])


class EveryExample(unittest.TestCase):
    """README and script headers: a command that runs ICM names the engine."""

    def examples(self) -> list[tuple[str, str]]:
        found = [("README.md", c) for block in bash_blocks(README) for c in commands(block)]
        for path in sorted(BENCH.glob("*.py")):
            header = path.read_text().split('"""')[1] if '"""' in path.read_text() else ""
            found += [(path.name, c) for c in commands(header)]
        return found

    def test_memory_icm_always_comes_with_an_engine(self):
        runs = [(src, c) for src, c in self.examples() if re.search(r"--memory[ =]icm\b", c) and ".py" in c]
        self.assertGreaterEqual(len(runs), 6)
        for src, command in runs:
            self.assertRegex(command, r"ICM_AMB_ENGINE=(v2|legacy|binary-default-no-dates)\b", f"{src}: {command}")

    def test_compare_builds_names_an_engine_on_each_side(self):
        runs = [(src, c) for src, c in self.examples() if "compare_builds.py" in c and "--before" in c]
        self.assertGreaterEqual(len(runs), 3)
        for src, command in runs:
            self.assertRegex(command, r"--before-env\s+ICM_AMB_ENGINE=\w", f"{src}: {command}")
            self.assertRegex(command, r"--after-env\s+ICM_AMB_ENGINE=\w", f"{src}: {command}")

    def test_the_longmemeval_command_to_compare_with_published_figures_sends_no_date(self):
        section = README[README.index("## Recall only (no model)"):README.index("## Second, strict judge")]
        icm = [c for c in commands(bash_blocks(section)[0]) if "--memory icm" in c]
        self.assertEqual(len(icm), 2)
        self.assertIn("ICM_AMB_STORE_DATE=0", icm[0])
        self.assertIn("ICM_AMB_QUERY_NOW=0", icm[0])
        self.assertIn("nodate", icm[0])
        self.assertNotIn("ICM_AMB_STORE_DATE", icm[1])
        self.assertIn("dated", icm[1])

    def test_no_dollar_figure_is_written_in_the_page(self):
        cost = README[README.index("## What a run costs"):README.index("## Recall only (no model)")]
        self.assertNotRegex(cost, r"\$\s?\d")
        self.assertIn("https://cloud.google.com/vertex-ai/generative-ai/pricing", cost)


if __name__ == "__main__":
    unittest.main(verbosity=2)
