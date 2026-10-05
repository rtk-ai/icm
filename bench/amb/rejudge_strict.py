#!/usr/bin/env python3
"""Second, strict judge on a sample of an answer run: how much does the harness' judge give away?

    # what would be sent, and its size: no model call
    python rejudge_strict.py --results results/locomo10-icm-v2-k50-gke-20261005.json --out out/rejudge --dry-run

    # the real thing: 200 judge calls on Vertex AI with the active gcloud account
    GOOGLE_CLOUD_PROJECT=rtk-ai-labs-01 GOOGLE_CLOUD_LOCATION=global \\
    python rejudge_strict.py --results results/locomo10-icm-v2-k50-gke-20261005.json \\
        --model <judge model> --out out/rejudge

The LoCoMo judge prompt of the harness tells the model twice to "be generous" and to
accept an answer that "touches on the same topic" as the gold answer. This script
draws a sample of the answers of a finished run, has them judged again without those
instructions, and reports what changes: agreement between the two judges, confusion
matrix, accuracy under the strict judge with its interval, and every disagreement
for a human to read.

Sample. `--n` answers (default 200), drawn without replacement with a fixed `--seed`
(default 20261005), stratified by question type in proportion to the run (largest
remainders). The draw depends on the seed, the type and the question ids only, not
on the order of the file: the same file always gives the same sample.

Strict prompt. It is the harness' own LoCoMo prompt (`LoComoDataset.build_judge_prompt`
of the checkout in AMB_HOME, read at run time, not copied here) with the edits listed
in STRICT_EDITS and nothing else: each edit replaces one of the lenient sentences.
If the harness prompt no longer contains a sentence to replace, the script stops
instead of judging with a prompt nobody reviewed. `--show-diff` prints the unified
diff of the two prompts. `--prompt-file` replaces the whole template (placeholders
{query}, {gold}, {answer}); the report then says so.

What the judge sees is what the harness' judge saw: the question, the first gold
answer, the generated `answer` field. Nothing else (no context, no reasoning, no
first verdict).

Judge. `--model` on Vertex AI, temperature 0, JSON output `{reason, correct}`.
`--auth gcloud` (default) uses the access token of the active gcloud account, the
mechanism of ICM_AMB_GCLOUD_AUTH in run_amb.py; `--auth adc` uses Application
Default Credentials. Unlike the harness, an output that is not that JSON object is
never turned into a verdict: the call is retried, then the row is reported as
unjudged, left out of every figure, and the script exits 3.

Which gap is measured. The strict judge differs from the harness' judge by its
prompt and, unless `--model` is the harness' own judge model, by its model: the
difference between the two then mixes both. `--control` separates them: the same
sample is also judged with the harness' UNCHANGED prompt by `--model` (twice the
calls), and the report gives the model gap (harness prompt, new model against the
run's judge) and the prompt gap (strict against harness prompt, same model) side
by side. Without it, the report says in so many words when the gap is a mix.

Errors that will not go away. A request the API refuses for good (HTTP 400, 401,
403, 404: unknown model, no access, malformed request) is not retried and stops
the run at once, exit 2: the first answer of the batch is asked alone, before the
others, so a wrong model name costs one call and a few seconds. Transient errors
(quota, 5xx, transport) are retried per call; if `--max-failures` rows in a row
(default 8) end without a verdict the run stops too, exit 2, instead of walking
through the whole sample at a minute per row. Verdicts already obtained stay in
`verdicts.jsonl`.

Files written in `--out`: `sample.json` (the draw), `verdicts.jsonl` (one line per
judged answer as soon as it is judged), `report.json`, `report.md`. A second run
reuses a verdict only for the very prompt it was given for: each line carries the
sha256 of the rendered prompt (question, gold answer and generated answer
included) and the model, and is reused on equality of both. `--out` belongs to one
result file: a directory whose `sample.json` was drawn from another file is
refused, so the verdicts of one run are never read as the verdicts of another.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import datetime as _dt
import difflib
import functools
import hashlib
import json
import math
import os
import random
import subprocess
import sys
import threading
import time
from collections import Counter
from pathlib import Path

_HERE = Path(__file__).resolve().parent

# (sentence of the harness' LoCoMo judge prompt, what the strict prompt says instead)
STRICT_EDITS: list[tuple[str, str]] = [
    (
        "The generated answer might be much longer, but you should be generous with your grading - as long as it "
        "touches on the same topic as the gold answer, it should be counted as CORRECT.",
        "The generated answer might be much longer. Count it as CORRECT only if it states the same fact as the gold "
        "answer: the same person, object, place, number or event. Touching on the same topic is not enough. If the "
        "gold answer lists several items, the generated answer must give all of them. If the generated answer gives "
        "the gold answer together with a different answer that contradicts it, count it as WRONG.",
    ),
    (
        "The generated answer might be much longer or use relative time references (like \"last Tuesday\" or \"next "
        "month\"), but you should be generous with your grading - as long as it refers to the same date or time period "
        "as the gold answer, it should be counted as CORRECT.",
        "The generated answer might be much longer or use relative time references (like \"last Tuesday\" or \"next "
        "month\"). Count it as CORRECT only if it resolves to the same date or time period as the gold answer, at the "
        "precision the gold answer gives: another day, month or year is WRONG, and so is a relative reference that "
        "cannot be resolved to the gold date.",
    ),
    (
        "if the generated answer says that it cannot be answered or it doesn't know all the details, it should be "
        "counted as CORRECT.",
        "in that case only, a generated answer that says the question cannot be answered should be counted as "
        "CORRECT. When the gold answer states a fact, a generated answer that says it cannot answer, or that does "
        "not commit to an answer, is WRONG.",
    ),
    (
        "If it's correct, set correct=true.",
        "Set correct=true only if the generated answer is CORRECT under the rules above; otherwise set correct=false.",
    ),
]

_SENTINELS = {"query": "\u0000QUERY\u0000", "gold": "\u0000GOLD\u0000", "answer": "\u0000ANSWER\u0000"}


class JudgeError(RuntimeError):
    """The judge gave no usable verdict."""


class JudgeFatal(JudgeError):
    """The API refused the request for good: asking again, for this row or another, cannot help."""


_FATAL_STATUS = (400, 401, 403, 404)


def http_status(error: BaseException) -> int | None:
    """HTTP status carried by an API error (google-genai: `.code`), or None."""
    for attr in ("code", "status_code"):
        value = getattr(error, attr, None)
        if isinstance(value, int) and not isinstance(value, bool):
            return value
    return None


# ------------------------------------------------------------------------ prompts

@functools.lru_cache(maxsize=1)
def harness_template() -> str:
    """The harness' LoCoMo judge prompt as a `{query}/{gold}/{answer}` template."""
    home = Path(os.environ.get("AMB_HOME") or _HERE / "agent-memory-benchmark").resolve()
    if not (home / "src" / "memory_bench").is_dir():
        sys.exit(f"rejudge_strict: AMB checkout not found at {home}: set AMB_HOME (the strict prompt is derived "
                 f"from the harness' own prompt), or give a full template with --prompt-file")
    sys.path.insert(0, str(_HERE))
    import run_amb

    run_amb.bootstrap()
    from memory_bench.dataset.locomo import LoComoDataset

    prompt = LoComoDataset().build_judge_prompt(_SENTINELS["query"], [_SENTINELS["gold"]], _SENTINELS["answer"])
    for sentinel in _SENTINELS.values():
        if prompt.count(sentinel) != 1:
            sys.exit("rejudge_strict: the harness judge prompt does not use question, gold and answer exactly once")
    prompt = prompt.replace("{", "{{").replace("}", "}}")
    for key, sentinel in _SENTINELS.items():
        prompt = prompt.replace(sentinel, "{" + key + "}")
    return prompt


