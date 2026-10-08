#!/usr/bin/env python3
"""What an answer run costs: tokens read from files, dollars from the prices you give.

    # a finished run, as measured by the API (the usage file entrypoint.sh keeps):
    python estimate_cost.py --results out/merged/locomo/icm-v2/rag/locomo10.json \\
        --usage out-gke/RUN/shard-*/usage-*.jsonl \\
        --answer-price 2.00 12.00 --judge-price 0.10 0.40

    # a run not launched yet, sized with dry_run_tokens.py, from a finished run of the
    # same dataset for everything the answer model and the judge write:
    python estimate_cost.py --results results/locomo10-icm-v2-k50-gke-20261005.json \\
        --answer-input-tokens 61857481 \\
        --answer-price 2.00 12.00 --judge-price 0.10 0.40 \\
        --token-ratio 1.0 1.3 --thinking-per-answer 500 4000

Prices are arguments, in US dollars per million tokens, input then output: this
script holds none, because they change. Read them on the provider's price list the
day you launch (Vertex AI: https://cloud.google.com/vertex-ai/generative-ai/pricing;
the output price covers "response and reasoning", i.e. thinking tokens are billed as
output) and check the long-context tier: `--long-context THRESHOLD IN OUT` gives the
tier's prices, and a call whose input exceeds THRESHOLD is charged at them, input and
output. With a usage file each call is priced at its own tier; without one the whole
run is priced at the long-context tier as soon as its largest call (`--max-call-input`
for a planned run) is over the threshold.

Two kinds of figures, never mixed without a label:

  measured   read in a file. From `--results`: the context tokens of each question
             (`context_tokens`, the harness' tiktoken cl100k count), the text the
             answer model wrote that the file kept (`answer`, and `reasoning` when the
             file has it), the judge's `judge_reason`. From `--usage` (JSONL written
             when ICM_AMB_USAGE is set, see run_amb.py): the API's own counts for
             every response, per model: prompt, output, thinking, cached tokens.
  assumed    what no file holds, given on the command line as a low and a high value:
             `--token-ratio` (billed tokens per cl100k token: the model's tokenizer is
             not tiktoken) and `--thinking-per-answer` (thinking tokens of the answer
             model per question; a result file never records them).

With `--usage` nothing is assumed: the bill is the measured counts times the prices,
and the script prints the two ratios the next estimate needs (billed input tokens
per cl100k token, thinking tokens per answer). Without it a dollar figure needs both
assumptions spelled out; the script refuses to print one otherwise, and always
prints the floor (visible output only, ratio 1) next to the range.

Not counted, to keep in mind: calls the harness repeats (an answer it cannot parse is
asked again and billed again; requests answered 429 are not billed), the judge's own
thinking tokens if its model thinks, and any discount or batch price.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

ANSWER_PROMPT_OVERHEAD = 196  # cl100k tokens of instructions + question around the context (LoCoMo, dry_run_tokens.py)
JUDGE_PROMPT_OVERHEAD = 394   # cl100k tokens of the LoCoMo judge prompt around question, gold answer and answer


def count_tokens(text: str) -> int:
    import tiktoken

    global _ENC
    try:
        enc = _ENC
    except NameError:
        enc = _ENC = tiktoken.get_encoding("cl100k_base")
    return len(enc.encode(text or "", disallowed_special=()))


def read_results(path: Path) -> dict:
    """Per-question cl100k figures of a finished answer run."""
    run = json.loads(path.read_text())
    rows = run.get("results") or []
    if not rows:
        sys.exit(f"estimate_cost: {path} holds no result row")
    missing = [r.get("query_id") for r in rows if r.get("context_tokens") is None]
    if missing:
        sys.exit(f"estimate_cost: {len(missing)} rows of {path} have no context_tokens (e.g. {missing[:3]})")
    has_reasoning = all(isinstance(r.get("reasoning"), str) for r in rows)
    return {
        "file": str(path), "dataset": run.get("dataset"), "split": run.get("split"), "run_name": run.get("run_name"),
        "answer_llm": run.get("answer_llm"), "judge_llm": run.get("judge_llm"), "questions": len(rows),
        "context_tokens": sum(r["context_tokens"] for r in rows),
        "max_context_tokens": max(r["context_tokens"] for r in rows),
        "answer_visible_tokens": sum(count_tokens(r.get("answer") or "") + (count_tokens(r["reasoning"]) if has_reasoning else 0)
                                     for r in rows),
        "answer_visible_fields": "answer + reasoning" if has_reasoning else "answer only (the file has no `reasoning` field)",
        "judge_text_tokens": sum(count_tokens(r.get("query") or "") + count_tokens(str((r.get("gold_answers") or [""])[0]))
                                 + count_tokens(r.get("answer") or "") for r in rows),
        "judge_visible_tokens": sum(count_tokens(r.get("judge_reason") or "") for r in rows),
    }


def read_usage(paths: list[Path]) -> dict[str, dict]:
    """Per model: calls and the API's token counts, summed over the usage files."""
    models: dict[str, dict] = {}
    for path in paths:
        for line in path.read_text().splitlines():
            if not line.strip():
                continue
            row = json.loads(line)
            entry = models.setdefault(row.get("model") or "unknown", {
                "calls": 0, "unreported": 0, "prompt_tokens": 0, "output_tokens": 0, "thinking_tokens": 0,
                "cached_tokens": 0, "total_tokens": 0, "max_prompt_tokens": 0, "prompts": []})
            entry["calls"] += 1
            entry["unreported"] += not row.get("reported", True)
            for key in ("prompt_tokens", "output_tokens", "thinking_tokens", "cached_tokens", "total_tokens"):
                entry[key] += int(row.get(key) or 0)
            entry["max_prompt_tokens"] = max(entry["max_prompt_tokens"], int(row.get("prompt_tokens") or 0))
            entry["prompts"].append((int(row.get("prompt_tokens") or 0),
                                     int(row.get("output_tokens") or 0) + int(row.get("thinking_tokens") or 0)))
    return models


