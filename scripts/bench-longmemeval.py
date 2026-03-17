#!/usr/bin/env python3
"""
LongMemEval Benchmark for ICM
==============================
Standard benchmark (ICLR 2025) — 500 questions across 5 memory abilities:
  1. Single-session extraction
  2. Multi-session reasoning
  3. Knowledge updates
  4. Temporal reasoning
  5. Abstention (knowing when you don't know)

Typical scores:
  - Mastra (gpt-5-mini): 94.9%
  - Oracle + GPT-4o:      87-92%
  - Zep:                   71.2%
  - Naive RAG:             52%
  - Best guess:            18.8%

Usage:
  pip install datasets
  python3 scripts/bench-longmemeval.py [--icm /path/to/icm] [--variant oracle]

Requires: ICM binary with embeddings support.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.request
import urllib.error
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

try:
    from huggingface_hub import snapshot_download
except ImportError:
    print("Install huggingface_hub: pip install huggingface_hub")
    sys.exit(1)


def run_icm(icm_bin: str, db: str, args: list[str], timeout: int = 60) -> str:
    """Run ICM CLI command and return stdout."""
    cmd = [icm_bin, "--db", db] + args
    try:
        result = subprocess.run(
            cmd, capture_output=True, text=True, timeout=timeout
        )
        return result.stdout.strip()
    except subprocess.TimeoutExpired:
        return ""
    except Exception as e:
        return f"ERROR: {e}"


def extract_session_content(session: list[dict]) -> str:
    """Extract conversation content from a session's turns."""
    lines = []
    for turn in session:
        role = turn.get("role", "unknown")
        content = turn.get("content", "")
        if content.strip():
            lines.append(f"{role}: {content.strip()}")
    return "\n".join(lines)


def compute_f1(prediction: str, reference: str) -> float:
    """Token-level F1 score between prediction and reference."""
    pred_tokens = set(prediction.lower().split())
    ref_tokens = set(reference.lower().split())

    if not ref_tokens:
        return 1.0 if not pred_tokens else 0.0
    if not pred_tokens:
        return 0.0

    common = pred_tokens & ref_tokens
    if not common:
        return 0.0

    precision = len(common) / len(pred_tokens)
    recall = len(common) / len(ref_tokens)
    return 2 * precision * recall / (precision + recall)


def keyword_hit(response: str, answer: str, threshold: float = 0.3) -> bool:
    """Check if enough answer keywords appear in the response."""
    # Extract significant words (>3 chars, not stopwords)
    stopwords = {
        "the", "and", "that", "this", "with", "from", "have", "has",
        "was", "were", "been", "being", "would", "could", "should",
        "they", "them", "their", "there", "then", "than", "what",
        "when", "where", "which", "who", "whom", "will", "about",
        "into", "over", "after", "before", "between", "under",
        "pour", "dans", "avec", "mais", "que", "qui", "les", "des",
        "une", "est", "sont", "par", "sur", "pas", "plus",
    }
    answer_words = {
        w.lower() for w in re.findall(r'\w+', answer)
        if len(w) > 3 and w.lower() not in stopwords
    }
    if not answer_words:
        return True

    response_lower = response.lower()
    hits = sum(1 for w in answer_words if w in response_lower)
    return (hits / len(answer_words)) >= threshold


def claude_generate(prompt: str, model: str = "claude-sonnet-4-20250514", api_key: str = "") -> str:
    """Call Claude via `claude -p` (pipe mode). Handles auth automatically."""
    env = os.environ.copy()
    # Prevent nested session detection
    env.pop("CLAUDECODE", None)
    env.pop("CLAUDE_CODE_SESSION", None)
    cmd = ["claude", "-p", prompt]
    if model:
        cmd.extend(["--model", model])
    try:
        result = subprocess.run(
            cmd, capture_output=True, text=True, timeout=120, env=env
        )
        if result.returncode != 0:
            return f"ERROR: {result.stderr[:200]}"
        return result.stdout.strip()
    except subprocess.TimeoutExpired:
        return "ERROR: timeout"
    except Exception as e:
        return f"ERROR: {e}"


