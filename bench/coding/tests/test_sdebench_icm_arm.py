"""Offline tests of the sdebench ICM arm.

Unit tests always run. The integration tests execute the REAL upstream harness
(sdebench/harness/run.py and the AMB runner) with a stand-in `docker`
(fake_docker.py): no container, no model, no `icm`, no key. They need:

    ICM_SDE_TEST_AMB_HOME   agent-memory-benchmark checkout at the pinned commit,
                            with the sde-bench submodule
    ICM_SDE_TEST_BOLTONS    clone of https://github.com/vectorize-io/boltons

and are skipped when either is missing.

    python -m unittest discover -s bench/coding/tests -v
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
import uuid
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
CODING = HERE.parent
sys.path.insert(0, str(CODING))

import icm_corpus
import icm_sde_run
import run_sdebench
import sdebench_stats

AMB_HOME = os.environ.get("ICM_SDE_TEST_AMB_HOME")
BOLTONS = os.environ.get("ICM_SDE_TEST_BOLTONS")
INTEGRATION = bool(AMB_HOME and BOLTONS)
TASK = "boltons-dedupe"


class UnitTests(unittest.TestCase):
    def test_parse_sections(self):
        text = "noise\n@@a\n1\n2\n@@b\n\n@@c\nx\n"
        self.assertEqual(icm_sde_run.parse_sections(text), {"a": "1\n2", "b": "", "c": "x"})
        self.assertEqual(icm_sde_run.parse_sections(""), {})

    def test_int(self):
        self.assertEqual(icm_sde_run._int("3\n"), 3)
        self.assertIsNone(icm_sde_run._int(""))
        self.assertIsNone(icm_sde_run._int("No hook events recorded yet."))

    def test_split_args_defaults_to_the_icm_arm(self):
        dry, rest = icm_sde_run.split_args(["--task", "t.json", "--agent", "claude-code"])
        self.assertFalse(dry)
        self.assertEqual(rest[-2:], ["--history", "icm"])

    def test_split_args_keeps_an_explicit_arm(self):
        _, rest = icm_sde_run.split_args(["--task", "t.json", "--history", "full"])
        self.assertEqual(rest.count("--history"), 1)
        self.assertIn("full", rest)

    def test_dry_run_grades_once(self):
        dry, rest = icm_sde_run.split_args(
            ["--task", "t.json", "--max-interventions", "5", "--dry-run", "--max-interventions=3"])
        self.assertTrue(dry)
        self.assertNotIn("--dry-run", rest)
        self.assertEqual(rest[-2:], ["--max-interventions", "0"])
        self.assertEqual(sum(a.startswith("--max-interventions") for a in rest), 1)

    def test_container_scripts_exist_and_are_posix_sh(self):
        for name in ("setup.sh", "probe.sh", "collect.sh", "seed.sh"):
            path = CODING / "container" / name
            self.assertTrue(path.is_file(), name)
            done = subprocess.run(["sh", "-n", str(path)], capture_output=True, text=True)
            self.assertEqual(done.returncode, 0, done.stderr)

    def test_unpinned_checkout_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            (Path(tmp) / "sdebench" / "harness").mkdir(parents=True)
            (Path(tmp) / "sdebench" / "harness" / "run.py").write_text("")
            saved = os.environ.pop("ICM_SDE_ALLOW_UNPINNED", None)
            try:
                with self.assertRaises(SystemExit):
                    icm_sde_run.check_pin(Path(tmp))
            finally:
                if saved is not None:
                    os.environ["ICM_SDE_ALLOW_UNPINNED"] = saved


def corpus_documents(task: str = "t-001", chats: int = 1, decoys: int = 140, commits: int = 100,
                     decision_commit: bool = False) -> list:
    """Documents shaped and ordered like the dataset's: the task's own first."""
    def chat(doc_id: str, text: str):
        return SimpleNamespace(id=doc_id, content=text, messages=[
            {"role": "user", "content": f"{text} question"},
            {"role": "assistant", "content": f"{text} answer"}])

    docs = [chat(f"{task}:chat{i}", f"the decision of the task, part {i}") for i in range(chats)]
    if decision_commit:
        docs.append(SimpleNamespace(id=f"{task}:decision-commit", messages=None,
                                    content="Git commit: the decision of the task"))
    docs += [chat(f"{task}:decoy-{i}", f"decoy number {i}") for i in range(decoys)]
    docs += [SimpleNamespace(id=f"{task}:git-{i:07x}", messages=None, content=f"Git commit: noise {i}")
             for i in range(commits)]
    return docs


class CorpusOrderTests(unittest.TestCase):
    """The import order must not hand the task's own documents to the outputs of
    ICM that select by insertion order (icm_corpus.py)."""

    def ranks(self, docs: list, seed: str) -> tuple[list[int], int]:
        order = icm_corpus.import_order(docs, seed)
        self.assertEqual(sorted(d.id for d in order), sorted(d.id for d in docs))
        own = [i for i, d in enumerate(order) if icm_corpus.is_task_document(d.id)]
        return own, len(order)

    def test_task_documents_are_never_at_either_end(self):
        for kind in ({"chats": 1}, {"chats": 2}, {"chats": 0, "decision_commit": True}):
            docs = corpus_documents(**kind)
            for n in range(200):
                own, total = self.ranks(docs, f"seed-{n}")
                self.assertTrue(own, kind)
                for rank in own:
                    self.assertGreaterEqual(rank, total // 4, (kind, n))
                    self.assertLess(rank, total - total // 4, (kind, n))

    def test_an_amending_conversation_stays_after_the_one_it_amends(self):
        docs = corpus_documents(chats=2)
        for n in range(200):
            order = icm_corpus.import_order(docs, f"seed-{n}")
            own = [d.id for d in order if icm_corpus.is_task_document(d.id)]
            self.assertEqual(own, ["t-001:chat0", "t-001:chat1"])

    def test_order_depends_on_the_seed_only(self):
        docs = corpus_documents()
        first = [d.id for d in icm_corpus.import_order(docs, "a")]
        self.assertEqual(first, [d.id for d in icm_corpus.import_order(list(reversed(docs)), "a")])
        self.assertNotEqual(first, [d.id for d in icm_corpus.import_order(docs, "b")])
        self.assertNotEqual(first, [d.id for d in docs])
        # The position of the task's document moves with the seed: it is not pinned.
        self.assertGreater(len({self.ranks(docs, f"seed-{n}")[0][0] for n in range(50)}), 20)

    def test_corpus_is_one_directory_and_names_say_nothing(self):
        docs = corpus_documents(chats=2, decoys=30, commits=20)
        with tempfile.TemporaryDirectory() as tmp:
            corpus = Path(tmp) / "corpus"
            counts = icm_corpus.write_corpus(docs, corpus, seed="s")
            files = sorted(p.name for p in corpus.iterdir())
            self.assertEqual(len(files), 52)
            self.assertTrue(all(p.is_file() for p in corpus.iterdir()))
            self.assertEqual([f.split(".")[0] for f in files], [f"{i:04d}" for i in range(52)])
            self.assertEqual((counts["sessions"], counts["commits"], counts["order_seed"]), (32, 20, "s"))
            own = counts["task_documents"]
            self.assertEqual([d["id"] for d in own], ["t-001:chat0", "t-001:chat1"])
            # The files are written in the import order, not in the dataset's (where
            # the task's conversations come first): the recorded ranks are the file
            # names, and they are in the middle half.
            expected = [i for i, d in enumerate(icm_corpus.import_order(docs, "s"))
                        if icm_corpus.is_task_document(d.id)]
            self.assertEqual([d["rank"] for d in own], expected)
            for doc in own:
                self.assertEqual(doc["file"], f"{doc['rank']:04d}.jsonl")
                self.assertTrue(52 // 4 <= doc["rank"] < 52 - 52 // 4, doc)
            first = json.loads((corpus / "0000.jsonl").read_text().splitlines()[0]) \
                if (corpus / "0000.jsonl").is_file() else {"message": {"content": (corpus / "0000.md").read_text()}}
            self.assertNotIn("the decision of the task", first["message"]["content"])
            for doc in own:
                turn = json.loads((corpus / doc["file"]).read_text().splitlines()[0])
                self.assertEqual(turn["session_id"], doc["file"].split(".")[0])
                self.assertIn("the decision of the task", turn["message"]["content"])
            # ICM_SDE_GIT_INGEST=none: conversations only, same guarantee.
            counts = icm_corpus.write_corpus(docs, corpus, seed="s", commits=False)
            self.assertEqual((counts["sessions"], counts["commits"]), (32, 0))
            self.assertFalse(list(corpus.glob("*.md")))

    def test_lines_of_the_task_documents_are_recognised(self):
        task = {"conversations": [{"role": "user", "text": "Keep the most-filled record, a tie keeps the primary."},
                                  {"role": "assistant", "text": "Understood.\nSame email and same day is one contact."}],
                "decision_subject": "fix: stop duplicating contacts", "decision_rationale": "Dedupe on email."}
        texts = icm_corpus.task_texts(task)
        pack = ("# ICM Wake-up\n- [user]: Keep the most-filled record, a tie keeps the primary.\n"
                "- Same email and same day is one contact.\n- Keep the most-filled […]\n"
                "- a decoy about cache eviction policies\n- Understood.\nnot a bullet: Dedupe on email.\n")
        # Role labels and the renderer's cut mark are looked through; a line too
        # short to identify a passage ("Understood.") is not counted.
        self.assertEqual(icm_corpus.task_lines(pack, texts),
                         ["[user]: Keep the most-filled record, a tie keeps the primary.",
                          "Same email and same day is one contact.", "Keep the most-filled […]"])
        self.assertEqual(icm_corpus.task_lines("", texts), [])
        amended = {"conversations": [[{"role": "user", "text": "first rule of the two chats"}],
                                     [{"role": "user", "text": "second rule, amending the first"}]]}
        self.assertEqual(len(icm_corpus.task_texts(amended)), 2)


class StatsTests(unittest.TestCase):
    @staticmethod
    def row(task: str, source: str, reasoning: str, context: str = "", solved: bool = True) -> dict:
        return {"query_id": task, "reasoning": reasoning, "context": context,
                "meta": {"source": source, "solved": solved, "interventions": 0, "cost_usd": 0.5, "wall_s": 60}}

    ICM = ("arm=icm interventions=0 cost=$0.5 turns=3 dry_run=False hook_fired={fired} "
           "session_start_fired=True seed_task_facts={seed} start_task_lines={start} "
           "prompt_task_lines={prompt} stored_during_task=2")

    def run_of(self, *rows: dict) -> dict:
        return sdebench_stats.summarize(sdebench_stats.task_rows({"results": list(rows)}))

    def test_a_task_without_hooks_is_not_counted_as_a_memory_run(self):
        s = self.run_of(
            self.row("a", "conversation", self.ICM.format(fired=True, seed=3, start=0, prompt=1),
                     "## Memory (ICM SessionStart hook; replayed before the agent started)\nx"),
            self.row("b", "conversation", self.ICM.format(fired=False, seed=0, start=0, prompt=0),
                     "## Memory (ICM SessionStart hook; replayed before the agent started)\nx"))
        self.assertEqual(s["icm"]["hooks"], [1, 2])
        self.assertEqual(s["icm"]["in_seed"], [1, 2])
        self.assertEqual(s["icm"]["by_search"], [1, 2])
        self.assertEqual(s["icm"]["stored_during_task"], 4)
        # The replayed text is not what the agent received: not an "injected" row.
        self.assertEqual(s["injected"], 0)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            sdebench_stats.print_summary("icm", "claude", s)
        self.assertIn("hooks 1/2", out.getvalue())
        self.assertIn("1 row(s) WITHOUT ICM's hooks", out.getvalue())
        self.assertNotIn("injected", out.getvalue())

    def test_other_arms_keep_the_injected_column(self):
        s = self.run_of(self.row("a", "conversation", "arm=hscoding interventions=0", "## Memory (reflect)\nx"),
                        self.row("b", "history", "arm=full interventions=0", "memory=vanilla arm=full"))
        self.assertIsNone(s["icm"])
        self.assertEqual(s["injected"], 1)

    def test_gate(self):
        dry = self.ICM.replace("dry_run=False", "dry_run=True")
        good = [self.row(f"c{i}", "conversation", dry.format(fired=False, seed=2, start=0, prompt=i % 2), solved=False)
                for i in range(10)]
        self.assertEqual(sdebench_stats.gate(self.run_of(*good), 0.9, 0.5), [])
        by_position = good[:-1] + [self.row("c9", "conversation", dry.format(fired=False, seed=2, start=1, prompt=1))]
        self.assertIn("by position", " ".join(sdebench_stats.gate(self.run_of(*by_position), 0.9, 0.5)))
        self.assertIn("retrieves", " ".join(sdebench_stats.gate(self.run_of(*good), 0.9, 0.6)))
        empty = [self.row(f"c{i}", "conversation", dry.format(fired=False, seed=0, start=0, prompt=0)) for i in range(4)]
        self.assertIn("in the seed", " ".join(sdebench_stats.gate(self.run_of(*empty), 0.9, 0.0)))
        self.assertEqual(sdebench_stats.gate(self.run_of(self.row("a", "history", "arm=full")), 0.9, 0.5),
                         ["not a run of the ICM arm"])

    def test_a_run_split_by_task_is_read_as_one(self):
        with tempfile.TemporaryDirectory() as tmp:
            parts = []
            for name, tasks in (("p1", ("a", "b")), ("p2", ("c",))):
                path = Path(tmp) / f"{name}.json"
                path.write_text(json.dumps({"run_name": name, "results": [
                    self.row(t, "conversation", "arm=full interventions=0") for t in tasks]}))
                parts.append(str(path))
            data = sdebench_stats.load(",".join(parts))
            self.assertEqual(sorted(sdebench_stats.task_rows(data)), ["a", "b", "c"])
            self.assertEqual(data["run_name"], "p1+p2")
            with self.assertRaises(SystemExit):
                sdebench_stats.load(f"{parts[0]},{parts[0]}")


class SetupScriptTests(unittest.TestCase):
    """container/setup.sh against a stand-in `icm` that writes what `icm init`
    writes in a Claude Code settings file (hooks, with `"matcher": "Bash"`)."""

    HOOKS = ('"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"icm hook pre"}],"matcher":"Bash"}],'
             '"PostToolUse":[{"hooks":[{"type":"command","command":"icm hook post"}]}],'
             '"UserPromptSubmit":[{"hooks":[{"type":"command","command":"icm hook prompt"}]}],'
             '"SessionStart":[{"hooks":[{"type":"command","command":"icm hook start"}]}],'
             '"SessionEnd":[{"hooks":[{"type":"command","command":"icm hook end"}]}]}')
    IMAGE = '"permissions":{"allow":["Bash","Edit","mcp__icm"],"defaultMode":"acceptEdits"}'

    def setup_sh(self, settings_after_init: str, mode: str, provider: str = "none",
                 wanted: str | None = None) -> subprocess.CompletedProcess:
        with tempfile.TemporaryDirectory() as tmp:
            home, bin_dir = Path(tmp) / "home", Path(tmp) / "bin"
            (home / ".claude").mkdir(parents=True)
            bin_dir.mkdir()
            (Path(tmp) / "settings.json").write_text(settings_after_init)
            (bin_dir / "icm").write_text(
                '#!/bin/sh\ncase "$1" in\n'
                '  --version) echo "icm 0.0.0-stub" ;;\n'
                f'  init) cp "{tmp}/settings.json" "$HOME/.claude/settings.json"\n'
                '        echo "<!-- icm:start -->" > "$HOME/.claude/CLAUDE.md"\n'
                '        echo \'{"mcpServers":{"icm":{}}}\' > "$HOME/.claude.json" ;;\n'
                '  serve) cat > /dev/null; echo \'{"name": "icm_memory_recall"}\' ;;\n'
                # `icm config`, as the real binary prints the section.
                f'  config) printf "[extraction]\\n  enabled = true\\n\\n[extraction.summarizer]\\n"\n'
                f'          printf "  provider = {provider}\\n  model = (provider default)\\n\\n"\n'
                '          printf "[consolidate.summarizer]\\n  provider = other\\n" ;;\n'
                '  *) echo "Memories: 0" ;;\nesac\n')
            (bin_dir / "claude").write_text('#!/bin/sh\necho "0.0.0 (stub)"\n')
            (bin_dir / "timeout").write_text('#!/bin/sh\nshift\nexec "$@"\n')
            for tool in bin_dir.iterdir():
                tool.chmod(0o755)
            return subprocess.run(
                ["sh", str(CODING / "container" / "setup.sh"), "claude-code"], capture_output=True, text=True,
                env={"PATH": f"{bin_dir}:/usr/bin:/bin", "HOME": str(home), "ICM_SDE_INIT_MODE": mode,
                     "ICM_DB": str(home / "icm" / "memories.db"),
                     **({"ICM_SDE_EXTRACTION": wanted} if wanted else {})}, timeout=60)

    def test_wiring_is_accepted_when_the_allow_list_survives(self):
        for mode in ("all", "standard"):
            done = self.setup_sh("{" + self.IMAGE + "," + self.HOOKS + "}", mode)
            self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
            self.assertIn("ok permissions-kept", done.stdout)
            self.assertEqual(icm_sde_run.parse_sections(done.stdout).get("ok"), "1")

    def test_a_lost_allow_list_fails_the_setup(self):
        # The hooks alone still contain the string "Bash" (PreToolUse matcher).
        for mode in ("all", "standard"):
            done = self.setup_sh("{" + self.HOOKS + "}", mode)
            self.assertNotEqual(done.returncode, 0, done.stdout)
            self.assertIn("MISSING permissions-kept", done.stdout)
            self.assertIn("wiring incomplete", icm_sde_run.parse_sections(done.stdout).get("error", ""))

    def test_an_extraction_provider_that_calls_a_model_fails_the_setup(self):
        settings = "{" + self.IMAGE + "," + self.HOOKS + "}"
        # ICM's default: its SessionEnd hook would run the agent's CLI on its own.
        done = self.setup_sh(settings, "all", provider="auto")
        self.assertNotEqual(done.returncode, 0, done.stdout)
        sections = icm_sde_run.parse_sections(done.stdout)
        self.assertEqual(sections.get("extraction"), "auto")
        self.assertIn("MISSING extraction-none (icm config says: auto)", done.stdout)
        self.assertIn("extraction-none", sections.get("error", ""))
        # The image's setting passes, and is reported.
        done = self.setup_sh(settings, "all")
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn("ok extraction-none", done.stdout)
        self.assertEqual(icm_sde_run.parse_sections(done.stdout).get("extraction"), "none")
        # Asked for explicitly, ICM's default is accepted; the mismatch is refused both ways.
        self.assertEqual(self.setup_sh(settings, "all", provider="auto", wanted="auto").returncode, 0)
        self.assertNotEqual(self.setup_sh(settings, "all", provider="none", wanted="auto").returncode, 0)


class SeedScriptTests(unittest.TestCase):
    """container/seed.sh against a stand-in `icm` that records its arguments. The
    script's container paths (/corpus, /out, its scratch directory) are pointed at
    a temporary directory; nothing else is changed."""

    def seed_sh(self, mode: str | None) -> tuple[subprocess.CompletedProcess, list[str], Path]:
        tmp = Path(tempfile.mkdtemp(prefix="seed-script-"))
        self.addCleanup(shutil.rmtree, tmp, ignore_errors=True)
        for name in ("corpus", "out", "bin"):
            (tmp / name).mkdir()
        (tmp / "corpus" / "0000.md").write_text("Git commit: noise\n")
        (tmp / "bin" / "icm").write_text(
            '#!/bin/sh\nprintf \'%s\\n\' "$*" >> "' + str(tmp) + '/icm-calls"\n'
            'case " $* " in\n'
            '  *" --version "*) echo "icm 0.0.0-stub" ;;\n'
            '  *" import "*) : > "$2"; echo "Imported 1 facts from 1 files." ;;\n'
            '  *" backup "*) for a in "$@"; do last="$a"; done; : > "$last" ;;\n'
            '  *) echo "Memories: 1" ;;\nesac\n')
        (tmp / "bin" / "icm").chmod(0o755)
        script = (CODING / "container" / "seed.sh").read_text()
        for container, host in (("/tmp/icm-sde-seed", tmp / "work"), ("/corpus", tmp / "corpus"),
                                ("/out", tmp / "out")):
            script = script.replace(container, str(host))
        env = {"PATH": f"{tmp / 'bin'}:/usr/bin:/bin", "ICM_SDE_PROJECT": "boltons"}
        if mode is not None:
            env["ICM_SDE_IMPORT"] = mode
        done = subprocess.run(["sh", "-s"], input=script, capture_output=True, text=True, env=env, timeout=60)
        calls = (tmp / "icm-calls").read_text().splitlines() if (tmp / "icm-calls").is_file() else []
        return done, calls, tmp

    def test_import_is_one_pass_with_icm_s_default_extractor(self):
        done, calls, tmp = self.seed_sh(None)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        imports = [c for c in calls if " import " in f" {c} "]
        self.assertEqual(len(imports), 1, calls)                    # one directory, one pass
        self.assertNotIn("--no-embeddings", imports[0])
        self.assertTrue(imports[0].endswith("--project boltons"))
        sections = icm_sde_run.parse_sections((tmp / "out" / "seed-report.txt").read_text())
        self.assertEqual((sections.get("import_mode"), sections.get("ok")), ("default", "1"))
        self.assertTrue((tmp / "out" / "memories.db").is_file())

    def test_rules_import_passes_no_embeddings(self):
        done, calls, tmp = self.seed_sh("rules")
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        (imported,) = [c for c in calls if " import " in f" {c} "]
        self.assertIn("--no-embeddings import", imported)
        report = icm_sde_run.parse_sections((tmp / "out" / "seed-report.txt").read_text())
        self.assertEqual(report.get("import_mode"), "rules")
        done, calls, _ = self.seed_sh("llm")
        self.assertNotEqual(done.returncode, 0)
        self.assertFalse(any(" import " in f" {c} " for c in calls))


class PreflightTests(unittest.TestCase):
    def problems(self, argv: list[str], tools: tuple[str, ...]) -> str:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            (tmp_path / "amb" / "sdebench" / "datasets" / "boltons-x" / "tasks" / "main").mkdir(parents=True)
            (tmp_path / "amb" / "sdebench" / "datasets" / "boltons-x" / "tasks" / "main" / "task.json").write_text("{}")
            (tmp_path / "boltons" / ".git").mkdir(parents=True)
            (tmp_path / "bin").mkdir()
            for tool in tools:
                (tmp_path / "bin" / tool).write_text("#!/bin/sh\n")
                (tmp_path / "bin" / tool).chmod(0o755)
            saved = dict(os.environ)
            os.environ.update({"AMB_HOME": str(tmp_path / "amb"), "PATH": str(tmp_path / "bin"),
                               "SDEBENCH_BOLTONS_HOST": str(tmp_path / "boltons")})
            try:
                run_sdebench.preflight(argv)
                return ""
            except SystemExit as stop:
                return str(stop)
            finally:
                os.environ.clear()
                os.environ.update(saved)

    def test_upstream_arms_need_uv(self):
        base = ("python", "git", "docker")
        coding = ["--dataset", "sdebench", "--mode", "coding", "--memory"]
        self.assertIn("`uv` is not on PATH", self.problems([*coding, "vanilla"], base))
        self.assertIn("`uv` is not on PATH", self.problems([*coding, "hindsight-coding"], base))
        self.assertEqual(self.problems([*coding, "vanilla"], (*base, "uv")), "")
        # The ICM arm starts its own runner with this interpreter: no uv needed.
        self.assertEqual(self.problems([*coding, "icm-coding"], base), "")


@unittest.skipUnless(INTEGRATION, "set ICM_SDE_TEST_AMB_HOME and ICM_SDE_TEST_BOLTONS")
class IntegrationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="icm-sde-test-"))
        self.state = self.tmp / "docker-state"
        self.state.mkdir()
        fake_bin = self.tmp / "bin"
        fake_bin.mkdir()
        shutil.copy(HERE / "fake_docker.py", fake_bin / "docker")
        (fake_bin / "docker").chmod(0o755)
        # `python` must resolve: upstream calls `python build.py`.
        (fake_bin / "python").symlink_to(sys.executable)
        self.env = {
            "PATH": f"{fake_bin}:{Path(sys.executable).parent}:/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin",
            "HOME": str(self.tmp / "home"),
            "AMB_HOME": AMB_HOME,
            "SDEBENCH_BOLTONS_HOST": BOLTONS,
            "FAKE_DOCKER_STATE": str(self.state),
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "SDE_AGENT": "claude-code",
            "LANG": "C.UTF-8",
            # Not a key: the stand-in docker starts nothing. Upstream mounts a
            # credentials file when no Claude credential is set.
            "ANTHROPIC_API_KEY": "test-not-a-key",
        }
        (self.tmp / "home").mkdir()
        self.task = Path(AMB_HOME) / "sdebench" / "datasets" / TASK / "tasks" / "main" / "task.json"
        self.run_id = "t" + uuid.uuid4().hex[:8]
        self.work = Path("/tmp/sdebench/run") / f"{TASK}-001_icm_{self.run_id}"
        self.seed = self.tmp / "memories.db"
        self.seed.write_bytes(b"SQLite format 3\x00fake-seed")
        # A line of the task's own conversation, as ICM would store and print it.
        self.own_line = json.loads(self.task.read_text())["conversations"][0]["text"].splitlines()[0]

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)
        shutil.rmtree(self.work, ignore_errors=True)

    def calls(self) -> list[list[str]]:
        path = self.state / "calls.jsonl"
        return [json.loads(line)["argv"] for line in path.read_text().splitlines()] if path.is_file() else []

    def wrapper(self, *args: str, env: dict | None = None) -> subprocess.CompletedProcess:
        cmd = [sys.executable, str(CODING / "icm_sde_run.py"), "--task", str(self.task),
               "--agent", "claude-code", "--run-id", self.run_id, *args]
        return subprocess.run(cmd, capture_output=True, text=True, stdin=subprocess.DEVNULL,
                              env={**self.env, **(env or {})}, timeout=600)

    def test_dry_run_validates_the_wiring_without_any_agent(self):
        done = self.wrapper("--dry-run", env={"ICM_SDE_DB": str(self.seed)})
        self.assertEqual(done.returncode, 0, done.stderr[-2000:])
        result = json.loads((self.work / "result.json").read_text())
        self.assertEqual(result["history"], "icm")
        self.assertFalse(result["solved"])
        self.assertEqual(result["interventions"], 0)
        self.assertEqual(result["cost_usd"], 0)
        icm = result["icm"]
        self.assertTrue(icm["dry_run"])
        self.assertTrue(icm["seeded"])
        self.assertTrue(icm["probe_injects"])
        self.assertEqual(icm["mcp_tools"], 4)
        self.assertEqual(icm["extraction"], "none")                # ICM's write hooks call no model
        self.assertEqual(icm["amb_commit"], icm_sde_run.PINNED_AMB)
        # Both injecting hooks are replayed, SessionStart first, and kept apart.
        self.assertEqual([e["event"] for e in result["memory_diag"]],
                         ["icm_setup", "icm_probe_start", "icm_probe_prompt", "icm_collect"])
        start = next(e for e in result["memory_diag"] if e["event"] == "icm_probe_start")
        self.assertIn("ICM Wake-up", start["answer"])
        self.assertEqual(json.loads((self.state / "probe-start.json").read_text()),
                         {"session_id": "icm-sde-probe", "cwd": "/work",
                          "hook_event_name": "SessionStart", "source": "startup"})
        self.assertGreater(icm["start_chars"], 0)
        self.assertEqual((icm["start_task_lines"], icm["prompt_task_lines"]), (0, 0))
        self.assertIsNone(icm["memory_run"])                       # a dry run is not a run
        self.assertFalse(icm["hook_fired"])
        calls = self.calls()
        flat = [" ".join(c) for c in calls]
        # No agent process, hence no model call.
        self.assertFalse(any("claude" in c and "-p" in c for c in calls))
        # The agent container is the ICM image; grading stays upstream's image.
        self.assertTrue(any(c[:2] == ["run", "-d"] and "icm-sde-agent-claude" in c for c in calls))
        self.assertTrue(any("pytest" in c and "sdebench-base" in c for c in calls))
        # Seed copied in, never mounted.
        self.assertTrue(any(c[0] == "cp" and c[2].endswith(":/root/icm/memories.db") for c in calls))
        self.assertFalse(any("memories.db:" in f for f in flat))
        # Same exposure as the reference memory arm: no transcripts on disk, no Hindsight hook.
        self.assertFalse(any("project-history" in f or "hindsight-coding-agents" in f for f in flat))
        # The container is removed.
        self.assertTrue(any(c[:2] == ["rm", "-f"] for c in calls))
        # The probe received the real first prompt.
        probe = next(e for e in result["memory_diag"] if e["event"] == "icm_probe_prompt")
        self.assertTrue(probe["query"].startswith("You are a maintainer of the `boltons-dedupe`"))
        self.assertLessEqual(len(probe["query"].encode()), 200)
        payload = json.loads((self.state / "probe-prompt.json").read_text())
        self.assertEqual(payload["cwd"], "/work")
        self.assertIn("merge_records", payload["prompt"])

    def test_dry_run_shows_what_each_hook_carries_of_the_task_documents(self):
        done = self.wrapper("--dry-run", env={"ICM_SDE_DB": str(self.seed),
                                              "FAKE_DOCKER_START_LINE": self.own_line,
                                              "FAKE_DOCKER_PROMPT_LINE": f"[user]: {self.own_line}"})
        self.assertEqual(done.returncode, 0, done.stderr[-2000:])
        result = json.loads((self.work / "result.json").read_text())
        # Reported, not refused: the dry run is where this is looked at.
        self.assertEqual(result["icm"]["start_task_lines"], 1)
        self.assertEqual(result["icm"]["prompt_task_lines"], 1)
        start = next(e for e in result["memory_diag"] if e["event"] == "icm_probe_start")
        self.assertEqual(start["task_lines"], [self.own_line])

    def test_paid_run_refuses_a_task_delivered_by_position(self):
        done = self.wrapper(env={"ICM_SDE_DB": str(self.seed), "FAKE_DOCKER_START_LINE": self.own_line})
        self.assertNotEqual(done.returncode, 0)
        self.assertIn("answered by position", done.stderr)
        self.assertFalse((self.work / "result.json").exists())
        calls = self.calls()
        self.assertFalse(any("claude" in c and "-p" in c for c in calls))       # nothing was paid
        self.assertTrue(any(c[:2] == ["rm", "-f"] for c in calls))
        # The same line found by the search hook is retrieval: the task runs.
        shutil.rmtree(self.state)
        self.state.mkdir()
        done = self.wrapper("--max-interventions", "0",
                            env={"ICM_SDE_DB": str(self.seed), "FAKE_DOCKER_PROMPT_LINE": self.own_line})
        self.assertEqual(done.returncode, 0, done.stderr[-2000:])
        result = json.loads((self.work / "result.json").read_text())
        self.assertEqual((result["icm"]["start_task_lines"], result["icm"]["prompt_task_lines"]), (0, 1))

    def test_paid_task_without_hooks_is_not_a_memory_run(self):
        done = self.wrapper("--max-interventions", "0",
                            env={"ICM_SDE_DB": str(self.seed), "FAKE_DOCKER_BREAK": "nohook"})
        self.assertEqual(done.returncode, icm_sde_run.EXIT_NO_HOOK, done.stderr[-2000:])
        self.assertIn("not a memory run", done.stderr)
        result = json.loads((self.work / "result.json").read_text())
        self.assertFalse(result["icm"]["hook_fired"])
        self.assertIs(result["icm"]["memory_run"], False)
        self.assertEqual(sum("claude" in c and "-p" in c for c in self.calls()), 1)   # the agent did run

    def test_dry_run_without_key_mounts_no_credentials_file(self):
        env = {k: v for k, v in self.env.items() if k != "ANTHROPIC_API_KEY"}
        cmd = [sys.executable, str(CODING / "icm_sde_run.py"), "--task", str(self.task),
               "--agent", "claude-code", "--run-id", self.run_id, "--dry-run"]
        done = subprocess.run(cmd, capture_output=True, text=True, stdin=subprocess.DEVNULL, env=env, timeout=600)
        self.assertEqual(done.returncode, 0, done.stderr[-2000:])
        started = next(c for c in self.calls() if c[:2] == ["run", "-d"])
        self.assertFalse(any("claude_creds" in a or ".credentials.json" in a for a in started), started)
        self.assertIn(f"ANTHROPIC_API_KEY={icm_sde_run.DRY_RUN_KEY}", started)
        self.assertFalse((self.tmp / "home" / ".sdebench").exists())

    def test_paid_run_without_credentials_starts_no_container(self):
        env = {k: v for k, v in self.env.items() if k != "ANTHROPIC_API_KEY"}
        cmd = [sys.executable, str(CODING / "icm_sde_run.py"), "--task", str(self.task),
               "--agent", "claude-code", "--run-id", self.run_id]
        done = subprocess.run(cmd, capture_output=True, text=True, stdin=subprocess.DEVNULL, env=env, timeout=600)
        self.assertNotEqual(done.returncode, 0)
        self.assertIn("no Claude credential", done.stderr)
        self.assertFalse(any(c[:2] == ["run", "-d"] for c in self.calls()))

    def test_agent_path_with_a_scripted_agent(self):
        done = self.wrapper("--max-interventions", "1", env={"ICM_SDE_DB": str(self.seed)})
        self.assertEqual(done.returncode, 0, done.stderr[-2000:])
        result = json.loads((self.work / "result.json").read_text())
        agent_calls = [c for c in self.calls() if "claude" in c and "-p" in c]
        self.assertEqual(len(agent_calls), 2)                      # initial + one correction
        self.assertNotIn("--continue", agent_calls[0])
        self.assertIn("--continue", agent_calls[1])
        self.assertIn("claude-sonnet-5", agent_calls[0])
        self.assertEqual(result["interventions"], 1)
        self.assertTrue(result["capped"])
        self.assertEqual(result["tokens"]["output"], 40)
        self.assertAlmostEqual(result["cost_usd"], 0.02)
        self.assertFalse(result["icm"]["dry_run"])
        self.assertEqual(result["icm"]["hook_prompt_rows"], 2)
        self.assertTrue(result["icm"]["hook_fired"])
        self.assertTrue(result["icm"]["session_start_fired"])
        self.assertIs(result["icm"]["memory_run"], True)
        self.assertEqual(result["icm"]["rounds"], 2)
        # What ICM's own write hooks stored between the first turn and the correction.
        self.assertEqual(result["icm"]["stored_during_task"], 2)
        # Each hook is replayed once, before the first turn; a correction replays nothing.
        self.assertEqual([e["event"] for e in result["memory_diag"]],
                         ["icm_setup", "icm_probe_start", "icm_probe_prompt", "icm_collect"])

    def test_solved_task_is_reported_by_upstream_grading(self):
        done = self.wrapper(env={"ICM_SDE_DB": str(self.seed), "FAKE_DOCKER_BREAK": "grade-pass"})
        self.assertEqual(done.returncode, 0, done.stderr[-2000:])
        result = json.loads((self.work / "result.json").read_text())
        self.assertTrue(result["solved"])
        self.assertEqual(result["interventions"], 0)

    def test_broken_wiring_stops_the_task(self):
        for broken in ("wiring", "mcp", "llm-extraction"):
            with self.subTest(broken=broken):
                shutil.rmtree(self.state)
                self.state.mkdir()
                done = self.wrapper(env={"ICM_SDE_DB": str(self.seed), "FAKE_DOCKER_BREAK": broken})
                self.assertNotEqual(done.returncode, 0)
                self.assertIn("WiringError", done.stderr)
                self.assertFalse((self.work / "result.json").exists())
                calls = self.calls()
                self.assertTrue(any(c[:2] == ["rm", "-f"] for c in calls))
                self.assertFalse(any("claude" in c and "-p" in c for c in calls))

    def test_the_extraction_provider_asked_for_reaches_the_setup(self):
        # The image runs ICM's default provider (it calls a model): refused unless the
        # run asks for it, and then recorded with the result.
        env = {"ICM_SDE_DB": str(self.seed), "FAKE_DOCKER_BREAK": "llm-extraction"}
        done = self.wrapper("--dry-run", env={**env, "ICM_SDE_EXTRACTION": "auto"})
        self.assertEqual(done.returncode, 0, done.stderr[-2000:])
        self.assertEqual(json.loads((self.work / "result.json").read_text())["icm"]["extraction"], "auto")
        setup = next(c for c in self.calls() if c[0] == "exec" and "claude-code" in c)
        self.assertIn("ICM_SDE_EXTRACTION=auto", setup)
        shutil.rmtree(self.state)
        self.state.mkdir()
        done = self.wrapper("--dry-run", env=env)
        self.assertNotEqual(done.returncode, 0)
        self.assertIn("extraction-none", done.stderr)
        setup = next(c for c in self.calls() if c[0] == "exec" and "claude-code" in c)
        self.assertIn("ICM_SDE_EXTRACTION=none", setup)

    def test_missing_seed_file_is_refused_before_any_container(self):
        done = self.wrapper(env={"ICM_SDE_DB": str(self.tmp / "absent.db")})
        self.assertNotEqual(done.returncode, 0)
        self.assertIn("does not exist", done.stderr)
        self.assertEqual(self.calls(), [])

    def test_unseeded_run_is_labelled_unseeded(self):
        done = self.wrapper("--dry-run")
        self.assertEqual(done.returncode, 0, done.stderr[-2000:])
        result = json.loads((self.work / "result.json").read_text())
        self.assertFalse(result["icm"]["seeded"])
        self.assertFalse(any(c[0] == "cp" for c in self.calls()))

    def test_other_arms_go_through_untouched(self):
        work = Path("/tmp/sdebench/run") / f"{TASK}-001_full_{self.run_id}"
        try:
            done = self.wrapper("--history", "full", "--max-interventions", "0")
            self.assertEqual(done.returncode, 0, done.stderr[-2000:])
            result = json.loads((work / "result.json").read_text())
            self.assertEqual(result["history"], "full")
            self.assertNotIn("icm", result)
            calls = self.calls()
            self.assertTrue(any(c[:2] == ["run", "-d"] and "sdebench-agent-claude" in c for c in calls))
            self.assertFalse(any("icm-sde-agent-claude" in c for c in calls))
            # Upstream's own vanilla exposure: transcripts written outside the repo.
            self.assertTrue(any("project-history" in " ".join(c) for c in calls))
        finally:
            shutil.rmtree(work, ignore_errors=True)

    def amb(self, *args: str, env: dict | None = None) -> subprocess.CompletedProcess:
        cmd = [sys.executable, str(CODING / "run_sdebench.py"), "run", "--dataset", "sdebench",
               "--split", "boltons", "--mode", "coding", "--output-dir", str(self.tmp / "out"), *args]
        return subprocess.run(cmd, capture_output=True, text=True, stdin=subprocess.DEVNULL,
                              env={**self.env, "SDE_TASK_FILTER": TASK, **(env or {})},
                              timeout=900)

    def test_amb_runner_dry_run_end_to_end(self):
        done = self.amb("--memory", "icm-coding", "--name", "icm-dry", "--query-id", f"{TASK}-history-001",
                        env={"ICM_SDE_DRY_RUN": "1"})
        self.assertEqual(done.returncode, 0, (done.stdout + done.stderr)[-3000:])
        out = json.loads((self.tmp / "out" / "sdebench" / "icm-dry" / "coding" / "boltons.json").read_text())
        self.assertEqual(out["memory_provider"], "icm-coding")
        self.assertEqual(out["total_queries"], 1)
        row = out["results"][0]
        self.assertEqual(row["query_id"], f"{TASK}-history-001")
        self.assertFalse(row["correct"])
        self.assertEqual(row["meta"]["interventions"], 0)
        # The two hooks' outputs, apart, SessionStart first.
        self.assertTrue(row["context"].startswith("## Memory (ICM SessionStart hook"))
        self.assertIn("\n\n## Memory (ICM UserPromptSubmit hook, first prompt", row["context"])
        for field in ("dry_run=True", "hook_fired=False", "seed_task_facts=1",
                      "start_task_lines=0", "prompt_task_lines=0"):
            self.assertIn(field, row["reasoning"])
        unit = (self.tmp / "out" / "sdebench" / "icm-dry" / "_store" / "boltons" / "all"
                / "icm-coding" / f"{TASK}-history-001")
        seed = json.loads((unit / "seed.json").read_text())
        # History task: 140 decoy conversations, the decision commit + 100 host commits.
        self.assertEqual(seed["corpus"]["sessions"], 140)
        self.assertEqual(seed["corpus"]["commits"], 101)
        self.assertEqual(seed["project"], "boltons")
        self.assertEqual(seed["import"], "default")
        self.assertEqual(seed["task_facts"], 1)
        # One directory, imported in one pass; the decision commit is in the middle
        # half of the recorded order and its file name does not give it away.
        self.assertEqual(seed["corpus"]["order_seed"], icm_corpus.ORDER_SEED_DEFAULT)
        files = sorted(p.name for p in (unit / "corpus").iterdir())
        self.assertEqual(len(files), 241)
        self.assertFalse(any("decision" in f or "chat" in f or "decoy" in f for f in files))
        (own,) = seed["corpus"]["task_documents"]
        self.assertEqual((own["id"], own["kind"], own["of"]), (f"{TASK}-history-001:decision-commit", "commit", 241))
        self.assertTrue(60 <= own["rank"] < 181, own)
        self.assertIn("Git commit: ", (unit / "corpus" / own["file"]).read_text())
        seeding = next(c for c in self.calls() if any(a.endswith(":/corpus:ro") for a in c))
        self.assertFalse(any("GIT_INGEST" in a for a in seeding))
        self.assertIn("ICM_SDE_IMPORT=default", seeding)
        turn = json.loads(next((unit / "corpus").glob("*.jsonl")).read_text().splitlines()[0])
        self.assertIn(turn["type"], ("user", "assistant"))
        self.assertTrue(turn["message"]["content"])
        # The stats script reads the adapter's measures back from the row.
        stats = sdebench_stats.summarize(sdebench_stats.task_rows(out))
        self.assertEqual((stats["icm"]["in_seed"], stats["icm"]["by_position"], stats["icm"]["hooks"]),
                         ([1, 1], [0, 1], [0, 1]))
        calls = self.calls()
        seeded_db = (unit / "memories.db").resolve()
        self.assertTrue(any(c[0] == "cp" and Path(c[1]).resolve() == seeded_db for c in calls))
        self.assertFalse(any("claude" in c and "-p" in c for c in calls))
        for d in Path("/tmp/sdebench/run").glob(f"{TASK}-history-001_icm_icm-*"):
            shutil.rmtree(d, ignore_errors=True)

    def test_repetition_reuses_the_seed(self):
        task = f"{TASK}-001"
        first = self.amb("--memory", "icm-coding", "--name", "rep-1", "--query-id", task,
                         env={"ICM_SDE_DRY_RUN": "1", "ICM_SDE_IMPORT": "rules"})
        self.assertEqual(first.returncode, 0, (first.stdout + first.stderr)[-2000:])
        out = self.tmp / "out" / "sdebench"
        unit = out / "rep-1" / "_store" / "boltons" / "all" / "icm-coding" / task
        self.assertEqual(json.loads((unit / "seed.json").read_text())["import"], "rules")
        seeding = next(c for c in self.calls() if any(a.endswith(":/corpus:ro") for a in c))
        self.assertIn("ICM_SDE_IMPORT=rules", seeding)
        (out / "rep-2").mkdir()
        shutil.copytree(out / "rep-1" / "_store", out / "rep-2" / "_store")
        (self.state / "calls.jsonl").unlink()
        second = self.amb("--memory", "icm-coding", "--name", "rep-2", "--query-id", task,
                          "--skip-ingestion", env={"ICM_SDE_DRY_RUN": "1"})
        self.assertEqual(second.returncode, 0, (second.stdout + second.stderr)[-2000:])
        calls = self.calls()
        self.assertFalse(any(":/corpus:ro" in " ".join(c) for c in calls))      # no re-seeding
        seeded = (out / "rep-2" / "_store" / "boltons" / "all" / "icm-coding" / task / "memories.db").resolve()
        self.assertTrue(any(c[0] == "cp" and Path(c[1]).resolve() == seeded for c in calls))
        row = json.loads((out / "rep-2" / "coding" / "boltons.json").read_text())["results"][0]
        self.assertEqual(row["query_id"], task)
        for d in Path("/tmp/sdebench/run").glob(f"{task}_icm_icm-*"):
            shutil.rmtree(d, ignore_errors=True)

    def test_preflight_names_what_is_missing(self):
        env = {k: v for k, v in self.env.items() if k != "SDEBENCH_BOLTONS_HOST"}
        cmd = [sys.executable, str(CODING / "run_sdebench.py"), "run", "--dataset", "sdebench",
               "--split", "boltons", "--mode", "coding", "--memory", "icm-coding"]
        done = subprocess.run(cmd, capture_output=True, text=True, stdin=subprocess.DEVNULL, env=env, timeout=120)
        self.assertNotEqual(done.returncode, 0)
        self.assertIn("SDEBENCH_BOLTONS_HOST is not set", done.stderr)
        self.assertEqual(self.calls(), [])

    def test_amb_runner_refuses_to_score_a_task_without_hooks(self):
        done = self.amb("--memory", "icm-coding", "--name", "icm-nohook", "--query-id", f"{TASK}-001",
                        env={"FAKE_DOCKER_BREAK": "nohook"})
        self.assertNotEqual(done.returncode, 0)
        self.assertIn("without ICM's hooks", done.stdout + done.stderr)
        out = self.tmp / "out" / "sdebench" / "icm-nohook" / "coding" / "boltons.json"
        rows = json.loads(out.read_text()).get("results", []) if out.is_file() else []
        self.assertEqual(rows, [])
        self.assertEqual(sum("claude" in c and "-p" in c for c in self.calls()), 6)   # one task, then stop
        for d in Path("/tmp/sdebench/run").glob(f"{TASK}-001_icm_icm-*"):
            shutil.rmtree(d, ignore_errors=True)

    def test_amb_runner_runs_the_upstream_control_arm(self):
        # Upstream starts every arm but icm-coding with `uv run python run.py` in AMB_HOME.
        uv = self.tmp / "bin" / "uv"
        uv.write_text('#!/bin/sh\n[ "$1" = run ] && shift\nexec "$@"\n')
        uv.chmod(0o755)
        done = self.amb("--memory", "vanilla", "--name", "vanilla-1", "--query-id", f"{TASK}-001",
                        env={"SDE_AGENT_IMAGE_CLAUDE": "icm-sde-agent-claude"})
        self.assertEqual(done.returncode, 0, (done.stdout + done.stderr)[-3000:])
        out = json.loads((self.tmp / "out" / "sdebench" / "vanilla-1" / "coding" / "boltons.json").read_text())
        row = out["results"][0]
        self.assertEqual((out["memory_provider"], row["query_id"]), ("vanilla", f"{TASK}-001"))
        self.assertIn("arm=full", row["reasoning"])
        self.assertEqual(row["meta"]["interventions"], 5)             # the scripted agent never fixes it
        calls = self.calls()
        # Same image as the ICM arm, upstream's own exposure, and nothing of ICM's.
        self.assertTrue(any(c[:2] == ["run", "-d"] and "icm-sde-agent-claude" in c for c in calls))
        self.assertTrue(any("project-history" in " ".join(c) for c in calls))
        self.assertFalse(any(c[0] == "cp" or "/corpus" in " ".join(c) for c in calls))
        self.assertIsNone(sdebench_stats.summarize(sdebench_stats.task_rows(out))["icm"])
        for d in Path("/tmp/sdebench/run").glob(f"{TASK}-001_full_omb-*"):
            shutil.rmtree(d, ignore_errors=True)

    def test_amb_runner_stops_on_failed_seed(self):
        done = self.amb("--memory", "icm-coding", "--name", "icm-bad", "--query-id", f"{TASK}-001",
                        env={"ICM_SDE_DRY_RUN": "1", "FAKE_DOCKER_BREAK": "seed"})
        self.assertNotEqual(done.returncode, 0)
        self.assertIn("seed", (done.stdout + done.stderr).lower())
        self.assertFalse(any(c[:2] == ["run", "-d"] for c in self.calls()))


if __name__ == "__main__":
    unittest.main()
