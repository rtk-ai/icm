<p align="center">
  <img src="assets/banner.png" alt="ICM — Infinite Context Memory" width="600">
</p>

<h1 align="center">ICM</h1>

<p align="center">
  Permanent memory for AI agents. Single binary, zero dependencies, MCP native.
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Source--Available-orange.svg" alt="Source-Available"></a>
</p>

---

ICM gives your AI agent a real memory — not a note-taking tool, not a context manager, a **memory**.

```
                       ICM (Infinite Context Memory)
            ┌──────────────────────┬─────────────────────────┐
            │   MEMORIES (Topics)  │   MEMOIRS (Knowledge)   │
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
            │             SQLite + FTS5 + sqlite-vec          │
            │        Hybrid search: BM25 (30%) + cosine (70%) │
            └─────────────────────────────────────────────────┘
```

**Two memory models:**

- **Memories** — store/recall with temporal decay by importance. Critical memories never fade, low-importance ones decay naturally. Filter by topic or keyword.
- **Memoirs** — permanent knowledge graphs. Concepts linked by typed relations (`depends_on`, `contradicts`, `superseded_by`, ...). Filter by label.
- **Feedback** — record corrections when AI predictions are wrong. Search past mistakes before making new predictions. Closed-loop learning.

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

```bash
# Auto-detect and configure all supported tools
icm init
```

Configures **14 tools** in one command:

| Tool | Config file | Format |
|------|------------|--------|
| Claude Code | `~/.claude.json` | JSON |
| Claude Desktop | `~/Library/.../claude_desktop_config.json` | JSON |
| Cursor | `~/.cursor/mcp.json` | JSON |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` | JSON |
| VS Code / Copilot | `~/Library/.../Code/User/mcp.json` | JSON |
| Gemini Code Assist | `~/.gemini/settings.json` | JSON |
| Zed | `~/.zed/settings.json` | JSON |
| Amp | `~/.config/amp/settings.json` | JSON |
| Amazon Q | `~/.aws/amazonq/mcp.json` | JSON |
| Cline | VS Code globalStorage | JSON |
| Roo Code | VS Code globalStorage | JSON |
| Kilo Code | VS Code globalStorage | JSON |
| OpenAI Codex CLI | `~/.codex/config.toml` | TOML |
| OpenCode | `~/.config/opencode/opencode.json` | JSON |

Or manually:

```bash
# Claude Code
claude mcp add icm -- icm serve

# Compact mode (shorter responses, saves tokens)
claude mcp add icm -- icm serve --compact

# Any MCP client: command = "icm", args = ["serve"]
```

### Skills / rules

```bash
icm init --mode skill
```

Installs slash commands and rules for Claude Code (`/recall`, `/remember`), Cursor (`.mdc` rule), Roo Code (`.md` rule), and Amp (`/icm-recall`, `/icm-remember`).

## CLI

### Memories (episodic, with decay)

```bash
# Store
icm store -t "my-project" -c "Use PostgreSQL for the main DB" -i high -k "db,postgres"

# Recall
icm recall "database choice"
icm recall "auth setup" --topic "my-project" --limit 10
icm recall "architecture" --keyword "postgres"

# Manage
icm forget <memory-id>
icm consolidate --topic "my-project"
icm topics
icm stats

# Extract facts from text (rule-based, zero LLM cost)
echo "The parser uses Pratt algorithm" | icm extract -p my-project
```

### Memoirs (permanent knowledge graphs)

```bash
# Create a memoir
icm memoir create -n "system-architecture" -d "System design decisions"

# Add concepts with labels
icm memoir add-concept -m "system-architecture" -n "auth-service" \
  -d "Handles JWT tokens and OAuth2 flows" -l "domain:auth,type:service"

# Link concepts
icm memoir link -m "system-architecture" --from "api-gateway" --to "auth-service" -r depends-on

# Search with label filter
icm memoir search -m "system-architecture" "authentication"
icm memoir search -m "system-architecture" "service" --label "domain:auth"