def dollars(input_tokens: float, output_tokens: float, price: tuple[float, float]) -> float:
    return (input_tokens * price[0] + output_tokens * price[1]) / 1e6


def measured_cost(entry: dict, price: tuple[float, float], long_context: tuple[float, float, float] | None) -> float:
    """Bill of one model from its usage lines: each call at the tier its own input falls in."""
    total = 0.0
    for prompt, output in entry["prompts"]:
        tier = (long_context[1], long_context[2]) if long_context and prompt > long_context[0] else price
        total += dollars(prompt, output, tier)
    return total


def _model_of(llm_id: str | None) -> str | None:
    return llm_id.split(":", 1)[-1] if llm_id else None


def estimate(args: argparse.Namespace) -> dict:
    base = read_results(args.results)
    n_base = base["questions"]
    questions = args.questions or n_base
    planned = args.answer_input_tokens is not None
    answer_input = args.answer_input_tokens if planned else base["context_tokens"] + n_base * args.answer_overhead
    if not planned and questions != n_base:
        sys.exit("estimate_cost: --questions without --answer-input-tokens: the input of the planned run is unknown")
    scale = questions / n_base  # per-question output and judge figures are carried over from --results
    judge_input = (base["judge_text_tokens"] + n_base * args.judge_overhead) * scale
    out: dict = {
        "based_on": base,
        "run": {"questions": questions, "planned": planned},
        "measured_cl100k": {
            "answer_input_tokens": round(answer_input),
            "answer_input_source": ("--answer-input-tokens (dry_run_tokens.py)" if planned else
                                    f"context_tokens of the file + {args.answer_overhead} prompt tokens per question"),
            "answer_visible_output_tokens": round(base["answer_visible_tokens"] * scale),
            "answer_visible_output_fields": base["answer_visible_fields"],
            "judge_input_tokens": round(judge_input),
            "judge_visible_output_tokens": round(base["judge_visible_tokens"] * scale),
            "thinking_tokens": "not in a result file",
        },
    }
    usage = read_usage(args.usage) if args.usage else {}
    if usage:
        answer_model, judge_model = _model_of(base["answer_llm"]), _model_of(base["judge_llm"])
        report = {}
        for model, entry in usage.items():
            role = "answer" if model == answer_model else "judge" if model == judge_model else "other"
            price = {"answer": args.answer_price, "judge": args.judge_price}.get(role)
            item = {k: v for k, v in entry.items() if k != "prompts"}
            item["role"] = role
            item["calls_over_long_context_threshold"] = (
                sum(1 for prompt, _ in entry["prompts"] if prompt > args.long_context[0]) if args.long_context else None)
            if price:
                item["cost_usd"] = round(measured_cost(entry, tuple(price), args.long_context if role == "answer" else None), 2)
            report[model] = item
        out["measured_api"] = report
        answer = usage.get(answer_model)
        if answer and not planned:
            out["for_the_next_estimate"] = {
                "billed_input_tokens_per_cl100k_token": round(answer["prompt_tokens"] / answer_input, 4),
                "thinking_tokens_per_answer_call": round(answer["thinking_tokens"] / answer["calls"], 1),
                "output_tokens_per_answer_call": round(answer["output_tokens"] / answer["calls"], 1),
                "answer_calls_per_question": round(answer["calls"] / questions, 3),
            }
        costs = [item.get("cost_usd") for item in report.values() if item["role"] in ("answer", "judge")]
        if costs and all(c is not None for c in costs):
            out["cost_usd"] = {"measured": round(sum(costs), 2), "source": "usage files x prices given"}
        return out

    m = out["measured_cl100k"]
    out["assumed"] = {"token_ratio": args.token_ratio, "thinking_per_answer": args.thinking_per_answer}
    if args.answer_price and args.judge_price:
        answer_price, judge_price = tuple(args.answer_price), tuple(args.judge_price)

        # The largest call decides: without per-call counts the whole run is priced at the
        # long-context tier as soon as that call is over the threshold (an upper bound).
        largest = args.max_call_input or (answer_input / questions if planned
                                          else base["max_context_tokens"] + args.answer_overhead)

        def total(ratio: float, thinking: float) -> dict:
            price = answer_price
            long_tier = bool(args.long_context) and largest * ratio > args.long_context[0]
            if long_tier:
                price = (args.long_context[1], args.long_context[2])
            answer = dollars(m["answer_input_tokens"] * ratio,
                             m["answer_visible_output_tokens"] * ratio + thinking * questions, price)
            judge = dollars(m["judge_input_tokens"] * ratio, m["judge_visible_output_tokens"] * ratio, judge_price)
            return {"answer_usd": round(answer, 2), "judge_usd": round(judge, 2), "total_usd": round(answer + judge, 2),
                    "answer_input_usd": round(m["answer_input_tokens"] * ratio * price[0] / 1e6, 2),
                    "long_context_tier": long_tier}

        out["cost_usd"] = {"floor": dict(total(1.0, 0.0), reading="visible output only, 1 billed token per cl100k token: "
                                                                    "measured quantities, the bill cannot be lower")}
        if args.token_ratio and args.thinking_per_answer:
            out["cost_usd"]["low"] = total(args.token_ratio[0], args.thinking_per_answer[0])
            out["cost_usd"]["high"] = total(args.token_ratio[1], args.thinking_per_answer[1])
    return out


