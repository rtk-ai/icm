# Demonstrations and micro-benchmarks

Small demonstrations and storage micro-benchmarks. They show the mechanisms; they are not evidence of how ICM ranks against other systems. For that, see [Benchmark comparison](../README.md#benchmark-comparison).

### Storage micro-benchmark

```
ICM Benchmark (1000 memories, 384d embeddings)
──────────────────────────────────────────────────────────
Store (no embeddings)      1000 ops      34.2 ms      34.2 µs/op
Store (with embeddings)    1000 ops      51.6 ms      51.6 µs/op
FTS5 search                 100 ops       4.7 ms      46.6 µs/op
Vector search (KNN)         100 ops      59.0 ms     590.0 µs/op
Hybrid search               100 ops      95.1 ms     951.1 µs/op
Decay (batch)                 1 ops       5.8 ms       5.8 ms/op
──────────────────────────────────────────────────────────
```

Apple M1 Pro, in-memory SQLite, 1,000 synthetic memories, single-threaded (`icm bench --count 1000`). This isolates the storage layer. End-to-end recall on a real database, embedding the query included, takes 52 to 65 ms (median, warm server, Apple Silicon) and 141 ms on a 4-vCPU cloud VM.

### Agent efficiency (demonstration)

Three runs, one model, one small project: an illustration, not a benchmark. Multi-session workflow with a real Rust project (12 files, ~550 lines). Sessions 2+ show the biggest gains as ICM recalls instead of re-reading files.

```
ICM Agent Benchmark (10 sessions, model: haiku, 3 runs averaged)
══════════════════════════════════════════════════════════════════
                            Without ICM         With ICM      Delta
Session 2 (recall)
  Turns                             5.7              4.0       -29%
  Context (input)                 99.9k            67.5k       -32%
  Cost                          $0.0298          $0.0249       -17%

Session 3 (recall)
  Turns                             3.3              2.0       -40%
  Context (input)                 74.7k            41.6k       -44%
  Cost                          $0.0249          $0.0194       -22%
══════════════════════════════════════════════════════════════════
```

`icm bench-agent --sessions 10 --model haiku`

### Knowledge retention (demonstration)

Five runs, ten questions, scored by keyword matching. Agent recalls specific facts from a dense technical document across sessions. Session 1 reads and memorizes; sessions 2+ answer 10 factual questions **without** the source text.

```
ICM Recall Benchmark (10 questions, model: haiku, 5 runs averaged)
══════════════════════════════════════════════════════════════════════
                                               No ICM     With ICM
──────────────────────────────────────────────────────────────────────
Average score                                      5%          68%
Questions passed                                 0/10         5/10
══════════════════════════════════════════════════════════════════════
```

`icm bench-recall --model haiku`

### Local LLMs (ollama)

Same test with local models — pure context injection, no tool use needed.

```
Model               Params   No ICM   With ICM     Delta
─────────────────────────────────────────────────────────
qwen2.5:14b           14B       4%       97%       +93%
mistral:7b             7B       4%       93%       +89%
llama3.1:8b            8B       4%       93%       +89%
qwen2.5:7b             7B       4%       90%       +86%
phi4:14b              14B       6%       79%       +73%
llama3.2:3b            3B       0%       76%       +76%
gemma2:9b              9B       4%       76%       +72%
qwen2.5:3b             3B       2%       58%       +56%
─────────────────────────────────────────────────────────
```

`scripts/bench-ollama.sh qwen2.5:14b`

### Test protocol

All benchmarks use **real API calls** — no mocks, no simulated responses, no cached answers.

- **Agent benchmark**: Creates a real Rust project in a tempdir. Runs N sessions with `claude -p --output-format json`. Without ICM: empty MCP config. With ICM: real MCP server + auto-extraction + context injection.
- **Knowledge retention**: Uses a fictional technical document (the "Meridian Protocol"). Scores answers by keyword matching against expected facts. 120s timeout per invocation.
- **Isolation**: Each run uses its own tempdir and fresh SQLite DB. No session persistence.

### Multi-agent unified memory

All supported tools share the same SQLite database. A memory stored by Claude is instantly available to Gemini, Codex, Copilot, Cursor, and every other tool.

A demonstration with 10 facts seeded through ICM, then asked back from five CLI agents:

| Agent | Facts recalled | Latency |
|-------|:--------------:|:-------:|
| Claude Code | 10 / 10 | ~15 s |
| Gemini CLI | 10 / 10 | ~33 s |
| Copilot CLI | 10 / 10 | ~10 s |
| Cursor Agent | 10 / 10 | ~16 s |
| Aider | 10 / 10 | ~5 s |

Ten facts and one run per agent: this shows the shared database works across tools, not how well recall scales.
