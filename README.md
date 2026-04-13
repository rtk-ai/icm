[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

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
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-blue.svg" alt="Apache-2.0"></a>
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

Configures **17 tools** in one command ([full integration guide](docs/integrations.md)):

| Tool | MCP | Hooks | CLI | Skills |
|------|:---:|:-----:|:---:|:------:|
| Claude Code | `~/.claude.json` | 5 hooks | `CLAUDE.md` | `/recall` `/remember` |
| Claude Desktop | JSON | — | — | — |
| Gemini CLI | `~/.gemini/settings.json` | 5 hooks | `GEMINI.md` | — |
| Codex CLI | `~/.codex/config.toml` | 4 hooks | `AGENTS.md` | — |
| Copilot CLI | `~/.copilot/mcp-config.json` | 4 hooks | `.github/copilot-instructions.md` | — |
| Cursor | `~/.cursor/mcp.json` | — | — | `.mdc` rule |
| Windsurf | JSON | — | `.windsurfrules` | — |
| VS Code | `~/Library/.../Code/User/mcp.json` | — | — | — |
| Amp | JSON | — | — | `/icm-recall` `/icm-remember` |
| Amazon Q | JSON | — | — | — |
| Cline | VS Code globalStorage | — | — | — |
| Roo Code | VS Code globalStorage | — | — | `.md` rule |
| Kilo Code | VS Code globalStorage | — | — | — |
| Zed | `~/.zed/settings.json` | — | — | — |
| OpenCode | JSON | TS plugin | — | — |
| Continue.dev | `~/.continue/config.yaml` | — | — | — |
| Aider | — | — | `.aider.conventions.md` | — |

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

### CLI instructions

```bash
icm init --mode cli
```

Injects ICM instructions into each tool's instruction file:

| Tool | File |
|------|------|
| Claude Code | `CLAUDE.md` |
| GitHub Copilot | `.github/copilot-instructions.md` |
| Windsurf | `.windsurfrules` |
| OpenAI Codex | `AGENTS.md` |
| Gemini | `~/.gemini/GEMINI.md` |

### Hooks (5 tools)

```bash
icm init --mode hook
```

Installs auto-extraction and auto-recall hooks for all supported tools:

| Tool | SessionStart | PreTool | PostTool | Compact | PromptRecall | Config |
|------|:-----------:|:-------:|:--------:|:-------:|:------------:|--------|
| Claude Code | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.claude/settings.json` |
| Gemini CLI | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.gemini/settings.json` |
| Codex CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `~/.codex/hooks.json` |
| Copilot CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `.github/hooks/icm.json` |
| OpenCode | session start | — | tool extract | compaction | — | `~/.config/opencode/plugins/icm.ts` |

**What each hook does:**

| Hook | What it does |
|------|-------------|
| `icm hook start` | Inject a wake-up pack of critical/high memories at session start (~500 tokens) |
| `icm hook pre` | Auto-allow `icm` CLI commands (no permission prompt) |
| `icm hook post` | Extract facts from tool output every N calls (auto-extraction) |
| `icm hook compact` | Extract memories from transcript before context compression |
| `icm hook prompt` | Inject recalled context at the start of each user prompt |

## CLI vs MCP

ICM can be used via CLI (`icm` commands) or MCP server (`icm serve`). Both access the same database.

| | CLI | MCP |
|---|-----|-----|
| **Latency** | ~30ms (direct binary) | ~50ms (JSON-RPC stdio) |
| **Token cost** | 0 (hook-based, invisible) | ~20-50 tokens/call (tool schema) |
| **Setup** | `icm init --mode hook` | `icm init --mode mcp` |
| **Works with** | Claude Code, Gemini, Codex, Copilot, OpenCode (via hooks) | All 17 MCP-compatible tools |
| **Auto-extraction** | Yes (hooks trigger `icm extract`) | Yes (MCP tools call store) |
| **Best for** | Power users, token savings | Universal compatibility |

## Dashboard

```bash
icm dashboard    # or: icm tui
```

Interactive TUI with 5 tabs: Overview, Topics, Memories, Health, Memoirs. Keyboard navigation (vim-style: j/k, g/G, Tab, 1-5), live search (/), auto-refresh.

Requires the `tui` feature (enabled by default). Build without: `cargo install --path crates/icm-cli --no-default-features --features embeddings`.

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

### Transcripts (verbatim session replay)

Store every message exchanged with an agent as-is — no summarization, no extraction.
Search later with FTS5 (BM25 + boolean + phrase + prefix). Useful for session replay,
post-mortem review, compliance audit, training data. Complementary to curated memories.

```bash
# 1. Start a session
SID=$(icm transcript start-session --agent claude-code --project myapp)

# 2. Record every turn verbatim
icm transcript record -s "$SID" -r user      -c "Pourquoi on avait choisi Postgres ?"
icm transcript record -s "$SID" -r assistant -c "JSONB natif, BRIN pour les logs, auto-vacuum tuné."
icm transcript record -s "$SID" -r tool      -c '{"cmd":"psql -c ..."}' -t Bash --tokens 42

# 3. Replay, search, inspect
icm transcript list-sessions --project myapp
icm transcript show "$SID" --limit 200
icm transcript search "postgres JSONB"                    # BM25 ranked
icm transcript search '"auto-vacuum"'                     # phrase match
icm transcript search "postgres OR mysql" --session "$SID" # boolean, scoped
icm transcript stats

# 4. Delete a session (cascade deletes its messages)
icm transcript forget "$SID"
```

Rust + SQLite + FTS5 — 0 Python, 0 ChromaDB, 0 external service. Writes are ~10× faster than
ChromaDB-based verbatim stores; the whole transcript lives in the same SQLite file as your
memories and memoirs.

## MCP Tools (27)

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

### Transcript tools (verbatim session replay)

| Tool | Description |
|------|-------------|
| `icm_transcript_start_session` | Create a session for verbatim message capture; returns `session_id` |
| `icm_transcript_record` | Append a raw message (role, content, optional tool + tokens + metadata) |
| `icm_transcript_search` | FTS5 search across messages (BM25, boolean, phrase, prefix) |
| `icm_transcript_show` | Replay full message thread of a session, chronologically |
| `icm_transcript_stats` | Sessions, messages, bytes, breakdown by role/agent/top-sessions |

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
  Layer 0: Pattern hooks              Layer 1: PreCompact           Layer 2: UserPromptSubmit
  (zero LLM cost)                     (zero LLM cost)               (zero LLM cost)
  ┌──────────────────┐                ┌──────────────────┐          ┌──────────────────┐
  │ PostToolUse hook  │                │ PreCompact hook   │          │ UserPromptSubmit  │
  │                   │                │                   │          │                   │
  │ • Bash errors     │                │ Context about to  │          │ User sends prompt │
  │ • git commits     │                │ be compressed →   │          │ → icm recall      │
  │ • config changes  │                │ extract memories  │          │ → inject context  │
  │ • decisions       │                │ from transcript   │          │                   │
  │ • preferences     │                │ before they're    │          │ Agent starts with  │
  │ • learnings       │                │ lost forever      │          │ relevant memories  │
  │ • constraints     │                │                   │          │ already loaded     │
  │                   │                │ Same patterns +   │          │                   │
  │ Rule-based, no LLM│                │ --store-raw fallbk│          │                   │
  └──────────────────┘                └──────────────────┘          └──────────────────┘
```

| Layer | Status | LLM cost | Hook command | Description |
|-------|--------|----------|-------------|-------------|
| Layer 0 | Implemented | 0 | `icm hook post` | Rule-based keyword extraction from tool output |
| Layer 1 | Implemented | 0 | `icm hook compact` | Extract from transcript before context compression |
| Layer 2 | Implemented | 0 | `icm hook prompt` | Inject recalled memories on each user prompt |

All 3 layers are installed automatically by `icm init --mode hook`.

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

### LongMemEval (ICLR 2025)

Standard academic benchmark — 500 questions across 6 memory abilities, from the [LongMemEval paper](https://arxiv.org/abs/2410.10813) (ICLR 2025).

```
LongMemEval Results — ICM (oracle variant, 500 questions)
════════════════════════════════════════════════════════════════
Category                        Retrieval     Answer (Sonnet)
────────────────────────────────────────────────────────────────
single-session-user                100.0%           91.4%
temporal-reasoning                 100.0%           85.0%
single-session-assistant           100.0%           83.9%
multi-session                      100.0%           81.2%
knowledge-update                   100.0%           80.8%
single-session-preference          100.0%           50.0%
────────────────────────────────────────────────────────────────
OVERALL                            100.0%           82.0%
════════════════════════════════════════════════════════════════
```

- **Retrieval** = does ICM find the right information? **100% across all categories.**
- **Answer** = can the LLM produce the correct answer from retrieved context? Depends on the LLM, not ICM.
- The retrieval score is the ICM benchmark. The answer score reflects the downstream LLM capability.

`scripts/bench-longmemeval.py --judge claude --workers 8`

### Test protocol

All benchmarks use **real API calls** — no mocks, no simulated responses, no cached answers.

- **Agent benchmark**: Creates a real Rust project in a tempdir. Runs N sessions with `claude -p --output-format json`. Without ICM: empty MCP config. With ICM: real MCP server + auto-extraction + context injection.
- **Knowledge retention**: Uses a fictional technical document (the "Meridian Protocol"). Scores answers by keyword matching against expected facts. 120s timeout per invocation.
- **Isolation**: Each run uses its own tempdir and fresh SQLite DB. No session persistence.

### Multi-agent unified memory

All 17 tools share the same SQLite database. A memory stored by Claude is instantly available to Gemini, Codex, Copilot, Cursor, and every other tool.

```
ICM Multi-Agent Efficiency Benchmark (10 seeded facts, 5 CLI agents)
╔══════════════╦═══════╦══════════╦════════╦═══════════╦═══════╗
║ Agent        ║ Facts ║ Accuracy ║ Detail ║ Latency   ║ Score ║
╠══════════════╬═══════╬══════════╬════════╬═══════════╬═══════╣
║ Claude Code  ║ 10/10 ║   100%   ║  5/5   ║    ~15s   ║   99  ║
║ Gemini CLI   ║ 10/10 ║   100%   ║  5/5   ║    ~33s   ║   94  ║
║ Copilot CLI  ║ 10/10 ║   100%   ║  5/5   ║    ~10s   ║  100  ║
║ Cursor Agent ║ 10/10 ║   100%   ║  5/5   ║    ~16s   ║   99  ║
║ Aider        ║ 10/10 ║   100%   ║  5/5   ║     ~5s   ║  100  ║
╠══════════════╬═══════╬══════════╬════════╬═══════════╬═══════╣
║ AVERAGE      ║       ║          ║        ║           ║   98  ║
╚══════════════╩═══════╩══════════╩════════╩═══════════╩═══════╝
```

Score = 60% recall accuracy + 30% fact detail + 10% speed. **98% multi-agent efficiency.**

## Why ICM

| Capability | ICM | Mem0 | Engram | AgentMemory |
|-----------|:---:|:----:|:------:|:-----------:|
| Tool support | **17** | SDK only | ~6-8 | ~10 |
| One-command setup | `icm init` | manual SDK | manual | manual |
| Hooks (auto-recall at startup) | 5 tools | none | via MCP | 1 tool |
| Hybrid search (FTS5 + vector) | 30/70 weighted | vector only | FTS5 only | FTS5+vector |
| Multilingual embeddings | 100+ langs (768d) | depends | none | English 384d |
| Knowledge graph | Memoir system | none | none | none |
| Temporal decay + consolidation | access-aware | none | basic | basic |
| TUI dashboard | `icm dashboard` | none | yes | web viewer |
| Auto-extraction from tool output | 3 layers, zero LLM | none | none | none |
| Feedback/correction loop | `icm_feedback_*` | none | none | none |
| Runtime | Rust single binary | Python | Go | Node.js |
| Local-first, zero dependencies | SQLite file | cloud-first | SQLite | SQLite |
| Multi-agent recall accuracy | **98%** | N/A | N/A | 95.2% |

## Documentation

| Document | Description |
|----------|-------------|
| [Integration Guide](docs/integrations.md) | Setup for all 17 tools: Claude Code, Copilot, Cursor, Windsurf, Zed, Amp, etc. |
| [Technical Architecture](docs/architecture.md) | Crate structure, search pipeline, decay model, sqlite-vec integration, testing |
| [User Guide](docs/guide.md) | Installation, topic organization, consolidation, extraction, troubleshooting |
| [Product Overview](docs/product.md) | Use cases, benchmarks, comparison with alternatives |

## License

[Apache-2.0](LICENSE)
