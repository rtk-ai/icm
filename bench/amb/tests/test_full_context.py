#!/usr/bin/env python3
"""The full-context baseline: what it hands the answer model, and that it is given
exactly what the other providers are given. Offline: the harness runs for real, the
two models are counting stand-ins (dry_run_tokens.py).

    AMB_HOME=/path/to/agent-memory-benchmark python tests/test_full_context.py

The LoCoMo part needs the dataset file in the harness cache (AMB_HOME/.datasets/locomo);
it is skipped, not downloaded, when the file is absent.
"""
import json
import os
import sys
import unittest
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from _common import BENCH, PY, amb_home, env, fake_icm_bin, run, workdir

sys.path.insert(0, str(BENCH))


class Provider(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        os.environ["AMB_HOME"] = str(amb_home())
        for var in [v for v in os.environ if v.startswith("ICM_AMB_")]:
            del os.environ[var]
        cls.root = workdir("full-context-unit")
        os.environ["ICM_AMB_BIN"] = str(fake_icm_bin(cls.root))
        os.environ["ICM_AMB_ENGINE"] = "legacy"  # the ICM provider is only asked for its header here
        import run_amb
        cls.cli = run_amb.bootstrap()

    def docs(self):
        from memory_bench.models import Document
        return [
            Document(id="c1_s10", content="ten", user_id="c1", timestamp="2023-03-01T10:00:00+00:00", context="prov 10"),
            Document(id="c1_s2", content="two", user_id="c1", timestamp="2023-01-05T09:00:00+00:00", context="prov 2"),
            Document(id="c1_s0", content="undated", user_id="c1"),
            Document(id="c1_s3", content="three", user_id="c1", timestamp="2023-02-01T00:00:00+00:00"),
            Document(id="c2_s1", content="other unit", user_id="c2", timestamp="2022-01-01T00:00:00+00:00"),
        ]

    def provider(self):
        from full_context_provider import FullContextMemoryProvider
        p = FullContextMemoryProvider()
        p.prepare(self.root / "store")
        return p

    def test_registered_next_to_icm(self):
        import memory_bench.memory as memory
        from full_context_provider import FullContextMemoryProvider
        from icm_provider import IcmMemoryProvider
        self.assertIs(memory.REGISTRY["full-context"], FullContextMemoryProvider)
        self.assertIs(memory.REGISTRY["icm"], IcmMemoryProvider)
        self.assertEqual(memory.REGISTRY["bm25"].__name__, "BM25MemoryProvider")

    def test_whole_unit_oldest_first_whatever_the_query_and_k(self):
        p = self.provider()
        p.ingest(self.docs())
        for query, k in (("anything", 1), ("", None), ("two", 50)):
            got, raw = p.retrieve(query, k, "c1", "2024-01-01T00:00:00+00:00")
            self.assertEqual([d.id for d in got], ["c1_s2", "c1_s3", "c1_s10", "c1_s0"])
            self.assertIsNone(raw)
        self.assertEqual([d.id for d in p.retrieve("q", 5, "c2")[0]], ["c2_s1"])

    def test_text_is_the_document_with_the_header_icm_writes(self):
        from icm_provider import IcmMemoryProvider
        icm = IcmMemoryProvider()
        p = self.provider()
        p.ingest(self.docs())
        by_id = {d.id: d for d in self.docs()}
        for got in p.retrieve("q", 10, "c1")[0]:
            self.assertEqual(got.content, icm._memory_text(by_id[got.id], by_id[got.id].content))
        self.assertEqual(p.retrieve("q", 10, "c1")[0][0].content, "[2023-01-05T09:00:00+00:00] prov 2\ntwo")
        os.environ["ICM_AMB_HEADER"] = "0"
        try:
            bare = self.provider()
            bare.ingest(self.docs())
            self.assertEqual([d.content for d in bare.retrieve("q", 10, "c1")[0]], ["two", "three", "ten", "undated"])
        finally:
            del os.environ["ICM_AMB_HEADER"]

    def test_nothing_is_counted_twice_and_nothing_is_dropped(self):
        from memory_bench.models import Document
        p = self.provider()
        p.ingest(self.docs())
        p.ingest([d for d in self.docs() if d.user_id == "c1"])  # the same unit handed again
        self.assertEqual(len(p.retrieve("q", 10, "c1")[0]), 4)
        self.assertEqual(len(p.retrieve("q", 10, "c2")[0]), 1)   # the other unit is untouched
        twin = Document(id="c3_s1", content="same id twice", user_id="c3", timestamp="2023-01-01T00:00:00+00:00")
        p.ingest([twin, twin])
        self.assertEqual(len(p.retrieve("q", 10, "c3")[0]), 2)

    def test_a_unit_handed_in_two_calls_keeps_every_document(self):
        from memory_bench.models import Document
        docs = [Document(id=f"u_s{i:02d}", content=f"session {i}", user_id="u", timestamp=f"2023-01-{i + 1:02d}T00:00:00+00:00")
                for i in range(19)]
        p = self.provider()
        p.ingest(docs[:14])
        p.ingest(docs[14:])
        self.assertEqual([d.id for d in p.retrieve("q", 5, "u")[0]], [d.id for d in docs])
        # overlapping calls: what was already handed is not added again, what is new is
        p = self.provider()
        p.ingest(docs[:14])
        p.ingest(docs[10:])
        p.ingest(docs)
        self.assertEqual([d.id for d in p.retrieve("q", 5, "u")[0]], [d.id for d in docs])
        # later documents land at their date, not at the end
        p = self.provider()
        p.ingest(docs[10:])
        p.ingest(docs[:10])
        self.assertEqual([d.id for d in p.retrieve("q", 5, "u")[0]], [d.id for d in docs])
        # a new prepare() starts from nothing
        p.prepare(self.root / "store")
        p.ingest(docs[:3])
        self.assertEqual(len(p.retrieve("q", 5, "u")[0]), 3)

    def test_no_silent_empty_context(self):
        from full_context_provider import FullContextMemoryProvider
        p = self.provider()
        with self.assertRaisesRegex(RuntimeError, "no document was ingested for unit 'nobody'"):
            p.retrieve("q", 10, "nobody")
        with self.assertRaisesRegex(RuntimeError, "--skip-ingestion"):
            FullContextMemoryProvider().prepare(self.root / "store", reset=False)


class LoCoMo(unittest.TestCase):
    """Real harness, real locomo10 file, both models replaced by counters."""

    @classmethod
    def setUpClass(cls):
        cls.data = amb_home() / ".datasets" / "locomo" / "locomo10.json"
        if not cls.data.exists():
            raise unittest.SkipTest(f"{cls.data} is not cached; not downloading it in a test")
        cls.root = workdir("full-context-locomo")
        cls.reports = {}
        for memory in ("full-context", "bm25"):
            run(PY, BENCH / "dry_run_tokens.py", "--dataset", "locomo", "--split", "locomo10", "--memory", memory,
                "--output-dir", cls.root / memory, "--record-documents", cls.root / f"{memory}-docs.json",
                "--json", cls.root / f"{memory}.json", env_=env())
            cls.reports[memory] = json.loads((cls.root / f"{memory}.json").read_text())

    def test_same_documents_as_the_bm25_provider(self):
        full = json.loads((self.root / "full-context-docs.json").read_text())
        bm25 = json.loads((self.root / "bm25-docs.json").read_text())
        self.assertEqual(full, bm25)
        self.assertEqual(len(full), 10)
        self.assertEqual(sum(len(v) for v in full.values()), 272)

    def test_context_is_every_session_of_the_conversation_in_date_order_and_nothing_else(self):
        raw = json.loads(self.data.read_text())
        expected = {}
        for item in raw:
            conv, sample = item["conversation"], item["sample_id"]
            blocks = []
            for order, key in enumerate(sorted(k for k, v in conv.items()
                                               if k.startswith("session_") and not k.endswith("_date_time") and isinstance(v, list))):
                if not conv[key]:
                    continue
                try:
                    when = datetime.strptime(conv.get(f"{key}_date_time") or "", "%I:%M %p on %d %B, %Y").replace(tzinfo=timezone.utc)
                except ValueError:
                    when = None
                head = (f"[{when.isoformat()}] " if when else "") + \
                    f"Conversation between {conv['speaker_a']} and {conv['speaker_b']} ({key} of {sample})"
                blocks.append((when is None, when.timestamp() if when else 0, order, f"{head}\n{json.dumps(conv[key])}"))
            expected[sample] = "\n\n".join(f"## Memory {i + 1}\n{text}" for i, (*_, text) in enumerate(sorted(blocks)))
        result = json.loads((self.root / "full-context" / "locomo" / "dry-run-full-context" / "rag" / "locomo10.json").read_text())
        self.assertEqual(len(result["results"]), 1540)
        for row in result["results"]:
            self.assertEqual(row["context"], expected[row["meta"]["sample_id"]], row["query_id"])
        # the context of a question does not depend on the question: one context per conversation
        self.assertEqual(len({row["context"] for row in result["results"]}), 10)

    def test_measured_volume(self):
        full, bm25 = self.reports["full-context"], self.reports["bm25"]
        for report in (full, bm25):
            self.assertEqual((report["questions"], report["ingested_docs"], report["empty_contexts"]), (1540, 272, 0))
            self.assertEqual((report["answer_calls"], report["judge_calls"]), (1540, 1540))
        self.assertGreater(full["context_tokens"]["min"], bm25["context_tokens"]["max"])
        print(f"\n      full-context: {full['context_tokens']}\n      answer prompts: {full['answer_prompt_tokens']}"
              f"\n      bm25: {bm25['context_tokens']}\n      answer prompts: {bm25['answer_prompt_tokens']}")


if __name__ == "__main__":
    unittest.main(verbosity=2)