def render(out: dict, args: argparse.Namespace) -> str:
    base, m = out["based_on"], out["measured_cl100k"]
    lines = [
        f"{base['dataset']}/{base['split']}: {out['run']['questions']} questions"
        + (f" (planned run; output and judge figures per question from {base['file']}, {base['questions']} questions)"
           if out["run"]["planned"] else f", {base['file']}"),
        f"models: answer {base['answer_llm']}, judge {base['judge_llm']}",
        "",
        "measured, tiktoken cl100k (the harness' unit, not the billing unit):",
        f"  answer model input            {m['answer_input_tokens']:>12,}  ({m['answer_input_source']})",
        f"  answer model visible output   {m['answer_visible_output_tokens']:>12,}  ({m['answer_visible_output_fields']})",
        f"  judge input                   {m['judge_input_tokens']:>12,}",
        f"  judge visible output          {m['judge_visible_output_tokens']:>12,}",
        "  thinking tokens                     not in a result file (billed as output)",
    ]
    if "measured_api" in out:
        lines += ["", "measured, the API's own counts (usage files):"]
        for model, item in out["measured_api"].items():
            lines.append(f"  {model} ({item['role']}): {item['calls']:,} calls, prompt {item['prompt_tokens']:,}, output "
                         f"{item['output_tokens']:,}, thinking {item['thinking_tokens']:,}, cached {item['cached_tokens']:,}"
                         + (f", {item['unreported']} responses without usage" if item["unreported"] else "")
                         + (f", {item['calls_over_long_context_threshold']} calls over the long-context threshold"
                            if item["calls_over_long_context_threshold"] else "")
                         + (f" -> ${item['cost_usd']:,.2f}" if "cost_usd" in item else ""))
        if "for_the_next_estimate" in out:
            f = out["for_the_next_estimate"]
            lines.append(f"  for the next estimate: --token-ratio {f['billed_input_tokens_per_cl100k_token']} (billed input per "
                         f"cl100k token), --thinking-per-answer {f['thinking_tokens_per_answer_call']} (measured mean); "
                         f"{f['answer_calls_per_question']} answer calls per question")
        if "cost_usd" in out:
            lines += ["", f"cost, measured: ${out['cost_usd']['measured']:,.2f} ({out['cost_usd']['source']})"]
        else:
            lines += ["", "no dollar figure: give --answer-price and --judge-price"]
        return "\n".join(lines)
    cost = out.get("cost_usd")
    if not cost:
        lines += ["", "no dollar figure: give --answer-price IN OUT and --judge-price IN OUT (USD per million tokens)"]
        return "\n".join(lines)
    lines += ["", f"prices given (USD per 1M tokens, input / output): answer {args.answer_price[0]} / {args.answer_price[1]}, "
                  f"judge {args.judge_price[0]} / {args.judge_price[1]}"
              + (f"; long context over {args.long_context[0]:,.0f} input tokens: {args.long_context[1]} / {args.long_context[2]}"
                 if args.long_context else "; no long-context tier given")]

    def line(name: str, c: dict, note: str) -> str:
        return (f"  {name:<6} ${c['total_usd']:>10,.2f}  = answer ${c['answer_usd']:,.2f} (of which input "
                f"${c['answer_input_usd']:,.2f}) + judge ${c['judge_usd']:,.2f}  {note}"
                + ("  [long-context tier]" if c["long_context_tier"] else ""))

    lines.append(line("floor", cost["floor"], "measured: visible output only, ratio 1"))
    if "low" in cost:
        r, t = args.token_ratio, args.thinking_per_answer
        lines.append(line("low", cost["low"], f"assumed: ratio {r[0]}, {t[0]:,.0f} thinking tokens per answer"))
        lines.append(line("high", cost["high"], f"assumed: ratio {r[1]}, {t[1]:,.0f} thinking tokens per answer"))
    else:
        lines.append("  no range: the billed-token ratio and the thinking tokens are in no file. State both assumptions "
                     "(--token-ratio LOW HIGH --thinking-per-answer LOW HIGH), or measure them with a usage file.")
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--results", required=True, type=Path, help="result file of a finished answer run (merged or one shard)")
    ap.add_argument("--usage", type=Path, nargs="+", help="usage JSONL files of that run (ICM_AMB_USAGE)")
    ap.add_argument("--answer-input-tokens", type=float,
                    help="answer-model input of a PLANNED run, cl100k tokens (dry_run_tokens.py: `answer model input`)")
    ap.add_argument("--max-call-input", type=float,
                    help="largest answer prompt of the planned run, cl100k tokens (dry_run_tokens.py: `max ... per call`); "
                         "compared with the long-context threshold. Default: the mean prompt of the planned run")
    ap.add_argument("--questions", type=int, help="questions of the planned run (default: as many as --results)")
    ap.add_argument("--answer-price", type=float, nargs=2, metavar=("IN", "OUT"), help="USD per 1M tokens")
    ap.add_argument("--judge-price", type=float, nargs=2, metavar=("IN", "OUT"), help="USD per 1M tokens")
    ap.add_argument("--long-context", type=float, nargs=3, metavar=("THRESHOLD", "IN", "OUT"),
                    help="answer model: input tokens above which the long-context prices apply, and those prices")
    ap.add_argument("--token-ratio", type=float, nargs=2, metavar=("LOW", "HIGH"),
                    help="assumed billed tokens per cl100k token (no usage file)")
    ap.add_argument("--thinking-per-answer", type=float, nargs=2, metavar=("LOW", "HIGH"),
                    help="assumed thinking tokens of the answer model per question (no usage file)")
    ap.add_argument("--answer-overhead", type=int, default=ANSWER_PROMPT_OVERHEAD,
                    help=f"cl100k tokens of the answer prompt around the context (default {ANSWER_PROMPT_OVERHEAD}, LoCoMo)")
    ap.add_argument("--judge-overhead", type=int, default=JUDGE_PROMPT_OVERHEAD,
                    help=f"cl100k tokens of the judge prompt around question, gold and answer (default {JUDGE_PROMPT_OVERHEAD}, LoCoMo)")
    ap.add_argument("--json", type=Path, help="also write the figures as JSON")
    args = ap.parse_args(argv)
    out = estimate(args)
    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(json.dumps(out, indent=1))
    print(render(out, args))
    return 0


if __name__ == "__main__":
    sys.exit(main())
