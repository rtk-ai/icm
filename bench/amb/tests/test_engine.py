#!/usr/bin/env python3
"""The recall engine is never implicit: the provider and every launcher refuse a run
that names none, and a run that names one says so in its result file, in its
description and on the wire. Offline: `fake_icm.py` stands in for the binary (and,
with FAKE_ICM_NO_ENGINE_FIELD=1, for a build older than v2), no model is called.

    AMB_HOME=/path/to/agent-memory-benchmark python tests/test_engine.py
"""
import json
import os
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from _common import BENCH, PY, amb_home, env, fake_icm_bin, jsonl, run, workdir, write_lme

sys.path.insert(0, str(BENCH))

V2 = "engine v2, document date sent (created_at), question date sent (now)"
V2_NO_DATE = "engine v2, no date sent"
LEGACY = "engine legacy, no date sent"
BINARY_DEFAULT = "engine: the binary's own default (no `engine` field sent), no date sent"


class Base(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        os.environ["AMB_HOME"] = str(amb_home())
        for var in [v for v in os.environ if v.startswith("ICM_AMB_")]:
            del os.environ[var]
        cls.root = workdir(cls.__name__)
        cls.icm = fake_icm_bin(cls.root)
        cls.data = write_lme(cls.root)
        import run_amb
        run_amb.bootstrap()

    def env(self, **extra) -> dict:
        return env(LONGMEMEVAL_DATA_PATH=self.data, ICM_AMB_BIN=self.icm, **extra)


class Settings(Base):
    def settings(self, **environ):
        from icm_provider import engine_settings
        return engine_settings(environ)

    def test_no_engine_is_refused_and_the_message_names_the_three_choices(self):
        for environ in ({}, {"ICM_AMB_ENGINE": ""}, {"ICM_AMB_ENGINE": "default"}, {"ICM_AMB_ENGINE": "v3"}):
            with self.assertRaises(RuntimeError) as stop:
                self.settings(**environ)
            for word in ("ICM_AMB_ENGINE", "`v2`", "`legacy`", "`binary-default-no-dates`"):
                self.assertIn(word, str(stop.exception))

    def test_labels_tell_the_configurations_apart(self):
        cases = {
            V2: {"ICM_AMB_ENGINE": "v2"},
            V2_NO_DATE: {"ICM_AMB_ENGINE": "V2", "ICM_AMB_STORE_DATE": "0", "ICM_AMB_QUERY_NOW": "0"},
            "engine v2, document date sent (created_at), question date not sent": {"ICM_AMB_ENGINE": "v2", "ICM_AMB_QUERY_NOW": "0"},
            "engine v2, document date not sent, question date sent (now)": {"ICM_AMB_ENGINE": "v2", "ICM_AMB_STORE_DATE": "0"},
            LEGACY: {"ICM_AMB_ENGINE": "legacy"},
            BINARY_DEFAULT: {"ICM_AMB_ENGINE": "binary-default-no-dates"},
        }
        self.assertEqual(len(set(cases)), 6)
        for label, environ in cases.items():
            self.assertEqual(self.settings(**environ).label, label)
        dated, bare = self.settings(ICM_AMB_ENGINE="v2"), self.settings(ICM_AMB_ENGINE="legacy")
        self.assertEqual((dated.store_date, dated.query_now, bare.store_date, bare.query_now), (True, True, False, False))

    def test_date_switches_and_budget_belong_to_v2(self):
        for environ in ({"ICM_AMB_ENGINE": "legacy", "ICM_AMB_STORE_DATE": "0"},
                        {"ICM_AMB_ENGINE": "binary-default-no-dates", "ICM_AMB_QUERY_NOW": "1"},
                        {"ICM_AMB_ENGINE": "legacy", "ICM_AMB_MAX_TOKENS": "8000"},
                        {"ICM_AMB_MAX_TOKENS": "8000"}):  # a budget no longer implies an engine
            with self.assertRaises(RuntimeError, msg=environ):
                self.settings(**environ)
        self.assertEqual(self.settings(ICM_AMB_ENGINE="v2", ICM_AMB_MAX_TOKENS="8000").max_tokens, 8000)

    def test_description_gets_the_label_once(self):
        s = self.settings(ICM_AMB_ENGINE="legacy")
        self.assertEqual(s.describe(None), LEGACY)
        self.assertEqual(s.describe("k=50"), f"k=50 | {LEGACY}")
        self.assertEqual(s.describe(s.describe("k=50")), f"k=50 | {LEGACY}")


class Provider(Base):
    def drive(self, name: str, **environ) -> tuple[dict, list[dict], list[dict], list[dict], str]:
        """Ingest two dated documents and ask one dated question; return what was written and sent."""
        from icm_provider import IcmMemoryProvider
        from memory_bench.models import Document

        store = self.root / name
        trace = self.root / f"{name}-http.jsonl"
        keep = dict(os.environ)
        os.environ.update(ICM_AMB_BIN=str(self.icm), ICM_AMB_HTTP_TRACE=str(trace), **environ)
        try:
            p = IcmMemoryProvider()
            p.prepare(store)
            p.ingest([Document(id="u_s1", content="alice adopted a zebra", user_id="u", timestamp="2023-05-10T00:00:00+00:00"),
                      Document(id="u_s2", content="bob baked bread", user_id="u", timestamp="2023-05-11T00:00:00+00:00")])
            found, _ = p.retrieve("which zebra", 5, "u", "2023-05-30T00:00:00+00:00")
            self.assertEqual([d.id for d in found], ["u_s1"])
            description = p.description
            p.cleanup()
        finally:
            os.environ.clear()
            os.environ.update(keep)
        stats = json.loads((store / "icm" / "ingest-stats.json").read_text())
        stores = jsonl(next((store / "icm").glob("u-*.store.jsonl")))
        recalls = [r for r in jsonl(next((store / "icm").glob("u-*.recall.jsonl"))) if r["query"] != "capability probe"]
        return stats, stores, recalls, jsonl(trace), description

    def test_refuses_to_start_without_an_engine_before_touching_the_binary(self):
        from icm_provider import IcmMemoryProvider
        keep = dict(os.environ)
        os.environ["ICM_AMB_BIN"] = "/nonexistent/icm"  # were the binary looked at first, the error would be about it
        try:
            with self.assertRaisesRegex(RuntimeError, "ICM_AMB_ENGINE is not set"):
                IcmMemoryProvider()
        finally:
            os.environ.clear()
            os.environ.update(keep)

    def test_v2_sends_the_engine_and_both_dates(self):
        stats, stores, recalls, trace, description = self.drive("v2", ICM_AMB_ENGINE="v2")
        self.assertEqual([s["created_at"] for s in stores], ["2023-05-10T00:00:00+00:00", "2023-05-11T00:00:00+00:00"])
        self.assertEqual((recalls[0]["engine"], recalls[0]["now"]), ("v2", "2023-05-30T00:00:00+00:00"))
        self.assertEqual((stats["engine"], stats["store_date"], stats["query_now"], stats["engine_label"]), ("v2", True, True, V2))
        self.assertTrue(stats["engine_field_in_binary"])
        self.assertIn(V2, description)
        # the trace is what was sent: same fields as the server received, text replaced by its length
        self.assertEqual([t["path"] for t in trace], ["/store", "/store", "/recall"])
        self.assertEqual(trace[0]["body"], {"topic": "conversations", "content": len(stores[0]["content"]),
                                            "created_at": "2023-05-10T00:00:00+00:00"})
        self.assertEqual(trace[2]["body"], recalls[0])

    def test_v2_without_dates_sends_the_engine_only(self):
        stats, stores, recalls, _, description = self.drive("v2-nodate", ICM_AMB_ENGINE="v2", ICM_AMB_STORE_DATE="0",
                                                            ICM_AMB_QUERY_NOW="0")
        self.assertTrue(all("created_at" not in s for s in stores))
        self.assertEqual(recalls[0]["engine"], "v2")
        self.assertNotIn("now", recalls[0])
        self.assertEqual((stats["engine"], stats["store_date"], stats["query_now"], stats["engine_label"]),
                         ("v2", False, False, V2_NO_DATE))
        self.assertIn(V2_NO_DATE, description)

    def test_legacy_names_itself_and_sends_no_date(self):
        stats, stores, recalls, _, description = self.drive("legacy", ICM_AMB_ENGINE="legacy")
        self.assertTrue(all("created_at" not in s for s in stores))
        self.assertEqual(recalls[0]["engine"], "legacy")
        self.assertNotIn("now", recalls[0])
        self.assertEqual((stats["engine"], stats["engine_label"], stats["engine_field_in_binary"]), ("legacy", LEGACY, True))
        self.assertIn(LEGACY, description)

    def test_binary_default_sends_neither_engine_nor_date_and_says_so(self):
        stats, stores, recalls, _, description = self.drive("binary-default", ICM_AMB_ENGINE="binary-default-no-dates")
        self.assertTrue(all("created_at" not in s for s in stores))
        self.assertNotIn("engine", recalls[0])
        self.assertNotIn("now", recalls[0])
        self.assertEqual((stats["engine"], stats["engine_label"]), ("binary-default-no-dates", BINARY_DEFAULT))
        self.assertIn(BINARY_DEFAULT, description)

    def test_a_build_without_the_engine_field_is_measured_with_legacy_and_refused_with_v2(self):
        stats, stores, recalls, _, _ = self.drive("old-legacy", ICM_AMB_ENGINE="legacy", FAKE_ICM_NO_ENGINE_FIELD="1")
        self.assertEqual((stats["engine"], stats["engine_field_in_binary"], stats["recalls"]), ("legacy", False, 1))
        self.assertEqual(recalls[0]["engine"], "legacy")  # sent; that build drops it and runs its only engine
        with self.assertRaisesRegex(RuntimeError, "no v2 HTTP fields.*ICM_AMB_ENGINE=legacy"):
            self.drive("old-v2", ICM_AMB_ENGINE="v2", FAKE_ICM_NO_ENGINE_FIELD="1")


class Launchers(Base):
    def test_run_amb_refuses_icm_without_engine_and_writes_it_in_the_description(self):
        import run_amb
        argv = ["run", "--dataset", "locomo", "--split", "locomo10", "--memory", "icm"]
        with self.assertRaises(SystemExit) as stop:
            run_amb.name_engine(argv, {})
        self.assertIn("ICM_AMB_ENGINE is not set", str(stop.exception))
        named = run_amb.name_engine(argv, {"ICM_AMB_ENGINE": "v2"})
        self.assertEqual(named, argv + ["--description", V2])
        self.assertEqual(run_amb.name_engine(named, {"ICM_AMB_ENGINE": "v2"}), named)
        self.assertEqual(run_amb.name_engine(argv + ["--description", "k=50"], {"ICM_AMB_ENGINE": "legacy"})[-1], f"k=50 | {LEGACY}")
        self.assertEqual(run_amb.name_engine(argv + ["-d", "k=50"], {"ICM_AMB_ENGINE": "legacy"})[-1], f"k=50 | {LEGACY}")
        self.assertEqual(run_amb.name_engine(argv + ["--description=k=50"], {"ICM_AMB_ENGINE": "legacy"})[-1],
                         f"--description=k=50 | {LEGACY}")
        self.assertEqual(run_amb.name_engine(["run", "--memory=icm"], {"ICM_AMB_ENGINE": "legacy"})[-1], LEGACY)
        # another provider, or another command, is not concerned
        for other in (["run", "--dataset", "locomo", "--memory", "bm25"], ["run", "--dataset", "locomo"], ["datasets"]):
            self.assertEqual(run_amb.name_engine(other, {}), other)
        # the command line itself: stops before the harness starts, whatever the binary
        proc = run(PY, BENCH / "run_amb.py", *argv, env_=dict(self.env(), ICM_AMB_BIN="/nonexistent/icm"), check=False)
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("run_amb: ICM_AMB_ENGINE is not set", proc.stderr)

    def test_recall_only_refuses_without_engine_and_records_it(self):
        out = self.root / "recall"
        cmd = (PY, BENCH / "recall_only.py", "run", "--dataset", "longmemeval", "--split", "s", "--memory", "icm",
               "--query-limit", 2, "--output-dir", out, "--description", "tiny file")
        proc = run(*cmd, env_=self.env(), check=False)
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("recall_only: ICM_AMB_ENGINE is not set", proc.stderr)
        self.assertFalse((out / "longmemeval").exists())
        run(*cmd, env_=self.env(ICM_AMB_ENGINE="legacy"))
        doc = json.loads((out / "longmemeval/icm/recall/s.json").read_text())
        self.assertEqual(doc["description"], f"tiny file | {LEGACY}")
        self.assertEqual((doc["provider"]["engine"], doc["provider"]["store_date"], doc["provider"]["query_now"]),
                         ("legacy", False, False))
        self.assertIn(LEGACY, doc["provider"]["description"])
        self.assertTrue(doc["engine_field_in_binary"])

    def test_compare_builds_wants_an_engine_per_side_and_reports_what_was_sent(self):
        out = self.root / "compare"
        base = (PY, BENCH / "compare_builds.py", "--no-llm", "--before", self.icm, "--after", self.icm,
                "--dataset", "longmemeval", "--split", "s", "--query-limit", 2, "--k", 5, "--out", out)
        for missing in ((), ("--after-env", "ICM_AMB_ENGINE=v2"), ("--before-env", "ICM_AMB_ENGINE=legacy")):
            # the caller's environment does not name a side's engine either
            proc = run(*base, *missing, env_=self.env(ICM_AMB_ENGINE="v2"), check=False)
            self.assertNotEqual(proc.returncode, 0, missing)
            self.assertIn("ICM_AMB_ENGINE is not set", proc.stderr)
            self.assertFalse((out / "longmemeval").exists(), "refused before either side ran")
        sides = ("--before-env", "ICM_AMB_ENGINE=legacy", "--after-env", "ICM_AMB_ENGINE=v2")
        proc = run(*base, *sides, env_=self.env())
        report = json.loads((out / "compare-longmemeval-s-before-vs-after.json").read_text())
        before, after = report["before"], report["after"]
        self.assertEqual((before["engine"], before["store_date"], before["query_now"]), ("legacy", False, False))
        self.assertEqual((after["engine"], after["store_date"], after["query_now"]), ("v2", True, True))
        self.assertEqual(before["sent"], {"store_requests": 7, "store_with_created_at": 0, "recall_requests": 2,
                                          "recall_with_now": 0, "recall_engine_field": {"legacy": 2}})
        self.assertEqual(after["sent"], {"store_requests": 7, "store_with_created_at": 7, "recall_requests": 2,
                                         "recall_with_now": 2, "recall_engine_field": {"v2": 2}})
        for label, side in (("before", LEGACY), ("after", V2)):
            result = json.loads((out / "longmemeval" / label / "recall-only" / "s.json").read_text())
            self.assertEqual(result["description"], f"compare_builds side {label} | {side}")
            self.assertIn(side, proc.stdout)
        self.assertIn("/store with created_at 7/7; /recall with now 2/2; engine field: v2 x2", proc.stdout)
        # a result kept by --reuse is not relabelled with another engine
        proc = run(*base, "--reuse", "--before-env", "ICM_AMB_ENGINE=legacy", "--after-env", "ICM_AMB_ENGINE=legacy",
                   env_=self.env(), check=False)
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("was produced with", proc.stderr)

    def test_dry_run_tokens_with_icm_needs_an_engine_too(self):
        proc = run(PY, BENCH / "dry_run_tokens.py", "--dataset", "longmemeval", "--split", "s", "--memory", "icm",
                   "--query-limit", 1, env_=self.env(), check=False)
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("ICM_AMB_ENGINE is not set", proc.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
