#!/usr/bin/env python3
"""rejudge_strict.py with stand-in judges: the sample, the strict prompt, the figures,
the resume, and that an unusable judge output never becomes a verdict. No model is called.

    AMB_HOME=/path/to/agent-memory-benchmark python tests/test_rejudge_strict.py
"""
import contextlib
import io
import json
import os
import random
import re
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from _common import BENCH, amb_home, workdir

sys.path.insert(0, str(BENCH))
import rejudge_strict as rs  # noqa: E402

RESULTS = BENCH / "results" / "locomo10-icm-v2-k50-gke-20261005.json"
# The prompt also holds a worked example ("Question: Do you remember..."): the real block follows a blank line.
_PARTS = re.compile(r"\n\nQuestion: (.*)\nGold answer: (.*)\nGenerated answer: (.*)\nFirst, provide", re.S)


def literal_judge(prompt: str) -> dict:
    """Stand-in: correct only when the gold answer appears word for word in the generated answer."""
    _, gold, answer = _PARTS.search(prompt).groups()
    return {"correct": gold.strip().lower() in answer.lower(), "reason": "stand-in: literal containment"}


def forbidden_judge(prompt: str) -> dict:
    raise AssertionError("the judge must not be called here")


def call(*argv, judge=None) -> tuple[int, str]:
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        code = rs.main([str(a) for a in argv], judge=judge)
    return code, out.getvalue()


