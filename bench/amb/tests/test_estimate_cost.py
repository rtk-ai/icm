#!/usr/bin/env python3
"""estimate_cost.py and the usage log of run_amb.py: the bill is the API's own counts
when a run kept them, and an estimate never passes an assumption for a measurement.
Offline: the usage lines are written from stand-in responses, no model is called.

    AMB_HOME=/path/to/agent-memory-benchmark python tests/test_estimate_cost.py
"""
import contextlib
import io
import json
import os
import sys
import types
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from _common import BENCH, amb_home, workdir

sys.path.insert(0, str(BENCH))
import estimate_cost as ec  # noqa: E402

RESULTS = BENCH / "results" / "locomo10-icm-v2-k50-gke-20261005.json"
PRICES = ("--answer-price", "2", "12", "--judge-price", "0.1", "0.4")


def call(*argv) -> tuple[dict, str]:
    out = io.StringIO()
    target = Path(os.environ["ICM_AMB_COST_JSON"])
    with contextlib.redirect_stdout(out):
        ec.main([str(a) for a in argv] + ["--json", str(target)])
    return json.loads(target.read_text()), out.getvalue()


class Estimate(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.root = workdir("estimate-cost")
        os.environ["ICM_AMB_COST_JSON"] = str(cls.root / "out.json")
        cls.rows = json.loads(RESULTS.read_text())["results"]

    def test_what_the_published_file_holds_and_what_it_does_not(self):
        doc, text = call("--results", RESULTS)
        m = doc["measured_cl100k"]
        self.assertEqual(doc["run"], {"questions": 1540, "planned": False})
        self.assertEqual(m["answer_input_tokens"], sum(r["context_tokens"] for r in self.rows) + 1540 * 196)
        self.assertEqual(m["answer_visible_output_tokens"], sum(ec.count_tokens(r["answer"]) for r in self.rows))
        self.assertIn("answer only", m["answer_visible_output_fields"])  # this file has no `reasoning` field
        self.assertEqual(m["judge_visible_output_tokens"], sum(ec.count_tokens(r["judge_reason"]) for r in self.rows))
        self.assertEqual(m["thinking_tokens"], "not in a result file")
        self.assertNotIn("cost_usd", doc)
        self.assertIn("no dollar figure", text)

    def test_no_range_without_both_assumptions_and_the_floor_is_arithmetic(self):
        doc, text = call("--results", RESULTS, *PRICES)
        m, floor = doc["measured_cl100k"], doc["cost_usd"]["floor"]
        self.assertEqual(set(doc["cost_usd"]), {"floor"})
        self.assertIn("no range", text)
        answer = (m["answer_input_tokens"] * 2 + m["answer_visible_output_tokens"] * 12) / 1e6
        judge = (m["judge_input_tokens"] * 0.1 + m["judge_visible_output_tokens"] * 0.4) / 1e6
        self.assertAlmostEqual(floor["answer_usd"], answer, places=2)
        self.assertAlmostEqual(floor["judge_usd"], judge, places=2)
        self.assertAlmostEqual(floor["total_usd"], answer + judge, places=1)
        for one in (("--token-ratio", 1, 1.3), ("--thinking-per-answer", 500, 4000)):
            self.assertEqual(set(call("--results", RESULTS, *PRICES, *one)[0]["cost_usd"]), {"floor"})
        doc, text = call("--results", RESULTS, *PRICES, "--token-ratio", 1, 1.3, "--thinking-per-answer", 500, 4000)
        low, high = doc["cost_usd"]["low"], doc["cost_usd"]["high"]
        self.assertAlmostEqual(low["answer_usd"], answer + 500 * 1540 * 12 / 1e6, places=2)
        self.assertAlmostEqual(high["answer_usd"], (m["answer_input_tokens"] * 1.3 * 2 + (m["answer_visible_output_tokens"] * 1.3
                                                                                          + 4000 * 1540) * 12) / 1e6, places=2)
        self.assertLess(floor["total_usd"], low["total_usd"])
        self.assertLess(low["total_usd"], high["total_usd"])
        self.assertEqual(text.count("assumed:"), 2)
        self.assertEqual(text.count("measured:"), 1)

    def test_a_planned_run_takes_its_input_from_the_dry_run(self):
        doc, _ = call("--results", RESULTS, *PRICES, "--answer-input-tokens", 61857481, "--max-call-input", 52501,
                      "--long-context", 200000, 4, 18, "--token-ratio", 1, 1.3, "--thinking-per-answer", 500, 4000)
        self.assertEqual(doc["run"], {"questions": 1540, "planned": True})
        self.assertEqual(doc["measured_cl100k"]["answer_input_tokens"], 61857481)
        self.assertAlmostEqual(doc["cost_usd"]["floor"]["answer_input_usd"], 123.71, places=2)
        self.assertFalse(doc["cost_usd"]["high"]["long_context_tier"])  # 52,501 x 1.3 is far under 200,000
        # a run whose largest call crosses the threshold is priced at the long-context tier, input and output
        doc, text = call("--results", RESULTS, *PRICES, "--answer-input-tokens", 61857481, "--max-call-input", 180000,
                         "--long-context", 200000, 4, 18, "--token-ratio", 1, 1.3, "--thinking-per-answer", 500, 4000)
        self.assertFalse(doc["cost_usd"]["low"]["long_context_tier"])
        self.assertTrue(doc["cost_usd"]["high"]["long_context_tier"])
        self.assertAlmostEqual(doc["cost_usd"]["high"]["answer_input_usd"], 61857481 * 1.3 * 4 / 1e6, places=2)
        self.assertIn("[long-context tier]", text)
        with self.assertRaises(SystemExit):
            call("--results", RESULTS, "--questions", 500)

    def test_with_a_usage_file_the_bill_is_measured_and_nothing_is_assumed(self):
        usage = self.root / "usage.jsonl"
        lines = []
        for i, row in enumerate(self.rows):
            lines.append({"model": "gemini-3.1-pro-preview", "reported": True, "prompt_tokens": 26000, "output_tokens": 120,
                          "thinking_tokens": 900 if i % 2 else 1100, "cached_tokens": 0, "total_tokens": 27120})
            lines.append({"model": "gemini-2.5-flash-lite", "reported": True, "prompt_tokens": 480, "output_tokens": 30,
                          "thinking_tokens": 0, "cached_tokens": 0, "total_tokens": 510})
        lines.append({"model": "gemini-3.1-pro-preview", "reported": True, "prompt_tokens": 250000, "output_tokens": 100,
                      "thinking_tokens": 900, "cached_tokens": 0, "total_tokens": 251000})  # one call over the threshold
        usage.write_text("".join(json.dumps(l) + "\n" for l in lines))
        doc, text = call("--results", RESULTS, "--usage", usage, *PRICES, "--long-context", 200000, 4, 18,
                         "--token-ratio", 9, 9, "--thinking-per-answer", 9, 9)  # ignored: nothing is assumed here
        answer, judge = doc["measured_api"]["gemini-3.1-pro-preview"], doc["measured_api"]["gemini-2.5-flash-lite"]
        self.assertEqual((answer["role"], answer["calls"], judge["role"], judge["calls"]), ("answer", 1541, "judge", 1540))
        self.assertEqual(answer["thinking_tokens"], 1540 * 1000 + 900)
        self.assertEqual(answer["calls_over_long_context_threshold"], 1)
        expected_answer = 1540 * (26000 * 2 + (120 + 1000) * 12) / 1e6 + (250000 * 4 + 1000 * 18) / 1e6
        self.assertAlmostEqual(answer["cost_usd"], expected_answer, places=2)
        self.assertAlmostEqual(judge["cost_usd"], 1540 * (480 * 0.1 + 30 * 0.4) / 1e6, places=2)
        self.assertAlmostEqual(doc["cost_usd"]["measured"], answer["cost_usd"] + judge["cost_usd"], places=2)
        self.assertNotIn("assumed", doc)
        self.assertNotIn("assumed:", text)
        nxt = doc["for_the_next_estimate"]
        self.assertAlmostEqual(nxt["thinking_tokens_per_answer_call"], (1540 * 1000 + 900) / 1541, places=1)
        self.assertAlmostEqual(nxt["billed_input_tokens_per_cl100k_token"],
                               (1540 * 26000 + 250000) / doc["measured_cl100k"]["answer_input_tokens"], places=4)


class UsageLog(unittest.TestCase):
    """ICM_AMB_USAGE: one line per Gemini response with the API's counts, thinking tokens included."""

    @classmethod
    def setUpClass(cls):
        os.environ["AMB_HOME"] = str(amb_home())
        cls.root = workdir("usage-log")

    def test_counts_are_copied_from_usage_metadata_and_no_text_is_written(self):
        code = (
            "import json, os, sys, types; sys.path.insert(0, sys.argv[1]); import run_amb\n"
            "os.environ['ICM_AMB_USAGE'] = sys.argv[2]\n"
            "run_amb.bootstrap()\n"
            "import memory_bench.llm.gemini as g\n"
            "usage = types.SimpleNamespace(prompt_token_count=26123, candidates_token_count=118, thoughts_token_count=1375,\n"
            "                              cached_content_token_count=None, total_token_count=27616)\n"
            "class Models:\n"
            "    def generate_content(self, model, contents, config):\n"
            "        return types.SimpleNamespace(usage_metadata=usage if 'no-usage' not in contents else None,\n"
            "                                     parsed={'answer': 'SECRET-ANSWER'}, text='SECRET-ANSWER')\n"
            "llm = g.GeminiLLM.__new__(g.GeminiLLM)\n"
            "llm._client = types.SimpleNamespace(models=Models()); llm._model = 'gemini-3.1-pro-preview'\n"
            "print(llm._generate_raw('SECRET-PROMPT').parsed)\n"
            "print(llm._generate_raw('no-usage').parsed)\n"
        )
        import subprocess
        usage_file = self.root / "sub" / "usage.jsonl"
        env = {k: v for k, v in os.environ.items() if not k.startswith(("ICM_AMB_", "GOOGLE_", "GEMINI_"))}
        proc = subprocess.run([sys.executable, "-c", code, str(BENCH), str(usage_file)], env=env, capture_output=True, text=True)
        self.assertEqual(proc.returncode, 0, proc.stderr[-2000:])
        self.assertEqual(proc.stdout.count("SECRET-ANSWER"), 2, "the response reaches the harness unchanged")
        raw = usage_file.read_text()
        self.assertNotIn("SECRET", raw)
        first, second = [json.loads(l) for l in raw.splitlines()]
        self.assertEqual(first, {"model": "gemini-3.1-pro-preview", "reported": True, "prompt_tokens": 26123, "output_tokens": 118,
                                 "thinking_tokens": 1375, "cached_tokens": 0, "total_tokens": 27616})
        self.assertEqual((second["reported"], second["prompt_tokens"], second["thinking_tokens"]), (False, 0, 0))
        # and estimate_cost.py reads exactly these lines
        usage = ec.read_usage([usage_file])["gemini-3.1-pro-preview"]
        self.assertEqual((usage["calls"], usage["unreported"], usage["prompt_tokens"], usage["thinking_tokens"]), (2, 1, 26123, 1375))


if __name__ == "__main__":
    unittest.main(verbosity=2)