def strict_template(lenient: str) -> str:
    """The harness template with STRICT_EDITS applied; stops if one of them does not apply."""
    strict = lenient
    for old, new in STRICT_EDITS:
        if strict.count(old) != 1:
            sys.exit(f"rejudge_strict: the harness judge prompt does not contain exactly once the sentence to "
                     f"replace:\n  {old}\nThe harness changed: review STRICT_EDITS before paying for a judge run.")
        strict = strict.replace(old, new)
    for word in ("generous", "touches on the same topic"):
        if word in strict:
            sys.exit(f"rejudge_strict: the strict prompt still says {word!r}")
    return strict


def render(template: str, row: dict) -> str:
    gold = row["gold_answers"][0] if row.get("gold_answers") else ""
    return template.format(query=row["query"], gold=gold, answer=row["answer"])


def prompt_diff(lenient: str, strict: str) -> str:
    return "\n".join(difflib.unified_diff(lenient.splitlines(), strict.splitlines(), "harness LoCoMo judge prompt",
                                          "strict judge prompt", lineterm="", n=1))


# ------------------------------------------------------------------------- sample

def category_of(row: dict) -> str:
    meta = row.get("meta") or {}
    axes = row.get("category_axes") or {}
    return meta.get("category") or meta.get("question_type") or (axes.get("Question Type") or ["unknown"])[0]


def allocate(sizes: dict[str, int], n: int) -> dict[str, int]:
    """Proportional allocation of n over the strata, largest remainders, never above the stratum size."""
    total = sum(sizes.values())
    if n > total:
        sys.exit(f"rejudge_strict: --n {n} is more than the {total} answers of the run")
    exact = {k: n * v / total for k, v in sizes.items()}
    out = {k: int(math.floor(x)) for k, x in exact.items()}
    for k in sorted(sizes, key=lambda k: (-(exact[k] - out[k]), k))[: n - sum(out.values())]:
        out[k] += 1
    return out