class Base(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        os.environ["AMB_HOME"] = str(amb_home())
        for var in [v for v in os.environ if v.startswith(("ICM_AMB_", "GOOGLE_", "GEMINI_"))]:
            del os.environ[var]
        cls.doc = json.loads(RESULTS.read_text())
        cls.rows = cls.doc["results"]
        cls.root = workdir(cls.__name__)


class Sample(Base):
    def test_stratified_fixed_seed_and_independent_of_file_order(self):
        sample, design = rs.stratified_sample(self.rows, 200, 20261005)
        self.assertEqual({k: (v["population"], v["sample"]) for k, v in design.items()},
                         {"multi-hop": (96, 12), "open-domain": (841, 109), "single-hop": (282, 37), "temporal": (321, 42)})
        ids = [r["query_id"] for r in sample]
        self.assertEqual(len(set(ids)), 200)
        shuffled = list(self.rows)
        random.Random(1).shuffle(shuffled)
        self.assertEqual([r["query_id"] for r in rs.stratified_sample(shuffled, 200, 20261005)[0]], ids)
        self.assertEqual([r["query_id"] for r in rs.stratified_sample(self.rows, 200, 20261005)[0]], ids)
        other = [r["query_id"] for r in rs.stratified_sample(self.rows, 200, 7)[0]]
        self.assertLess(len(set(ids) & set(other)), 100)
        with self.assertRaises(SystemExit):
            rs.stratified_sample(self.rows, 2000, 1)

    def test_allocation_never_exceeds_a_stratum(self):
        self.assertEqual(rs.allocate({"a": 3, "b": 997}, 200), {"a": 1, "b": 199})
        self.assertEqual(sum(rs.allocate({"a": 1, "b": 1, "c": 1}, 2).values()), 2)


class Prompt(Base):
    def test_strict_prompt_is_the_harness_prompt_minus_the_generosity(self):
        lenient = rs.harness_template()
        strict = rs.strict_template(lenient)
        self.assertEqual(lenient.count("generous"), 2)
        self.assertIn("touches on the same topic", lenient)
        self.assertNotIn("generous", strict)
        self.assertNotIn("touches on the same topic as the gold answer, it should be counted as CORRECT", strict)
        diff = rs.prompt_diff(lenient, strict)
        removed = [l for l in diff.splitlines() if l.startswith("-") and not l.startswith("---")]
        added = [l for l in diff.splitlines() if l.startswith("+") and not l.startswith("+++")]
        self.assertEqual((len(removed), len(added)), (4, 4), diff)
        # everything the edits do not touch is the harness' text, unchanged
        same = [l for l in lenient.splitlines() if l in strict.splitlines()]
        self.assertEqual(len(same), len(lenient.splitlines()) - 4)

    def test_judge_sees_question_gold_and_answer_only(self):
        row = dict(self.rows[0], context="SECRET-CONTEXT", reasoning="SECRET-REASONING", judge_reason="SECRET-VERDICT")
        prompt = rs.render(rs.strict_template(rs.harness_template()), row)
        for needle in (row["query"], str(row["gold_answers"][0]), row["answer"]):
            self.assertIn(needle, prompt)
        for secret in ("SECRET-CONTEXT", "SECRET-REASONING", "SECRET-VERDICT"):
            self.assertNotIn(secret, prompt)
        # the stand-in used below reads the three fields back from the prompt
        self.assertEqual(_PARTS.search(prompt).groups(), (row["query"], str(row["gold_answers"][0]), row["answer"]))
        tricky = dict(row, answer="uses {braces} and {query}")
        self.assertIn("uses {braces} and {query}", rs.render(rs.strict_template(rs.harness_template()), tricky))

    def test_a_changed_harness_prompt_stops_the_run(self):
        with self.assertRaises(SystemExit) as stop:
            rs.strict_template(rs.harness_template().replace("generous", "kind"))
        self.assertIn("review STRICT_EDITS", str(stop.exception))


class Verdicts(unittest.TestCase):
    def test_only_a_boolean_verdict_is_a_verdict(self):
        self.assertEqual(rs.parse_verdict('{"reason": "r", "correct": false}'), {"correct": False, "reason": "r"})
        self.assertEqual(rs.parse_verdict(None, {"reason": "r", "correct": True}), {"correct": True, "reason": "r"})
        for bad in ("CORRECT", "", '{"correct": "true", "reason": "r"}', '{"correct": 1, "reason": "r"}', '{"reason": "r"}', "[true]"):
            with self.assertRaises(rs.JudgeError, msg=bad):
                rs.parse_verdict(bad)

    def test_vertex_judge_retries_then_reports_instead_of_guessing(self):
        class Response:
            def __init__(self, text=None, parsed=None):
                self.text, self.parsed = text, parsed

        class Client:
            def __init__(self, script):
                self.script, self.calls, self.models = list(script), [], self

            def generate_content(self, model, contents, config):
                self.calls.append((model, contents, config))
                step = self.script.pop(0)
                if isinstance(step, Exception):
                    raise step
                return step

        good = Client([RuntimeError("429 RESOURCE_EXHAUSTED"), Response(text="The answer is CORRECT"),
                       Response(text='{"reason": "same date", "correct": true}')])
        judge = rs.VertexJudge("some-model", client=good, attempts=5, backoff_s=0)
        self.assertEqual(judge("prompt"), {"correct": True, "reason": "same date"})
        self.assertEqual(len(good.calls), 3)
        model, contents, config = good.calls[0]
        self.assertEqual((model, contents, config.temperature, config.response_mime_type), ("some-model", "prompt", 0.0, "application/json"))
        # the harness would turn this text into "correct": here it is no verdict at all
        bad = Client([Response(text="CORRECT, the answer matches")] * 3)
        with self.assertRaisesRegex(rs.JudgeError, "no verdict after 3 attempts"):
            rs.VertexJudge("some-model", client=bad, attempts=3, backoff_s=0)("prompt")
        self.assertEqual(len(bad.calls), 3)

    def test_an_error_that_will_not_go_away_is_not_retried(self):
        class ApiError(Exception):  # the shape of google.genai.errors.APIError: an int `.code`
            def __init__(self, code, message):
                super().__init__(f"{code} {message}")
                self.code = code

        class Client:
            def __init__(self, error):
                self.error, self.calls, self.models = error, 0, self

            def generate_content(self, model, contents, config):
                self.calls += 1
                raise self.error

        for code in (400, 401, 403, 404):
            client = Client(ApiError(code, "NOT_FOUND. Publisher model was not found"))
            with self.assertRaises(rs.JudgeFatal) as stop:
                rs.VertexJudge("no-such-model", client=client, attempts=5, backoff_s=0)("prompt")
            self.assertEqual(client.calls, 1, f"HTTP {code} asked once")
            self.assertIn(f"HTTP {code}", str(stop.exception))
            self.assertIn("no-such-model", str(stop.exception))
        for code in (429, 500, 503):  # these pass: retried to the end, and not fatal
            client = Client(ApiError(code, "RESOURCE_EXHAUSTED"))
            with self.assertRaises(rs.JudgeError) as stop:
                rs.VertexJudge("some-model", client=client, attempts=3, backoff_s=0)("prompt")
            self.assertNotIsInstance(stop.exception, rs.JudgeFatal)
            self.assertEqual(client.calls, 3)
        real = __import__("google.genai.errors", fromlist=["ClientError"]).ClientError(404, {"error": {"message": "x", "status": "NOT_FOUND"}})
        self.assertEqual(rs.http_status(real), 404)
        self.assertIsNone(rs.http_status(RuntimeError("404 in the text is not a status")))


class EndToEnd(Base):
    def test_report_matches_an_independent_count(self):
        out = self.root / "full"
        code, text = call("--results", RESULTS, "--out", out, "--model", "stand-in", judge=literal_judge)
        self.assertEqual(code, 0, text)
        report = json.loads((out / "report.json").read_text())
        sample = json.loads((out / "sample.json").read_text())
        by_id = {r["query_id"]: r for r in self.rows}
        picked = [by_id[q] for q in sample["query_ids"]]
        strict = {r["query_id"]: str(r["gold_answers"][0]).strip().lower() in r["answer"].lower() for r in picked}
        tt = sum(1 for r in picked if r["correct"] and strict[r["query_id"]])
        tf = sum(1 for r in picked if r["correct"] and not strict[r["query_id"]])
        ft = sum(1 for r in picked if not r["correct"] and strict[r["query_id"]])
        ff = 200 - tt - tf - ft
        a = report["analysis"]
        self.assertEqual(a["confusion"], {"harness_correct_strict_correct": tt, "harness_correct_strict_wrong": tf,
                                          "harness_wrong_strict_correct": ft, "harness_wrong_strict_wrong": ff})
        self.assertEqual(a["judged"], 200)
        self.assertAlmostEqual(a["agreement"], (tt + ff) / 200, places=4)
        self.assertEqual(a["strict_judge_on_sample"]["correct"], tt + ft)
        self.assertEqual(a["harness_judge_on_sample"]["correct"], tt + tf)
        low, high = rs.wilson(tt + ft, 200)
        self.assertEqual(a["strict_judge_on_sample"]["ci95"], [round(low, 4), round(high, 4)])
        self.assertAlmostEqual(a["difference_strict_minus_harness"], (ft - tf) / 200, places=4)
        self.assertEqual(sum(v["n"] for v in a["by_category"].values()), 200)
        self.assertEqual(len(report["disagreements"]), tf + ft)
        self.assertGreater(tf, 0, "the literal stand-in must disagree with the generous judge somewhere")
        self.assertEqual(report["strict_judge"]["model"], "stand-in")
        self.assertEqual(report["run"]["judge_llm"], "gemini:gemini-2.5-flash-lite")
        self.assertEqual(report["unjudged"], [])
        md = (out / "report.md").read_text()
        self.assertEqual(md.count("- Human verdict: "), tf + ft)
        self.assertIn("## Confusion matrix", md)
        self.assertIn("-The generated answer might be much longer, but you should be generous", md)
        self.assertEqual(len((out / "verdicts.jsonl").read_text().splitlines()), 200)
        print(f"\n      stand-in judge: confusion {a['confusion']}, agreement {a['agreement']}, "
              f"strict {a['strict_judge_on_sample']}, run estimate {a['strict_judge_run_estimate']['accuracy']}")

        # a second run pays for nothing and writes the same report
        code, text = call("--results", RESULTS, "--out", out, "--model", "stand-in", judge=forbidden_judge)
        self.assertEqual(code, 0, text)
        self.assertIn("200 verdicts reused", text)
        self.assertEqual(json.loads((out / "report.json").read_text())["analysis"], a)
        # another judge model does not reuse them
        calls = []
        code, _ = call("--results", RESULTS, "--out", out, "--model", "other-model",
                       judge=lambda p: calls.append(1) or literal_judge(p))
        self.assertEqual((code, len(calls)), (0, 200))

    def other_run(self, name: str) -> Path:
        """A result file with the same question ids and other answers: another run of the same split."""
        doc = json.loads(RESULTS.read_text())
        for row in doc["results"]:
            row["answer"] = "I do not know."
        path = self.root / name
        path.write_text(json.dumps(doc))
        return path

    def test_verdicts_of_one_run_are_never_reused_for_another(self):
        out = self.root / "two-runs"
        code, _ = call("--results", RESULTS, "--out", out, "--model", "stand-in", judge=literal_judge)
        self.assertEqual(code, 0)
        first = (out / "report.json").read_text()
        other = self.other_run("other-run.json")
        # same --out, another result file: refused before any call, nothing overwritten
        for extra in ((), ("--dry-run",)):
            with self.assertRaises(SystemExit) as stop:
                call("--results", other, "--out", out, "--model", "stand-in", *extra, judge=forbidden_judge)
            self.assertIn("another result file", str(stop.exception))
        self.assertEqual((out / "report.json").read_text(), first)
        self.assertEqual(json.loads((out / "sample.json").read_text())["results_file"], str(RESULTS))
        # and were the directory check gone, the lines still would not match: a verdict
        # belongs to the rendered prompt, i.e. to the answer it judged
        sample = json.loads((out / "sample.json").read_text())
        sample["results_sha256"] = __import__("hashlib").sha256(other.read_bytes()).hexdigest()
        (out / "sample.json").write_text(json.dumps(sample))
        calls = []
        code, text = call("--results", other, "--out", out, "--model", "stand-in",
                          judge=lambda p: calls.append(1) or literal_judge(p))
        self.assertEqual((code, len(calls)), (0, 200), text)
        self.assertIn("0 verdicts reused", text)
        report = json.loads((out / "report.json").read_text())
        self.assertEqual(report["analysis"]["strict_judge_on_sample"]["correct"], 0)  # "I do not know." matches no gold answer
        # a line written before the rendered prompt was recorded is not trusted either
        legacy_out = self.root / "old-lines"
        code, _ = call("--results", RESULTS, "--out", legacy_out, "--model", "stand-in", judge=literal_judge)
        lines = [json.loads(l) for l in (legacy_out / "verdicts.jsonl").read_text().splitlines()]
        self.assertTrue(all(len(l["rendered_sha256"]) == 64 and l["arm"] == "strict" for l in lines))
        (legacy_out / "verdicts.jsonl").write_text("".join(
            json.dumps({k: v for k, v in l.items() if k != "rendered_sha256"}) + "\n" for l in lines))
        calls = []
        call("--results", RESULTS, "--out", legacy_out, "--model", "stand-in", judge=lambda p: calls.append(1) or literal_judge(p))
        self.assertEqual(len(calls), 200)

    def test_a_refused_model_costs_one_call_and_a_dead_judge_a_handful(self):
        out, calls = self.root / "fatal", []

        def refused(prompt):
            calls.append(1)
            raise rs.JudgeFatal("HTTP 404 from the API for model 'no-such-model', not retried")

        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            code, _ = call("--results", RESULTS, "--out", out, "--model", "no-such-model", judge=refused)
        self.assertEqual((code, len(calls)), (2, 1))
        self.assertIn("STOPPED: HTTP 404", err.getvalue())
        self.assertFalse((out / "report.json").exists())
        self.assertFalse((out / "verdicts.jsonl").exists())

        out, calls = self.root / "dead", []

        def dead(prompt):
            calls.append(1)
            raise rs.JudgeError("no verdict after 5 attempts: 429 RESOURCE_EXHAUSTED")

        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            code, _ = call("--results", RESULTS, "--out", out, "--model", "m", "--max-failures", 5, "--concurrency", 2, judge=dead)
        self.assertEqual(code, 2)
        self.assertLessEqual(len(calls), 5 + 2, "stops after 5 rows in a row, plus the calls already in flight")
        self.assertIn("5 rows in a row ended without a verdict", err.getvalue())
        self.assertFalse((out / "report.json").exists())
        # a judge that fails now and then is not stopped (see test_failed_calls_are_reported_and_left_out)

    def test_control_arm_separates_the_model_from_the_prompt(self):
        out = self.root / "control"
        seen = {"harness": 0, "strict": 0}

        def two_prompts(prompt):
            # a judge that passes everything under the harness prompt and reads literally under the strict one
            if "you should be generous" in prompt:
                seen["harness"] += 1
                return {"correct": True, "reason": "stand-in: generous"}
            seen["strict"] += 1
            return literal_judge(prompt)

        code, text = call("--results", RESULTS, "--out", out, "--model", "stand-in", "--control", judge=two_prompts)
        self.assertEqual(code, 0, text)
        self.assertEqual(seen, {"harness": 200, "strict": 200})
        report = json.loads((out / "report.json").read_text())
        sample = json.loads((out / "sample.json").read_text())
        by_id = {r["query_id"]: r for r in self.rows}
        picked = [by_id[q] for q in sample["query_ids"]]
        harness = sum(1 for r in picked if r["correct"])
        strict = sum(1 for r in picked if str(r["gold_answers"][0]).strip().lower() in r["answer"].lower())
        c = report["control"]
        self.assertEqual(c["judged_in_both_arms"], 200)
        self.assertEqual((c["model_gap"]["first_correct"], c["model_gap"]["second_correct"]), (harness, 200))
        self.assertEqual((c["prompt_gap"]["first_correct"], c["prompt_gap"]["second_correct"]), (200, strict))
        self.assertAlmostEqual(c["model_gap"]["difference_points"], (200 - harness) / 2, places=2)
        self.assertAlmostEqual(c["prompt_gap"]["difference_points"], (strict - 200) / 2, places=2)
        self.assertAlmostEqual(c["total_gap"]["difference_points"],
                               c["model_gap"]["difference_points"] + c["prompt_gap"]["difference_points"], places=2)
        self.assertAlmostEqual(c["total_gap"]["difference_points"],
                               report["analysis"]["difference_strict_minus_harness"] * 100, places=1)
        md = (out / "report.md").read_text()
        self.assertIn("## Model gap and prompt gap", md)
        self.assertIn("| stand-in | harness (control arm) | 200 | 100.0% |", md)
        self.assertEqual({json.loads(l)["arm"] for l in (out / "verdicts.jsonl").read_text().splitlines()}, {"strict", "control"})
        # resumed: both arms are reused, nothing is asked
        code, text = call("--results", RESULTS, "--out", out, "--model", "stand-in", "--control", judge=forbidden_judge)
        self.assertEqual(code, 0)
        self.assertIn("400 verdicts reused", text)
        code, text = call("--results", RESULTS, "--out", self.root / "control-dry", "--control", "--dry-run", judge=forbidden_judge)
        self.assertIn("400 judge calls", text)

    def test_without_control_the_report_says_what_the_gap_mixes(self):
        code, text = call("--results", RESULTS, "--out", self.root / "mix", "--model", "another-model", judge=literal_judge)
        self.assertIn("mixes the model and the prompt", text)
        report = json.loads((self.root / "mix" / "report.json").read_text())
        self.assertIsNone(report["control"])
        self.assertIn("--control", report["gap_reading"])
        self.assertIn("mixes the model and the prompt", (self.root / "mix" / "report.md").read_text())
        code, text = call("--results", RESULTS, "--out", self.root / "same", "--model", "gemini-2.5-flash-lite", judge=literal_judge)
        self.assertIn("is the effect of the prompt", text)
        self.assertNotIn("mixes", text)

    def test_failed_calls_are_reported_and_left_out(self):
        out = self.root / "errors"
        sample, _ = rs.stratified_sample(self.rows, 200, 20261005)
        broken = {sample[i]["query"] for i in (0, 50, 199)}

        def flaky(prompt):
            if _PARTS.search(prompt).group(1) in broken:
                return "CORRECT"  # free text: what the harness would have counted as a pass
            return literal_judge(prompt)

        code, text = call("--results", RESULTS, "--out", out, "--model", "stand-in", judge=flaky)
        report = json.loads((out / "report.json").read_text())
        n_broken = sum(1 for r in sample if r["query"] in broken)
        self.assertEqual(code, 3)
        self.assertIn("UNJUDGED", text)
        self.assertEqual(len(report["unjudged"]), n_broken)
        self.assertEqual(report["analysis"]["judged"], 200 - n_broken)
        self.assertEqual(len((out / "verdicts.jsonl").read_text().splitlines()), 200 - n_broken)
        calls = []
        code, _ = call("--results", RESULTS, "--out", out, "--model", "stand-in", judge=lambda p: calls.append(1) or literal_judge(p))
        self.assertEqual((code, len(calls)), (0, n_broken))  # only the missing ones are asked again

    def test_dry_run_sends_nothing(self):
        out = self.root / "dry"
        code, text = call("--results", RESULTS, "--out", out, "--dry-run", judge=forbidden_judge)
        self.assertEqual(code, 0)
        self.assertIn("200 judge calls", text)
        self.assertIn("--dry-run: nothing sent", text)
        self.assertFalse((out / "verdicts.jsonl").exists())
        self.assertTrue((out / "sample.json").exists())
        with self.assertRaises(SystemExit) as stop:   # the real path refuses to guess a judge model
            call("--results", RESULTS, "--out", out)
        self.assertIn("--model is required", str(stop.exception))

    def test_light_copy_is_the_merged_run_for_what_a_judge_needs(self):
        merged = os.environ.get("MERGED_LOCOMO_RESULT")
        if not merged or not Path(merged).exists():
            self.skipTest("set MERGED_LOCOMO_RESULT to the full merged result file to compare")
        full = {r["query_id"]: r for r in json.loads(Path(merged).read_text())["results"]}
        for row in self.rows:
            ref = full[row["query_id"]]
            for key in ("query", "answer", "gold_answers", "correct", "judge_reason"):
                self.assertEqual(row[key], ref[key])
            self.assertEqual(rs.category_of(row), ref["meta"]["category"])
        a = [r["query_id"] for r in rs.stratified_sample(self.rows, 200, 20261005)[0]]
        b = [r["query_id"] for r in rs.stratified_sample(list(full.values()), 200, 20261005)[0]]
        self.assertEqual(a, b)


if __name__ == "__main__":
    unittest.main(verbosity=2)
