#!/usr/bin/env python3
"""recall_only.py and merge_recall.py, run as the command lines they are. Offline.

A six-question file in the LongMemEval-S format stands in for the dataset
(LONGMEMEVAL_DATA_PATH), `fake_icm.py` for the `icm` binary. No model, no network.

    AMB_HOME=/path/to/agent-memory-benchmark python tests/test_recall_only.py
"""
import json
import os
import shutil
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from _common import BENCH, PY, amb_home, env, fake_icm_bin, jsonl, lme_items, path_with_python, run, workdir, write_lme

sys.path.insert(0, str(BENCH))
import merge_recall  # noqa: E402
import recall_only  # noqa: E402

REL = "longmemeval/{name}/recall/s.json"


def published_unit(turns: list[dict], unit: str) -> str | None:
    """The indexed text as the two published protocols define it (written from their code, not ours)."""
    if unit == "session-user":  # MemPalace longmemeval_bench.py, build_palace_and_retrieve
        user = [t["content"] for t in turns if t["role"] == "user"]
        return "\n".join(user) if user else None
    return "\n".join(f"{t['role']}: {t['content']}" for t in turns)  # agentmemory longmemeval-bench.ts


class Case(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        amb_home()
        cls.root = workdir(cls.__name__)
        cls.data = write_lme(cls.root)
        cls.icm = fake_icm_bin(cls.root)

    def env(self, **extra) -> dict:
        return env(LONGMEMEVAL_DATA_PATH=self.data, ICM_AMB_BIN=self.icm, **extra)

    def recall(self, out: Path, *args, env_: dict | None = None, check: bool = True):
        return run(PY, BENCH / "recall_only.py", "run", "--dataset", "longmemeval", "--split", "s",
                   "--output-dir", out, *args, env_=env_ or self.env(), check=check)

    def merge(self, *args, check: bool = True):
        return run(PY, BENCH / "merge_recall.py", *args, env_=self.env(), check=check)

    @staticmethod
    def rows(path: Path) -> dict[str, dict]:
        return {r["query_id"]: r for r in json.loads(path.read_text())["results"]}


class Units(unittest.TestCase):
    def test_split_bytes_keeps_everything_under_the_limit(self):
        text = "\n".join(f"line {i} é" for i in range(5000)) + "\n" + "x" * 150_000
        parts = recall_only.split_bytes(text, 60_000)
        self.assertTrue(all(len(p.encode()) <= 60_000 for p in parts))
        self.assertEqual("".join(p.replace("\n", "") for p in parts), text.replace("\n", ""))
        self.assertEqual(recall_only.split_bytes("short", 60_000), ["short"])

    def test_tokenizers(self):
        self.assertEqual(recall_only.tokenize("What degree, did I get?", "words"), ["what", "degree", "did", "i", "get"])
        self.assertEqual(recall_only.tokenize("What degree, did I get?", "harness"), ["what", "degree,", "did", "i", "get?"])


class Bm25(Case):
    def test_units_are_the_published_ones_and_scores_follow(self):
        out = self.root / "bm25"
        self.recall(out, "--memory", "bm25", "--name", "user")
        self.recall(out, "--memory", "bm25", "--name", "all", "--unit", "session-all")
        user, every = self.rows(out / REL.format(name="user")), self.rows(out / REL.format(name="all"))

        # qa: d3 (assistant only) is not indexed by session-user, is found by session-all
        self.assertEqual(user["qa"]["ranked_ids"], ["qa_answer_qa_1"])
        self.assertIn("qa_d3", every["qa"]["ranked_ids"])
        self.assertEqual((user["qa"]["corpus_docs"], user["qa"]["corpus_units"]), (4, 3))
        self.assertEqual(every["qa"]["corpus_units"], 4)
        # gold is the dataset's answer_session_ids, also where no turn carries has_answer
        self.assertEqual(user["qc_abs"]["gold_ids"], ["qc_abs_answer_qc_1"])
        self.assertEqual(user["qc_abs"]["gold_ids_harness"], [])
        self.assertEqual(user["qb"]["gold_ids"], ["qb_answer_qb_1", "qb_answer_qb_2"])
        # a unit sharing no word with the question is not returned
        self.assertNotIn("qb_answer_qb_2", user["qb"]["ranked_ids"])
        # the oversize session is split in two units, and found through the part that holds the words
        self.assertEqual(user["qd"]["corpus_units"], 4)
        self.assertEqual(user["qd"]["ranked_ids"].count("qd_answer_qd_1"), 1)
        self.assertEqual(user["qd"]["ranked_ids"].count("qd_dup_1"), 2)
        # an assistant-only answer is out of reach of the session-user unit when the user turn does not match
        self.assertEqual(every["qf"]["ranked_ids"][0], "qf_answer_qf_1")

        merged = self.root / "bm25-merged.json"
        proc = self.merge(out / REL.format(name="user"), "--expect-queries", 6, "--expect-docs", 16, "-o", merged)
        doc = json.loads(merged.read_text())
        m = doc["metrics"]
        self.assertEqual((doc["ingested_docs"], doc["ingested_units"], doc["docs_not_indexed"], doc["docs_split"]), (16, 16, 1, 1))
        self.assertEqual(m["all"]["questions"], 6)
        self.assertEqual(m["without_abstention"]["questions"], 5)
        self.assertEqual(m["all"]["recall_any@5_count"], 6)
        self.assertEqual(m["all"]["recall_all@5_count"], 5)       # qb misses its second session
        self.assertEqual(m["by_question_type"]["multi-session"]["recall_all@5"], 0.0)
        self.assertEqual(m["harness_gold_definition"]["questions"], 5)  # qc_abs has no has_answer turn
        self.assertEqual((doc["answer_llm"], doc["judge_llm"], doc["llm_calls"]), (None, None, 0))
        self.assertIn("gold = answer_session_ids", proc.stdout)

    def test_header_and_chunk_units(self):
        out = self.root / "variants"
        self.recall(out, "--memory", "bm25", "--name", "chunk", "--unit", "chunk", env_=self.env(ICM_AMB_CHUNK_TOKENS=64))
        doc = json.loads((out / REL.format(name="chunk")).read_text())
        self.assertEqual((doc["unit"], doc["header"], doc["chunk_tokens"]), ("chunk", True, 64))
        self.assertGreater(doc["units"]["qd"]["units"], 100)  # the long session became many 64-token chunks
        proc = self.recall(out, "--memory", "bm25", "--name", "bad", "--unit", "sentence", check=False)
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("expected one of", proc.stderr)
        proc = self.recall(out, "--memory", "qdrant", "--name", "bad", check=False)
        self.assertIn("expected `icm` or `bm25`", proc.stderr)


class Icm(Case):
    def stored(self, out: Path, name: str) -> dict[str, list[dict]]:
        store = out / "longmemeval" / name / "_store" / "s" / "all" / "icm"
        return {p.name.split("-")[0]: jsonl(p) for p in sorted(store.glob("*.store.jsonl"))}

    def test_icm_gets_the_same_units_as_the_published_protocols(self):
        items = {i["question_id"]: i for i in lme_items()}
        for unit in ("session-user", "session-all"):
            out = self.root / f"icm-{unit}"
            self.recall(out, "--memory", "icm", "--name", "icm", "--unit", unit, env_=self.env(ICM_AMB_ENGINE="v2"))
            stored = self.stored(out, "icm")
            for qid, item in items.items():
                want = [published_unit(turns, unit) for turns in item["haystack_sessions"]]
                want = [w for w in want if w is not None]
                got = [s["content"] for s in stored[qid]]
                if qid != "qd":
                    self.assertEqual(got, want, f"{unit} {qid}")
                else:  # the oversize session arrives in two parts that add up to the published text
                    self.assertEqual(len(got), len(want) + 1)
                    self.assertTrue(all(len(g.encode()) <= 64 * 1024 for g in got))
                    self.assertEqual("\n".join(got[1:3]), want[1])
                self.assertTrue(all(s["topic"] == "conversations" for s in stored[qid]))
                self.assertTrue(all(s["created_at"].startswith("2023-05-1") for s in stored[qid]), "v2: document date sent")
        out = self.root / "icm-session-user"
        rows = self.rows(out / REL.format(name="icm"))
        self.assertEqual(rows["qa"]["ranked_ids"], ["qa_answer_qa_1"])  # memory ids mapped back to session ids
        self.assertIn("qd_answer_qd_1", rows["qd"]["ranked_ids"])
        recalls = jsonl(next((out / "longmemeval/icm/_store/s/all/icm").glob("qa-*.recall.jsonl")))
        asked = [r for r in recalls if r["query"] != "capability probe"]
        self.assertEqual(len(asked), 1)
        self.assertEqual((asked[0]["query"], asked[0]["limit"], asked[0]["engine"]), ("which zebra did alice adopt", 50, "v2"))
        self.assertTrue(asked[0]["now"].startswith("2023-05-30"), "v2: question date sent")
        doc = json.loads((out / REL.format(name="icm")).read_text())
        self.assertEqual(doc["provider"]["engine"], "v2")
        self.assertEqual(doc["provider"]["icm_version"], "0.0.0-fake")

    def test_chunk_unit_is_what_an_answer_run_stores(self):
        """`--unit chunk` must hand ICM the very memories the default provider writes in a rag run."""
        out = self.root / "icm-chunk"
        e = self.env(ICM_AMB_CHUNK_TOKENS=64, ICM_AMB_ENGINE="v2")
        self.recall(out, "--memory", "icm", "--name", "icm", "--unit", "chunk", "--query-limit", 3, env_=e)
        via_recall = self.stored(out, "icm")
        ref = self.root / "icm-default"
        code = (
            "import sys, pathlib; sys.path.insert(0, sys.argv[1]); import run_amb; run_amb.bootstrap()\n"
            "from memory_bench.dataset import get_dataset; from icm_provider import IcmMemoryProvider\n"
            "ds = get_dataset('longmemeval'); p = IcmMemoryProvider(); p.prepare(pathlib.Path(sys.argv[2]))\n"
            "qs = ds.load_queries('s', limit=3)\n"
            "for q in qs: p.ingest(ds.load_documents('s', user_ids={q.user_id}))\n"
            "p.cleanup()\n"
        )
        run(PY, "-c", code, BENCH, ref, env_=e)
        default = {p.name.split("-")[0]: jsonl(p) for p in sorted((ref / "icm").glob("*.store.jsonl"))}
        self.assertEqual(sorted(default), ["qa", "qb", "qc_abs"])
        for qid in default:
            self.assertEqual([s["content"] for s in via_recall[qid]], [s["content"] for s in default[qid]], qid)
            # (the harness' LongMemEval loader keeps the day of a session and drops its time of day)
            self.assertTrue(via_recall[qid][0]["content"].startswith("[2023-05-10T00:00:00+00:00] Session "), "date header kept")

    def test_token_budget_is_refused(self):
        proc = self.recall(self.root / "budget", "--memory", "icm",
                           env_=self.env(ICM_AMB_MAX_TOKENS=8000, ICM_AMB_ENGINE="v2"), check=False)
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("ICM_AMB_MAX_TOKENS", proc.stderr)

    def test_v2_without_dates_is_what_a_system_given_no_date_gets(self):
        """The setting to put next to published figures whose protocol sends the question text only."""
        out = self.root / "icm-nodate"
        self.recall(out, "--memory", "icm", "--name", "icm-v2-nodate-user", "--description", "LongMemEval-S recall only",
                    env_=self.env(ICM_AMB_ENGINE="v2", ICM_AMB_STORE_DATE=0, ICM_AMB_QUERY_NOW=0))
        stored = self.stored(out, "icm-v2-nodate-user")
        self.assertEqual(sum(len(v) for v in stored.values()), 16)
        self.assertTrue(all("created_at" not in s for unit in stored.values() for s in unit), "no document date on /store")
        store = out / "longmemeval/icm-v2-nodate-user/_store/s/all/icm"
        asked = [r for p in store.glob("*.recall.jsonl") for r in jsonl(p) if r["query"] != "capability probe"]
        self.assertEqual(len(asked), 6)
        self.assertTrue(all(r["engine"] == "v2" and "now" not in r for r in asked), "v2 named, no question date on /recall")
        self.assertTrue(all(set(r) == {"query", "limit", "engine"} for r in asked), "the question text and nothing about time")
        doc = json.loads((out / REL.format(name="icm-v2-nodate-user")).read_text())
        self.assertEqual((doc["provider"]["engine"], doc["provider"]["store_date"], doc["provider"]["query_now"]), ("v2", False, False))
        self.assertEqual(doc["description"], "LongMemEval-S recall only | engine v2, no date sent")
        # the same units, the same ranking input, as the dated run: only the dates differ
        dated = self.stored(self.root / "icm-session-user", "icm") if (self.root / "icm-session-user").exists() else None
        if dated:
            self.assertEqual({q: [s["content"] for s in v] for q, v in stored.items()},
                             {q: [s["content"] for s in v] for q, v in dated.items()})

    def test_session_units_never_load_the_tokenizer(self):
        """A session unit is stored as it is: no tiktoken file, cached or downloaded, is needed for it."""
        poison = self.root / "no-tiktoken"
        poison.mkdir(exist_ok=True)
        (poison / "tiktoken.py").write_text(
            "def get_encoding(*a, **k):\n    raise RuntimeError('tiktoken was loaded: the encoding file is not available here')\n"
            "def encoding_for_model(*a, **k):\n    raise RuntimeError('tiktoken was loaded')\n")
        e = self.env(ICM_AMB_ENGINE="v2", PYTHONPATH=poison)
        for unit in ("session-user", "session-all"):
            out = self.root / f"icm-no-tiktoken-{unit}"
            self.recall(out, "--memory", "icm", "--name", "icm", "--unit", unit, env_=e)
            self.assertEqual(len(self.rows(out / REL.format(name="icm"))), 6)
        # the stand-in does bite: the chunk unit, which cuts by tokens, cannot run with it
        proc = self.recall(self.root / "icm-no-tiktoken-chunk", "--memory", "icm", "--unit", "chunk", env_=e, check=False)
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("tiktoken was loaded", proc.stderr)

    def test_a_memory_id_that_is_no_document_stops_the_run(self):
        out = self.root / "icm-foreign"
        proc = self.recall(out, "--memory", "icm", "--name", "icm", env_=self.env(ICM_AMB_ENGINE="legacy", FAKE_ICM_FOREIGN_ID=1),
                           check=False)
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("are not documents of unit 'qa'", proc.stderr)
        self.assertIn("mem-from-elsewhere", proc.stderr)
        self.assertFalse((out / REL.format(name="icm")).exists(), "nothing scored with an id that maps to no session")
        stats = json.loads((out / "longmemeval/icm/_store/s/all/icm/ingest-stats.json").read_text())
        self.assertEqual(stats["unknown_memory_ids"], 1)


class NoLlm(Case):
    def test_every_llm_entry_point_raises(self):
        code = (
            "import sys; sys.path.insert(0, sys.argv[1]); import run_amb, recall_only; run_amb.bootstrap(); recall_only.forbid_llm()\n"
            "import memory_bench.llm as llm; from memory_bench.llm.gemini import GeminiLLM; from memory_bench.llm.base import Schema\n"
            "hits = 0\n"
            "for call in (llm.get_judge_llm, llm.get_answer_llm, lambda: GeminiLLM.generate(object(), 'p', Schema({}, [])),\n"
            "             lambda: GeminiLLM._generate_raw(object(), 'p')):\n"
            "    try: call()\n"
            "    except RuntimeError as e: hits += 'recall-only run' in str(e)\n"
            "print('refused', hits)\n"
        )
        self.assertIn("refused 4", run(PY, "-c", code, BENCH, env_=self.env()).stdout)

    def test_llm_calls_is_a_count_of_attempts_not_a_constant(self):
        """An attempt swallowed by the caller raises nothing visible: the result file still shows it."""
        out = self.root / "attempts"
        code = (
            "import sys; sys.path.insert(0, sys.argv[1]); import recall_only\n"
            "plain = recall_only.UnitBm25.retrieve\n"
            "def sneaky(self, *a, **k):\n"
            "    import memory_bench.llm as llm\n"
            "    try: llm.get_judge_llm()\n"
            "    except RuntimeError: pass\n"
            "    return plain(self, *a, **k)\n"
            "recall_only.UnitBm25.retrieve = sneaky\n"
            "recall_only.main(['run', '--dataset', 'longmemeval', '--split', 's', '--memory', 'bm25', '--output-dir', sys.argv[2]])\n"
        )
        run(PY, "-c", code, BENCH, out, env_=self.env())
        doc = json.loads((out / REL.format(name="bm25")).read_text())
        self.assertEqual(doc["llm_calls"], 6)  # one swallowed attempt per question
        proc = self.merge(out / REL.format(name="bm25"), "--expect-queries", 6, "-o", self.root / "never-attempts.json", check=False)
        self.assertEqual(proc.returncode, 1)
        self.assertIn("records an LLM (llm_calls=6", proc.stderr)
        # a clean run counts zero, and a resumed file keeps the count of its earlier attempt
        clean = self.root / "attempts-clean"
        self.recall(clean, "--memory", "bm25")
        self.assertEqual(json.loads((clean / REL.format(name="bm25")).read_text())["llm_calls"], 0)
        self.recall(out, "--memory", "bm25", "--skip-ingested")
        self.assertEqual(json.loads((out / REL.format(name="bm25")).read_text())["llm_calls"], 6)

    def test_runs_without_any_credential_or_network_setting(self):
        e = self.env()
        self.assertFalse([k for k in e if k.startswith(("GOOGLE_", "GEMINI_", "OMB_"))])
        self.recall(self.root / "nocreds", "--memory", "bm25", env_=e)


class ResumeAndShards(Case):
    def test_resume_only_redoes_what_is_missing(self):
        out = self.root / "resume"
        self.e = self.env(ICM_AMB_ENGINE="legacy")
        self.recall(out, "--memory", "icm", "--name", "icm", env_=self.e)
        full = json.loads((out / REL.format(name="icm")).read_text())
        # a pod that stopped after 2 units: keep the first two rows, as its last save would have
        partial = dict(full, results=full["results"][:2], units={k: full["units"][k] for k in ("qa", "qb")}, total_queries=2)
        (out / REL.format(name="icm")).write_text(json.dumps(partial))
        proc = self.recall(out, "--memory", "icm", "--name", "icm", "--skip-ingested", env_=self.e)
        self.assertIn("2 already in", proc.stdout)
        store = out / "longmemeval/icm/_store/s/all/icm"
        self.assertEqual(sorted(p.name.split("-")[0] for p in store.glob("*.store.jsonl")), ["qc_abs", "qd", "qe", "qf"])
        again = json.loads((out / REL.format(name="icm")).read_text())
        strip = lambda rows: [{k: v for k, v in r.items() if k != "retrieve_time_ms"} for r in rows]  # noqa: E731
        self.assertEqual(strip(again["results"]), strip(full["results"]))
        self.assertEqual(sorted(again["units"]), sorted(full["units"]))
        # resuming another configuration into the same file is refused
        proc = self.recall(out, "--memory", "icm", "--name", "icm", "--skip-ingested", "--unit", "session-all", env_=self.e,
                           check=False)
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("not resuming a different configuration", proc.stderr)
        # nor another engine, nor the same engine with other dates
        for other in (self.env(ICM_AMB_ENGINE="v2"), self.env(ICM_AMB_ENGINE="binary-default-no-dates")):
            proc = self.recall(out, "--memory", "icm", "--name", "icm", "--skip-ingested", env_=other, check=False)
            self.assertNotEqual(proc.returncode, 0)
            self.assertIn("not resuming a different configuration", proc.stderr)
        # an unreadable previous file stops the run instead of starting over
        (out / REL.format(name="icm")).write_text("{ truncated")
        proc = self.recall(out, "--memory", "icm", "--name", "icm", "--skip-ingested", env_=self.e, check=False)
        self.assertIn("not readable JSON", proc.stderr)

    def test_shards_merge_to_the_unsharded_run_and_incomplete_sets_are_refused(self):
        whole = self.root / "whole"
        self.recall(whole, "--memory", "bm25", "--name", "bm25")
        files = []
        for i in range(3):
            out = self.root / "run" / f"shard-{i}-of-3"
            self.recall(out, "--memory", "bm25", "--name", "bm25", env_=self.env(ICM_AMB_SHARD=f"{i}/3", ICM_AMB_SHARD_BY="rank"))
            files.append(out / REL.format(name="bm25"))
        self.assertEqual([len(self.rows(f)) for f in files], [2, 2, 2])
        merged, ref = self.root / "merged.json", self.root / "ref.json"
        self.merge(*files, "--expect-queries", 6, "--expect-docs", 16, "-o", merged)
        self.merge(whole / REL.format(name="bm25"), "--expect-queries", 6, "--expect-docs", 16, "-o", ref)
        a, b = json.loads(merged.read_text()), json.loads(ref.read_text())
        self.assertEqual(a["metrics"], b["metrics"])
        self.assertEqual({r["query_id"]: r["ranked_ids"] for r in a["results"]}, {r["query_id"]: r["ranked_ids"] for r in b["results"]})

        def refused(*args, needle: str):
            target = self.root / "never.json"
            proc = self.merge(*args, "-o", target, check=False)
            self.assertEqual(proc.returncode, 1, proc.stdout + proc.stderr)
            self.assertIn(needle, proc.stderr)
            self.assertFalse(target.exists())

        refused(*files[:2], "--expect-queries", 6, needle="missing [2]")
        refused(*files, "--expect-queries", 7, needle="6 questions, expected 7")
        refused(*files, "--expect-queries", 6, "--expect-docs", 17, needle="16 documents ingested, expected 17")
        refused(*files, "--expect-queries", 6, "--k", 5, 100, needle="deeper than the lists")
        # a shard run with another indexed unit
        other = self.root / "other" / "shard-2-of-3"
        self.recall(other, "--memory", "bm25", "--name", "bm25", "--unit", "session-all",
                    env_=self.env(ICM_AMB_SHARD="2/3", ICM_AMB_SHARD_BY="rank"))
        refused(files[0], files[1], other / REL.format(name="bm25"), "--expect-queries", 6, needle="shards disagree on unit")
        # an answer-run file, or a file that records a model
        fake = self.root / "rag" / "shard-2-of-3" / REL.format(name="bm25")
        fake.parent.mkdir(parents=True)
        doc = json.loads(files[2].read_text())
        fake.write_text(json.dumps(dict(doc, mode="rag")))
        refused(files[0], files[1], fake, "--expect-queries", 6, needle="not a recall-only result file")
        fake.write_text(json.dumps(dict(doc, judge_llm="gemini:gemini-2.5-flash-lite")))
        refused(files[0], files[1], fake, "--expect-queries", 6, needle="records an LLM")
        # the same question in two shards
        fake.write_text(json.dumps(dict(doc, results=doc["results"] + json.loads(files[0].read_text())["results"][:1])))
        refused(files[0], files[1], fake, "--expect-queries", 6, needle="appears twice")


class Scorer(unittest.TestCase):
    def test_any_all_mrr_and_scopes(self):
        rows = [
            {"query_id": "a", "question_type": "t1", "gold_ids": ["g1"], "ranked_ids": ["x", "g1", "y"], "corpus_units": 9},
            {"query_id": "b", "question_type": "t1", "gold_ids": ["g1", "g2"], "ranked_ids": ["g2", "x", "y", "z", "w", "g1"], "corpus_units": 9},
            {"query_id": "c_abs", "question_type": "t2", "gold_ids": ["g1"], "ranked_ids": [], "corpus_units": 9},
            {"query_id": "d", "question_type": "t2", "gold_ids": [], "ranked_ids": ["x"], "corpus_units": 9},
            {"query_id": "e", "question_type": "t2", "gold_ids": ["g1", "g2"], "ranked_ids": ["g1", "g1", "g1", "g2"], "corpus_units": 9},
        ]
        m = merge_recall.report(rows, [1, 3, 5])
        self.assertEqual((m["all"]["questions"], m["all"]["without_gold"]), (4, 1))
        self.assertEqual([m["all"][f"recall_any@{k}_count"] for k in (1, 3, 5)], [2, 3, 3])
        self.assertEqual([m["all"][f"recall_all@{k}_count"] for k in (1, 3, 5)], [0, 1, 2])
        self.assertAlmostEqual(m["all"]["mrr"], round((1 / 2 + 1 + 0 + 1) / 4, 4))
        self.assertEqual(m["without_abstention"]["questions"], 3)
        self.assertEqual(m["abstention_only"]["recall_any@5"], 0.0)
        self.assertEqual(m["all"]["short_lists@3"], 1)  # c_abs returned nothing out of 9 units
        # top k units vs top k distinct sessions: e needs rank 4 as units, rank 2 as sessions
        self.assertTrue(m["a_session_takes_several_ranks"])
        self.assertEqual(m["distinct_sessions"]["all"]["recall_all@3_count"], 2)
        low, high = merge_recall.wilson(483, 500)
        self.assertAlmostEqual(low, 0.9463, places=3)
        self.assertAlmostEqual(high, 0.9786, places=3)


class Entrypoint(Case):
    def test_mode_recall_runs_the_recall_script_and_resumes(self):
        if not shutil.which("sh"):
            self.skipTest("no sh")
        work, remote = self.root / "pod" / "work", self.root / "pod" / "remote"
        e = self.env(TOOLS=BENCH, WORK_DIR=work, RESULTS_REMOTE=remote, RUN_ID="lme-test", DATASET="longmemeval", SPLIT="s",
                     MODE="recall", MEMORY="bm25", RUN_NAME="bm25-user", SHARDS=3, JOB_COMPLETION_INDEX=1, ICM_AMB_SHARD_BY="rank",
                     ICM_AMB_RECALL_UNIT="session-user", RUN_DESCRIPTION="recall only, bm25", SYNC_INTERVAL=1000,
                     PATH=path_with_python(self.root))
        proc = run("sh", BENCH / "entrypoint.sh", env_=e)
        result = remote / "lme-test" / "shard-1-of-3" / "longmemeval" / "bm25-user" / "recall" / "s.json"
        doc = json.loads(result.read_text())
        self.assertEqual((doc["mode"], doc["memory_provider"], doc["unit"], len(doc["results"])), ("recall", "bm25", "session-user", 2))
        self.assertEqual(doc["description"], "recall only, bm25")
        self.assertIn("[recall_only]", proc.stdout)
        attempts = jsonl(remote / "lme-test" / "shard-1-of-3" / "attempts.jsonl")
        self.assertEqual([(a["event"], a.get("resume"), a.get("status")) for a in attempts], [("start", False, None), ("exit", None, 0)])
        # a replacement pod: downloads the file, resumes, has nothing left to do
        shutil.rmtree(work)
        proc = run("sh", BENCH / "entrypoint.sh", env_=e)
        self.assertIn("2 already in", proc.stdout)
        self.assertEqual(json.loads(result.read_text())["results"], doc["results"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