def stratified_sample(rows: list[dict], n: int, seed: int) -> tuple[list[dict], dict]:
    """Deterministic draw: depends on the seed, the stratum name and the sorted question ids only."""
    strata: dict[str, list[dict]] = {}
    for row in rows:
        strata.setdefault(category_of(row), []).append(row)
    allocation = allocate({k: len(v) for k, v in strata.items()}, n)
    picked: list[dict] = []
    for name in sorted(strata):
        members = sorted(strata[name], key=lambda r: r["query_id"])
        rng = random.Random(f"{seed}:{name}")
        picked.extend(rng.sample(members, allocation[name]))
    picked.sort(key=lambda r: r["query_id"])
    design = {name: {"population": len(strata[name]), "sample": allocation[name]} for name in sorted(strata)}
    return picked, design


# -------------------------------------------------------------------------- judge

def parse_verdict(text: str | None, parsed=None) -> dict:
    """`{reason: str, correct: bool}` or JudgeError. Never guesses a verdict from free text."""
    data = parsed
    if not isinstance(data, dict):
        try:
            data = json.loads(text or "")
        except ValueError as e:
            raise JudgeError(f"not JSON: {(text or '')[:120]!r}") from e
    if not isinstance(data, dict) or not isinstance(data.get("correct"), bool) or not isinstance(data.get("reason"), str):
        raise JudgeError(f"not a {{reason: string, correct: boolean}} object: {str(data)[:120]}")
    return {"correct": data["correct"], "reason": data["reason"]}


class VertexJudge:
    """Gemini on Vertex AI. `client` can be injected (tests): no network, no credentials then."""

    def __init__(self, model: str, auth: str = "gcloud", client=None, attempts: int = 5, backoff_s: float = 5.0):
        self.model = model
        self._attempts = attempts
        self._backoff_s = backoff_s
        self._client = client or self._make_client(auth)

    @staticmethod
    def _make_client(auth: str):
        import google.auth.credentials
        from google import genai
        from google.genai import types

        project = os.environ.get("GOOGLE_CLOUD_PROJECT")
        if not project:
            sys.exit("rejudge_strict: set GOOGLE_CLOUD_PROJECT (and optionally GOOGLE_CLOUD_LOCATION, default `global`)")
        kwargs = dict(vertexai=True, project=project, location=os.environ.get("GOOGLE_CLOUD_LOCATION", "global"),
                      http_options=types.HttpOptions(timeout=300_000))
        if auth == "adc":
            return genai.Client(**kwargs)

        class GcloudCredentials(google.auth.credentials.Credentials):
            """Access token of the active gcloud account (same mechanism as ICM_AMB_GCLOUD_AUTH)."""

            def __init__(self):
                super().__init__()
                self._refresh_lock = threading.Lock()

            def refresh(self, request):
                with self._refresh_lock:
                    out = subprocess.run(["gcloud", "auth", "print-access-token"], capture_output=True, text=True, timeout=60)
                    if out.returncode != 0 or not out.stdout.strip():
                        raise RuntimeError("`gcloud auth print-access-token` failed; run `gcloud auth login`")
                    self.token = out.stdout.strip()
                    self.expiry = _dt.datetime.utcnow() + _dt.timedelta(minutes=10)

        return genai.Client(credentials=GcloudCredentials(), **kwargs)

    def __call__(self, prompt: str) -> dict:
        from google.genai import types

        # Same settings as the harness' judge call (JSON output, temperature 0); reason before verdict.
        config = types.GenerateContentConfig(
            temperature=0.0, response_mime_type="application/json",
            response_schema=types.Schema(
                type=types.Type.OBJECT, required=["reason", "correct"], property_ordering=["reason", "correct"],
                properties={"reason": types.Schema(type=types.Type.STRING), "correct": types.Schema(type=types.Type.BOOLEAN)},
            ),
        )
        delay, last = self._backoff_s, None
        for attempt in range(self._attempts):
            try:
                response = self._client.models.generate_content(model=self.model, contents=prompt, config=config)
                return parse_verdict(getattr(response, "text", None), getattr(response, "parsed", None))
            except Exception as e:  # noqa: BLE001 - quota, 5xx, transport, unusable output: retried, then reported
                if http_status(e) in _FATAL_STATUS:
                    # Unknown model, no access, malformed request: the same answer every time.
                    raise JudgeFatal(f"HTTP {http_status(e)} from the API for model {self.model!r}, not retried: "
                                     f"{str(e)[:300]}") from e
                last = e
                if attempt < self._attempts - 1:
                    time.sleep(delay)
                    delay = min(delay * 2, 60.0)
        raise JudgeError(f"no verdict after {self._attempts} attempts: {type(last).__name__}: {str(last)[:200]}")