def ollama_generate(prompt: str, model: str = "phi4:14b", url: str = "http://localhost:11434") -> str:
    """Call Ollama API to generate a response."""
    payload = json.dumps({
        "model": model,
        "prompt": prompt,
        "stream": False,
        "options": {"temperature": 0.0, "num_predict": 512},
    }).encode()
    req = urllib.request.Request(
        f"{url}/api/generate",
        data=payload,
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            data = json.loads(resp.read())
            return data.get("response", "").strip()
    except Exception as e:
        return f"ERROR: {e}"


def llm_call(prompt: str, backend: str, model: str, ollama_url: str, api_key: str) -> str:
    """Route LLM call to the right backend."""
    if backend == "claude":
        return claude_generate(prompt, model=model)
    else:
        return ollama_generate(prompt, model=model, url=ollama_url)


def llm_answer(question: str, context: str, backend: str, model: str, ollama_url: str, api_key: str) -> str:
    """Use LLM to answer a question given retrieved context."""
    prompt = f"""You are answering questions based on memory context retrieved from past conversations.

Context from memory:
{context[:3000]}

Question: {question}

Answer the question concisely based ONLY on the context above. If the context doesn't contain enough information, say "I don't know".
Answer:"""
    return llm_call(prompt, backend, model, ollama_url, api_key)


def llm_judge(question: str, gold_answer: str, predicted: str, backend: str, model: str, ollama_url: str, api_key: str) -> dict:
    """Use LLM as judge to score predicted answer vs gold answer."""
    prompt = f"""You are a strict judge evaluating whether a predicted answer is correct.

Question: {question}
Gold answer: {gold_answer}
Predicted answer: {predicted}

Is the predicted answer semantically correct? It doesn't need to match exactly, but must convey the same key information.

Reply with ONLY a JSON object: {{"correct": true/false, "reason": "brief explanation"}}"""
    raw = llm_call(prompt, backend, model, ollama_url, api_key)
    # Parse JSON from response
    try:
        match = re.search(r'\{[^}]+\}', raw)
        if match:
            return json.loads(match.group())
    except (json.JSONDecodeError, AttributeError):
        pass
    # Fallback: check if "true" appears
    correct = "true" in raw.lower() and "false" not in raw.lower()
    return {"correct": correct, "reason": raw[:100]}


def main():
    parser = argparse.ArgumentParser(description="LongMemEval benchmark for ICM")
    parser.add_argument("--icm", default="./target/release/icm", help="Path to ICM binary")
    parser.add_argument("--variant", default="oracle", choices=["oracle", "s", "m"],
                        help="Dataset variant: oracle (evidence only), s (small), m (medium)")
    parser.add_argument("--limit", type=int, default=0, help="Limit number of questions (0=all)")
    parser.add_argument("--db", default="", help="DB path (default: temp)")
    parser.add_argument("--skip-seed", action="store_true", help="Skip seeding (reuse existing DB)")
    parser.add_argument("--verbose", "-v", action="store_true", help="Show details per question")
    parser.add_argument("--output", "-o", default="", help="Save results JSON to file")
    parser.add_argument("--judge", default="none", choices=["none", "ollama", "claude"],
                        help="LLM judge mode: none (keyword only), ollama (local), claude (API)")
    parser.add_argument("--judge-model", default="", help="Judge model (default: phi4:14b for ollama, claude-sonnet-4-20250514 for claude)")
    parser.add_argument("--ollama-url", default="http://localhost:11434", help="Ollama API URL")
    parser.add_argument("--api-key", default="", help="Anthropic API key (or set ANTHROPIC_API_KEY env)")
    parser.add_argument("--workers", "-w", type=int, default=1, help="Parallel workers for judge (default: 1)")
    args = parser.parse_args()

    icm = args.icm
    if not Path(icm).exists():
        print(f"ICM binary not found: {icm}")
        sys.exit(1)

    # Setup DB
    if args.db:
        db = args.db
    else:
        tmp = tempfile.mkdtemp(prefix="icm-longmemeval-")
        db = os.path.join(tmp, "bench.db")
    print(f"DB: {db}")

    # Load dataset — download from HuggingFace and read raw JSON
    print(f"Loading LongMemEval ({args.variant})...")
    variant_map = {"oracle": "longmemeval_oracle", "s": "longmemeval_s", "m": "longmemeval_m"}
    filename = variant_map[args.variant]
    repo_path = snapshot_download(repo_id="xiaowu0162/longmemeval", repo_type="dataset")
    data_file = os.path.join(repo_path, filename)
    with open(data_file) as f:
        instances = json.load(f)
    if args.limit > 0:
        instances = instances[:args.limit]

    print(f"Loaded {len(instances)} instances")

    # --- Phase 1: Seed memories ---
    if not args.skip_seed:
        print("\n=== Phase 1: Seeding memories ===")
        seed_start = time.time()
        total_sessions = 0
        total_turns = 0

        for i, inst in enumerate(instances):
            # Use haystack_sessions (list of session conversations)
            sessions = inst.get("haystack_sessions", [])
            session_ids = inst.get("haystack_session_ids", [])
            dates = inst.get("haystack_dates", [])

            for j, session in enumerate(sessions):
                session_id = session_ids[j] if j < len(session_ids) else f"s{j}"
                date = dates[j] if j < len(dates) else ""
                content = extract_session_content(session)

                if not content.strip():
                    continue

                # Truncate very long sessions to avoid CLI arg limits
                if len(content) > 4000:
                    content = content[:4000] + "..."

                topic = f"longmemeval-{session_id}"
                keywords = f"session,{session_id}"
                if date:
                    content = f"[{date}] {content}"

                run_icm(icm, db, [
                    "store", "-t", topic, "-c", content,
                    "-i", "medium", "-k", keywords
                ])
                total_sessions += 1
                total_turns += len(session)

            if (i + 1) % 50 == 0:
                print(f"  Seeded {i+1}/{len(instances)} instances ({total_sessions} sessions)...")

        seed_time = time.time() - seed_start
        print(f"  Seeded {total_sessions} sessions ({total_turns} turns) in {seed_time:.1f}s")

        # Embed
        print("\n=== Phase 1b: Embedding ===")
        embed_start = time.time()
        out = run_icm(icm, db, ["embed", "--force"], timeout=600)
        embed_time = time.time() - embed_start
        print(f"  {out}")
        print(f"  Embedded in {embed_time:.1f}s")
    else:
        print("Skipping seed (--skip-seed)")
        seed_time = 0
        embed_time = 0

    # --- Phase 2: Query and evaluate ---
    use_judge = args.judge != "none"
    judge_backend = args.judge
    api_key = args.api_key or os.environ.get("ANTHROPIC_API_KEY", "")

    # Default models per backend
    if not args.judge_model:
        if judge_backend == "claude":
            args.judge_model = "claude-sonnet-4-20250514"
        else:
            args.judge_model = "phi4:14b"

    scoring_mode = f"LLM judge ({judge_backend}/{args.judge_model})" if use_judge else "keyword retrieval"
    print(f"\n=== Phase 2: Evaluating {len(instances)} questions [{scoring_mode}] ===\n")

    if use_judge and judge_backend == "ollama":
        # Test Ollama connectivity
        try:
            req = urllib.request.Request(f"{args.ollama_url}/api/tags")
            with urllib.request.urlopen(req, timeout=5) as resp:
                models = json.loads(resp.read())
                available = [m["name"] for m in models.get("models", [])]
                if args.judge_model not in available:
                    print(f"  WARNING: {args.judge_model} not in Ollama models: {available[:5]}")
                else:
                    print(f"  Ollama OK — using {args.judge_model}")
        except Exception as e:
            print(f"  ERROR: Cannot reach Ollama at {args.ollama_url}: {e}")
            sys.exit(1)

    if use_judge and judge_backend == "claude":
        # Test claude -p
        test = claude_generate("Reply with just OK", model=args.judge_model)
        if test.startswith("ERROR"):
            print(f"  ERROR: claude -p test failed: {test}")
            sys.exit(1)
        print(f"  claude -p OK — using {args.judge_model}")

    results_by_type = defaultdict(lambda: {"total": 0, "hits": 0, "f1_sum": 0.0, "judge_correct": 0})
    all_results = [None] * len(instances)
    eval_start = time.time()
    completed_count = 0
    print_lock = __import__("threading").Lock()

    def evaluate_one(idx_inst):
        i, inst = idx_inst
        question = inst.get("question", "")
        answer = inst.get("answer_text", "")
        q_type = inst.get("question_type", "unknown")

        # Query ICM
        response = run_icm(icm, db, ["recall", question, "--limit", "5"])

        # Keyword score (always computed)
        f1 = compute_f1(response, answer)
        hit = keyword_hit(response, answer, threshold=0.3)

        # LLM judge score
        judge_correct = False
        judge_answer = ""
        judge_reason = ""
        if use_judge:
            judge_answer = llm_answer(question, response, judge_backend, args.judge_model, args.ollama_url, api_key)
            verdict = llm_judge(question, answer, judge_answer, judge_backend, args.judge_model, args.ollama_url, api_key)
            judge_correct = verdict.get("correct", False)
            judge_reason = verdict.get("reason", "")

        result_entry = {
            "id": i,
            "type": q_type,
            "question": question,
            "answer": answer,
            "response_preview": response[:200] if response else "",
            "f1": round(f1, 3),
            "hit": hit,
        }
        if use_judge:
            result_entry["judge_answer"] = judge_answer[:200]
            result_entry["judge_correct"] = judge_correct
            result_entry["judge_reason"] = judge_reason

        return i, q_type, f1, hit, judge_correct, result_entry

    num_workers = args.workers if use_judge else 1
    if num_workers > 1:
        print(f"  Using {num_workers} parallel workers\n")

    with ThreadPoolExecutor(max_workers=num_workers) as executor:
        futures = {executor.submit(evaluate_one, (i, inst)): i for i, inst in enumerate(instances)}
        for future in as_completed(futures):
            i, q_type, f1, hit, judge_correct, result_entry = future.result()
            all_results[i] = result_entry

            results_by_type[q_type]["total"] += 1
            results_by_type[q_type]["f1_sum"] += f1
            if hit:
                results_by_type[q_type]["hits"] += 1
            if judge_correct:
                results_by_type[q_type]["judge_correct"] += 1

            completed_count += 1

            with print_lock:
                if args.verbose:
                    status = "HIT" if hit else "MISS"
                    judge_str = f" JUDGE={'OK' if judge_correct else 'FAIL'}" if use_judge else ""
                    print(f"  [{completed_count:3d}] {status}{judge_str} F1={f1:.2f} [{q_type}] {result_entry['question'][:60]}")

                if completed_count % 25 == 0:
                    elapsed = time.time() - eval_start
                    rate = completed_count / elapsed
                    eta = (len(instances) - completed_count) / rate if rate > 0 else 0
                    print(f"  Progress: {completed_count}/{len(instances)}... ({rate:.1f} q/s, ETA {eta:.0f}s)")

    eval_time = time.time() - eval_start

    # --- Phase 3: Results ---
    print("\n" + "=" * 80)
    print("  LongMemEval Results — ICM")
    print("=" * 80)

    total_hits = 0
    total_q = 0
    total_f1 = 0.0
    total_judge = 0

    if use_judge:
        print(f"\n  {'Category':<30s}  {'Hits':>5s}  {'Judge':>5s}  {'Total':>5s}  {'Ret%':>5s}  {'Judge%':>6s}  {'F1':>5s}")
        print(f"  {'-'*30}  {'-'*5}  {'-'*5}  {'-'*5}  {'-'*5}  {'-'*6}  {'-'*5}")
    else:
        print(f"\n  {'Category':<35s}  {'Hits':>6s}  {'Total':>6s}  {'Acc%':>6s}  {'Avg F1':>6s}")
        print(f"  {'-'*35}  {'-'*6}  {'-'*6}  {'-'*6}  {'-'*6}")

    for q_type in sorted(results_by_type.keys()):
        r = results_by_type[q_type]
        acc = r["hits"] / r["total"] * 100 if r["total"] > 0 else 0
        avg_f1 = r["f1_sum"] / r["total"] if r["total"] > 0 else 0
        judge_acc = r["judge_correct"] / r["total"] * 100 if r["total"] > 0 else 0

        if use_judge:
            print(f"  {q_type:<30s}  {r['hits']:>5d}  {r['judge_correct']:>5d}  {r['total']:>5d}  {acc:>4.1f}%  {judge_acc:>5.1f}%  {avg_f1:>4.3f}")
        else:
            print(f"  {q_type:<35s}  {r['hits']:>6d}  {r['total']:>6d}  {acc:>5.1f}%  {avg_f1:>5.3f}")

        total_hits += r["hits"]
        total_q += r["total"]
        total_f1 += r["f1_sum"]
        total_judge += r["judge_correct"]

    overall_acc = total_hits / total_q * 100 if total_q > 0 else 0
    overall_f1 = total_f1 / total_q if total_q > 0 else 0
    overall_judge = total_judge / total_q * 100 if total_q > 0 else 0

    if use_judge:
        print(f"  {'-'*30}  {'-'*5}  {'-'*5}  {'-'*5}  {'-'*5}  {'-'*6}  {'-'*5}")
        print(f"  {'OVERALL':<30s}  {total_hits:>5d}  {total_judge:>5d}  {total_q:>5d}  {overall_acc:>4.1f}%  {overall_judge:>5.1f}%  {overall_f1:>4.3f}")
    else:
        print(f"  {'-'*35}  {'-'*6}  {'-'*6}  {'-'*6}  {'-'*6}")
        print(f"  {'OVERALL':<35s}  {total_hits:>6d}  {total_q:>6d}  {overall_acc:>5.1f}%  {overall_f1:>5.3f}")

    print(f"\n  Timing:")
    print(f"    Seed:     {seed_time:.1f}s")
    print(f"    Embed:    {embed_time:.1f}s")
    print(f"    Evaluate: {eval_time:.1f}s ({eval_time/max(total_q,1):.2f}s/query)")
    print(f"    Total:    {seed_time + embed_time + eval_time:.1f}s")
    print(f"\n  DB: {db}")

    # Reference scores
    print(f"\n  Reference scores (LongMemEval leaderboard):")
    print(f"    Mastra (gpt-5-mini):  94.9%")
    print(f"    Oracle + GPT-4o:      87-92%")
    print(f"    Zep:                  71.2%")
    print(f"    Naive RAG:            52%")
    print(f"    Best guess:           18.8%")
    print("=" * 70)

    # Save results
    if args.output:
        output_data = {
            "benchmark": "LongMemEval",
            "variant": args.variant,
            "scoring": "llm_judge" if use_judge else "keyword_retrieval",
            "judge_model": args.judge_model if use_judge else None,
            "total_questions": total_q,
            "overall_retrieval_acc": round(overall_acc, 2),
            "overall_judge_acc": round(overall_judge, 2) if use_judge else None,
            "overall_f1": round(overall_f1, 4),
            "by_type": {
                k: {
                    "hits": v["hits"],
                    "judge_correct": v["judge_correct"],
                    "total": v["total"],
                    "retrieval_acc": round(v["hits"] / v["total"] * 100, 2) if v["total"] > 0 else 0,
                    "judge_acc": round(v["judge_correct"] / v["total"] * 100, 2) if v["total"] > 0 and use_judge else None,
                    "avg_f1": round(v["f1_sum"] / v["total"], 4) if v["total"] > 0 else 0,
                }
                for k, v in results_by_type.items()
            },
            "timing": {
                "seed_s": round(seed_time, 1),
                "embed_s": round(embed_time, 1),
                "eval_s": round(eval_time, 1),
            },
            "results": all_results,
        }
        with open(args.output, "w") as f:
            json.dump(output_data, f, indent=2)
        print(f"\nResults saved to: {args.output}")


if __name__ == "__main__":
    main()
