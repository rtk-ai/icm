# icm

[![CI](https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg)](https://github.com/rtk-ai/icm/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

Permanent memory for AI agents. Single binary, zero dependencies, MCP native.

## What is ICM?

ICM gives your AI agent a real memory — not a note-taking tool, not a context manager, a **memory**.

```
                         ICM (Infinite Context Memory)
              ┌──────────────────────┬─────────────────────────┐
              │     MEMORY (Topic)   │     MEMOIR (Knowledge)  │
              │                      │                         │
              │  Episodic, temporal  │  Permanent, structured  │
              │                      │                         │
              │  ┌───┐ ┌───┐ ┌───┐  │    ┌───┐               │
              │  │ m │ │ m │ │ m │  │    │ C │──depends_on──┐ │
              │  └─┬─┘ └─┬─┘ └─┬─┘  │    └───┘              │ │
              │    │decay │     │    │      │ refines      ┌─▼─┐│
              │    ▼      ▼     ▼    │    ┌─▼─┐            │ C ││
              │  weight decreases    │    │ C │──part_of──>└───┘│
              │  over time unless    │    └───┘                 │
              │  accessed/critical   │  Concepts + Relations    │
              ├──────────────────────┴─────────────────────────┤
              │              SQLite + FTS5 + sqlite-vec         │
              │          Hybrid search: BM25 (30%) + cosine (70%)│
              └────────────────────────────────────────────────┘
```

Two memory models:

- **Topics** — store/recall with temporal decay by importance. Critical memories never fade, low-importance ones decay naturally. Auto-decay triggers on recall (every 24h). Consolidate when a topic grows too large.
- **Memories** — permanent knowledge graphs. Concepts linked by typed relations (`depends_on`, `contradicts`, `refines`, `part_of`, ...). The agent builds structured knowledge it can traverse and reason over.

## How it works

### Dual memory model

**Episodic memory (Topics)** captures decisions, errors, preferences — things that happen during work. Each memory has a weight that decays over time based on importance level:

| Importance | Decay | Behavior |
|-----------|-------|----------|
| `critical` | none | Never forgotten |
| `high` | slow (0.5x rate) | Fades slowly |
| `medium` | normal | Standard decay |
| `low` | fast (2x rate) | Quickly forgotten |

Decay is applied automatically when you recall memories (if >24h since last decay), so there's no cron job needed.

**Semantic memory (Memories)** captures structured knowledge as a graph. Concepts are permanent — they get refined, never decayed. Relations between concepts form a traversable knowledge graph the agent can reason over.

### Hybrid search

When you have embeddings enabled, ICM uses hybrid search combining:
- **FTS5 BM25** (30% weight) — full-text keyword matching
- **Cosine similarity** (70% weight) — semantic vector search via sqlite-vec

Without embeddings, it falls back to FTS5 then keyword LIKE search.

### Storage

Everything lives in a single SQLite file (`~/Library/Application Support/dev.icm.icm/memories.db` on macOS). No external services, no network dependency.

## Performance

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

In-memory SQLite, Apple M1 Pro, single-threaded, release build. Reproduce on your machine:

```bash
icm bench --count 1000
```

### Agent benchmark

Measures AI agent efficiency with and without ICM across multi-session workflows. The test project is a Rust math library (12 files, ~550 lines) with expression parsing, matrix ops, statistics, and complex numbers.

```
ICM Agent Benchmark (10 sessions, model: haiku, 3 runs averaged)
══════════════════════════════════════════════════════════════════
                            Without ICM         With ICM      Delta
Session 1 (explore)
  Turns                            13.3             13.0        -3%
  Context (input)                127.5k           156.9k       +23%
  Cost                          $0.0536          $0.0519        -3%

Session 2 (recall)
  Turns                             5.7              4.0       -29%
  Context (input)                 99.9k            67.5k       -32%
  Cost                          $0.0298          $0.0249       -17%

Session 3 (recall)
  Turns                             3.3              2.0       -40%
  Context (input)                 74.7k            41.6k       -44%
  Cost                          $0.0249          $0.0194       -22%
  ...
──────────────────────────────────────────────────────────────────
Total (averaged over 3 runs)
  Turns                              48               42       -14%
  Context (input)                752.2k           692.3k        -8%
  Cost                          $0.2686          $0.2564        -5%

Variance across 3 runs:
  Turns delta:   -26% to +7%
  Context delta: -26% to +17%
  Cost delta:    -15% to +10%
══════════════════════════════════════════════════════════════════
```

Session 1 has similar cost (both explore). Sessions 2-3 show the biggest gains: **-29% to -40% turns, -32% to -44% context** as ICM recalls instead of re-reading files. Results vary significantly between runs due to LLM non-determinism.

```bash
icm bench-agent --sessions 10 --model haiku
```

### Knowledge retention benchmark

Measures how well an agent recalls specific facts from a dense technical document across sessions. Session 1 reads and memorizes; sessions 2+ answer 10 factual questions **without** the source text.

```
ICM Recall Benchmark (10 questions, model: haiku, 5 runs averaged)
══════════════════════════════════════════════════════════════════════
Question                                       No ICM     With ICM
──────────────────────────────────────────────────────────────────────
Who proposed the Meridian Protocol,...     0.0/5 (0%)  2.0/5 (67%)
What are the three phases of Meridi...     0.0/6 (0%)  3.0/6 (75%)
What is the maximum cluster size fo...     0.0/3 (0%) 2.8/3 (100%)
What ports does the Meridian gossip...     0.0/4 (0%)   0.0/4 (0%)
What throughput did Meridian achiev...     0.0/3 (0%)  0.4/3 (40%)
What is the Byzantine fault toleran...    1.0/4 (50%) 2.0/4 (100%)
Name the three implementations of M...     0.0/6 (0%) 4.0/6 (100%)
Which companies deployed Meridian i...     0.0/3 (0%)   0.0/3 (0%)
What was Dr. Tanaka's prior work th...     0.0/4 (0%) 4.0/4 (100%)
What is the BLAME threshold and wha...     0.0/5 (0%) 4.0/5 (100%)
──────────────────────────────────────────────────────────────────────
Average score                                      5%          68%
Questions passed                                 0/10         5/10
══════════════════════════════════════════════════════════════════════
```

Without ICM: 0/10 questions passed (agent has no memory of previous session).
With ICM: **5/10 questions passed, 68% average score** (stable across 5 runs) — the agent recalls specific names, numbers, and technical details from a document it read in a previous session. The 3 consistently failing questions (ports, throughput, deployments) show extraction limitations — facts that aren't captured can't be recalled.

```bash
icm bench-recall --model haiku
```

### Local LLM benchmark (ollama)

Same knowledge retention test but with local models via ollama — no cloud API, pure context injection (Layer 0+2). Facts are stored in ICM, then injected into the system prompt for each question.

```
Model               Params   No ICM   With ICM     Delta
─────────────────────────────────────────────────────────
qwen2.5:14b           14B       4%       88%       +84%
mistral:7b             7B       2%       81%       +79%
qwen2.5:7b             7B       2%       79%       +77%
llama3.1:8b            8B       4%       67%       +63%
─────────────────────────────────────────────────────────
Hardware: RTX 4070 16GB, ollama, temperature 0.1
```

Even 7B models go from near-zero to 67-81% recall with ICM context injection. No tool use needed — pure system prompt injection.

```bash
scripts/bench-ollama.sh qwen2.5:14b        # run with specific model
scripts/bench-ollama.sh mistral:7b verbose  # see answers
```

### Test protocol

Both benchmarks use **real Claude API calls** — no mocks, no simulated responses, no cached answers. Every session is a fresh `claude -p` invocation with `--output-format json`.

**Agent benchmark (`bench-agent`):**

1. Creates a real Rust project in a tempdir (12 files, ~550 lines: expression parser, matrix ops, statistics, complex numbers)
2. Runs N sessions sequentially. Session prompts cycle through: explore architecture, ask about specific modules, ask about implementation details
3. **Without ICM**: each session uses `--mcp-config {}` (empty — zero MCP servers). Every session starts from scratch
4. **With ICM**: each session gets a real ICM MCP server (`icm serve`) backed by a fresh SQLite DB in the tempdir. After each session, Layer 0 auto-extraction stores facts from Claude's response. Before sessions 2+, Layer 2 injects recalled context into the prompt
5. Metrics are parsed from Claude's JSON output: `num_turns`, `input_tokens + cache_creation_input_tokens + cache_read_input_tokens`, `total_cost_usd`, `duration_ms`
6. Tempdir is cleaned up automatically on exit

**Knowledge retention benchmark (`bench-recall`):**

1. Uses a 3-page fictional technical document (the "Meridian Protocol" — distributed consensus, specific names/dates/numbers/configs) hardcoded in the binary
2. **Session 1** (both modes): Claude reads the full document with instructions to memorize key facts
3. **Sessions 2-11** (both modes): Claude answers 10 factual questions **without** the source document. Questions require specific recall: "What ports does the gossip protocol use?", "What is the Byzantine fault tolerance formula?"
4. **Without ICM**: Claude has no memory of session 1 — it must guess or say "I don't know"
5. **With ICM**: After session 1, Layer 0 extracts facts from both the document and Claude's response. Before each question, Layer 2 injects relevant recalled facts. Claude also has access to `icm_recall` via MCP
6. Answers are scored by keyword matching against expected answers (e.g., question about ports expects "9471", "UDP", "9472", "TCP"). A question passes if it hits the minimum keyword threshold
7. Each `claude` invocation has a 120s timeout to prevent hung sessions

**Isolation guarantees:**
- Each benchmark run uses its own tempdir and fresh SQLite DB
- Sessions use `--mcp-config` to control exactly which MCP servers are available
- No session persistence (`claude -p` = single prompt mode, no conversation history)
- The ICM binary under test is the same `icm` binary running the benchmark (`std::env::current_exe()`)

## Install

```bash
# Homebrew (macOS / Linux)
brew tap rtk-ai/tap && brew install icm

# Quick install
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# From source
cargo install --path crates/icm-cli
```

## Setup

### Auto-detect and configure all tools

```bash
icm init
```

Detects and configures: **Claude Code**, **Claude Desktop**, **Cursor**, **Windsurf**. Uses the MCP protocol — any tool that supports MCP servers works out of the box.

### Manual setup

```bash
# Claude Code
claude mcp add icm -- icm serve

# Cursor: add to ~/.cursor/mcp.json
# Windsurf: add to ~/.codeium/windsurf/mcp_config.json
# Any MCP client: command = "icm", args = ["serve"]
```

### Configuration

```bash
# Show active config
icm config

# Edit: ~/.config/icm/config.toml (or $ICM_CONFIG)
```

See [config/default.toml](config/default.toml) for all options.

## CLI

```bash
# Store a memory
icm store -t "my-project" -c "Use PostgreSQL for the main DB" -i high

# Recall memories
icm recall "database choice"
icm recall "auth setup" --topic "my-project" --limit 10

# Forget a memory
icm forget <memory-id>

# Consolidate a topic into a single summary
icm consolidate --topic "my-project"

# List topics
icm topics

# Stats
icm stats

# Extract facts from text (rule-based, zero LLM cost)
echo "The parser uses Pratt algorithm" | icm extract -p my-project
icm extract -p my-project --dry-run -t "some text to extract from"

# Recall context for prompt injection
icm recall-context "database choice" --limit 10
```

### Memory (knowledge graphs)

```bash
# Create a memory
icm memory create "system-architecture"

# Add concepts
icm memory add-concept "system-architecture" "auth-service" "Handles JWT tokens and OAuth2 flows"
icm memory add-concept "system-architecture" "api-gateway" "Routes requests, rate limiting"

# Link concepts
icm memory link "system-architecture" "api-gateway" "depends_on" "auth-service"

# Inspect a concept and its neighborhood
icm memory inspect "system-architecture" "auth-service" --depth 2

# Search within a memory
icm memory search "system-architecture" "authentication"

# Search across ALL memories
icm memory search-all "authentication"
```

## MCP Tools (16)

### Memory tools

| Tool | Description |
|------|-------------|
| `icm_memory_store` | Store a memory with topic, importance, keywords |
| `icm_memory_recall` | Search memories by query, optionally filtered by topic |
| `icm_memory_forget` | Delete a memory by ID |
| `icm_memory_consolidate` | Merge all memories of a topic into one summary |
| `icm_memory_list_topics` | List all topics with counts |
| `icm_memory_stats` | Global memory statistics |
| `icm_memory_embed_all` | Backfill embeddings for vector search (requires embeddings feature) |

### Memory tools (knowledge graphs)

| Tool | Description |
|------|-------------|
| `icm_memory_create` | Create a new memory (knowledge container) |
| `icm_memory_list` | List all memories |
| `icm_memory_show` | Show memory details and all concepts |
| `icm_memory_add_concept` | Add a concept to a memory |
| `icm_memory_refine` | Update a concept's definition |
| `icm_memory_search` | Full-text search within a memory |
| `icm_memory_search_all` | Full-text search across all memories |
| `icm_memory_link` | Create typed relation between concepts |
| `icm_memory_inspect` | Inspect a concept and its graph neighborhood (BFS) |

## Relation types

`part_of` · `depends_on` · `related_to` · `contradicts` · `refines` · `alternative_to` · `caused_by` · `instance_of`

## Auto-extraction

ICM extracts memories automatically via three layers — no need to rely on the LLM calling `icm_store` proactively.

```
  Layer 0: Pattern hooks              Layer 1: PreCompact           Layer 2: SessionStart
  (zero LLM cost)                     (cheap LLM, ~500 tok)         (zero LLM cost)
  ┌──────────────────┐                ┌──────────────────┐          ┌──────────────────┐
  │ PostToolUse hook  │                │ PreCompact hook   │          │ SessionStart hook │
  │                   │                │                   │          │                   │
  │ • Bash exit != 0  │                │ Context about to  │          │ Read cwd project  │
  │   → store error   │                │ be compressed →   │          │ → icm recall      │
  │ • git commit      │                │ extract memories  │          │ → inject as       │
  │   → store commit  │                │ before they're    │          │   additionalContext│
  │ • Edit CLAUDE.md  │                │ lost forever      │          │                   │
  │   → store context │                │                   │          │ Agent starts with  │
  │                   │                │ This is the #1    │          │ relevant memories  │
  │ Pure pattern      │                │ extraction point   │          │ already loaded     │
  │ matching, no LLM  │                │ nobody else does  │          │                   │
  └──────────────────┘                └──────────────────┘          └──────────────────┘
```

### Layer 0: `icm extract` — rule-based capture (implemented)

Keyword scoring extracts facts from text without any LLM call:

```bash
# Extract from text
echo "The parser uses Pratt algorithm for precedence" | icm extract -p my-project

# Preview without storing
icm extract -p my-project --dry-run -t "some text..."
```

Scores sentences by architecture, algorithm, decision, and technical keywords. Deduplicates via Jaccard similarity. Configurable in `config.toml`:

```toml
[extraction]
enabled = true
min_score = 3.0
max_facts = 10
```

### Layer 1: PreCompact extraction (planned)

When an agent's context fills up, compaction **destroys information forever**. A `PreCompact` hook captures context before it's lost:

```bash
icm extract --from-transcript ~/.claude/projects/.../transcript.jsonl
```

This is what makes ICM unique — **no other memory system captures context before compaction**.

### Layer 2: `icm recall-context` — session injection (implemented)

Auto-inject relevant memories before a session starts:

```bash
# Recall and format context for a query
icm recall-context "database architecture" --limit 10
```

Uses FTS5 search to find relevant memories and formats them as a context preamble. Configurable:

```toml
[recall]
enabled = true
limit = 15
```

### Comparison with alternatives

| System | Method | LLM cost | Latency | Captures compaction? |
|--------|--------|----------|---------|---------------------|
| **ICM** | 3-layer extraction | 0 to ~500 tok/session | 0ms | **Yes (PreCompact)** |
| Mem0 | 2 LLM calls/message | ~2k tok/message | 200-2000ms | No |
| claude-mem | PostToolUse + async worker | ~1-5k tok/session | 8ms hook | No |
| MemGPT/Letta | Agent self-manages | 0 marginal | 0ms | No |
| DiffMem | Git-based diffs | 0 | 0ms | No |

## License

[MIT](LICENSE)