# -------------------------------------------------------------------------- stats

def wilson(successes: int, n: int, z: float = 1.959964) -> tuple[float, float]:
    if n == 0:
        return 0.0, 0.0
    p = successes / n
    centre = (p + z * z / (2 * n)) / (1 + z * z / n)
    half = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / (1 + z * z / n)
    return max(0.0, centre - half), min(1.0, centre + half)


def mcnemar_exact(b: int, c: int) -> float:
    """Two-sided exact McNemar p-value on the discordant pairs."""
    n = b + c
    if n == 0:
        return 1.0
    tail = sum(math.comb(n, i) for i in range(0, min(b, c) + 1)) / 2 ** n
    return min(1.0, 2 * tail)


def paired(rows: list[dict], a: str, b: str) -> dict:
    """Two verdicts on the same answers: accuracy of each, agreement, second minus first, exact McNemar."""
    n = len(rows)
    both = sum(1 for r in rows if r[a] and r[b])
    only_a = sum(1 for r in rows if r[a] and not r[b])
    only_b = sum(1 for r in rows if not r[a] and r[b])
    hits_a, hits_b = both + only_a, both + only_b
    return {
        "n": n, "first": a, "second": b, "first_correct": hits_a, "second_correct": hits_b,
        "first_accuracy": round(hits_a / n, 4) if n else None, "second_accuracy": round(hits_b / n, 4) if n else None,
        "agreement": round((n - only_a - only_b) / n, 4) if n else None,
        "only_first_correct": only_a, "only_second_correct": only_b,
        "difference_points": round((hits_b - hits_a) / n * 100, 2) if n else None,
        "mcnemar_exact_p": round(mcnemar_exact(only_a, only_b), 6),
    }


def decompose(rows: list[dict]) -> dict:
    """Model gap and prompt gap on the answers judged in both arms.

    `harness_correct`: the run's judge (its model, the harness prompt).
    `control_correct`: the strict judge's model with the harness prompt.
    `strict_correct`:  the strict judge's model with the strict prompt."""
    model_gap = paired(rows, "harness_correct", "control_correct")
    prompt_gap = paired(rows, "control_correct", "strict_correct")
    total = paired(rows, "harness_correct", "strict_correct")
    return {
        "judged_in_both_arms": len(rows),
        "model_gap": dict(model_gap, reading="harness prompt, strict judge's model, minus the run's own judge: "
                                              "what changing the model alone does"),
        "prompt_gap": dict(prompt_gap, reading="strict prompt minus harness prompt, same model: what the prompt "
                                                "alone does"),
        "total_gap": dict(total, reading="strict judge minus the run's judge: the sum of the two gaps above"),
    }


def analyse(judged: list[dict], design: dict) -> dict:
    """`judged` rows carry `category`, `harness_correct`, `strict_correct`."""
    n = len(judged)
    tt = sum(1 for r in judged if r["harness_correct"] and r["strict_correct"])
    tf = sum(1 for r in judged if r["harness_correct"] and not r["strict_correct"])
    ft = sum(1 for r in judged if not r["harness_correct"] and r["strict_correct"])
    ff = n - tt - tf - ft
    agree = (tt + ff) / n if n else 0.0
    p_h, p_s = ((tt + tf) / n, (tt + ft) / n) if n else (0.0, 0.0)
    expected = p_h * p_s + (1 - p_h) * (1 - p_s)
    kappa = (agree - expected) / (1 - expected) if n and expected < 1 else None

    def side(key: str) -> dict:
        hits = sum(1 for r in judged if r[key])
        low, high = wilson(hits, n)
        return {"correct": hits, "n": n, "accuracy": round(hits / n, 4) if n else None, "ci95": [round(low, 4), round(high, 4)]}

    by_category = {}
    for name in sorted({r["category"] for r in judged}):
        part = [r for r in judged if r["category"] == name]
        by_category[name] = {
            "n": len(part),
            "harness_correct": sum(1 for r in part if r["harness_correct"]),
            "strict_correct": sum(1 for r in part if r["strict_correct"]),
            "disagreements": sum(1 for r in part if r["harness_correct"] != r["strict_correct"]),
        }

    # Estimate for the whole run: each stratum weighted by its share of the run.
    population = sum(d["population"] for d in design.values())
    estimate = variance = 0.0
    covered = True
    for name, d in design.items():
        part = by_category.get(name)
        if not part or not part["n"]:
            covered = covered and d["sample"] == 0
            continue
        weight = d["population"] / population
        p = part["strict_correct"] / part["n"]
        fpc = 1 - part["n"] / d["population"]
        estimate += weight * p
        variance += weight ** 2 * p * (1 - p) / part["n"] * fpc
    half = 1.959964 * math.sqrt(variance)
    return {
        "judged": n,
        "confusion": {"harness_correct_strict_correct": tt, "harness_correct_strict_wrong": tf,
                      "harness_wrong_strict_correct": ft, "harness_wrong_strict_wrong": ff},
        "agreement": round(agree, 4), "cohen_kappa": round(kappa, 4) if kappa is not None else None,
        "harness_judge_on_sample": side("harness_correct"),
        "strict_judge_on_sample": side("strict_correct"),
        "difference_strict_minus_harness": round(p_s - p_h, 4),
        "mcnemar_exact_p": round(mcnemar_exact(tf, ft), 6),
        "strict_judge_run_estimate": {
            "accuracy": round(estimate, 4), "ci95": [round(max(0.0, estimate - half), 4), round(min(1.0, estimate + half), 4)],
            "method": "strata weighted by their share of the run, normal interval with finite-population correction",
            "every_stratum_judged": covered,
        },
        "by_category": by_category,
    }