# Inspect neighborhood
icm memoir inspect -m "system-architecture" "auth-service" -D 2

# Export graph (formats: json, dot, ascii, ai)
icm memoir export -m "system-architecture" -f ascii   # Box-drawing with confidence bars
icm memoir export -m "system-architecture" -f dot      # Graphviz DOT (color = confidence level)
icm memoir export -m "system-architecture" -f ai       # Markdown optimized for LLM context
icm memoir export -m "system-architecture" -f json     # Structured JSON with all metadata

# Generate SVG visualization
icm memoir export -m "system-architecture" -f dot | dot -Tsvg > graph.svg
```

## MCP Tools (22)

### Memory tools

| Tool | Description |
|------|-------------|
| `icm_memory_store` | Store with auto-dedup (>85% similarity → update instead of duplicate) |
| `icm_memory_recall` | Search by query, filter by topic and/or keyword |
| `icm_memory_update` | Edit a memory in-place (content, importance, keywords) |
| `icm_memory_forget` | Delete a memory by ID |
| `icm_memory_consolidate` | Merge all memories of a topic into one summary |
| `icm_memory_list_topics` | List all topics with counts |
| `icm_memory_stats` | Global memory statistics |
| `icm_memory_health` | Per-topic hygiene audit (staleness, consolidation needs) |
| `icm_memory_embed_all` | Backfill embeddings for vector search |

### Memoir tools (knowledge graphs)

| Tool | Description |
|------|-------------|
| `icm_memoir_create` | Create a new memoir (knowledge container) |
| `icm_memoir_list` | List all memoirs |
| `icm_memoir_show` | Show memoir details and all concepts |
| `icm_memoir_add_concept` | Add a concept with labels |
| `icm_memoir_refine` | Update a concept's definition |
| `icm_memoir_search` | Full-text search, optionally filtered by label |
| `icm_memoir_search_all` | Search across all memoirs |
| `icm_memoir_link` | Create typed relation between concepts |
| `icm_memoir_inspect` | Inspect concept and graph neighborhood (BFS) |
| `icm_memoir_export` | Export graph (json, dot, ascii, ai) with confidence levels |

### Feedback tools (learning from mistakes)

| Tool | Description |
|------|-------------|
| `icm_feedback_record` | Record a correction when an AI prediction was wrong |
| `icm_feedback_search` | Search past corrections to inform future predictions |
| `icm_feedback_stats` | Feedback statistics: total count, breakdown by topic, most applied |

### Relation types

`part_of` · `depends_on` · `related_to` · `contradicts` · `refines` · `alternative_to` · `caused_by` · `instance_of` · `superseded_by`

## How it works

### Dual memory model

**Episodic memory (Topics)** captures decisions, errors, preferences. Each memory has a weight that decays over time based on importance:

| Importance | Decay | Prune | Behavior |
|-----------|-------|-------|----------|
| `critical` | none | never | Never forgotten, never pruned |
| `high` | slow (0.5x rate) | never | Fades slowly, never auto-deleted |
| `medium` | normal | yes | Standard decay, pruned when weight < threshold |
| `low` | fast (2x rate) | yes | Quickly forgotten |

Decay is **access-aware**: frequently recalled memories decay slower (`decay / (1 + access_count × 0.1)`). Applied automatically on recall (if >24h since last decay).

**Memory hygiene** is built-in:
- **Auto-dedup**: storing content >85% similar to an existing memory in the same topic updates it instead of creating a duplicate
- **Consolidation hints**: when a topic exceeds 7 entries, `icm_memory_store` warns the caller to consolidate
- **Health audit**: `icm_memory_health` reports per-topic entry count, average weight, stale entries, and consolidation needs
- **No silent data loss**: critical and high-importance memories are never auto-pruned

**Semantic memory (Memoirs)** captures structured knowledge as a graph. Concepts are permanent — they get refined, never decayed. Use `superseded_by` to mark obsolete facts instead of deleting them.

### Hybrid search

With embeddings enabled, ICM uses hybrid search:
- **FTS5 BM25** (30%) — full-text keyword matching
- **Cosine similarity** (70%) — semantic vector search via sqlite-vec

Default model: `intfloat/multilingual-e5-base` (768d, 100+ languages). Configurable in your [config file](#configuration):

```toml
[embeddings]
# enabled = false                          # Disable entirely (no model download)
model = "intfloat/multilingual-e5-base"    # 768d, multilingual (default)
# model = "intfloat/multilingual-e5-small" # 384d, multilingual (lighter)
# model = "intfloat/multilingual-e5-large" # 1024d, multilingual (best accuracy)
# model = "Xenova/bge-small-en-v1.5"      # 384d, English-only (fastest)
# model = "jinaai/jina-embeddings-v2-base-code"  # 768d, code-optimized
```

To skip the embedding model download entirely, use any of these:
```bash
icm --no-embeddings serve          # CLI flag
ICM_NO_EMBEDDINGS=1 icm serve     # Environment variable
```
Or set `enabled = false` in your config file. ICM will fall back to FTS5 keyword search (still works, just no semantic matching).

Changing the model automatically re-creates the vector index (existing embeddings are cleared and can be regenerated with `icm_memory_embed_all`).

### Storage

Single SQLite file. No external services, no network dependency.

```
~/Library/Application Support/dev.icm.icm/memories.db                    # macOS
~/.local/share/dev.icm.icm/memories.db                                   # Linux
C:\Users\<user>\AppData\Local\icm\icm\data\memories.db                   # Windows
```

### Configuration

```bash
icm config                    # Show active config
```

Config file location (platform-specific, or `$ICM_CONFIG`):

```
~/Library/Application Support/dev.icm.icm/config.toml                    # macOS
~/.config/icm/config.toml                                                # Linux
C:\Users\<user>\AppData\Roaming\icm\icm\config\config.toml              # Windows
```

See [config/default.toml](config/default.toml) for all options.

## Auto-extraction

ICM extracts memories automatically via three layers:

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

| Layer | Status | LLM cost | Description |
|-------|--------|----------|-------------|
| Layer 0 | Implemented | 0 | Rule-based keyword extraction via `icm extract` |
| Layer 1 | Planned | ~500 tok | PreCompact hook captures context before compaction |
| Layer 2 | Implemented | 0 | `icm recall-context` injects memories at session start |

### Comparison with alternatives

| System | Method | LLM cost | Latency | Captures compaction? |
|--------|--------|----------|---------|---------------------|
| **ICM** | 3-layer extraction | 0 to ~500 tok/session | 0ms | **Yes (PreCompact)** |
| Mem0 | 2 LLM calls/message | ~2k tok/message | 200-2000ms | No |
| claude-mem | PostToolUse + async | ~1-5k tok/session | 8ms hook | No |
| MemGPT/Letta | Agent self-manages | 0 marginal | 0ms | No |
| DiffMem | Git-based diffs | 0 | 0ms | No |

## Benchmarks

### Storage performance

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

Apple M1 Pro, in-memory SQLite, single-threaded. `icm bench --count 1000`

### Agent efficiency

Multi-session workflow with a real Rust project (12 files, ~550 lines). Sessions 2+ show the biggest gains as ICM recalls instead of re-reading files.

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

### Knowledge retention

Agent recalls specific facts from a dense technical document across sessions. Session 1 reads and memorizes; sessions 2+ answer 10 factual questions **without** the source text.

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

## Documentation

| Document | Description |
|----------|-------------|
| [Technical Architecture](docs/architecture.md) | Crate structure, search pipeline, decay model, sqlite-vec integration, testing |
| [User Guide](docs/guide.md) | Installation, topic organization, consolidation, extraction, troubleshooting |
| [Product Overview](docs/product.md) | Use cases, benchmarks, comparison with alternatives |

## License

[Source-Available](LICENSE) — Free for individuals and teams ≤ 20 people. Enterprise license required for larger organizations. Contact: license@rtk.ai
