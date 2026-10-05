#!/usr/bin/env python3
"""What a paid run would send to the answer model, measured without calling any model.

    python dry_run_tokens.py --dataset locomo --split locomo10 --memory full-context
    python dry_run_tokens.py --dataset locomo --split locomo10 --memory bm25
    ICM_AMB_BIN=target/release/icm ICM_AMB_ENGINE=v2 ICM_AMB_K=10 \\
        python dry_run_tokens.py --dataset locomo --split locomo10 --memory icm

Runs the real harness (its runner, its dataset loader, its `rag` mode, its prompt
builder) with the provider asked for, and replaces the two models by stand-ins that
only count: the answer stand-in measures the prompt it is handed and returns a fixed
string, the judge stand-in measures its prompt and answers "wrong". The provider is
real: `full-context` and `bm25` need nothing else, `icm` ingests for real with the
binary in ICM_AMB_BIN (minutes of CPU, no model cost).

Printed, and written as JSON with --json: the number of questions, the context
tokens per question as the harness counts them (tiktoken cl100k: the
`avg_context_tokens` a real run would report), and the input tokens of the answer
prompts (context + instructions + question). Tokens are cl100k tokens, the harness'
unit, not the provider's billing unit: Gemini counts differently, so read the
figures as an order of magnitude for the bill. The judge's input is not
representative here (it would judge the stand-in answer): size it from a real run.

`--record-documents FILE` also writes, per isolation unit, the digest of every
document the provider was handed at ingestion: two providers were given the same
documents if and only if their files are equal.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import statistics
import sys
import tempfile
import threading
from pathlib import Path

import run_amb


class _Counter:
    """Stand-in for a harness LLM: counts the prompts, never calls anything."""

    def __init__(self, role: str):
        self.model_id = f"dry-run:{role}"
        self.role = role
        self.tokens: list[int] = []
        self._lock = threading.Lock()

    def generate(self, prompt: str, schema) -> dict:
        from memory_bench.utils import count_tokens

        n = count_tokens(prompt)
        with self._lock:
            self.tokens.append(n)
        if self.role == "judge":
            return {"correct": False, "reason": "dry run: no judge was called"}
        return {key: ("a" if key == "choice" else "dry run: no answer model was called") for key in schema.required}

    def tool_loop(self, *args, **kwargs):
        raise RuntimeError("dry run: tool loops are not simulated")


def _digest(doc) -> str:
    payload = json.dumps([doc.id, doc.user_id, doc.timestamp, doc.context, doc.content], ensure_ascii=False)
    return hashlib.sha256(payload.encode()).hexdigest()


def dry_run(dataset_name: str, split: str, memory_name: str, output_dir: Path, query_limit: int | None = None,
            category: str | None = None) -> dict:
    """Run the harness once with counting stand-ins. Returns the measurements."""
    run_amb.bootstrap()
    import memory_bench.llm as llm_pkg
    from memory_bench.dataset import get_dataset
    from memory_bench.memory import get_memory_provider
    from memory_bench.modes.rag import RAGMode
    from memory_bench.runner import EvalRunner

    from recall_only import forbid_llm

    forbid_llm()  # every real LLM class of the harness now raises instead of calling out
    answer, judge = _Counter("answer"), _Counter("judge")
    # The runner builds its judge from these factories: no real client is ever constructed.
    llm_pkg.get_answer_llm = lambda: answer
    llm_pkg.get_judge_llm = lambda: judge
    llm_pkg.get_llm = lambda name="gemini": judge

    memory = get_memory_provider(memory_name)
    ingested: dict[str, list[str]] = {}
    original_ingest = memory.ingest

    def ingest(documents):
        for doc in documents:
            ingested.setdefault(str(doc.user_id), []).append(_digest(doc))
        return original_ingest(documents)

    memory.ingest = ingest
    dataset = get_dataset(dataset_name)
    summary = EvalRunner(output_dir=output_dir).run(
        dataset=dataset, split=split, memory=memory, mode=RAGMode(llm=answer), category=category,
        query_limit=query_limit, run_name=f"dry-run-{memory_name}", description="DRY RUN: no model was called")
    context = [r.context_tokens for r in summary.results]
    prompts = answer.tokens

    def spread(values: list[int]) -> dict:
        if not values:
            return {"n": 0}
        ordered = sorted(values)
        return {"n": len(values), "total": sum(values), "mean": round(statistics.mean(values), 1), "min": ordered[0],
                "median": ordered[len(ordered) // 2], "p95": ordered[min(len(ordered) - 1, int(0.95 * len(ordered)))],
                "max": ordered[-1]}

    return {
        "dataset": dataset_name, "split": split, "memory": memory_name, "provider_description": memory.description,
        "questions": summary.total_queries, "ingested_docs": summary.ingested_docs,
        "empty_contexts": sum(1 for r in summary.results if not r.context),
        "context_tokens": spread(context), "answer_prompt_tokens": spread(prompts),
        "answer_calls": len(prompts), "judge_calls": len(judge.tokens),
        "tokenizer": "tiktoken cl100k_base (the harness' unit; not the model's billing unit)",
        "documents_by_unit": {unit: sorted(digests) for unit, digests in sorted(ingested.items())},
        "result_file": str(Path(output_dir) / dataset_name / f"dry-run-{memory_name}" / "rag" / f"{split}.json"),
    }


def main(argv: list[str] | None = None) -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--dataset", required=True)
    ap.add_argument("--split", required=True)
    ap.add_argument("--memory", required=True, help="full-context, bm25, icm, ...")
    ap.add_argument("--query-limit", type=int)
    ap.add_argument("--category")
    ap.add_argument("--output-dir", type=Path, help="keep the harness result file (contexts included) here")
    ap.add_argument("--record-documents", type=Path, help="write the digests of the ingested documents, per unit")
    ap.add_argument("--json", type=Path, help="write the measurements as JSON")
    args = ap.parse_args(argv)
    for var in ("GEMINI_API_KEY", "GOOGLE_API_KEY"):
        os.environ.pop(var, None)  # nothing here may reach a model, even by accident

    with tempfile.TemporaryDirectory() as scratch:
        report = dry_run(args.dataset, args.split, args.memory, args.output_dir or Path(scratch),
                         query_limit=args.query_limit, category=args.category)
        if not args.output_dir:
            report["result_file"] = None
    documents = report.pop("documents_by_unit")
    if args.record_documents:
        args.record_documents.parent.mkdir(parents=True, exist_ok=True)
        args.record_documents.write_text(json.dumps(documents, indent=1))
    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(json.dumps(report, indent=1))
    c, p = report["context_tokens"], report["answer_prompt_tokens"]
    print(f"\n{report['dataset']}/{report['split']} with --memory {report['memory']}: {report['questions']} questions, "
          f"{report['ingested_docs']} documents ingested, {report['empty_contexts']} empty contexts, no model called")
    if c["n"]:
        print(f"context tokens per question (harness count): mean {c['mean']}, min {c['min']}, median {c['median']}, "
              f"p95 {c['p95']}, max {c['max']}")
        print(f"answer model input: {p['total']} tokens over {p['n']} calls (mean {p['mean']}, max {p['max']} per call)")
    print(report["tokenizer"])


if __name__ == "__main__":
    main()
    sys.exit(0)