# ------------------------------------------------------------------------- report

def _cell(text: str, limit: int = 700) -> str:
    text = " ".join(str(text).split())
    return (text[:limit] + " [...]") if len(text) > limit else text


def markdown(report: dict, disagreements: list[dict]) -> str:
    a, c = report["analysis"], report["analysis"]["confusion"]
    h, s, est = a["harness_judge_on_sample"], a["strict_judge_on_sample"], a["strict_judge_run_estimate"]
    pct = lambda x: "n/a" if x is None else f"{x:.1%}"  # noqa: E731
    strata = ", ".join("%s %d/%d" % (k, v["sample"], v["population"]) for k, v in report["sample"]["strata"].items())
    lines = [
        f"# Strict re-judging of {report['results_file']}",
        "",
        f"- Run: {report['run']['dataset']}/{report['run']['split']}, {report['run']['run_name']}, "
        f"{report['run']['total_queries']} answers, answer model {report['run']['answer_llm']}, "
        f"harness judge {report['run']['judge_llm']}, accuracy under that judge {pct(report['run']['accuracy'])}.",
        f"- Sample: {report['sample']['n']} answers, seed {report['sample']['seed']}, stratified by question type "
        f"({strata}).",
        f"- Strict judge: {report['strict_judge']['model']}, prompt {report['strict_judge']['prompt']} "
        f"(sha256 {report['strict_judge']['prompt_sha256'][:12]}).",
        f"- Judged: {a['judged']} of {report['sample']['n']}; unjudged: {len(report['unjudged'])}.",
        "",
        "## Result",
        "",
        f"- Agreement between the two judges: {pct(a['agreement'])} (Cohen's kappa {a['cohen_kappa']}).",
        f"- Harness judge on the sample: {h['correct']}/{h['n']} = {pct(h['accuracy'])} "
        f"(95% interval {pct(h['ci95'][0])} to {pct(h['ci95'][1])}).",
        f"- Strict judge on the sample: {s['correct']}/{s['n']} = {pct(s['accuracy'])} "
        f"(95% interval {pct(s['ci95'][0])} to {pct(s['ci95'][1])}).",
        f"- Difference, strict minus harness, same answers: {a['difference_strict_minus_harness'] * 100:+.1f} points "
        f"(exact McNemar p = {a['mcnemar_exact_p']}).",
        f"- Strict judge, estimate for the whole run: {pct(est['accuracy'])} "
        f"(95% interval {pct(est['ci95'][0])} to {pct(est['ci95'][1])}; {est['method']}).",
        "",
        "## Confusion matrix",
        "",
        "| | strict: correct | strict: wrong |",
        "|---|---|---|",
        f"| harness: correct | {c['harness_correct_strict_correct']} | {c['harness_correct_strict_wrong']} |",
        f"| harness: wrong | {c['harness_wrong_strict_correct']} | {c['harness_wrong_strict_wrong']} |",
        "",
        "## By question type",
        "",
        "| type | n | harness correct | strict correct | disagreements |",
        "|---|---|---|---|---|",
    ]
    lines += [f"| {k} | {v['n']} | {v['harness_correct']} | {v['strict_correct']} | {v['disagreements']} |"
              for k, v in a["by_category"].items()]
    control = report.get("control")
    lines += ["", "## Model gap and prompt gap", ""]
    if control:
        m, pg, t = control["model_gap"], control["prompt_gap"], control["total_gap"]
        lines += [
            f"On the {control['judged_in_both_arms']} answers judged in both arms by {report['strict_judge']['model']}:",
            "",
            "| judge | prompt | correct | accuracy |",
            "|---|---|---|---|",
            f"| {report['run']['judge_llm']} (the run's judge) | harness | {m['first_correct']} | {pct(m['first_accuracy'])} |",
            f"| {report['strict_judge']['model']} | harness (control arm) | {m['second_correct']} | {pct(m['second_accuracy'])} |",
            f"| {report['strict_judge']['model']} | strict | {pg['second_correct']} | {pct(pg['second_accuracy'])} |",
            "",
            f"- Model gap (same harness prompt, other model): {m['difference_points']:+.1f} points "
            f"(agreement {pct(m['agreement'])}, exact McNemar p = {m['mcnemar_exact_p']}).",
            f"- Prompt gap (same model, strict against harness prompt): {pg['difference_points']:+.1f} points "
            f"(agreement {pct(pg['agreement'])}, exact McNemar p = {pg['mcnemar_exact_p']}).",
            f"- Total, strict judge against the run's judge: {t['difference_points']:+.1f} points.",
        ]
    else:
        lines += [report["gap_reading"]]
    lines += ["", "## What the strict prompt changes", "", "```diff", report["strict_judge"]["diff"] or "(custom prompt file: no diff)", "```", "",
              f"## Disagreements to read ({len(disagreements)})", "",
              "For each one, write who is right in the last line. The strict judge is a model too: these rows are "
              "the evidence, not its verdict.", ""]
    for d in disagreements:
        lines += [
            f"### {d['query_id']} ({d['category']}): harness {'CORRECT' if d['harness_correct'] else 'WRONG'}, "
            f"strict {'CORRECT' if d['strict_correct'] else 'WRONG'}",
            "",
            f"- Question: {_cell(d['query'])}",
            f"- Gold answer: {_cell(d['gold'])}",
            f"- Generated answer: {_cell(d['answer'])}",
            f"- Harness judge: {_cell(d['harness_reason'])}",
            f"- Strict judge: {_cell(d['strict_reason'])}",
            "- Human verdict: ",
            "",
        ]
    if report["unjudged"]:
        lines += ["## Unjudged (left out of every figure)", ""]
        lines += [f"- {u['query_id']} ({u.get('arm', 'strict')} arm): {_cell(u['error'], 200)}" for u in report["unjudged"]]
    return "\n".join(lines) + "\n"


# --------------------------------------------------------------------------- main

def count_tokens(text: str) -> int:
    try:
        import tiktoken
        return len(tiktoken.get_encoding("cl100k_base").encode(text, disallowed_special=()))
    except Exception:  # noqa: BLE001 - offline without the encoding file: a rough figure is enough here
        return len(text) // 4


def same_model(judge_llm: str | None, model: str) -> bool:
    """Is `model` the model of the run's own judge (`gemini:<model>` in a result file)?"""
    return bool(judge_llm) and judge_llm.split(":", 1)[-1] == model


def main(argv: list[str] | None = None, judge=None) -> int:
    """`judge`: callable prompt -> {correct, reason}; given by the tests in place of Vertex AI."""
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--results", required=True, type=Path, help="merged result file of the answer run")
    ap.add_argument("--out", required=True, type=Path, help="directory for sample, verdicts and report; one per result file")
    ap.add_argument("--model", help="strict judge model on Vertex AI (required unless --dry-run)")
    ap.add_argument("--n", type=int, default=200)
    ap.add_argument("--seed", type=int, default=20261005)
    ap.add_argument("--prompt-file", type=Path, help="full template with {query} {gold} {answer}; replaces the derived prompt")
    ap.add_argument("--control", action="store_true",
                    help="also judge the sample with the harness' unchanged prompt on the same model (twice the calls): "
                         "separates what the model changes from what the prompt changes")
    ap.add_argument("--auth", choices=["gcloud", "adc"], default="gcloud")
    ap.add_argument("--concurrency", type=int, default=4)
    ap.add_argument("--max-failures", type=int, default=8,
                    help="stop when this many rows in a row end without a verdict (default 8)")
    ap.add_argument("--show-diff", action="store_true", help="print harness prompt vs strict prompt and exit")
    ap.add_argument("--dry-run", action="store_true", help="draw the sample, size the prompts, call nothing")
    args = ap.parse_args(argv)

    lenient = None
    if args.prompt_file:
        strict, diff, prompt_name = args.prompt_file.read_text(), "", f"custom file {args.prompt_file.name}"
        for key in ("{query}", "{gold}", "{answer}"):
            if key not in strict:
                sys.exit(f"rejudge_strict: {args.prompt_file} has no {key} placeholder")
        if args.control:
            lenient = harness_template()
    else:
        lenient = harness_template()
        strict = strict_template(lenient)
        diff, prompt_name = prompt_diff(lenient, strict), f"harness LoCoMo judge prompt with {len(STRICT_EDITS)} edits"
    if args.show_diff:
        print(diff or strict)
        return 0
    prompt_sha = hashlib.sha256(strict.encode()).hexdigest()

    run = json.loads(args.results.read_text())
    rows = run.get("results") or []
    if run.get("dataset") != "locomo" and (not args.prompt_file or args.control):
        sys.exit(f"rejudge_strict: {args.results} is a {run.get('dataset')!r} run; the derived strict prompt and the "
                 f"control arm are LoCoMo's. Give the prompt to use with --prompt-file, without --control.")
    unusable = [r.get("query_id") for r in rows if not isinstance(r.get("answer"), str) or not r.get("gold_answers")
                or not isinstance(r.get("correct"), bool)]
    if unusable:
        sys.exit(f"rejudge_strict: {len(unusable)} rows have no answer, gold answer or first verdict (e.g. {unusable[:3]})")
    sample, design = stratified_sample(rows, args.n, args.seed)
    # arm -> query id -> the exact text sent. `strict` is the measurement, `control` the harness prompt unchanged.
    prompts = {"strict": {r["query_id"]: render(strict, r) for r in sample}}
    if args.control:
        prompts["control"] = {r["query_id"]: render(lenient, r) for r in sample}

    results_sha = hashlib.sha256(args.results.read_bytes()).hexdigest()
    sample_path = args.out / "sample.json"
    if sample_path.exists():
        try:
            earlier = json.loads(sample_path.read_text())
        except ValueError:
            sys.exit(f"rejudge_strict: {sample_path} is not readable JSON; use another --out")
        if earlier.get("results_sha256") != results_sha:
            sys.exit(f"rejudge_strict: {args.out} holds the sample and verdicts of another result file "
                     f"({earlier.get('results_file')}, sha256 {str(earlier.get('results_sha256'))[:12]}); this one is "
                     f"{args.results} (sha256 {results_sha[:12]}). One --out per result file: use another directory.")
    args.out.mkdir(parents=True, exist_ok=True)
    sample_doc = {"results_file": str(args.results), "results_sha256": results_sha,
                  "n": len(sample), "seed": args.seed, "strata": design, "query_ids": [r["query_id"] for r in sample]}
    sample_path.write_text(json.dumps(sample_doc, indent=1))
    tokens = {arm: sum(count_tokens(p) for p in texts.values()) for arm, texts in prompts.items()}
    n_calls = sum(len(texts) for texts in prompts.values())
    print(f"sample: {len(sample)} of {len(rows)} answers, seed {args.seed}: "
          + ", ".join(f"{k} {v['sample']}/{v['population']}" for k, v in design.items()))
    print(f"strict prompt: {prompt_name}, sha256 {prompt_sha[:12]}; {n_calls} judge calls, "
          f"{sum(tokens.values())} input tokens in all (cl100k count, {sum(tokens.values()) / n_calls:.0f} per call)"
          + (f"; of which control arm (harness prompt): {len(prompts['control'])} calls, {tokens['control']} tokens"
             if args.control else ""))
    if args.dry_run:
        first = sample[0]["query_id"]
        print(f"--dry-run: nothing sent. First prompt ({first}):\n{prompts['strict'][first]}")
        return 0

    if judge is None:
        if not args.model:
            sys.exit("rejudge_strict: --model is required (the strict judge model on Vertex AI)")
        judge = VertexJudge(args.model, args.auth)
    model = args.model or getattr(judge, "model", None) or "injected-judge"

    def rendered_sha(arm: str, qid: str) -> str:
        return hashlib.sha256(prompts[arm][qid].encode()).hexdigest()

    # A verdict is reused for the prompt it answered and for nothing else: same model,
    # same arm, same rendered text (so same question, gold answer and generated answer).
    verdict_path = args.out / "verdicts.jsonl"
    verdicts: dict[tuple[str, str], dict] = {}
    if verdict_path.exists():
        for line in verdict_path.read_text().splitlines():
            try:
                v = json.loads(line)
            except ValueError:
                continue
            arm, qid = v.get("arm", "strict"), v.get("query_id")
            if (v.get("model") == model and arm in prompts and qid in prompts[arm]
                    and v.get("rendered_sha256") == rendered_sha(arm, qid)
                    and isinstance(v.get("correct"), bool) and isinstance(v.get("reason"), str)):
                verdicts[(arm, qid)] = v
    todo = [(arm, r["query_id"]) for arm in prompts for r in sample if (arm, r["query_id"]) not in verdicts]
    print(f"{len(verdicts)} verdicts reused from {verdict_path.name}, {len(todo)} to judge with {model}")

    errors: dict[tuple[str, str], str] = {}
    lock = threading.Lock()
    stop = {"reason": None, "streak": 0}

    def one(item: tuple[str, str]) -> None:
        arm, qid = item
        if stop["reason"]:
            return
        try:
            verdict = parse_verdict(None, judge(prompts[arm][qid]))
        except JudgeFatal as e:
            with lock:
                stop["reason"] = stop["reason"] or f"{e}"
            return
        except Exception as e:  # noqa: BLE001 - one failed call must not lose the verdicts already paid for
            with lock:
                errors[item] = f"{type(e).__name__}: {e}"
                stop["streak"] += 1
                if stop["streak"] >= max(1, args.max_failures) and not stop["reason"]:
                    stop["reason"] = (f"{stop['streak']} rows in a row ended without a verdict "
                                      f"(last: {errors[item][:200]})")
            return
        record = {"query_id": qid, "arm": arm, "model": model, "prompt_sha256": prompt_sha,
                  "rendered_sha256": rendered_sha(arm, qid), **verdict}
        with lock:
            stop["streak"] = 0
            verdicts[item] = record
            with open(verdict_path, "a") as fh:
                fh.write(json.dumps(record) + "\n")

    if todo:
        one(todo[0])  # alone first: a refusal of the model or of the account shows here, for the price of one call
    if not stop["reason"]:
        with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, args.concurrency)) as pool:
            list(pool.map(one, todo[1:]))
    if stop["reason"]:
        kept = sum(1 for item in todo if item in verdicts)
        print(f"STOPPED: {stop['reason']}\n{kept} of {len(todo)} new verdicts were obtained and are kept in {verdict_path}; "
              f"no report written. Fix the cause and run the same command again.", file=sys.stderr)
        return 2

    judged, disagreements = [], []
    for row in sample:
        v = verdicts.get(("strict", row["query_id"]))
        if v is None:
            continue
        item = {"query_id": row["query_id"], "category": category_of(row), "harness_correct": row["correct"],
                "strict_correct": v["correct"]}
        c = verdicts.get(("control", row["query_id"]))
        if c is not None:
            item["control_correct"] = c["correct"]
        judged.append(item)
        if item["harness_correct"] != item["strict_correct"]:
            disagreements.append(dict(item, query=row["query"], gold=row["gold_answers"][0], answer=row["answer"],
                                      harness_reason=row.get("judge_reason") or "", strict_reason=v["reason"]))
    if not judged:
        print(f"UNJUDGED: none of the {len(sample)} answers got a verdict; no report written. "
              f"First error: {next(iter(errors.values()), 'none recorded')[:300]}", file=sys.stderr)
        return 3
    if args.control:
        gap_reading = "See the model gap and the prompt gap below (control arm)."
    elif same_model(run.get("judge_llm"), model):
        gap_reading = (f"The strict judge is the run's own judge model ({model}): the difference between the two judges "
                       f"is the effect of the prompt (and of asking a second time).")
    else:
        gap_reading = (f"The strict judge ({model}) is not the run's judge model ({run.get('judge_llm')}): the difference "
                       f"between the two judges mixes the model and the prompt. Run again with --control to separate them.")
    report = {
        "results_file": str(args.results), "results_sha256": results_sha,
        "run": {k: run.get(k) for k in ("dataset", "split", "run_name", "memory_provider", "total_queries", "accuracy",
                                        "answer_llm", "judge_llm", "description")},
        "sample": {"n": len(sample), "seed": args.seed, "strata": design},
        "strict_judge": {"model": model, "prompt": prompt_name, "prompt_sha256": prompt_sha, "template": strict, "diff": diff},
        "gap_reading": gap_reading,
        "unjudged": [{"query_id": qid, "arm": arm, "error": err} for (arm, qid), err in sorted(errors.items())],
        "analysis": analyse(judged, design),
        "control": decompose([r for r in judged if "control_correct" in r]) if args.control else None,
        "disagreements": disagreements,
    }
    (args.out / "report.json").write_text(json.dumps(report, indent=1))
    (args.out / "report.md").write_text(markdown(report, disagreements))
    a = report["analysis"]
    print(f"judged {a['judged']}/{len(sample)}; agreement {a['agreement']:.1%}; harness {a['harness_judge_on_sample']['accuracy']:.1%}, "
          f"strict {a['strict_judge_on_sample']['accuracy']:.1%} (95% {a['strict_judge_on_sample']['ci95'][0]:.1%} to "
          f"{a['strict_judge_on_sample']['ci95'][1]:.1%}); {len(disagreements)} disagreements -> {args.out / 'report.md'}")
    if report["control"]:
        c = report["control"]
        print(f"control arm, {c['judged_in_both_arms']} answers: model gap {c['model_gap']['difference_points']:+.1f} points, "
              f"prompt gap {c['prompt_gap']['difference_points']:+.1f} points")
    else:
        print(gap_reading)
    if errors:
        print(f"UNJUDGED: {len(errors)} calls got no verdict and are left out of the figures: "
              f"{', '.join(sorted(qid for _, qid in errors)[:10])}")
        return 3
    return 0


if __name__ == "__main__":
    sys.exit(main())
